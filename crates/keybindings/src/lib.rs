pub mod action;
pub mod conflicts;
pub mod controller;
pub mod defaults;
pub mod store;
pub mod trigger;

pub use action::{ActionScope, ShortcutCategory, ShortcutId, TerminalInputMode};
pub use conflicts::{ActionDiagnostics, ChordStatus, OutgoingOverride};
pub use controller::{FocusContext, WindowShortcutHost, attach_window_controller};
pub use defaults::{ACTION_SPECS, ActionSpec, spec_for};
pub use store::{ConfigWarning, OverrideValue, get_keybindings_path};
pub use trigger::{ACCEL_MOD_MASK, Chord, TriggerParseError, is_standalone_modifier};

use arc_swap::ArcSwap;
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::sync::{Arc, LazyLock};
use tokio::sync::broadcast;

/// Broadcasts `()` whenever the compiled keymap changes (UI edit, reset, or `keybindings.toml` reload).
pub static KEYBINDINGS_EVENT_BUS: LazyLock<broadcast::Sender<()>> = LazyLock::new(|| {
    let (tx, _) = broadcast::channel(16);
    tx
});

static WRITE_LOCK: Mutex<()> = Mutex::new(());

static KEYBINDINGS: LazyLock<ArcSwap<CompiledKeymap>> =
    LazyLock::new(|| ArcSwap::from_pointee(CompiledKeymap::load_from_disk()));

/// Flattened, priority-ordered entry scanned during `PropagationPhase::Capture` key dispatch.
#[derive(Clone, Copy, Debug)]
pub struct DispatchEntry {
    pub id: ShortcutId,
    pub chord: Chord,
}

/// Immutable snapshot of resolved keybindings, custom override state, and conflict diagnostics.
#[derive(Clone, Debug)]
pub struct CompiledKeymap {
    overrides: BTreeMap<ShortcutId, OverrideValue>,
    resolved: [Vec<Chord>; ShortcutId::ALL.len()],
    diagnostics: Vec<ActionDiagnostics>,
    dispatch_table: Vec<DispatchEntry>,
    warnings: Vec<ConfigWarning>,
    stamp: Option<store::FileStamp>,
}

impl CompiledKeymap {
    #[must_use]
    pub fn from_overrides(
        overrides: BTreeMap<ShortcutId, OverrideValue>,
        warnings: Vec<ConfigWarning>,
        stamp: Option<store::FileStamp>,
    ) -> Self {
        let resolved: [Vec<Chord>; ShortcutId::ALL.len()] = std::array::from_fn(|idx| {
            let id = ShortcutId::ALL[idx];
            match overrides.get(&id) {
                Some(OverrideValue::Unbound) => Vec::new(),
                Some(OverrideValue::Chords(chords)) => chords.clone(),
                None => store::default_chords_for(id),
            }
        });

        let diagnostics = conflicts::analyze_diagnostics(&resolved);
        let dispatch_table = build_dispatch_table(&resolved, &overrides);

        Self {
            overrides,
            resolved,
            diagnostics,
            dispatch_table,
            warnings,
            stamp,
        }
    }

    #[must_use]
    fn load_from_disk() -> Self {
        let path = store::get_keybindings_path();
        let loaded = store::load_overrides_from_path(&path);
        Self::from_overrides(loaded.overrides, loaded.warnings, loaded.stamp)
    }

    /// Priority-ordered dispatch slice (`GlobalWindow` -> `PaneOrOverlay` -> `TerminalFocused`).
    #[must_use]
    pub fn dispatch_entries(&self) -> &[DispatchEntry] {
        &self.dispatch_table
    }

    /// User-visible resolved chords for `id` (empty slice if disabled/unbound).
    #[must_use]
    pub fn chords_for(&self, id: ShortcutId) -> &[Chord] {
        &self.resolved[id as usize]
    }

    /// Primary GTK accelerator string (e.g. `"<Control><Shift>t"`) for `adw::ShortcutLabel`
    /// or Command Palette badges, or `None` when unbound.
    #[must_use]
    pub fn primary_gtk_accel(&self, id: ShortcutId) -> Option<String> {
        self.chords_for(id).first().map(|c| c.to_gtk_accel())
    }

    /// Returns `true` when `id` has a user override in `keybindings.toml` that differs from default.
    #[must_use]
    pub fn is_customized(&self, id: ShortcutId) -> bool {
        self.overrides
            .get(&id)
            .is_some_and(|ov| !store::matches_default(id, ov))
    }

