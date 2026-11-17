//! Disk I/O, atomic TOML updates, and bundled-default extraction for the Character catalog.

mod bundled;
mod toml_patch;

pub use bundled::{bundled_default_avatar, bundled_default_color, incognito_avatar_bytes};

use crate::characters::{
    CharacterConfig, CharacterInfo, INCOGNITO_CHARACTER_ID, INCOGNITO_CHARACTER_SLUG,
};
use anyhow::{Context, Result};
use bundled::{BUNDLED_CHARACTERS, BUNDLED_JSON};
use std::collections::{HashMap, HashSet};
use std::fs::{self, DirEntry, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use toml_patch::{CharacterToml, patch_top_level_string_key};

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

const FALLBACK_DEFAULT_COLOR: &str = "#8035D4";

/// Writes `contents` to a hidden `.tmp` sibling in the same directory, flushes file data,
/// and atomically renames it over `path` so concurrent readers and `CharacterWatcher` never
/// observe partial or 0-byte files.
fn atomic_write(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("Target path has no parent directory: {}", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create directory {}", parent.display()))?;

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("character_file");
    let seq = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_path = parent.join(format!(".{file_name}.{}.{seq}.tmp", std::process::id()));

    let write_res = (|| -> std::io::Result<()> {
        let mut file = File::create(&tmp_path)?;
        file.write_all(contents.as_ref())?;
        file.sync_data()
    })();

    if let Err(err) = write_res {
        let _ = fs::remove_file(&tmp_path);
        return Err(err)
            .with_context(|| format!("Failed to write temporary file {}", tmp_path.display()));
    }

    if let Err(err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err).with_context(|| {
            format!(
                "Failed to rename {} to {}",
                tmp_path.display(),
                path.display()
            )
        });
    }
    Ok(())
}

/// Returns the base directory for characters: `~/.config/boxxy-terminal/boxxyclaw/characters/`.
///
/// # Errors
/// Infallible in practice; returns `Result` for backwards compatibility with existing callers.
pub fn get_characters_dir() -> Result<PathBuf> {
    let config_dir = boxxy_sys_utils::get_config_dir();
    Ok(config_dir.join("boxxyclaw").join("characters"))
}

/// Returns the original/default hex color for a character:
/// - For the three built-in characters (`niko-una`, `levi-kujo`, `kuro`), returns their bundled color.
/// - For custom characters, returns `default_color` from `CHARACTER.toml` if present, otherwise their current `color`.
#[must_use]
pub fn character_default_color(slug: &str, char_dir: &Path) -> Option<String> {
    if let Some(bundled) = bundled_default_color(slug) {
        return Some(bundled.to_string());
    }
    let toml_str = fs::read_to_string(char_dir.join("CHARACTER.toml")).ok()?;
    let parsed = toml::from_str::<CharacterToml>(&toml_str).ok()?;
    Some(parsed.default_color.unwrap_or(parsed.color))
}

/// Updates `color = "..."` inside `char_dir/CHARACTER.toml` in-place while preserving formatting.
///
/// For custom characters, records `default_color = "<original>"` on first edit so
/// [`reset_character_avatar`] can restore the original color.
///
/// # Errors
/// Returns an error if `CHARACTER.toml` cannot be read, parsed, or atomically written.
pub fn save_character_color(char_dir: &Path, new_color: &str) -> Result<()> {
    let toml_path = char_dir.join("CHARACTER.toml");
    let content = fs::read_to_string(&toml_path)
        .with_context(|| format!("Failed to read CHARACTER.toml at {}", toml_path.display()))?;

    let slug = char_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let is_bundled = bundled_default_color(slug).is_some();
    let parsed_existing: CharacterToml = toml::from_str(&content)
        .with_context(|| format!("Failed to parse CHARACTER.toml at {}", toml_path.display()))?;

    if parsed_existing.color == new_color {
        return Ok(());
    }

    let extra_default = if !is_bundled && parsed_existing.default_color.is_none() {
        Some(("default_color", parsed_existing.color.as_str()))
    } else {
        None
    };

    let patched = patch_top_level_string_key(&content, "color", new_color, extra_default, None)?;
    atomic_write(&toml_path, patched)
        .with_context(|| format!("Failed to write CHARACTER.toml at {}", toml_path.display()))
}

/// Returns `true` if the character at `char_dir` has a user-customised avatar or color.
#[must_use]
pub fn has_custom_character_avatar(slug: &str, char_dir: &Path) -> bool {
    let avatar_path = char_dir.join("AVATAR.png");
    let avatar_custom = bundled_default_avatar(slug).map_or_else(
        || avatar_path.is_file(),
        |default_bytes| match fs::metadata(&avatar_path) {
            Ok(meta) if meta.is_file() => {
                if meta.len() == default_bytes.len() as u64 {
                    fs::read(&avatar_path)
                        .is_ok_and(|disk_bytes| disk_bytes.as_slice() != default_bytes)
                } else {
                    true
                }
            }
            _ => false,
        },
    );

    if avatar_custom {
        return true;
    }

    if let Ok(toml_str) = fs::read_to_string(char_dir.join("CHARACTER.toml"))
        && let Ok(parsed) = toml::from_str::<CharacterToml>(&toml_str)
    {
        if let Some(default_color) = bundled_default_color(slug) {
            return !parsed.color.eq_ignore_ascii_case(default_color);
        }
        if let Some(default_color) = parsed.default_color {
            return !parsed.color.eq_ignore_ascii_case(&default_color);
        }
    }
    false
}

/// Resets a character's avatar and color to their default state:
/// - For the three bundled characters (`niko-una`, `levi-kujo`, `kuro`), restores the
///   bundled default `AVATAR.png` and default `color` in `CHARACTER.toml`, returning `Ok(Some(default_bytes))`.
/// - For custom characters (4th+), removes `AVATAR.png` so the UI falls back to standard
///   Adwaita initials, restores `default_color` if one was saved, and returns `Ok(None)`.
///
/// # Errors
/// Returns an error if restoring or removing `AVATAR.png` or updating `CHARACTER.toml` fails.
pub fn reset_character_avatar(slug: &str, char_dir: &Path) -> Result<Option<&'static [u8]>> {
    let avatar_path = char_dir.join("AVATAR.png");
    if let Some(default_color) = character_default_color(slug, char_dir)
        && char_dir.join("CHARACTER.toml").is_file()
    {
        save_character_color(char_dir, &default_color)?;
    }

    if let Some(default_bytes) = bundled_default_avatar(slug) {
        atomic_write(&avatar_path, default_bytes).with_context(|| {
            format!(
                "Failed to restore default avatar at {}",
                avatar_path.display()
            )
        })?;
        Ok(Some(default_bytes))
    } else {
        if let Err(e) = fs::remove_file(&avatar_path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(e)
                .with_context(|| format!("Failed to remove avatar at {}", avatar_path.display()));
        }
        Ok(None)
    }
}

