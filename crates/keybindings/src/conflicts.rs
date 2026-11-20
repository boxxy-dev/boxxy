use crate::action::{ActionScope, ShortcutId};
use crate::defaults::spec_for;
use crate::trigger::Chord;
use gtk4::gdk;

/// Status of an individual chord attached to an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChordStatus {
    /// Active and reachable.
    Active,
    /// Shadowed because a higher-priority action in an encompassing scope claims the same chord.
    ShadowedBy(ShortcutId),
}

/// Describes what a binding overrides when active.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutgoingOverride {
    /// Overrides a lower-priority Boxxy shortcut action on `chord`.
    Action {
        chord: Chord,
        shadowed_action: ShortcutId,
        /// `true` when `shadowed_action` still has at least one other unshadowed chord
        /// (for example `PrevPrompt` retaining `Ctrl+Shift+PageUp` when `SwapUp` claims `Ctrl+Shift+Up`).
        shadowed_has_active_alt: bool,
    },
    /// Intercepts a built-in terminal shell signal or navigation key before it can reach `boxxy-vte` / PTY.
    TerminalBuiltin {
        chord: Chord,
        builtin_label: &'static str,
        /// `true` when enabling `performable:` allows the key to pass through to the PTY when idle
        /// (for example `performable:Ctrl+C` on `Copy`).
        can_make_performable: bool,
    },
}

/// Complete conflict & override diagnostics for one `ShortcutId`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActionDiagnostics {
    pub chord_statuses: Vec<(Chord, ChordStatus)>,
    pub outgoing_overrides: Vec<OutgoingOverride>,
}

impl ActionDiagnostics {
    #[must_use]
    pub const fn has_outgoing_overrides(&self) -> bool {
        !self.outgoing_overrides.is_empty()
    }

    #[must_use]
    pub fn has_shadowed_chords(&self) -> bool {
        self.chord_statuses
            .iter()
            .any(|(_, st)| matches!(st, ChordStatus::ShadowedBy(_)))
    }
}

/// Returns `true` if two `ActionScope`s can be active at the same time (which is true for all
/// three scopes whenever a terminal widget has focus).
#[must_use]
pub const fn scopes_overlap(_a: ActionScope, _b: ActionScope) -> bool {
    true
}

/// Computes `(winner, loser)` priority key: broader `ActionScope` wins first (`GlobalWindow` <
/// `PaneOrOverlay` < `TerminalFocused`), breaking ties by `ShortcutId` declaration order.
const fn priority_key(id: ShortcutId) -> (ActionScope, usize) {
    (spec_for(id).scope, id as usize)
}