    #[must_use]
    pub fn has_any_custom_overrides(&self) -> bool {
        self.overrides
            .iter()
            .any(|(&id, ov)| !store::matches_default(id, ov))
    }

    #[must_use]
    pub fn diagnostics_for(&self, id: ShortcutId) -> &ActionDiagnostics {
        &self.diagnostics[id as usize]
    }

    #[must_use]
    pub fn warnings(&self) -> &[ConfigWarning] {
        &self.warnings
    }

    #[must_use]
    pub const fn overrides_map(&self) -> &BTreeMap<ShortcutId, OverrideValue> {
        &self.overrides
    }

    /// Finds any other action that already binds `chord` (ignoring `performable:`).
    #[must_use]
    pub fn find_conflicting_owner(
        &self,
        chord: Chord,
        excluding: Option<ShortcutId>,
    ) -> Option<ShortcutId> {
        ShortcutId::ALL.into_iter().find(|&id| {
            excluding != Some(id)
                && self.resolved[id as usize]
                    .iter()
                    .any(|c| c.signature() == chord.signature())
        })
    }

    /// Builds a `|`-separated `GtkShortcutTrigger` string (including implicit aliases like
    /// `KP_Add`/`equal` when at default) for isolated dialog widgets such as `BookmarkEditor`.
    #[must_use]
    pub fn gtk_trigger_string(&self, id: ShortcutId) -> Option<String> {
        let chords = self.chords_for(id);
        if chords.is_empty() {
            return None;
        }
        let mut parts: Vec<String> = chords.iter().map(|c| c.to_gtk_accel()).collect();
        if !self.is_customized(id) {
            for alias in spec_for(id).implicit_aliases {
                if let Ok(c) = Chord::parse(alias) {
                    let accel = c.to_gtk_accel();
                    if !parts.contains(&accel) {
                        parts.push(accel);
                    }
                }
            }
        }
        Some(parts.join("|"))
    }
}

fn build_dispatch_table(
    resolved: &[Vec<Chord>; ShortcutId::ALL.len()],
    overrides: &BTreeMap<ShortcutId, OverrideValue>,
) -> Vec<DispatchEntry> {
    let mut ordered_ids = ShortcutId::ALL;
    ordered_ids.sort_by_key(|&id| (spec_for(id).scope, id as usize));

    let mut table = Vec::with_capacity(64);
    for id in ordered_ids {
        let chords = &resolved[id as usize];
        for &chord in chords {
            table.push(DispatchEntry { id, chord });
        }

        // Attach implicit numpad/layout aliases (e.g. `Ctrl+=`, `Ctrl+KP_Add`, `Ctrl+Insert`)
        // while the action has not been customized away from its built-in defaults.
        let customized = overrides
            .get(&id)
            .is_some_and(|ov| !store::matches_default(id, ov));
        if !customized && !chords.is_empty() {
            for raw_alias in spec_for(id).implicit_aliases {
                if let Ok(alias_chord) = Chord::parse(raw_alias)
                    && !chords.iter().any(|c| c.signature() == alias_chord.signature())
                {
                    table.push(DispatchEntry {
                        id,
                        chord: alias_chord,
                    });
                }
            }
        }
    }
    table
}

/// Lock-free read of the current `CompiledKeymap`.
#[must_use]
pub fn keymap() -> Arc<CompiledKeymap> {
    KEYBINDINGS.load_full()
}

/// Initializes keybindings on startup and performs one-time migration of legacy
/// `settings.claw_msgbar_shortcut` if `keybindings.toml` does not exist yet.
pub fn init_with_legacy_claw_shortcut(legacy_claw_shortcut: &str) {
    let path = store::get_keybindings_path();
    migrate_legacy_claw_in(&path, legacy_claw_shortcut);
    reload_from_disk();
}

pub(crate) fn migrate_legacy_claw_in(path: &Path, legacy_claw_shortcut: &str) -> bool {
    if path.exists() {
        return false;
    }
    let trimmed = legacy_claw_shortcut.trim();
    if trimmed.is_empty() {
        return false;
    }
    let Ok(parsed) = Chord::parse(trimmed) else {
        return false;
    };
    let default_claw = store::default_chords_for(ShortcutId::AskClaw);
    if default_claw.as_slice() == [parsed] {
        return false;
    }

    let mut overrides = BTreeMap::new();
    overrides.insert(ShortcutId::AskClaw, OverrideValue::Chords(vec![parsed]));
    if let Err(e) = store::save_overrides_to_path(path, &overrides) {
        log::warn!("Failed to migrate legacy claw_msgbar_shortcut to keybindings.toml: {e}");
        return false;
    }
    true
}