/// Ensures the characters directory exists and has at least one valid character.
///
/// If the user has customised the directory (even a single valid character exists),
/// the bundled defaults are never re-extracted.
///
/// # Errors
/// Returns an error if the directory cannot be created or bundled defaults cannot be written.
pub fn ensure_default_character() -> Result<()> {
    let base_dir = get_characters_dir()?;
    ensure_default_character_in(&base_dir)
}

/// Directory-parameterized variant of [`ensure_default_character`].
///
/// # Errors
/// Returns an error if `base_dir` cannot be created or bundled defaults cannot be extracted.
pub fn ensure_default_character_in(base_dir: &Path) -> Result<()> {
    if !base_dir.exists() {
        fs::create_dir_all(base_dir).context("Failed to create characters directory")?;
    }

    if !has_any_valid_character(base_dir) {
        extract_bundled_defaults(base_dir)?;
    }

    Ok(())
}

/// Removes all existing character directories and re-extracts the three bundled defaults.
///
/// # Errors
/// Returns an error if removing existing directories or writing bundled defaults fails.
pub fn reset_to_defaults() -> Result<()> {
    let base_dir = get_characters_dir()?;
    reset_to_defaults_in(&base_dir)
}

/// Directory-parameterized variant of [`reset_to_defaults`].
///
/// Unlinks symlinks directly (`fs::remove_file`) rather than calling `fs::remove_dir_all`
/// so symlinked character folders neither fail with `ENOTDIR` nor delete external targets.
///
/// # Errors
/// Returns an error if removing existing entries or writing bundled defaults fails.
pub fn reset_to_defaults_in(base_dir: &Path) -> Result<()> {
    if base_dir.exists() {
        for entry in fs::read_dir(base_dir).context("Failed to read characters directory")? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .with_context(|| format!("Failed to inspect {}", path.display()))?;
            if file_type.is_symlink() {
                fs::remove_file(&path)
                    .with_context(|| format!("Failed to remove symlink {}", path.display()))?;
            } else if file_type.is_dir() {
                fs::remove_dir_all(&path)
                    .with_context(|| format!("Failed to remove {}", path.display()))?;
            }
        }
        let _ = fs::remove_file(base_dir.join("characters.json"));
    }

    extract_bundled_defaults(base_dir)
}

