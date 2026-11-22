//! Disk I/O, atomic TOML updates, Characters v2 manifest resolution, and bundled-default extraction.

mod bundled;
mod manifest;
mod toml_patch;

pub use bundled::{
    bundled_default_avatar, bundled_default_color, bundled_default_greetings,
    bundled_default_manifest, bundled_default_portrait, incognito_avatar_bytes,
    incognito_portrait_bytes,
};
pub use manifest::{
    CharacterDirDiagnostic, is_valid_hex_color, resolve_character_avatar_path,
    resolve_character_avatar_path_in, resolve_character_portrait_path,
    resolve_character_portrait_path_in, validate_character_package_dir,
    validate_relative_asset_path,
};

use crate::characters::{
    CharacterConfig, CharacterInfo, CharacterStatus, INCOGNITO_CHARACTER_ID,
    INCOGNITO_CHARACTER_SLUG,
};
use anyhow::{Context, Result};
use bundled::{BUNDLED_CHARACTERS, BUNDLED_JSON};
use manifest::{
    CUSTOM_AVATAR_PNG, CUSTOM_AVATAR_WEBP, CUSTOM_PORTRAIT_PNG, CUSTOM_PORTRAIT_WEBP,
    SEEDED_CHARACTERS_README, load_or_seed_manifest, parse_and_sanitize_manifest,
    read_custom_overrides, resolve_character_assets, resolve_package_base_avatar_rel,
    sync_characters_readme_once, write_custom_color_override,
};
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
pub(super) fn atomic_write(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
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

fn read_manifest_strict(slug: &str, char_dir: &Path) -> Option<crate::characters::CharacterManifest> {
    let raw = fs::read_to_string(char_dir.join("manifest.toml")).ok()?;
    parse_and_sanitize_manifest(&raw, slug).ok()
}

/// Returns the original/default hex color for a character:
/// - For the three built-in characters (`niko-una`, `levi-kujo`, `kuro`), returns their bundled color.
/// - For community/custom characters, matches `load_character_from_dir` non-override priority:
///   `CHARACTER.toml` `default_color` -> `CHARACTER.toml` `color` -> `manifest.toml` `[theme].accent_color` -> fallback.
#[must_use]
pub fn character_default_color(slug: &str, char_dir: &Path) -> Option<String> {
    if let Some(bundled) = bundled_default_color(slug) {
        return Some(bundled.to_string());
    }
    let parsed_toml = fs::read_to_string(char_dir.join("CHARACTER.toml"))
        .ok()
        .and_then(|s| toml::from_str::<CharacterToml>(&s).ok());

    if let Some(ref parsed) = parsed_toml {
        if let Some(ref def) = parsed.default_color
            && is_valid_hex_color(def.trim())
        {
            return Some(def.trim().to_string());
        }
        if let Some(ref col) = parsed.color
            && is_valid_hex_color(col.trim())
        {
            return Some(col.trim().to_string());
        }
    }

    if let Some(man) = read_manifest_strict(slug, char_dir)
        && let Some(accent) = man.theme.accent_color
    {
        return Some(accent);
    }

    parsed_toml.map(|_| FALLBACK_DEFAULT_COLOR.to_string())
}

/// Saves a user-cropped custom avatar for `char_dir`:
/// - For v2 packages (`manifest.toml` present) or characters with a non-`AVATAR.png` package
///   avatar (`avatar.png` / `avatar.webp`), writes non-destructively to `custom_avatar.png`
///   so updating the character package never overwrites user customizations and Reset can
///   restore the package's original artwork.
/// - For legacy v1 folders without `manifest.toml`, writes to `AVATAR.png`.
///
/// # Errors
/// Returns an error if the image bytes cannot be atomically written to disk.
pub fn save_custom_character_avatar(char_dir: &Path, png_bytes: &[u8]) -> Result<PathBuf> {
    let slug = char_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let has_v2_manifest = char_dir.join("manifest.toml").is_file();
    let has_lowercase_pkg_avatar =
        char_dir.join("avatar.png").is_file() || char_dir.join("avatar.webp").is_file();

    let target = if has_v2_manifest || has_lowercase_pkg_avatar || bundled_default_avatar(slug).is_some() {
        // Remove any stale `.webp` custom override so `custom_avatar.png` wins deterministically.
        let _ = fs::remove_file(char_dir.join(CUSTOM_AVATAR_WEBP));
        char_dir.join(CUSTOM_AVATAR_PNG)
    } else {
        char_dir.join("AVATAR.png")
    };

    atomic_write(&target, png_bytes)
        .with_context(|| format!("Failed to save custom avatar at {}", target.display()))?;
    Ok(target)
}

/// Saves a user-selected color override for `char_dir`:
/// - When `manifest.toml` is present (Characters v2 package), stores `color` in `custom.toml`
///   so neither `manifest.toml` nor `CHARACTER.toml` is mutated during package updates.
/// - When `manifest.toml` is absent (legacy v1 character), patches `color = "..."` in-place
///   inside `CHARACTER.toml` (recording `default_color` on first edit).
///
/// # Errors
/// Returns an error if reading, parsing, or atomically writing the configuration fails.
pub fn save_character_color(char_dir: &Path, new_color: &str) -> Result<()> {
    let slug = char_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();

    if char_dir.join("manifest.toml").is_file() {
        let pkg_default = character_default_color(slug, char_dir);
        let override_val = match pkg_default.as_deref() {
            Some(def) if def.eq_ignore_ascii_case(new_color) => None,
            _ => Some(new_color),
        };
        return write_custom_color_override(char_dir, override_val);
    }

    let toml_path = char_dir.join("CHARACTER.toml");
    let content = fs::read_to_string(&toml_path)
        .with_context(|| format!("Failed to read CHARACTER.toml at {}", toml_path.display()))?;

    let is_bundled = bundled_default_color(slug).is_some();
    let parsed_existing: CharacterToml = toml::from_str(&content)
        .with_context(|| format!("Failed to parse CHARACTER.toml at {}", toml_path.display()))?;

    let current_color = parsed_existing
        .color
        .as_deref()
        .unwrap_or(FALLBACK_DEFAULT_COLOR);
    if current_color.eq_ignore_ascii_case(new_color) {
        return Ok(());
    }

    let extra_default = if !is_bundled && parsed_existing.default_color.is_none() {
        Some(("default_color", current_color))
    } else {
        None
    };

    let patched = patch_top_level_string_key(&content, "color", new_color, extra_default, None)?;
    atomic_write(&toml_path, patched)
        .with_context(|| format!("Failed to write CHARACTER.toml at {}", toml_path.display()))
}

/// Returns `true` if the character at `char_dir` has a user-customised color override
/// (`custom.toml` on v2 packages, or a modified `CHARACTER.toml` `color` on legacy/bundled dirs).
#[must_use]
pub fn has_custom_character_color(slug: &str, char_dir: &Path) -> bool {
    if let Some(custom_color) = read_custom_overrides(char_dir).color {
        let default_col = character_default_color(slug, char_dir);
        if default_col
            .as_deref()
            .is_none_or(|d| !d.eq_ignore_ascii_case(&custom_color))
        {
            return true;
        }
    }

    if let Ok(toml_str) = fs::read_to_string(char_dir.join("CHARACTER.toml"))
        && let Ok(parsed) = toml::from_str::<CharacterToml>(&toml_str)
        && let Some(ref toml_color) = parsed.color
    {
        if let Some(default_color) = bundled_default_color(slug) {
            return !toml_color.eq_ignore_ascii_case(default_color);
        }
        if let Some(default_color) = parsed.default_color {
            return !toml_color.eq_ignore_ascii_case(&default_color);
        }
    }
    false
}

/// Resets only a character's color override back to its package/bundled default without
/// touching `avatar` or `portrait` files, returning the restored hex color if known.
///
/// # Errors
/// Returns an error if removing `custom.toml` or restoring `CHARACTER.toml` fails.
pub fn reset_character_color(slug: &str, char_dir: &Path) -> Result<Option<String>> {
    write_custom_color_override(char_dir, None)?;
    let restored = character_default_color(slug, char_dir);

    let has_v2_manifest = char_dir.join("manifest.toml").is_file();
    if let Some(ref default_color) = restored
        && !has_v2_manifest
        && char_dir.join("CHARACTER.toml").is_file()
    {
        save_character_color(char_dir, default_color)?;
    } else if let Some(bundled_col) = bundled_default_color(slug)
        && char_dir.join("CHARACTER.toml").is_file()
    {
        // Also repair direct legacy edits to CHARACTER.toml on bundled characters.
        let toml_path = char_dir.join("CHARACTER.toml");
        if let Ok(content) = fs::read_to_string(&toml_path)
            && let Ok(parsed) = toml::from_str::<CharacterToml>(&content)
            && parsed
                .color
                .as_deref()
                .is_some_and(|c| !c.eq_ignore_ascii_case(bundled_col))
            && let Ok(patched) = patch_top_level_string_key(&content, "color", bundled_col, None, None)
        {
            atomic_write(&toml_path, patched)?;
        }
    }

    Ok(restored)
}

/// Returns `true` if the character at `char_dir` has a user-customised avatar, portrait, or color.
#[must_use]
pub fn has_custom_character_avatar(slug: &str, char_dir: &Path) -> bool {
    if char_dir.join(CUSTOM_AVATAR_PNG).is_file()
        || char_dir.join(CUSTOM_AVATAR_WEBP).is_file()
        || char_dir.join(CUSTOM_PORTRAIT_WEBP).is_file()
        || char_dir.join(CUSTOM_PORTRAIT_PNG).is_file()
    {
        return true;
    }

    let has_v2_manifest = char_dir.join("manifest.toml").is_file();
    let avatar_path = char_dir.join("AVATAR.png");
    let avatar_custom = bundled_default_avatar(slug).map_or_else(
        // For v2 community packs (`manifest.toml` present), `AVATAR.png` / `avatar.png` is
        // part of the author's package rather than a user override (`custom_avatar.png` is used).
        || !has_v2_manifest && avatar_path.is_file(),
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

    avatar_custom || has_custom_character_color(slug, char_dir)
}

/// Resets a character's local user customizations (`custom_*`, color, and avatar) to package defaults:
/// - Removes `custom_avatar.*`, `custom_portrait.*`, and `custom.toml`.
/// - For the three bundled characters (`niko-una`, `levi-kujo`, `kuro`), restores bundled
///   `AVATAR.png` and default color, returning `Ok(Some(Vec<u8>))`.
/// - For v2 community packages (`manifest.toml` present), preserves the package's own
///   `avatar.png` / `AVATAR.png` and returns its bytes if present (`Ok(Some(Vec<u8>))`).
/// - For legacy v1 custom characters (no `manifest.toml`), removes `AVATAR.png` so the UI
///   falls back to Adwaita initials and restores `default_color`, returning `Ok(None)`.
///
/// # Errors
/// Returns an error if removing overrides or restoring default files fails.
pub fn reset_character_avatar(slug: &str, char_dir: &Path) -> Result<Option<Vec<u8>>> {
    for rel in [
        CUSTOM_AVATAR_PNG,
        CUSTOM_AVATAR_WEBP,
        CUSTOM_PORTRAIT_WEBP,
        CUSTOM_PORTRAIT_PNG,
    ] {
        let p = char_dir.join(rel);
        if let Err(e) = fs::remove_file(&p)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(e).with_context(|| format!("Failed to remove {}", p.display()));
        }
    }
    let _ = reset_character_color(slug, char_dir)?;

    let has_v2_manifest = char_dir.join("manifest.toml").is_file();
    let avatar_path = char_dir.join("AVATAR.png");
    if let Some(default_bytes) = bundled_default_avatar(slug) {
        atomic_write(&avatar_path, default_bytes).with_context(|| {
            format!(
                "Failed to restore default avatar at {}",
                avatar_path.display()
            )
        })?;
        if let Some(default_portrait) = bundled_default_portrait(slug) {
            let _ = atomic_write(&char_dir.join("portrait.webp"), default_portrait);
        }
        return Ok(Some(default_bytes.to_vec()));
    }

    if has_v2_manifest {
        let man = read_manifest_strict(slug, char_dir);
        if let Some(pkg_rel) = resolve_package_base_avatar_rel(char_dir, man.as_ref())
            && let Ok(pkg_bytes) = fs::read(char_dir.join(pkg_rel))
        {
            return Ok(Some(pkg_bytes));
        }
        return Ok(None);
    }

    if let Err(e) = fs::remove_file(&avatar_path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        return Err(e).with_context(|| format!("Failed to remove avatar at {}", avatar_path.display()));
    }
    Ok(None)
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
    let _ = atomic_write(&base_dir.join("README.md"), SEEDED_CHARACTERS_README);

    for bundled in BUNDLED_CHARACTERS {
        let dir = base_dir.join(bundled.slug);
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create {} directory", bundled.slug))?;
        atomic_write(&dir.join("manifest.toml"), bundled.manifest_bytes)
            .with_context(|| format!("Failed to write {} manifest.toml", bundled.slug))?;
        atomic_write(&dir.join("CHARACTER.toml"), bundled.toml_bytes)
            .with_context(|| format!("Failed to write {} CHARACTER.toml", bundled.slug))?;
        atomic_write(&dir.join("AVATAR.png"), bundled.avatar_bytes)
            .with_context(|| format!("Failed to write {} AVATAR.png", bundled.slug))?;
        atomic_write(&dir.join("portrait.webp"), bundled.portrait_bytes)
            .with_context(|| format!("Failed to write {} portrait.webp", bundled.slug))?;
    }

    Ok(())
}

/// Loads all character configurations from the active config's characters directory.
///
/// # Errors
/// Returns an error if the characters directory cannot be created or read.
pub fn load_characters() -> Result<Vec<CharacterInfo>> {
    let base_dir = get_characters_dir()?;
    let chars = load_characters_in(&base_dir)?;
    sync_characters_readme_once(&base_dir);
    Ok(chars)
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
                        if path.join("CHARACTER.toml").exists() {
                            log::warn!("Skipping character at {name}: {e}");
                        } else {
                            // A directory may be mid-creation while artwork or manifest.toml lands first.
                            log::debug!("Skipping incomplete character directory '{name}': {e}");
                        }
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

/// Loads a single character (v1 or v2) from `dir`.
fn load_character_from_dir(dir: &Path) -> Result<CharacterInfo> {
    let dir_name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .with_context(|| format!("Invalid character directory name: {}", dir.display()))?
        .to_string();

    let toml_path = dir.join("CHARACTER.toml");
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

    let manifest = load_or_seed_manifest(dir, &dir_name);
    let assets = resolve_character_assets(dir, &dir_name, manifest.as_ref());
    let custom_overrides = read_custom_overrides(dir);

    let display_name = if raw.display_name.trim().is_empty() {
        dir_name.clone()
    } else {
        raw.display_name
    };

    // Color priority: `custom.toml` user override -> `CHARACTER.toml` (if customized on v1
    // or explicitly differing from bundled default) -> `manifest.toml` `[theme].accent_color` -> fallback.
    let color = custom_overrides
        .color
        .as_deref()
        .map(str::trim)
        .filter(|c| is_valid_hex_color(c))
        .map(str::to_string)
        .or_else(|| {
            raw.color
                .as_deref()
                .map(str::trim)
                .filter(|c| is_valid_hex_color(c))
                .map(str::to_string)
        })
        .or_else(|| {
            manifest
                .as_ref()
                .and_then(|m| m.theme.accent_color.clone())
        })
        .unwrap_or_else(|| FALLBACK_DEFAULT_COLOR.to_string());

    let greetings = if raw.greetings.is_empty() {
        bundled_default_greetings(&dir_name).map_or_else(Vec::new, |defaults| {
            defaults.iter().map(|s| (*s).to_string()).collect()
        })
    } else {
        raw.greetings
    };

    let config = CharacterConfig {
        id,
        name: dir_name,
        display_name,
        color,
        duties: raw.duties,
        personality: raw.personality,
        greetings,
    };

    Ok(CharacterInfo {
        config,
        status: CharacterStatus::Available,
        has_avatar: assets.avatar_rel_path.is_some(),
        has_portrait: assets.portrait_rel_path.is_some(),
        avatar_rel_path: assets.avatar_rel_path,
        portrait_rel_path: assets.portrait_rel_path,
        manifest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bundled::find_bundled;
    use manifest::parse_semver_triplet;

    #[test]
    fn bundled_characters_table_matches_embedded_toml_and_manifests() {
        for b in BUNDLED_CHARACTERS {
            let toml_str = std::str::from_utf8(b.toml_bytes).unwrap();
            let parsed: CharacterToml = toml::from_str(toml_str).unwrap();
            assert_eq!(parsed.color.as_deref(), Some(b.default_color));
            assert_ne!(parsed.greetings.len(), 0);
            assert_ne!(b.avatar_bytes, b"" as &[u8]);
            assert_ne!(b.portrait_bytes, b"" as &[u8]);

            let man_str = std::str::from_utf8(b.manifest_bytes).unwrap();
            let man = parse_and_sanitize_manifest(man_str, b.slug).unwrap();
            assert_eq!(man.schema_version, 2);
            assert_eq!(parse_semver_triplet(&man.package.version), Some(b.version));
            assert_eq!(man.assets.portrait.as_deref(), Some("portrait.webp"));
            assert_eq!(man.theme.accent_color.as_deref(), Some(b.default_color));
        }
        assert_ne!(incognito_avatar_bytes(), b"" as &[u8]);
        assert_ne!(incognito_portrait_bytes(), b"" as &[u8]);
    }

    #[test]
    fn bundled_character_version_upgrade_skips_deleted_and_preserves_user_state() {
        assert_eq!(parse_semver_triplet("2.0.0"), Some((2, 0, 0)));
        assert_eq!(parse_semver_triplet("v2.0.1-beta.1+git"), Some((2, 0, 1)));
        assert_eq!(parse_semver_triplet("2.0"), None);

        let tmp = tempfile::tempdir().unwrap();
        let base_dir = tmp.path().join("chars");

        // Simulate an existing v2.0.0 user who kept `niko-una` and `levi-kujo`,
        // customized `levi-kujo` with `custom_avatar.png` + `custom.toml`, and deleted `kuro`.
        let niko_dir = base_dir.join("niko-una");
        let levi_dir = base_dir.join("levi-kujo");
        fs::create_dir_all(&niko_dir).unwrap();
        fs::create_dir_all(&levi_dir).unwrap();

        let old_niko_manifest = std::str::from_utf8(bundled_default_manifest("niko-una").unwrap())
            .unwrap()
            .replace("version = \"2.0.1\"", "version = \"2.0.0\"");
        let preserved_niko_toml = "id = \"11111111-2222-4333-8444-555555555555\"\ndisplay_name = \"Niko\"\ncolor = \"#8035D4\"\nduties = \"Custom Duties\"\npersonality = \"User-tweaked prompt\"\n";
        fs::write(niko_dir.join("manifest.toml"), old_niko_manifest).unwrap();
        fs::write(niko_dir.join("CHARACTER.toml"), preserved_niko_toml).unwrap();
        fs::write(niko_dir.join("AVATAR.png"), b"old-2.0.0-avatar-bytes").unwrap();
        fs::write(
            niko_dir.join("portrait.webp"),
            bundled_default_portrait("niko-una").unwrap(),
        )
        .unwrap();

        let old_levi_manifest = std::str::from_utf8(bundled_default_manifest("levi-kujo").unwrap())
            .unwrap()
            .replace("version = \"2.0.1\"", "version = \"2.0.0\"");
        fs::write(levi_dir.join("manifest.toml"), old_levi_manifest).unwrap();
        fs::write(
            levi_dir.join("CHARACTER.toml"),
            find_bundled("levi-kujo").unwrap().toml_bytes,
        )
        .unwrap();
        fs::write(levi_dir.join("AVATAR.png"), b"old-2.0.0-levi-avatar").unwrap();
        fs::write(
            levi_dir.join("portrait.webp"),
            bundled_default_portrait("levi-kujo").unwrap(),
        )
        .unwrap();
        save_custom_character_avatar(&levi_dir, b"user-custom-levi-avatar").unwrap();
        save_character_color(&levi_dir, "#12AB34").unwrap();

        let loaded = load_characters_in(&base_dir).unwrap();

        // 1. Deleted `kuro` must NOT be recreated.
        assert_eq!(loaded.len(), 2);
        assert!(!base_dir.join("kuro").exists());

        // 2. `niko-una` upgraded to 2.0.1 with new AVATAR.png while keeping UUID & CHARACTER.toml.
        let niko = loaded.iter().find(|c| c.config.name == "niko-una").unwrap();
        assert_eq!(
            niko.manifest.as_ref().unwrap().package.version.as_str(),
            "2.0.1"
        );
        assert_eq!(niko.config.id, "11111111-2222-4333-8444-555555555555");
        assert_eq!(niko.config.personality, "User-tweaked prompt");
        assert_eq!(
            fs::read(niko_dir.join("AVATAR.png")).unwrap().as_slice(),
            bundled_default_avatar("niko-una").unwrap()
        );
        assert_eq!(
            fs::read_to_string(niko_dir.join("CHARACTER.toml")).unwrap(),
            preserved_niko_toml
        );

        // 3. `levi-kujo` upgraded base AVATAR.png & manifest to 2.0.1 while keeping custom overrides.
        let levi = loaded
            .iter()
            .find(|c| c.config.name == "levi-kujo")
            .unwrap();
        assert_eq!(
            levi.manifest.as_ref().unwrap().package.version.as_str(),
            "2.0.1"
        );
        assert_eq!(levi.config.color, "#12AB34");
        assert_eq!(levi.avatar_rel_path.as_deref(), Some("custom_avatar.png"));
        assert_eq!(
            fs::read(levi_dir.join("AVATAR.png")).unwrap().as_slice(),
            bundled_default_avatar("levi-kujo").unwrap()
        );

        // 4. Newer on-disk version (e.g. `99.0.0`) is never downgraded by an older binary.
        let future_manifest = std::str::from_utf8(bundled_default_manifest("niko-una").unwrap())
            .unwrap()
            .replace("version = \"2.0.1\"", "version = \"99.0.0\"");
        fs::write(niko_dir.join("manifest.toml"), future_manifest).unwrap();
        fs::write(niko_dir.join("AVATAR.png"), b"future-99-avatar").unwrap();

        let reloaded = load_characters_in(&base_dir).unwrap();
        let niko_future = reloaded
            .iter()
            .find(|c| c.config.name == "niko-una")
            .unwrap();
        assert_eq!(
            niko_future
                .manifest
                .as_ref()
                .unwrap()
                .package
                .version
                .as_str(),
            "99.0.0"
        );
        assert_eq!(
            fs::read(niko_dir.join("AVATAR.png")).unwrap(),
            b"future-99-avatar"
        );
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
        assert_eq!(restored.as_deref(), Some(default_bytes));
        assert!(!has_custom_character_avatar("niko-una", &niko_dir));

        // 2. Legacy v1 4th custom character ("my-custom-agent", no manifest.toml)
        let custom_dir = tmp.path().join("my-custom-agent");
        fs::create_dir_all(&custom_dir).unwrap();
        assert!(!has_custom_character_avatar("my-custom-agent", &custom_dir));

        fs::write(custom_dir.join("AVATAR.png"), b"custom-png-bytes").unwrap();
        assert!(has_custom_character_avatar("my-custom-agent", &custom_dir));

        // Resetting removes AVATAR.png so v1 falls back to standard Adwaita initials
        let restored_custom = reset_character_avatar("my-custom-agent", &custom_dir).unwrap();
        assert!(restored_custom.is_none());
        assert!(!custom_dir.join("AVATAR.png").exists());
        assert!(!has_custom_character_avatar("my-custom-agent", &custom_dir));

        // 3. Color customization and reset on bundled and v1 custom characters
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

        // Single-line triple-quoted color preserves comments when patched in-place on v1
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
    fn v2_community_package_preserves_user_overrides_across_updates_and_resets_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        let base_dir = tmp.path().join("chars");
        let pkg_dir = base_dir.join("sora-dev");
        fs::create_dir_all(&pkg_dir).unwrap();

        let manifest_v1 = r##"
schema_version = 2

[package]
version = "1.0.0"
authors = ["community-author"]
description = "Calm systems mentor"

[assets]
avatar = "avatar.png"
portrait = "portrait.webp"

[theme]
accent_color = "#4E9A06"
"##;
        let char_v1 = r#"
id = "00000010-0000-4000-8000-000000000010"
display_name = "Sora"
duties = "Kernel Mentor"
personality = "Calm and thorough."
greetings = ["Ready to inspect the trace."]
"#;
        fs::write(pkg_dir.join("manifest.toml"), manifest_v1).unwrap();
        fs::write(pkg_dir.join("CHARACTER.toml"), char_v1).unwrap();
        fs::write(pkg_dir.join("avatar.png"), b"pkg-avatar-v1").unwrap();
        fs::write(pkg_dir.join("portrait.webp"), b"pkg-portrait-v1").unwrap();

        let loaded = load_characters_in(&base_dir).unwrap();
        assert_eq!(loaded.len(), 1);
        let sora = &loaded[0];
        assert!(sora.has_avatar);
        assert!(sora.has_portrait);
        assert_eq!(sora.avatar_rel_path.as_deref(), Some("avatar.png"));
        assert_eq!(sora.portrait_rel_path.as_deref(), Some("portrait.webp"));
        assert_eq!(sora.config.color, "#4E9A06");
        assert_eq!(
            sora.package_attribution().as_deref(),
            Some("v1.0.0 · by community-author")
        );
        assert_eq!(
            sora.deterministic_greeting("pane-1"),
            Some("Ready to inspect the trace.")
        );
        assert!(!has_custom_character_avatar("sora-dev", &pkg_dir));
        assert!(!has_custom_character_color("sora-dev", &pkg_dir));

        // User customizes color in Preferences (plus optional custom_avatar.png on disk) -> stored non-destructively!
        save_custom_character_avatar(&pkg_dir, b"user-cropped-avatar").unwrap();
        save_character_color(&pkg_dir, "#FF5500").unwrap();
        assert!(has_custom_character_color("sora-dev", &pkg_dir));
        assert!(has_custom_character_avatar("sora-dev", &pkg_dir));
        assert_eq!(fs::read(pkg_dir.join("avatar.png")).unwrap(), b"pkg-avatar-v1");

        // Simulate community author shipping v1.1.0 (overwriting manifest, CHARACTER.toml, avatar, portrait)
        let manifest_v2 = manifest_v1.replace("1.0.0", "1.1.0");
        fs::write(pkg_dir.join("manifest.toml"), manifest_v2).unwrap();
        fs::write(pkg_dir.join("avatar.png"), b"pkg-avatar-v2").unwrap();

        let after_update = load_characters_in(&base_dir).unwrap();
        let sora_updated = &after_update[0];
        assert_eq!(sora_updated.config.color, "#FF5500");
        assert_eq!(
            sora_updated.avatar_rel_path.as_deref(),
            Some("custom_avatar.png")
        );
        assert_eq!(
            sora_updated.package_attribution().as_deref(),
            Some("v1.1.0 · by community-author")
        );

        // Resetting only color restores package accent_color while preserving custom_avatar.png
        let restored_col = reset_character_color("sora-dev", &pkg_dir).unwrap();
        assert_eq!(restored_col.as_deref(), Some("#4E9A06"));
        assert!(!has_custom_character_color("sora-dev", &pkg_dir));
        assert!(pkg_dir.join("custom_avatar.png").is_file());

        // Full reset restores the updated v1.1.0 package avatar & theme color!
        let restored_bytes = reset_character_avatar("sora-dev", &pkg_dir).unwrap();
        assert_eq!(restored_bytes.as_deref(), Some(b"pkg-avatar-v2" as &[u8]));
        assert!(!has_custom_character_avatar("sora-dev", &pkg_dir));

        let after_reset = load_characters_in(&base_dir).unwrap();
        assert_eq!(after_reset[0].config.color, "#4E9A06");
        assert_eq!(after_reset[0].avatar_rel_path.as_deref(), Some("avatar.png"));
    }

    #[test]
    fn manifest_rejects_path_traversal_and_absolute_asset_paths() {
        assert!(validate_relative_asset_path("../secret.png", true).is_err());
        assert!(validate_relative_asset_path("sub/../../secret.png", true).is_err());
        assert!(validate_relative_asset_path("/etc/passwd", false).is_err());
        assert!(validate_relative_asset_path("avatar.exe", true).is_err());
        assert_eq!(
            validate_relative_asset_path("assets/portrait.webp", true).unwrap(),
            "assets/portrait.webp"
        );
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
