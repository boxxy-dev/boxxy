use crate::characters::{CharacterConfig, CharacterInfo};
use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::Deserialize;
use serde_json;
use std::fs;
use std::path::{Path, PathBuf};

/// Intermediate struct for TOML parsing — `id` is optional so we can detect
/// first-time loads and auto-generate a UUID. `name` is intentionally absent
/// since we always derive it from the directory name.
#[derive(Deserialize)]
struct CharacterToml {
    id: Option<String>,
    display_name: String,
    color: String,
    duties: String,
    personality: String,
}

/// Returns the base directory for characters: ~/.config/boxxy-terminal/boxxyclaw/characters/
pub fn get_characters_dir() -> Result<PathBuf> {
    let proj_dirs = ProjectDirs::from("org", "boxxy", "boxxy-terminal")
        .context("Could not determine project directories")?;
    let config_dir = proj_dirs.config_dir();
    Ok(config_dir.join("boxxyclaw").join("characters"))
}

/// Bundled character resources
const BUNDLED_JSON: &[u8] = include_bytes!("../../../resources/characters/characters.json");

const NIKO_TOML: &[u8] = include_bytes!("../../../resources/characters/niko-una/CHARACTER.toml");
const NIKO_AVATAR: &[u8] = include_bytes!("../../../resources/characters/niko-una/AVATAR.png");

const LEVI_TOML: &[u8] = include_bytes!("../../../resources/characters/levi-kujo/CHARACTER.toml");
const LEVI_AVATAR: &[u8] = include_bytes!("../../../resources/characters/levi-kujo/AVATAR.png");

const KURO_TOML: &[u8] = include_bytes!("../../../resources/characters/kuro/CHARACTER.toml");
const KURO_AVATAR: &[u8] = include_bytes!("../../../resources/characters/kuro/AVATAR.png");

/// Ensures the characters directory exists and has at least one valid character.
/// A valid character is a subdirectory containing a `CHARACTER.toml` file.
/// If the user has customised the directory (even a single valid character exists),
/// the bundled defaults are never re-extracted.
pub fn ensure_default_character() -> Result<()> {
    let base_dir = get_characters_dir()?;
    if !base_dir.exists() {
        fs::create_dir_all(&base_dir).context("Failed to create characters directory")?;
    }

    let has_valid = has_any_valid_character(&base_dir);
    if !has_valid {
        extract_bundled_defaults(&base_dir)?;
    }

    Ok(())
}

/// Removes all existing character directories and re-extracts the three bundled defaults.
pub fn reset_to_defaults() -> Result<()> {
    let base_dir = get_characters_dir()?;

    if base_dir.exists() {
        for entry in fs::read_dir(&base_dir).context("Failed to read characters directory")? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                fs::remove_dir_all(&path)
                    .with_context(|| format!("Failed to remove {:?}", path))?;
            }
        }
        // Remove order file so it gets rewritten cleanly.
        let _ = fs::remove_file(base_dir.join("characters.json"));
    }

    extract_bundled_defaults(&base_dir)
}

fn has_any_valid_character(base_dir: &std::path::Path) -> bool {
    fs::read_dir(base_dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .any(|e| e.path().is_dir() && e.path().join("CHARACTER.toml").exists())
        })
        .unwrap_or(false)
}

fn extract_bundled_defaults(base_dir: &std::path::Path) -> Result<()> {
    fs::write(base_dir.join("characters.json"), BUNDLED_JSON)
        .context("Failed to write bundled characters.json")?;
    // Recreate the bundled characters
    let niko_dir = base_dir.join("niko-una");
    fs::create_dir_all(&niko_dir).context("Failed to create niko directory")?;
    fs::write(niko_dir.join("CHARACTER.toml"), NIKO_TOML)
        .context("Failed to write niko CHARACTER.toml")?;
    fs::write(niko_dir.join("AVATAR.png"), NIKO_AVATAR)
        .context("Failed to write niko AVATAR.png")?;

    let levi_dir = base_dir.join("levi-kujo");
    fs::create_dir_all(&levi_dir).context("Failed to create levi directory")?;
    fs::write(levi_dir.join("CHARACTER.toml"), LEVI_TOML)
        .context("Failed to write levi CHARACTER.toml")?;
    fs::write(levi_dir.join("AVATAR.png"), LEVI_AVATAR)
        .context("Failed to write levi AVATAR.png")?;

    let kuro_dir = base_dir.join("kuro");
    fs::create_dir_all(&kuro_dir).context("Failed to create kuro directory")?;
    fs::write(kuro_dir.join("CHARACTER.toml"), KURO_TOML)
        .context("Failed to write kuro CHARACTER.toml")?;
    fs::write(kuro_dir.join("AVATAR.png"), KURO_AVATAR)
        .context("Failed to write kuro AVATAR.png")?;

    Ok(())
}