/// Reloads `keybindings.toml` unconditionally and notifies subscribers if anything changed.
pub fn reload_from_disk() {
    let next = CompiledKeymap::load_from_disk();
    KEYBINDINGS.store(Arc::new(next));
    let _ = KEYBINDINGS_EVENT_BUS.send(());
}

/// Checks `keybindings.toml`'s mtime/size stamp (call from `spawn_blocking` off the GTK thread)
/// and reloads + broadcasts only when the file on disk actually changed.
#[must_use]
pub fn reload_if_changed_on_disk() -> bool {
    let path = store::get_keybindings_path();
    let disk_stamp = store::stat_stamp(&path);
    let current_stamp = KEYBINDINGS.load().stamp;
    if disk_stamp == current_stamp {
        return false;
    }
    reload_from_disk();
    true
}

/// Ensures `keybindings.toml` exists on disk so it can be opened in an external text editor.
///
/// # Errors
/// Returns an error if creating or writing `keybindings.toml` fails.
pub fn ensure_keybindings_file() -> io::Result<std::path::PathBuf> {
    let _guard = WRITE_LOCK.lock();
    let path = store::get_keybindings_path();
    let current = KEYBINDINGS.load_full();
    store::ensure_keybindings_file_in(&path, current.overrides_map())?;
    let stamp = store::stat_stamp(&path);
    if stamp != current.stamp {
        let updated = CompiledKeymap::from_overrides(
            current.overrides.clone(),
            current.warnings.clone(),
            stamp,
        );
        KEYBINDINGS.store(Arc::new(updated));
    }
    Ok(path)
}

/// Atomically mutates user shortcut overrides, saves `keybindings.toml`, updates `KEYBINDINGS`,
/// and broadcasts once on `KEYBINDINGS_EVENT_BUS`.
pub fn update_overrides<F>(mutator: F) -> Arc<CompiledKeymap>
where
    F: FnOnce(&mut BTreeMap<ShortcutId, OverrideValue>),
{
    let _guard = WRITE_LOCK.lock();
    let current = KEYBINDINGS.load_full();
    let mut next_overrides = current.overrides.clone();
    mutator(&mut next_overrides);

    // Strip any entries that match built-in defaults so `keybindings.toml` stays sparse.
    next_overrides.retain(|&id, val| !store::matches_default(id, val));

    if next_overrides == current.overrides && current.warnings.is_empty() {
        return current;
    }

    let path = store::get_keybindings_path();
    if let Err(e) = store::save_overrides_to_path(&path, &next_overrides) {
        log::error!("Failed to save keybindings.toml: {e}");
    }
    let stamp = store::stat_stamp(&path);
    let next_map = Arc::new(CompiledKeymap::from_overrides(
        next_overrides,
        Vec::new(),
        stamp,
    ));
    KEYBINDINGS.store(next_map.clone());
    let _ = KEYBINDINGS_EVENT_BUS.send(());
    next_map
}

/// Sets `chords` for `target`, optionally removing any conflicting chord from other actions.
pub fn set_action_chords(
    target: ShortcutId,
    chords: Vec<Chord>,
    displace_conflicts: bool,
) {
    let _ = update_overrides(|map| {
        if displace_conflicts {
            let current = CompiledKeymap::from_overrides(map.clone(), Vec::new(), None);
            for &new_chord in &chords {
                for other_id in ShortcutId::ALL {
                    if other_id == target {
                        continue;
                    }
                    let other_chords = current.chords_for(other_id);
                    if other_chords
                        .iter()
                        .any(|c| c.signature() == new_chord.signature())
                    {
                        let remaining: Vec<Chord> = other_chords
                            .iter()
                            .copied()
                            .filter(|c| c.signature() != new_chord.signature())
                            .collect();
                        let next_val = if remaining.is_empty() {
                            OverrideValue::Unbound
                        } else {
                            OverrideValue::Chords(remaining)
                        };
                        map.insert(other_id, next_val);
                    }
                }
            }
        }

        let entry = if chords.is_empty() {
            OverrideValue::Unbound
        } else {
            OverrideValue::Chords(chords)
        };
        map.insert(target, entry);
    });
}

