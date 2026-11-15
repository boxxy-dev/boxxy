//! Disk I/O, atomic TOML updates, and bundled-default extraction for the Character catalog.

use crate::characters::{CharacterConfig, CharacterInfo};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Intermediate struct for TOML parsing — `id` is optional so we can detect
/// first-time loads and auto-generate a UUID. `name` is intentionally absent
/// since we always derive it from the directory name.
#[derive(Deserialize, Serialize)]
struct CharacterToml {
    id: Option<String>,
    display_name: String,
    color: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    default_color: Option<String>,
    duties: String,
    personality: String,
}

struct BundledCharacter {
    slug: &'static str,
    default_color: &'static str,
    toml_bytes: &'static [u8],
    avatar_bytes: &'static [u8],
}

const BUNDLED_JSON: &[u8] = include_bytes!("../../../resources/characters/characters.json");

const INCOGNITO_AVATAR: &[u8] =
    include_bytes!("../../../resources/characters/inco-gnito/AVATAR.png");

const BUNDLED_CHARACTERS: &[BundledCharacter] = &[
    BundledCharacter {
        slug: "niko-una",
        default_color: "#8035D4",
        toml_bytes: include_bytes!("../../../resources/characters/niko-una/CHARACTER.toml"),
        avatar_bytes: include_bytes!("../../../resources/characters/niko-una/AVATAR.png"),
    },
    BundledCharacter {
        slug: "levi-kujo",
        default_color: "#204A87",
        toml_bytes: include_bytes!("../../../resources/characters/levi-kujo/CHARACTER.toml"),
        avatar_bytes: include_bytes!("../../../resources/characters/levi-kujo/AVATAR.png"),
    },
    BundledCharacter {
        slug: "kuro",
        default_color: "#B31825",
        toml_bytes: include_bytes!("../../../resources/characters/kuro/CHARACTER.toml"),
        avatar_bytes: include_bytes!("../../../resources/characters/kuro/AVATAR.png"),
    },
];

fn find_bundled(slug: &str) -> Option<&'static BundledCharacter> {
    BUNDLED_CHARACTERS.iter().find(|b| b.slug == slug)
}

/// Writes `contents` to a hidden `.tmp` sibling in the same directory and atomically
/// renames it over `path` so concurrent readers and `CharacterWatcher` never observe
/// truncated files.
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
    let tmp_path = parent.join(format!(
        ".{file_name}.{}.{seq}.tmp",
        std::process::id()
    ));

    fs::write(&tmp_path, contents.as_ref())
        .with_context(|| format!("Failed to write temporary file {}", tmp_path.display()))?;
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

/// Returns the bundled 256×256 PNG avatar bytes for the `/incognito` persona (`Inco Gnito`).
#[must_use]
pub const fn incognito_avatar_bytes() -> &'static [u8] {
    INCOGNITO_AVATAR
}

/// Returns the bundled default PNG avatar bytes for the three built-in characters
/// (`niko-una`, `levi-kujo`, `kuro`), or `None` for user-created characters.
#[must_use]
pub fn bundled_default_avatar(slug: &str) -> Option<&'static [u8]> {
    find_bundled(slug).map(|b| b.avatar_bytes)
}

/// Returns the bundled default hex color for the three built-in characters
/// (`niko-una`, `levi-kujo`, `kuro`), or `None` for user-created characters.
#[must_use]
pub fn bundled_default_color(slug: &str) -> Option<&'static str> {
    find_bundled(slug).map(|b| b.default_color)
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum TomlScanState {
    Normal,
    MultiBasic,
    MultiLiteral,
}

/// Advances the multi-line TOML string lexer across a single line so top-level key matching
/// never mistakes lines inside `"""..."""` or `'''...'''` blocks for top-level keys.
fn advance_toml_scan_state(line: &str, mut state: TomlScanState) -> TomlScanState {
    let bytes = line.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        match state {
            TomlScanState::Normal => match bytes[i] {
                b'#' => break,
                b'"' => {
                    if bytes[i..].starts_with(b"\"\"\"") {
                        state = TomlScanState::MultiBasic;
                        i += 3;
                    } else {
                        i += 1;
                        while i < bytes.len() {
                            if bytes[i] == b'\\' {
                                i += 2;
                            } else if bytes[i] == b'"' {
                                i += 1;
                                break;
                            } else {
                                i += 1;
                            }
                        }
                    }
                }
                b'\'' => {
                    if bytes[i..].starts_with(b"'''") {
                        state = TomlScanState::MultiLiteral;
                        i += 3;
                    } else {
                        i += 1;
                        while i < bytes.len() {
                            if bytes[i] == b'\'' {
                                i += 1;
                                break;
                            }
                            i += 1;
                        }
                    }
                }
                _ => i += 1,
            },
            TomlScanState::MultiBasic => {
                if bytes[i] == b'\\' {
                    i += 2;
                } else if bytes[i..].starts_with(b"\"\"\"") {
                    i += 3;
                    // Consume up to 2 extra quotes in 4- or 5-quote closing sequences (`""""` / `"""""`).
                    let mut extra = 0;
                    while extra < 2 && i < bytes.len() && bytes[i] == b'"' {
                        i += 1;
                        extra += 1;
                    }
                    state = TomlScanState::Normal;
                } else {
                    i += 1;
                }
            }
            TomlScanState::MultiLiteral => {
                if bytes[i..].starts_with(b"'''") {
                    i += 3;
                    let mut extra = 0;
                    while extra < 2 && i < bytes.len() && bytes[i] == b'\'' {
                        i += 1;
                        extra += 1;
                    }
                    state = TomlScanState::Normal;
                } else {
                    i += 1;
                }
            }
        }
    }

    state
}