enum CandidateEntry {
    ValidDir(PathBuf),
    ReservedIncognito(String),
    Ignore,
}

fn classify_character_entry(entry: &DirEntry) -> CandidateEntry {
    let file_name = entry.file_name();
    let Some(name) = file_name.to_str() else {
        return CandidateEntry::Ignore;
    };
    if name.is_empty() || name.starts_with('.') {
        return CandidateEntry::Ignore;
    }

    let path = entry.path();
    let is_dir = match entry.file_type() {
        Ok(ft) if ft.is_symlink() => path.is_dir(),
        Ok(ft) => ft.is_dir(),
        Err(_) => path.is_dir(),
    };
    if !is_dir {
        return CandidateEntry::Ignore;
    }
    if name.eq_ignore_ascii_case(INCOGNITO_CHARACTER_SLUG) {
        return CandidateEntry::ReservedIncognito(name.to_string());
    }
    CandidateEntry::ValidDir(path)
}

fn has_any_valid_character(base_dir: &Path) -> bool {
    fs::read_dir(base_dir).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            matches!(classify_character_entry(&entry), CandidateEntry::ValidDir(path)
                if fs::read_to_string(path.join("CHARACTER.toml"))
                    .ok()
                    .and_then(|s| toml::from_str::<CharacterToml>(&s).ok())
                    .is_some())
        })
    })
}

fn extract_bundled_defaults(base_dir: &Path) -> Result<()> {
    fs::create_dir_all(base_dir).context("Failed to create characters directory")?;
    atomic_write(&base_dir.join("characters.json"), BUNDLED_JSON)
        .context("Failed to write bundled characters.json")?;

    for bundled in BUNDLED_CHARACTERS {
        let dir = base_dir.join(bundled.slug);
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create {} directory", bundled.slug))?;
        atomic_write(&dir.join("CHARACTER.toml"), bundled.toml_bytes)
            .with_context(|| format!("Failed to write {} CHARACTER.toml", bundled.slug))?;
        atomic_write(&dir.join("AVATAR.png"), bundled.avatar_bytes)
            .with_context(|| format!("Failed to write {} AVATAR.png", bundled.slug))?;
    }

    Ok(())
}

/// Loads all character configurations from the active config's characters directory.
///
/// # Errors
/// Returns an error if the characters directory cannot be created or read.
pub fn load_characters() -> Result<Vec<CharacterInfo>> {
    let base_dir = get_characters_dir()?;
    load_characters_in(&base_dir)
}

/// Loads all character configurations from `base_dir` in a single directory pass.
///
/// Restores bundled defaults if empty, repairs duplicate UUID collisions atomically,
/// and sorts entries according to `characters.json`.
///
/// # Errors
/// Returns an error if `base_dir` cannot be created or read.
pub fn load_characters_in(base_dir: &Path) -> Result<Vec<CharacterInfo>> {
    if !base_dir.exists() {
        fs::create_dir_all(base_dir).context("Failed to create characters directory")?;
    }

    let mut characters = scan_character_dirs(base_dir, true)?;

    // Guarantee at least the bundled characters are available if the directory was empty
    // or every existing subdirectory failed to parse.
    if characters.is_empty() {
        extract_bundled_defaults(base_dir)?;
        characters = scan_character_dirs(base_dir, false)?;
    }

    // Deterministic order before UUID collision checks so the first directory alphabetically
    // keeps its UUID and any copied directory gets a fresh one.
    characters.sort_by(|a, b| a.config.name.cmp(&b.config.name));

    // Guard against duplicate UUIDs (e.g., user copied a character directory).
    let mut seen_ids: HashSet<String> = HashSet::with_capacity(characters.len());
    for character in &mut characters {
        while !seen_ids.insert(character.config.id.clone()) {
            let new_id = uuid::Uuid::new_v4().to_string();
            let toml_path = base_dir.join(&character.config.name).join("CHARACTER.toml");
            if let Ok(content) = fs::read_to_string(&toml_path)
                && let Ok(patched) = patch_top_level_string_key(
                    &content,
                    "id",
                    &new_id,
                    None,
                    Some("# Stable identity — do not change this value"),
                )
                && let Err(err) = atomic_write(&toml_path, patched)
            {
                log::warn!(
                    "Failed to persist deduplicated UUID at {}: {err}",
                    toml_path.display()
                );
            }
            character.config.id = new_id;
        }
    }

    // Sort by `characters.json` order when present, falling back to slug order.
    let json_path = base_dir.join("characters.json");
    if let Ok(content) = fs::read_to_string(json_path)
        && let Ok(order) = serde_json::from_str::<Vec<String>>(&content)
    {
        let rank_by_slug: HashMap<&str, usize> = order
            .iter()
            .enumerate()
            .map(|(idx, slug)| (slug.as_str(), idx))
            .collect();
        characters.sort_by(|a, b| {
            let pos_a = rank_by_slug
                .get(a.config.name.as_str())
                .copied()
                .unwrap_or(usize::MAX);
            let pos_b = rank_by_slug
                .get(b.config.name.as_str())
                .copied()
                .unwrap_or(usize::MAX);
            pos_a
                .cmp(&pos_b)
                .then_with(|| a.config.name.cmp(&b.config.name))
        });
    }

    Ok(characters)
}

