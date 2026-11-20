use crate::action::ShortcutId;
use crate::defaults::spec_for;
use crate::trigger::{Chord, is_unbind_keyword};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

const KEYBINDINGS_FILENAME: &str = "keybindings.toml";

const FILE_HEADER: &str = r#"# Boxxy Terminal — Custom Keybindings
# Share or edit this file freely! Only customized shortcuts need to be listed;
# omitted actions automatically inherit Boxxy's built-in defaults.
#
# Syntax examples:
#   new-tab = "Ctrl+T"                                  # Single shortcut
#   toggle-sidebar = ["Ctrl+Shift+J", "F9"]             # Multiple shortcuts
#   copy = ["Ctrl+Shift+C", "performable:Ctrl+C"]       # Smart copy when text is selected
#   swap-up = "none"                                    # Disable a default shortcut

"#;

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Sparse user override for a single `ShortcutId`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverrideValue {
    /// Explicitly disabled (`"none"` or `[]`).
    Unbound,
    /// Custom list of one or more chords.
    Chords(Vec<Chord>),
}

/// Non-fatal validation warning collected when loading a hand-edited `keybindings.toml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigWarning {
    pub key: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStamp {
    pub modified: Option<SystemTime>,
    pub len: u64,
}

#[derive(Clone, Debug, Default)]
pub struct LoadedOverrides {
    pub overrides: BTreeMap<ShortcutId, OverrideValue>,
    pub warnings: Vec<ConfigWarning>,
    pub stamp: Option<FileStamp>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawBindingValue {
    Single(String),
    Multiple(Vec<String>),
}

/// Resolves `<config_dir>/keybindings.toml` using `boxxy_sys_utils::get_config_dir()`.
#[must_use]
pub fn get_keybindings_path() -> PathBuf {
    boxxy_sys_utils::get_config_dir().join(KEYBINDINGS_FILENAME)
}

#[must_use]
pub fn stat_stamp(path: &Path) -> Option<FileStamp> {
    let meta = fs::metadata(path).ok()?;
    Some(FileStamp {
        modified: meta.modified().ok(),
        len: meta.len(),
    })
}

/// Ensures `keybindings.toml` exists on disk (creating it with the commented template header
/// if missing) so "Edit keybindings.toml" can open it directly in the user's editor.
///
/// # Errors
/// Returns an error if creating parent directories or writing the file fails.
pub fn ensure_keybindings_file_in(
    path: &Path,
    overrides: &BTreeMap<ShortcutId, OverrideValue>,
) -> io::Result<()> {
    if path.is_file() {
        return Ok(());
    }
    save_overrides_to_path(path, overrides)
}

/// Loads sparse overrides from `path`. Missing files return empty overrides with zero warnings.
#[must_use]
pub fn load_overrides_from_path(path: &Path) -> LoadedOverrides {
    let stamp = stat_stamp(path);
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return LoadedOverrides {
                overrides: BTreeMap::new(),
                warnings: Vec::new(),
                stamp: None,
            };
        }
        Err(e) => {
            return LoadedOverrides {
                overrides: BTreeMap::new(),
                warnings: vec![ConfigWarning {
                    key: KEYBINDINGS_FILENAME.to_string(),
                    message: format!("Failed to read file: {e}"),
                }],
                stamp,
            };
        }
    };

    parse_overrides_toml(&content, stamp)
}

#[must_use]
pub fn parse_overrides_toml(content: &str, stamp: Option<FileStamp>) -> LoadedOverrides {
    // Parse top-level table permissively so a single bad line doesn't discard valid bindings.
    let root_val: toml::Value = match toml::from_str(content) {
        Ok(v) => v,
        Err(e) => {
            // Also try RawKeybindingsFile directly in case error formatting differs
            log::warn!("Invalid TOML syntax in keybindings.toml: {e}");
            return LoadedOverrides {
                overrides: BTreeMap::new(),
                warnings: vec![ConfigWarning {
                    key: KEYBINDINGS_FILENAME.to_string(),
                    message: format!("TOML syntax error: {e}"),
                }],
                stamp,
            };
        }
    };

    let mut overrides = BTreeMap::new();
    let mut warnings = Vec::new();

    let Some(root_table) = root_val.as_table() else {
        return LoadedOverrides {
            overrides,
            warnings,
            stamp,
        };
    };

    // Support `[bindings]` (canonical), while tolerating flat top-level keys if a user omits `[bindings]`.
    let bindings_table = root_table
        .get("bindings")
        .and_then(toml::Value::as_table)
        .unwrap_or(root_table);

    for (raw_key, raw_val) in bindings_table {
        if raw_key == "vi" {
            // Reserved for Stage 2 Vi Mode key table.
            continue;
        }

        let id = match raw_key.parse::<ShortcutId>() {
            Ok(id) => id,
            Err(err) => {
                warnings.push(ConfigWarning {
                    key: raw_key.clone(),
                    message: err.to_string(),
                });
                continue;
            }
        };

        let parsed_raw: Result<RawBindingValue, _> = raw_val.clone().try_into();
        let Ok(raw_binding) = parsed_raw else {
            warnings.push(ConfigWarning {
                key: raw_key.clone(),
                message: "expected a shortcut string or array of strings".to_string(),
            });
            continue;
        };

        if let Some(entry) = parse_binding_entry(raw_key, raw_binding, &mut warnings)
            && !matches_default(id, &entry)
        {
            overrides.insert(id, entry);
        }
    }

    LoadedOverrides {
        overrides,
        warnings,
        stamp,
    }
}