/// Checks if `trimmed_line` (known to be in `TomlScanState::Normal` outside any `[table]`)
/// assigns a single-line value to `target_key`.
fn matches_top_level_single_line_key(trimmed_line: &str, target_key: &str) -> bool {
    let Some(after_key) = trimmed_line.strip_prefix(target_key) else {
        return false;
    };
    let after_ws = after_key.trim_start_matches([' ', '\t']);
    let Some(after_eq) = after_ws.strip_prefix('=') else {
        return false;
    };
    let rhs = after_eq.trim_start_matches([' ', '\t']);
    !rhs.starts_with("\"\"\"") && !rhs.starts_with("'''")
}

fn format_toml_string_assignment(key: &str, value: &str) -> String {
    let encoded = toml::Value::String(value.to_string()).to_string();
    format!("{key} = {encoded}")
}

/// Safely updates or inserts a top-level string key in `CHARACTER.toml` while preserving
/// comments, key order, and multi-line `"""..."""` prompts, and validates that the patched
/// document still parses as a valid [`CharacterToml`].
fn patch_top_level_string_key(
    content: &str,
    key: &str,
    new_value: &str,
    extra_insert_after: Option<(&str, &str)>,
    prepend_comment_if_inserted: Option<&str>,
) -> Result<String> {
    let mut state = TomlScanState::Normal;
    let mut in_subtable = false;
    let mut replaced = false;
    let mut out_lines: Vec<String> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim_start();
        if state == TomlScanState::Normal && trimmed.starts_with('[') {
            in_subtable = true;
        }

        if !replaced
            && state == TomlScanState::Normal
            && !in_subtable
            && matches_top_level_single_line_key(trimmed, key)
        {
            replaced = true;
            out_lines.push(format_toml_string_assignment(key, new_value));
            if let Some((extra_k, extra_v)) = extra_insert_after {
                out_lines.push(format_toml_string_assignment(extra_k, extra_v));
            }
            // Matched line is guaranteed single-line, so `state` stays `Normal`.
            continue;
        }

        state = advance_toml_scan_state(line, state);
        out_lines.push(line.to_string());
    }

    let mut out = if replaced {
        let mut joined = out_lines.join("\n");
        if content.ends_with('\n') || joined.is_empty() {
            joined.push('\n');
        }
        joined
    } else {
        let mut prefix = String::new();
        if let Some(comment) = prepend_comment_if_inserted {
            prefix.push_str(comment);
            prefix.push('\n');
        }
        prefix.push_str(&format_toml_string_assignment(key, new_value));
        if let Some((extra_k, extra_v)) = extra_insert_after {
            prefix.push('\n');
            prefix.push_str(&format_toml_string_assignment(extra_k, extra_v));
        }
        prefix.push_str("\n\n");
        prefix.push_str(content);
        if !prefix.ends_with('\n') {
            prefix.push('\n');
        }
        prefix
    };

    // Fallback safety net: if an exotic TOML construct ever prevented clean line-patching,
    // serialize via `toml::to_string_pretty` rather than writing invalid TOML to disk.
    if toml::from_str::<CharacterToml>(&out).is_err() {
        let mut parsed: CharacterToml =
            toml::from_str(content).context("Original CHARACTER.toml is invalid")?;
        match key {
            "id" => parsed.id = Some(new_value.to_string()),
            "color" => parsed.color = new_value.to_string(),
            _ => bail!("Unsupported fallback key: {key}"),
        }
        if let Some(("default_color", extra_v)) = extra_insert_after {
            parsed.default_color = Some(extra_v.to_string());
        }
        out = toml::to_string_pretty(&parsed).context("Failed to serialize fallback TOML")?;
    }

    Ok(out)
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
/// # Errors
/// Returns an error if removing existing directories or writing bundled defaults fails.
pub fn reset_to_defaults_in(base_dir: &Path) -> Result<()> {
    if base_dir.exists() {
        for entry in fs::read_dir(base_dir).context("Failed to read characters directory")? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                fs::remove_dir_all(&path)
                    .with_context(|| format!("Failed to remove {}", path.display()))?;
            }
        }
        let _ = fs::remove_file(base_dir.join("characters.json"));
    }

    extract_bundled_defaults(base_dir)
}

fn is_candidate_character_dir(path: &Path) -> bool {
    path.is_dir()
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| !name.is_empty() && !name.starts_with('.'))
}