/// Loads all character configurations from the characters directory.
/// Each character must have its own subdirectory containing a CHARACTER.toml.
pub fn load_characters() -> Result<Vec<CharacterInfo>> {
    let _ = ensure_default_character();

    let base_dir = get_characters_dir()?;

    if !base_dir.exists() {
        return Ok(Vec::new());
    }

    let mut characters = Vec::new();

    for entry in fs::read_dir(&base_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            match load_character_from_dir(&path) {
                Ok(info) => characters.push(info),
                Err(e) => {
                    log::warn!(
                        "Skipping character at {:?}: {}",
                        path.file_name().unwrap_or_default(),
                        e
                    );
                }
            }
        }
    }

    // Guard against duplicate UUIDs (e.g., user copied a character directory).
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for character in &mut characters {
        loop {
            if seen_ids.insert(character.config.id.clone()) {
                break;
            }
            // Collision — regenerate and rewrite the TOML.
            let new_id = uuid::Uuid::new_v4().to_string();
            let toml_path = base_dir.join(&character.config.name).join("CHARACTER.toml");
            if let Ok(content) = fs::read_to_string(&toml_path) {
                let new_content = content
                    .lines()
                    .map(|line| {
                        if line.trim_start().starts_with("id =") {
                            format!("id = \"{}\"", new_id)
                        } else {
                            line.to_string()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let _ = fs::write(&toml_path, new_content);
            }
            character.config.id = new_id;
        }
    }

    // Try to sort them by characters.json if it exists
    let json_path = base_dir.join("characters.json");
    if json_path.exists() {
        if let Ok(content) = fs::read_to_string(json_path) {
            if let Ok(order) = serde_json::from_str::<Vec<String>>(&content) {
                characters.sort_by(|a, b| {
                    let pos_a = order
                        .iter()
                        .position(|name| name == &a.config.name)
                        .unwrap_or(usize::MAX);
                    let pos_b = order
                        .iter()
                        .position(|name| name == &b.config.name)
                        .unwrap_or(usize::MAX);
                    pos_a.cmp(&pos_b)
                });
            }
        }
    }

    Ok(characters)
}

/// Persists the character order to characters.json.
/// The order is a list of character names (directory names).
pub fn save_character_order(order: Vec<String>) -> Result<()> {
    let base_dir = get_characters_dir()?;
    let json_path = base_dir.join("characters.json");
    let content = serde_json::to_string_pretty(&order)?;
    fs::write(json_path, content).context("Failed to write character order")?;
    Ok(())
}

/// Loads a single character from a directory.
/// Expects a CHARACTER.toml and optionally an AVATAR.png.
/// Auto-generates a UUID and writes it back to the TOML if one is absent.
fn load_character_from_dir(dir: &Path) -> Result<CharacterInfo> {
    let toml_path = dir.join("CHARACTER.toml");
    let avatar_path = dir.join("AVATAR.png");

    let toml_content = fs::read_to_string(&toml_path)
        .with_context(|| format!("Failed to read CHARACTER.toml in {:?}", dir))?;

    let raw: CharacterToml = toml::from_str(&toml_content)
        .with_context(|| format!("Failed to parse CHARACTER.toml in {:?}", dir))?;

    let id = match raw.id.filter(|s| !s.is_empty()) {
        Some(id) => id,
        None => {
            let new_id = uuid::Uuid::new_v4().to_string();
            let new_content = format!(
                "# Stable identity — do not change this value\nid = \"{}\"\n\n{}",
                new_id, toml_content
            );
            fs::write(&toml_path, &new_content)
                .with_context(|| format!("Failed to write UUID to CHARACTER.toml in {:?}", dir))?;
            new_id
        }
    };

    let dir_name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    let config = CharacterConfig {
        id,
        name: dir_name,
        display_name: raw.display_name,
        color: raw.color,
        duties: raw.duties,
        personality: raw.personality,
    };

    Ok(CharacterInfo::new(config, avatar_path.exists()))
}