fn parse_binding_entry(
    raw_key: &str,
    raw: RawBindingValue,
    warnings: &mut Vec<ConfigWarning>,
) -> Option<OverrideValue> {
    match raw {
        RawBindingValue::Single(s) => {
            if s.trim().is_empty() || is_unbind_keyword(&s) {
                return Some(OverrideValue::Unbound);
            }
            match Chord::parse(&s) {
                Ok(chord) => Some(OverrideValue::Chords(vec![chord])),
                Err(e) => {
                    warnings.push(ConfigWarning {
                        key: raw_key.to_string(),
                        message: e.reason,
                    });
                    None
                }
            }
        }
        RawBindingValue::Multiple(list) => {
            if list.is_empty() || list.iter().all(|s| s.trim().is_empty() || is_unbind_keyword(s)) {
                return Some(OverrideValue::Unbound);
            }
            let mut chords = Vec::with_capacity(list.len());
            for item in list {
                if item.trim().is_empty() || is_unbind_keyword(&item) {
                    continue;
                }
                match Chord::parse(&item) {
                    Ok(c) => {
                        if !chords.iter().any(|existing: &Chord| existing.signature() == c.signature()) {
                            chords.push(c);
                        }
                    }
                    Err(e) => {
                        warnings.push(ConfigWarning {
                            key: raw_key.to_string(),
                            message: format!("'{}': {}", item, e.reason),
                        });
                    }
                }
            }
            if chords.is_empty() {
                None
            } else {
                Some(OverrideValue::Chords(chords))
            }
        }
    }
}

/// Returns `true` when `entry` equals the built-in default for `id`, allowing sparse diff cleanup.
#[must_use]
pub fn matches_default(id: ShortcutId, entry: &OverrideValue) -> bool {
    let defaults = default_chords_for(id);
    match entry {
        OverrideValue::Unbound => defaults.is_empty(),
        OverrideValue::Chords(chords) => *chords == defaults,
    }
}

#[must_use]
pub fn default_chords_for(id: ShortcutId) -> Vec<Chord> {
    spec_for(id)
        .default_triggers
        .iter()
        .filter_map(|s| Chord::parse(s).ok())
        .collect()
}

/// Serializes `overrides` to clean human-readable TOML.
#[must_use]
pub fn serialize_overrides_toml(overrides: &BTreeMap<ShortcutId, OverrideValue>) -> String {
    let mut out = String::with_capacity(FILE_HEADER.len() + overrides.len() * 40 + 16);
    out.push_str(FILE_HEADER);
    out.push_str("[bindings]\n");

    for (&id, val) in overrides {
        if matches_default(id, val) {
            continue;
        }
        out.push_str(id.as_str());
        out.push_str(" = ");
        match val {
            OverrideValue::Unbound => {
                out.push_str("\"none\"\n");
            }
            OverrideValue::Chords(chords) => match chords.as_slice() {
                [] => out.push_str("\"none\"\n"),
                [single] => {
                    out.push('"');
                    out.push_str(&escape_toml_basic(&single.to_human_string()));
                    out.push_str("\"\n");
                }
                many => {
                    out.push('[');
                    for (idx, c) in many.iter().enumerate() {
                        if idx > 0 {
                            out.push_str(", ");
                        }
                        out.push('"');
                        out.push_str(&escape_toml_basic(&c.to_human_string()));
                        out.push('"');
                    }
                    out.push_str("]\n");
                }
            },
        }
    }

    out
}

fn escape_toml_basic(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Atomically writes `overrides` to `path` via a `0600` temp file + `sync_data()` + `rename()`.
///
/// # Errors
/// Returns an error if writing, syncing, or renaming the temporary file fails.
pub fn save_overrides_to_path(
    path: &Path,
    overrides: &BTreeMap<ShortcutId, OverrideValue>,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let serialized = serialize_overrides_toml(overrides);
    let pid = std::process::id();
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(KEYBINDINGS_FILENAME);
    let tmp_path = parent.join(format!(".{file_name}.{pid}.{seq}.tmp"));

    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }

    let write_res = (|| -> io::Result<()> {
        let mut f = opts.open(&tmp_path)?;
        f.write_all(serialized.as_bytes())?;
        f.sync_data()?;
        drop(f);
        fs::rename(&tmp_path, path)?;
        Ok(())
    })();

    if write_res.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }

    write_res
}