/// Resets a single action back to its built-in default shortcut(s).
pub fn reset_action(id: ShortcutId) {
    let _ = update_overrides(|map| {
        map.remove(&id);
    });
}

/// Resets all shortcuts back to Boxxy's built-in defaults.
pub fn reset_all() {
    let _ = update_overrides(BTreeMap::clear);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_specs_align_one_to_one_with_shortcut_id_enum() {
        assert_eq!(ACTION_SPECS.len(), ShortcutId::ALL.len());
        for (idx, &id) in ShortcutId::ALL.iter().enumerate() {
            assert_eq!(idx, id as usize);
            assert_eq!(ACTION_SPECS[idx].id, id);
            assert_eq!(id.as_str().parse::<ShortcutId>(), Ok(id));

            // Every default and implicit alias must parse cleanly.
            for &trig in ACTION_SPECS[idx].default_triggers {
                assert!(
                    Chord::parse(trig).is_ok(),
                    "failed to parse default trigger '{trig}' on {id}"
                );
            }
            for &alias in ACTION_SPECS[idx].implicit_aliases {
                assert!(
                    Chord::parse(alias).is_ok(),
                    "failed to parse implicit alias '{alias}' on {id}"
                );
            }
        }
    }

    #[test]
    fn dual_syntax_and_symbol_chords_parse_and_roundtrip() {
        let human = Chord::parse("Ctrl+Shift+V").unwrap();
        let gtk_lower = Chord::parse("<Control><Shift>v").unwrap();
        let gtk_upper = Chord::parse("<Ctrl><Shift>V").unwrap();
        assert_eq!(human, gtk_lower);
        assert_eq!(human, gtk_upper);
        assert_eq!(human.to_human_string(), "Ctrl+Shift+V");

        let zoom_in = Chord::parse("Ctrl++").unwrap();
        assert_eq!(zoom_in, Chord::parse("<Control>plus").unwrap());
        assert_eq!(zoom_in.to_human_string(), "Ctrl++");
        assert_eq!(Chord::parse(&zoom_in.to_human_string()).unwrap(), zoom_in);

        let zoom_shifted = Chord::parse("Ctrl+Shift++").unwrap();
        assert_eq!(zoom_shifted, Chord::parse("<Control><Shift>plus").unwrap());

        let zoom_out = Chord::parse("Ctrl+-").unwrap();
        assert_eq!(zoom_out, Chord::parse("<Control>minus").unwrap());
        assert_eq!(zoom_out.to_human_string(), "Ctrl+-");

        let ask_claw = Chord::parse("Ctrl+/").unwrap();
        assert_eq!(ask_claw, Chord::parse("<Ctrl>slash").unwrap());
        assert_eq!(ask_claw, Chord::parse("<Ctrl>/").unwrap());
        assert_eq!(ask_claw.to_human_string(), "Ctrl+/");

        assert_eq!(
            Chord::parse("<Ctrl><Shift>?").unwrap(),
            Chord::parse("<Control><Shift>question").unwrap()
        );
        assert_eq!(
            Chord::parse("<Alt>Return").unwrap(),
            Chord::parse("Alt+Enter").unwrap()
        );

        let smart_copy = Chord::parse("performable:Ctrl+C").unwrap();
        assert!(smart_copy.performable);
        assert_eq!(smart_copy.to_human_string(), "performable:Ctrl+C");
        assert_eq!(Chord::parse(&smart_copy.to_human_string()).unwrap(), smart_copy);
    }

    #[test]
    fn default_diagnostics_detect_swap_up_down_overriding_prompt_block_jump() {
        let km = CompiledKeymap::from_overrides(BTreeMap::new(), Vec::new(), None);

        // SwapUp (`PaneOrOverlay`) overrides PrevPrompt (`TerminalFocused`) on `Ctrl+Shift+Up`,
        // while PrevPrompt keeps `Ctrl+Shift+PageUp` active.
        let swap_up_diag = km.diagnostics_for(ShortcutId::SwapUp);
        assert_eq!(
            swap_up_diag.outgoing_overrides,
            vec![OutgoingOverride::Action {
                chord: Chord::parse("Ctrl+Shift+Up").unwrap(),
                shadowed_action: ShortcutId::PrevPrompt,
                shadowed_has_active_alt: true,
            }]
        );

        let prev_prompt_diag = km.diagnostics_for(ShortcutId::PrevPrompt);
        assert_eq!(prev_prompt_diag.chord_statuses.len(), 2);
        assert_eq!(prev_prompt_diag.chord_statuses[0].1, ChordStatus::Active);
        assert_eq!(
            prev_prompt_diag.chord_statuses[1].1,
            ChordStatus::ShadowedBy(ShortcutId::SwapUp)
        );
    }

    #[test]
    fn terminal_signal_shadow_detected_until_performable_enabled() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            ShortcutId::Copy,
            OverrideValue::Chords(vec![Chord::parse("Ctrl+C").unwrap()]),
        );
        let km_hard = CompiledKeymap::from_overrides(overrides, Vec::new(), None);
        let copy_diag = km_hard.diagnostics_for(ShortcutId::Copy);
        assert!(matches!(
            copy_diag.outgoing_overrides.as_slice(),
            [OutgoingOverride::TerminalBuiltin {
                can_make_performable: true,
                ..
            }]
        ));

        let mut smart_overrides = BTreeMap::new();
        smart_overrides.insert(
            ShortcutId::Copy,
            OverrideValue::Chords(vec![Chord::parse("performable:Ctrl+C").unwrap()]),
        );
        let km_smart = CompiledKeymap::from_overrides(smart_overrides, Vec::new(), None);
        assert_eq!(km_smart.diagnostics_for(ShortcutId::Copy).outgoing_overrides, []);
    }

    #[test]
    fn sparse_toml_roundtrip_and_malformed_entry_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybindings.toml");

        let mut map = BTreeMap::new();
        map.insert(
            ShortcutId::NewTab,
            OverrideValue::Chords(vec![Chord::parse("Ctrl+T").unwrap()]),
        );
        map.insert(
            ShortcutId::ToggleSidebar,
            OverrideValue::Chords(vec![
                Chord::parse("Ctrl+Shift+J").unwrap(),
                Chord::parse("F9").unwrap(),
            ]),
        );
        map.insert(ShortcutId::SwapUp, OverrideValue::Unbound);
        // Setting an action to its exact factory default should be omitted from disk.
        map.insert(
            ShortcutId::Paste,
            OverrideValue::Chords(vec![Chord::parse("Ctrl+Shift+V").unwrap()]),
        );

        store::save_overrides_to_path(&path, &map).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("new-tab = \"Ctrl+T\""));
        assert!(raw.contains("toggle-sidebar = [\"Ctrl+Shift+J\", \"F9\"]"));
        assert!(raw.contains("swap-up = \"none\""));
        assert!(!raw.contains("paste ="));

        let loaded = store::load_overrides_from_path(&path);
        assert_eq!(loaded.warnings, []);
        let km = CompiledKeymap::from_overrides(loaded.overrides, loaded.warnings, loaded.stamp);
        assert!(km.is_customized(ShortcutId::NewTab));
        assert!(km.is_customized(ShortcutId::SwapUp));
        assert!(!km.is_customized(ShortcutId::Paste));
        assert_eq!(km.chords_for(ShortcutId::SwapUp), []);
        // With SwapUp unbound, PrevPrompt's Ctrl+Shift+Up is no longer shadowed!
        assert!(
            km.diagnostics_for(ShortcutId::PrevPrompt)
                .chord_statuses
                .iter()
                .all(|(_, st)| *st == ChordStatus::Active)
        );

        // Hand-edited file with one invalid key should preserve valid entries and surface 1 warning.
        let mixed = r#"
[bindings]
new-tab = "Ctrl+T"
close-tab = "Ctrl+NotARealKey"
"#;
        let parsed = store::parse_overrides_toml(mixed, None);
        assert_eq!(parsed.warnings.len(), 1);
        assert_eq!(parsed.warnings[0].key, "close-tab");
        assert!(parsed.overrides.contains_key(&ShortcutId::NewTab));
    }

    #[test]
    fn legacy_claw_shortcut_migrates_only_when_non_default_and_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keybindings.toml");

        assert!(!migrate_legacy_claw_in(&path, "<Ctrl>slash"));
        assert!(!path.exists());

        assert!(migrate_legacy_claw_in(&path, "<Alt>space"));
        assert!(path.exists());
        let loaded = store::load_overrides_from_path(&path);
        let km = CompiledKeymap::from_overrides(loaded.overrides, Vec::new(), None);
        assert_eq!(
            km.chords_for(ShortcutId::AskClaw),
            &[Chord::parse("Alt+Space").unwrap()]
        );

        // Subsequent startups never overwrite an existing `keybindings.toml`.
        assert!(!migrate_legacy_claw_in(&path, "<Ctrl>k"));
    }
}