/// Analyzes the resolved chords for all `ShortcutId::ALL` actions and computes incoming
/// `ShadowedBy` statuses as well as outgoing `Action` and `TerminalBuiltin` overrides.
#[must_use]
pub fn analyze_diagnostics(resolved: &[Vec<Chord>; ShortcutId::ALL.len()]) -> Vec<ActionDiagnostics> {
    let mut result: Vec<ActionDiagnostics> = ShortcutId::ALL
        .iter()
        .map(|&id| ActionDiagnostics {
            chord_statuses: resolved[id as usize]
                .iter()
                .map(|&c| (c, ChordStatus::Active))
                .collect(),
            outgoing_overrides: Vec::new(),
        })
        .collect();

    // 1. Pairwise action-vs-action collision & override resolution
    for &id_a in &ShortcutId::ALL {
        let spec_a = spec_for(id_a);
        for (idx_a, &chord_a) in resolved[id_a as usize].iter().enumerate() {
            let mut winning_blocker: Option<ShortcutId> = None;

            for &id_b in &ShortcutId::ALL {
                if id_a == id_b || !scopes_overlap(spec_a.scope, spec_for(id_b).scope) {
                    continue;
                }
                let shares_sig = resolved[id_b as usize]
                    .iter()
                    .any(|cb| cb.signature() == chord_a.signature());
                if shares_sig
                    && priority_key(id_b) < priority_key(id_a)
                    && winning_blocker.is_none_or(|cur| priority_key(id_b) < priority_key(cur))
                {
                    winning_blocker = Some(id_b);
                }
            }

            if let Some(winner) = winning_blocker {
                result[id_a as usize].chord_statuses[idx_a].1 = ChordStatus::ShadowedBy(winner);
            }
        }
    }

    // 2. Populate outgoing `Action` overrides now that all `ShadowedBy` statuses are known
    for &shadowed_id in &ShortcutId::ALL {
        let has_active_alt = result[shadowed_id as usize]
            .chord_statuses
            .iter()
            .any(|(_, st)| *st == ChordStatus::Active);

        let statuses = result[shadowed_id as usize].chord_statuses.clone();
        for (chord, status) in statuses {
            if let ChordStatus::ShadowedBy(winner_id) = status {
                result[winner_id as usize]
                    .outgoing_overrides
                    .push(OutgoingOverride::Action {
                        chord,
                        shadowed_action: shadowed_id,
                        shadowed_has_active_alt: has_active_alt,
                    });
            }
        }
    }

    // 3. Detect terminal/PTY built-in shadows for active non-performable chords
    for &id in &ShortcutId::ALL {
        let statuses = result[id as usize].chord_statuses.clone();
        for (chord, status) in statuses {
            if status != ChordStatus::Active || chord.performable {
                continue;
            }
            if let Some(builtin_label) = classify_terminal_builtin_shadow(id, chord) {
                result[id as usize]
                    .outgoing_overrides
                    .push(OutgoingOverride::TerminalBuiltin {
                        chord,
                        builtin_label,
                        can_make_performable: id.supports_performable(),
                    });
            }
        }
    }

    result
}

/// Checks whether `chord` shadows a standard shell C0 control signal, TUI function key,
/// or built-in terminal scrollback/selection gesture.
fn classify_terminal_builtin_shadow(owner: ShortcutId, chord: Chord) -> Option<&'static str> {
    if chord.is_bare_function_key() {
        return Some("Terminal TUI Function Key (htop, mc)");
    }

    // Unshifted `Ctrl+<letter>` C0 control signals
    if chord.modifiers == gdk::ModifierType::CONTROL_MASK {
        return match chord.keyval {
            gdk::Key::c => Some("Terminal Interrupt Signal (SIGINT / ^C)"),
            gdk::Key::d => Some("Terminal End-of-File (EOF / ^D)"),
            gdk::Key::z => Some("Terminal Suspend Signal (SIGTSTP / ^Z)"),
            gdk::Key::l => Some("Shell Clear Screen (^L)"),
            gdk::Key::r => Some("Shell Reverse History Search (^R)"),
            gdk::Key::w => Some("Shell Delete Previous Word (^W)"),
            gdk::Key::u => Some("Shell Clear Line (^U)"),
            gdk::Key::a => Some("Shell Beginning of Line (^A)"),
            gdk::Key::e => Some("Shell End of Line (^E)"),
            _ => None,
        };
    }

    // `Shift+PageUp/PageDown/Home/End` scrollback and `Shift+Arrow` selection adjustment
    if chord.modifiers == gdk::ModifierType::SHIFT_MASK {
        return match chord.keyval {
            gdk::Key::Page_Up | gdk::Key::Page_Down | gdk::Key::Home | gdk::Key::End => {
                Some("Terminal Scrollback Navigation")
            }
            gdk::Key::Left | gdk::Key::Right | gdk::Key::Up | gdk::Key::Down => {
                Some("Terminal Selection Adjustment")
            }
            _ => None,
        };
    }

    // If an action other than `PrevPrompt` / `NextPrompt` binds `Ctrl+Shift+PageUp/PageDown`
    // (or `Ctrl+Shift+Up/Down` when `PrevPrompt`/`NextPrompt` was customized away from it),
    // `boxxy-vte` also has a built-in OSC 133 prompt-jump fallback on those keys.
    if chord.modifiers
        == (gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK)
        && !matches!(owner, ShortcutId::PrevPrompt | ShortcutId::NextPrompt)
    {
        return match chord.keyval {
            gdk::Key::Page_Up | gdk::Key::Page_Down => {
                Some("Terminal Command Block Jump (OSC 133)")
            }
            _ => None,
        };
    }

    None
}