fn scan_character_dirs(base_dir: &Path, warn_on_invalid: bool) -> Result<Vec<CharacterInfo>> {
    let mut characters = Vec::new();
    for entry in fs::read_dir(base_dir)? {
        let entry = entry?;
        match classify_character_entry(&entry) {
            CandidateEntry::Ignore => {}
            CandidateEntry::ReservedIncognito(name) => {
                if warn_on_invalid {
                    log::warn!("Skipping reserved incognito slug '{name}' in characters directory");
                }
            }
            CandidateEntry::ValidDir(path) => match load_character_from_dir(&path) {
                Ok(info) => characters.push(info),
                Err(e) => {
                    if warn_on_invalid {
                        let name = path.file_name().map_or_else(
                            || path.display().to_string(),
                            |n| n.to_string_lossy().into_owned(),
                        );
                        log::warn!("Skipping character at {name}: {e}");
                    }
                }
            },
        }
    }
    Ok(characters)
}

/// Persists the character order (`Vec<slug>`) to `characters.json`.
///
/// # Errors
/// Returns an error if serializing or atomically writing `characters.json` fails.
#[expect(
    clippy::needless_pass_by_value,
    reason = "preserves public Vec<String> signature used by preferences UI"
)]
pub fn save_character_order(order: Vec<String>) -> Result<()> {
    let base_dir = get_characters_dir()?;
    save_character_order_in(&base_dir, &order)
}

/// Directory-parameterized variant of [`save_character_order`].
///
/// # Errors
/// Returns an error if serializing or atomically writing `characters.json` fails.
pub fn save_character_order_in(base_dir: &Path, order: &[String]) -> Result<()> {
    let json_path = base_dir.join("characters.json");
    let content = serde_json::to_string_pretty(order)?;
    atomic_write(&json_path, content).context("Failed to write character order")
}