fn has_any_valid_character(base_dir: &Path) -> bool {
    fs::read_dir(base_dir).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            let path = entry.path();
            is_candidate_character_dir(&path)
                && fs::read_to_string(path.join("CHARACTER.toml"))
                    .ok()
                    .and_then(|s| toml::from_str::<CharacterToml>(&s).ok())
                    .is_some()
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
        let path = entry.path();
        if !is_candidate_character_dir(&path) {
            continue;
        }
        match load_character_from_dir(&path) {
            Ok(info) => characters.push(info),
            Err(e) => {
                if warn_on_invalid {
                    let name = path
                        .file_name()
                        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                    log::warn!("Skipping character at {name}: {e}");
                }
            }
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
/// Auto-generates a UUID and writes it back to the TOML if absent or empty.
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
        .filter(|s| !s.is_empty())
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
        atomic_write(&toml_path, patched)
            .with_context(|| format!("Failed to write UUID to CHARACTER.toml in {}", dir.display()))?;
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

    let config = CharacterConfig {
        id,
        name: dir_name,
        display_name: raw.display_name,
        color: raw.color,
        duties: raw.duties,
        personality: raw.personality,
    };

    Ok(CharacterInfo::new(config, has_avatar))
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let custom_toml = "id = \"00000004-0000-4000-8000-000000000004\"\ndisplay_name = \"custom\"\ncolor = \"#abcdef\"\nduties = \"Helper\"\npersonality = \"Friendly\"\n";
        fs::write(custom_dir.join("CHARACTER.toml"), custom_toml).unwrap();
        assert!(!has_custom_character_avatar("my-custom-agent", &custom_dir));
        save_character_color(&custom_dir, "#fedcba").unwrap();
        assert!(has_custom_character_avatar("my-custom-agent", &custom_dir));
        assert_eq!(
            character_default_color("my-custom-agent", &custom_dir).as_deref(),
            Some("#abcdef")
        );
        reset_character_avatar("my-custom-agent", &custom_dir).unwrap();
        assert!(!has_custom_character_avatar("my-custom-agent", &custom_dir));
    }

    #[test]
    fn empty_id_and_duplicate_compact_id_repair_without_corrupting_multiline_prompts() {
        let tmp = tempfile::tempdir().unwrap();
        let base_dir = tmp.path();

        // Character 1: `id = ""` and a multi-line personality containing `id =` and `color =` lines
        let alpha_dir = base_dir.join("alpha");
        fs::create_dir_all(&alpha_dir).unwrap();
        fs::write(
            alpha_dir.join("CHARACTER.toml"),
            "id = \"\"\ndisplay_name = \"Alpha\"\ncolor = \"#112233\"\nduties = \"Ops\"\npersonality = \"\"\"\nNever rewrite lines like:\nid = \"keep-me\"\ncolor = \"#999999\"\n\"\"\"\n",
        )
        .unwrap();

        // First load repairs `id = ""` in-place -> second load must STILL succeed (no duplicate `id` key!)
        let loaded_first = load_characters_in(base_dir).unwrap();
        assert_eq!(loaded_first.len(), 1);
        let assigned_id = loaded_first[0].config.id.clone();
        assert_ne!(assigned_id, "");
        assert!(
            loaded_first[0]
                .config
                .personality
                .contains("id = \"keep-me\"")
        );
        assert!(
            loaded_first[0]
                .config
                .personality
                .contains("color = \"#999999\"")
        );

        let loaded_second = load_characters_in(base_dir).unwrap();
        assert_eq!(loaded_second.len(), 1);
        assert_eq!(loaded_second[0].config.id, assigned_id);

        // Character 2: copied directory with compact `id="..."` (no spaces around `=`)
        let beta_dir = base_dir.join("beta");
        fs::create_dir_all(&beta_dir).unwrap();
        fs::write(
            beta_dir.join("CHARACTER.toml"),
            format!(
                "id=\"{assigned_id}\"\ndisplay_name = \"Beta\"\ncolor=\"#445566\"\nduties = \"Dev\"\npersonality = \"Calm\"\n"
            ),
        )
        .unwrap();

        let deduped_first = load_characters_in(base_dir).unwrap();
        assert_eq!(deduped_first.len(), 2);
        assert_ne!(deduped_first[0].config.id, deduped_first[1].config.id);

        // Reloading again preserves the persisted deduplicated UUID on disk
        let beta_id = deduped_first
            .iter()
            .find(|c| c.config.name == "beta")
            .unwrap()
            .config
            .id
            .clone();
        let deduped_second = load_characters_in(base_dir).unwrap();
        let beta_id_after = deduped_second
            .iter()
            .find(|c| c.config.name == "beta")
            .unwrap()
            .config
            .id
            .clone();
        assert_eq!(beta_id, beta_id_after);

        // Custom order via `save_character_order_in`
        save_character_order_in(base_dir, &["beta".to_string(), "alpha".to_string()]).unwrap();
        let ordered = load_characters_in(base_dir).unwrap();
        assert_eq!(ordered[0].config.name, "beta");
        assert_eq!(ordered[1].config.name, "alpha");
    }
}