/// Loads a single character from a directory.
///
/// Expects a `CHARACTER.toml` and optionally an `AVATAR.png`.
/// Auto-generates a UUID and writes it back to the TOML if absent, empty, or colliding
/// with the reserved `__incognito__` ID.
fn load_character_from_dir(dir: &Path) -> Result<CharacterInfo> {
    let dir_name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .with_context(|| format!("Invalid character directory name: {}", dir.display()))?
        .to_string();

    let toml_path = dir.join("CHARACTER.toml");
    let avatar_path = dir.join("AVATAR.png");

    let toml_content = fs::read_to_string(&toml_path)
        .with_context(|| format!("Failed to read CHARACTER.toml in {}", dir.display()))?;

    let raw: CharacterToml = toml::from_str(&toml_content)
        .with_context(|| format!("Failed to parse CHARACTER.toml in {}", dir.display()))?;

    let id = if let Some(existing_id) = raw
        .id
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != INCOGNITO_CHARACTER_ID)
    {
        existing_id
    } else {
        let new_id = uuid::Uuid::new_v4().to_string();
        let patched = patch_top_level_string_key(
            &toml_content,
            "id",
            &new_id,
            None,
            Some("# Stable identity — do not change this value"),
        )?;
        atomic_write(&toml_path, patched).with_context(|| {
            format!(
                "Failed to write UUID to CHARACTER.toml in {}",
                dir.display()
            )
        })?;
        new_id
    };

    // Ensure the three bundled characters always fall back to their bundled avatar image
    // if AVATAR.png was removed from disk.
    let mut has_avatar = avatar_path.is_file();
    if !has_avatar
        && let Some(default_bytes) = bundled_default_avatar(&dir_name)
        && atomic_write(&avatar_path, default_bytes).is_ok()
    {
        has_avatar = true;
    }

    let display_name = if raw.display_name.trim().is_empty() {
        dir_name.clone()
    } else {
        raw.display_name
    };
    let color = {
        let trimmed = raw.color.trim();
        if trimmed.is_empty() {
            FALLBACK_DEFAULT_COLOR.to_string()
        } else {
            trimmed.to_string()
        }
    };

    let config = CharacterConfig {
        id,
        name: dir_name,
        display_name,
        color,
        duties: raw.duties,
        personality: raw.personality,
    };

    Ok(CharacterInfo::new(config, has_avatar))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bundled::find_bundled;

    #[test]
    fn bundled_characters_table_matches_embedded_toml_colors() {
        for b in BUNDLED_CHARACTERS {
            let toml_str = std::str::from_utf8(b.toml_bytes).unwrap();
            let parsed: CharacterToml = toml::from_str(toml_str).unwrap();
            assert_eq!(parsed.color, b.default_color);
            assert_ne!(b.avatar_bytes, b"" as &[u8]);
        }
        assert_ne!(incognito_avatar_bytes(), b"" as &[u8]);
    }

    #[test]
    fn bundled_and_custom_character_avatar_and_color_reset() {
        let tmp = tempfile::tempdir().unwrap();

        // 1. Bundled character ("niko-una") starts with default bytes -> not custom
        let niko_dir = tmp.path().join("niko-una");
        fs::create_dir_all(&niko_dir).unwrap();
        let default_bytes = bundled_default_avatar("niko-una").unwrap();
        fs::write(niko_dir.join("AVATAR.png"), default_bytes).unwrap();
        assert!(!has_custom_character_avatar("niko-una", &niko_dir));

        // User uploads a custom avatar -> now marked custom
        fs::write(niko_dir.join("AVATAR.png"), b"custom-png-bytes").unwrap();
        assert!(has_custom_character_avatar("niko-una", &niko_dir));

        // Resetting restores the bundled default avatar and clears custom state
        let restored = reset_character_avatar("niko-una", &niko_dir).unwrap();
        assert_eq!(restored, Some(default_bytes));
        assert!(!has_custom_character_avatar("niko-una", &niko_dir));

        // 2. 4th custom character ("my-custom-agent") has no bundled avatar
        let custom_dir = tmp.path().join("my-custom-agent");
        fs::create_dir_all(&custom_dir).unwrap();
        assert!(!has_custom_character_avatar("my-custom-agent", &custom_dir));

        fs::write(custom_dir.join("AVATAR.png"), b"custom-png-bytes").unwrap();
        assert!(has_custom_character_avatar("my-custom-agent", &custom_dir));

        // Resetting removes AVATAR.png so it falls back to standard Adwaita initials
        let restored_custom = reset_character_avatar("my-custom-agent", &custom_dir).unwrap();
        assert!(restored_custom.is_none());
        assert!(!custom_dir.join("AVATAR.png").exists());
        assert!(!has_custom_character_avatar("my-custom-agent", &custom_dir));

        // 3. Color customization and reset on bundled and custom characters
        let niko_toml = find_bundled("niko-una").unwrap().toml_bytes;
        fs::write(niko_dir.join("CHARACTER.toml"), niko_toml).unwrap();
        assert!(!has_custom_character_avatar("niko-una", &niko_dir));
        save_character_color(&niko_dir, "#123456").unwrap();
        assert!(has_custom_character_avatar("niko-una", &niko_dir));
        reset_character_avatar("niko-una", &niko_dir).unwrap();
        assert!(!has_custom_character_avatar("niko-una", &niko_dir));
        assert_eq!(
            character_default_color("niko-una", &niko_dir).as_deref(),
            Some("#8035D4")
        );

        // Single-line triple-quoted color preserves comments when patched in-place
        let custom_toml = "# Custom comment\nid = \"00000004-0000-4000-8000-000000000004\"\ndisplay_name = \"custom\"\ncolor = \"\"\"#abcdef\"\"\"\nduties = \"Helper\"\npersonality = \"Friendly\"\n";
        fs::write(custom_dir.join("CHARACTER.toml"), custom_toml).unwrap();
        assert!(!has_custom_character_avatar("my-custom-agent", &custom_dir));
        save_character_color(&custom_dir, "#fedcba").unwrap();
        assert!(
            fs::read_to_string(custom_dir.join("CHARACTER.toml"))
                .unwrap()
                .contains("# Custom comment")
        );
        assert!(has_custom_character_avatar("my-custom-agent", &custom_dir));
        assert_eq!(
            character_default_color("my-custom-agent", &custom_dir).as_deref(),
            Some("#abcdef")
        );
        reset_character_avatar("my-custom-agent", &custom_dir).unwrap();
        assert!(!has_custom_character_avatar("my-custom-agent", &custom_dir));
    }

    #[test]
    fn empty_id_reserved_incognito_and_symlinks_are_handled_safely() {
        let tmp = tempfile::tempdir().unwrap();
        let base_dir = tmp.path().join("chars");
        let external_dir = tmp.path().join("external-char");
        fs::create_dir_all(&base_dir).unwrap();
        fs::create_dir_all(&external_dir).unwrap();
        fs::write(
            external_dir.join("CHARACTER.toml"),
            "id = \"ext-1\"\ndisplay_name = \"Ext\"\ncolor = \"#111111\"\nduties = \"Ops\"\npersonality = \"Calm\"\n",
        )
        .unwrap();

        // Character 1: reserved `__incognito__` ID + blank display_name + multi-line prompt
        let alpha_dir = base_dir.join("alpha");
        fs::create_dir_all(&alpha_dir).unwrap();
        fs::write(
            alpha_dir.join("CHARACTER.toml"),
            "id = \"__incognito__\"\ndisplay_name = \"   \"\ncolor = \"#112233\"\nduties = \"Ops\"\npersonality = \"\"\"\nNever rewrite lines like:\nid = \"keep-me\"\ncolor = \"#999999\"\n\"\"\"\n",
        )
        .unwrap();

        // Reserved `inco-gnito` directory must be ignored
        let inco_dir = base_dir.join("inco-gnito");
        fs::create_dir_all(&inco_dir).unwrap();
        fs::write(
            inco_dir.join("CHARACTER.toml"),
            "id = \"bad\"\ndisplay_name = \"Fake\"\ncolor = \"#000000\"\nduties = \"x\"\npersonality = \"y\"\n",
        )
        .unwrap();

        let loaded_first = load_characters_in(&base_dir).unwrap();
        assert_eq!(loaded_first.len(), 1);
        let assigned_id = loaded_first[0].config.id.clone();
        assert_ne!(assigned_id, "");
        assert_ne!(assigned_id, INCOGNITO_CHARACTER_ID);
        assert_eq!(loaded_first[0].config.display_name, "alpha");
        assert!(
            loaded_first[0]
                .config
                .personality
                .contains("id = \"keep-me\"")
        );

        // Duplicate compact `id="..."` gets deduplicated and persisted
        let beta_dir = base_dir.join("beta");
        fs::create_dir_all(&beta_dir).unwrap();
        fs::write(
            beta_dir.join("CHARACTER.toml"),
            format!(
                "id=\"{assigned_id}\"\ndisplay_name = \"Beta\"\ncolor=\"#445566\"\nduties = \"Dev\"\npersonality = \"Calm\"\n"
            ),
        )
        .unwrap();

        let deduped = load_characters_in(&base_dir).unwrap();
        assert_eq!(deduped.len(), 2);
        assert_ne!(deduped[0].config.id, deduped[1].config.id);

        save_character_order_in(&base_dir, &["beta".to_string(), "alpha".to_string()]).unwrap();
        let ordered = load_characters_in(&base_dir).unwrap();
        assert_eq!(ordered[0].config.name, "beta");
        assert_eq!(ordered[1].config.name, "alpha");

        // Symlinked character directory is unlinked cleanly by `reset_to_defaults_in` without
        // failing with ENOTDIR or deleting the external target's contents.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&external_dir, base_dir.join("linked-char")).unwrap();
            reset_to_defaults_in(&base_dir).unwrap();
            assert!(external_dir.join("CHARACTER.toml").is_file());
            assert!(!base_dir.join("linked-char").exists());
            let defaults = load_characters_in(&base_dir).unwrap();
            assert_eq!(defaults.len(), 3);
        }
    }
}
