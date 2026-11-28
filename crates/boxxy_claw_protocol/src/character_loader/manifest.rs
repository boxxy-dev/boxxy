//! Characters v2 `manifest.toml` validation, non-destructive `custom.toml` overrides,
//! relative asset resolution (`avatar` / `portrait`), and `+doctor` diagnostics.

use super::atomic_write;
use super::bundled::{
    bundled_default_avatar, bundled_default_portrait, find_bundled, legacy_v1_avatar_len,
    matches_legacy_v1_bundled_avatar,
};
use super::toml_patch::CharacterToml;
use crate::characters::{CHARACTER_SCHEMA_VERSION_V2, CharacterInfo, CharacterManifest};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

pub(super) const CUSTOM_OVERRIDES_FILE: &str = "custom.toml";
pub(super) const CUSTOM_AVATAR_PNG: &str = "custom_avatar.png";
pub(super) const CUSTOM_AVATAR_WEBP: &str = "custom_avatar.webp";
pub(super) const CUSTOM_PORTRAIT_WEBP: &str = "custom_portrait.webp";
pub(super) const CUSTOM_PORTRAIT_PNG: &str = "custom_portrait.png";

const AUTO_AVATAR_CANDIDATES: &[&str] = &["avatar.png", "avatar.webp", "AVATAR.png", "AVATAR.webp"];
const AUTO_PORTRAIT_CANDIDATES: &[&str] = &[
    "portrait.webp",
    "portrait.png",
    "PORTRAIT.webp",
    "PORTRAIT.png",
];
const ALLOWED_IMAGE_EXTS: &[&str] = &["png", "webp", "jpg", "jpeg"];

static README_SYNCED_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

pub(super) const SEEDED_CHARACTERS_README: &str = r##"# Boxxy Characters (v2)

Drop character folders into this directory to add custom or community AI companions to Boxxy!
Changes to `manifest.toml`, `CHARACTER.toml`, `avatar.png`, or `portrait.webp` apply automatically within 500 ms.

## Package Structure (`<slug>/`)

```text
my-character/
├── manifest.toml     # Package metadata, credits, asset paths & default theme color (v2)
├── CHARACTER.toml    # Persona, duties, personality prompt & greetings
├── avatar.png        # 256×256 (1:1 square) — chat rows, badges & lists
└── portrait.webp     # 768×432 (16:9 landscape, optional) — character cards & sidebar hero
```

Legacy v1 folders containing only `CHARACTER.toml` (and optional `AVATAR.png`) remain 100% supported.

## Minimal `manifest.toml`

```toml
schema_version = 2

[package]
version = "1.0.0"
authors = ["your-handle"]
description = "Calm Rust & kernel systems mentor."
license = "CC-BY-4.0"
tags = ["rust", "systems"]

[assets]
avatar = "avatar.png"       # 256×256 (1:1) — .png or .webp
portrait = "portrait.webp"  # 768×432 (16:9) — .webp or .png (optional)

[theme]
accent_color = "#4E9A06"
```

## Minimal `CHARACTER.toml`

```toml
display_name = "Sora"
duties = "Systems & Kernel Mentor"
personality = """
Calm, encouraging, and crystal clear. Explain root causes concisely before suggesting fixes."""

greetings = [
  "Terminal is ready. What are we building today?",
  "Point me at a compiler error or trace whenever you're ready.",
]
```

## Non-Destructive Customizations

When you customize a character's color inside **Preferences → Characters**, Boxxy saves your local override to `custom.toml` so updating a community character package never overwrites your preference. You can also place `custom_avatar.png` or `custom_portrait.webp` inside a character folder to override package artwork non-destructively.
"##;

/// User-local overrides stored in `<char_dir>/custom.toml` so updating a community
/// character's `manifest.toml` or `CHARACTER.toml` preserves user preferences.
#[derive(Debug, Default, Deserialize, Serialize)]
pub(super) struct CustomOverridesToml {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) color: Option<String>,
}

/// Resolved on-disk assets for a single character directory.
pub(super) struct ResolvedCharacterAssets {
    pub(super) avatar_rel_path: Option<String>,
    pub(super) portrait_rel_path: Option<String>,
}

/// Diagnostic result for a single character directory (`boxxy-terminal +doctor`).
#[derive(Debug, Clone)]
pub struct CharacterDirDiagnostic {
    pub slug: String,
    pub schema_version: u32,
    pub has_portrait: bool,
    pub warnings: Vec<String>,
    pub error: Option<String>,
}

/// Validates a hex color string (`#RGB`, `#RGBA`, `#RRGGBB`, `#RRGGBBAA`).
#[must_use]
pub fn is_valid_hex_color(raw: &str) -> bool {
    let Some(hex) = raw.trim().strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// Validates that a manifest asset path is a clean relative path inside the character folder.
///
/// # Errors
/// Returns an error if `raw` is empty, absolute, contains NUL/backslash/traversal components,
/// or lacks an allowed image extension when `require_image_ext` is `true`.
pub fn validate_relative_asset_path(raw: &str, require_image_ext: bool) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("asset path cannot be empty");
    }
    if trimmed.contains('\0') || trimmed.contains('\\') {
        bail!("asset path '{trimmed}' contains invalid characters");
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        bail!("asset path '{trimmed}' must be relative to the character directory");
    }
    for comp in path.components() {
        if !matches!(comp, Component::Normal(_)) {
            bail!("asset path '{trimmed}' cannot contain '.' or '..' traversal components");
        }
    }
    if require_image_ext {
        let ext_ok = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| {
                ALLOWED_IMAGE_EXTS
                    .iter()
                    .any(|allowed| ext.eq_ignore_ascii_case(allowed))
            });
        if !ext_ok {
            bail!(
                "asset path '{trimmed}' must use a supported image extension (.png, .webp, .jpg, .jpeg)"
            );
        }
    }
    Ok(trimmed.to_string())
}

/// Parses a `major.minor.patch` version string (ignoring any leading `v`/`V` or
/// `-prerelease` / `+build` suffix) into a comparable `(u32, u32, u32)` tuple.
#[must_use]
pub(super) fn parse_semver_triplet(raw: &str) -> Option<(u32, u32, u32)> {
    let trimmed = raw.trim();
    let without_v = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    let core = without_v.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((major, minor, patch))
}

fn write_if_changed(path: &Path, expected: &[u8]) -> Result<()> {
    let unchanged = fs::metadata(path)
        .is_ok_and(|m| m.is_file() && m.len() == expected.len() as u64)
        && fs::read(path).is_ok_and(|disk| disk.as_slice() == expected);
    if unchanged {
        return Ok(());
    }
    atomic_write(path, expected)
}

/// Parses and validates `manifest.toml` if present in `dir`.
///
/// For the three bundled characters (`niko-una`, `levi-kujo`, `kuro`):
/// - Seeds `manifest.toml` if absent on disk (legacy v1 -> v2 upgrade).
/// - Transparently upgrades package-owned assets (`AVATAR.png`, `portrait.webp`) and
///   `manifest.toml` in-place when `[package].version` on disk is older than the bundled
///   version (e.g. `2.0.0` -> `2.0.1`), leaving deleted characters absent and preserving
///   `CHARACTER.toml` (UUID + persona) and local `custom_*` / `custom.toml` overrides.
pub(super) fn load_or_seed_manifest(dir: &Path, slug: &str) -> Option<CharacterManifest> {
    let manifest_path = dir.join("manifest.toml");
    let bundled = find_bundled(slug);

    if !manifest_path.is_file()
        && let Some(b) = bundled
    {
        let _ = atomic_write(&manifest_path, b.manifest_bytes);
    }

    let raw = fs::read_to_string(&manifest_path).ok()?;
    match parse_and_sanitize_manifest(&raw, slug) {
        Ok(mut manifest) => {
            if let Some(b) = bundled {
                let is_bundled_author = manifest.package.authors.is_empty()
                    || manifest
                        .package
                        .authors
                        .iter()
                        .any(|a| a.trim().eq_ignore_ascii_case("Boxxy Team"));
                let is_older_version = parse_semver_triplet(&manifest.package.version)
                    .is_none_or(|disk_ver| disk_ver < b.version);

                if is_bundled_author && (manifest.assets.portrait.is_none() || is_older_version) {
                    let assets_res = write_if_changed(&dir.join("AVATAR.png"), b.avatar_bytes)
                        .and_then(|()| {
                            write_if_changed(&dir.join("portrait.webp"), b.portrait_bytes)
                        });

                    if let Err(err) = assets_res {
                        log::warn!(
                            "Failed to upgrade bundled character assets for '{slug}': {err}"
                        );
                    } else if let Ok(default_str) = std::str::from_utf8(b.manifest_bytes)
                        && let Ok(default_man) = parse_and_sanitize_manifest(default_str, slug)
                        && atomic_write(&manifest_path, b.manifest_bytes).is_ok()
                    {
                        manifest = default_man;
                    }
                }
            }
            Some(manifest)
        }
        Err(err) => {
            log::warn!("Invalid manifest.toml in character '{slug}': {err}");
            None
        }
    }
}

pub(super) fn parse_and_sanitize_manifest(raw: &str, slug: &str) -> Result<CharacterManifest> {
    let mut manifest: CharacterManifest =
        toml::from_str(raw).context("failed to parse TOML syntax")?;

    if manifest.schema_version == 0 {
        bail!("schema_version cannot be 0");
    }
    if manifest.schema_version > CHARACTER_SCHEMA_VERSION_V2 {
        log::info!(
            "Character '{slug}' uses newer manifest schema_version = {} (running v{CHARACTER_SCHEMA_VERSION_V2}); unknown v3+ fields will be ignored",
            manifest.schema_version
        );
    }

    if let Some(ref av) = manifest.assets.avatar.clone() {
        match validate_relative_asset_path(av, true) {
            Ok(clean) => manifest.assets.avatar = Some(clean),
            Err(e) => {
                log::warn!("Ignoring unsafe [assets].avatar in '{slug}': {e}");
                manifest.assets.avatar = None;
            }
        }
    }

    if let Some(ref pt) = manifest.assets.portrait.clone() {
        match validate_relative_asset_path(pt, true) {
            Ok(clean) => manifest.assets.portrait = Some(clean),
            Err(e) => {
                log::warn!("Ignoring unsafe [assets].portrait in '{slug}': {e}");
                manifest.assets.portrait = None;
            }
        }
    }

    if let Some(ref l2d) = manifest.assets.live2d_model.clone() {
        match validate_relative_asset_path(l2d, false) {
            Ok(clean) => manifest.assets.live2d_model = Some(clean),
            Err(e) => {
                log::warn!("Ignoring unsafe [assets].live2d_model in '{slug}': {e}");
                manifest.assets.live2d_model = None;
            }
        }
    }

    if let Some(ref expr) = manifest.assets.expressions_dir.clone() {
        match validate_relative_asset_path(expr, false) {
            Ok(clean) => manifest.assets.expressions_dir = Some(clean),
            Err(e) => {
                log::warn!("Ignoring unsafe [assets].expressions_dir in '{slug}': {e}");
                manifest.assets.expressions_dir = None;
            }
        }
    }

    if let Some(ref hex) = manifest.theme.accent_color.clone() {
        let trimmed = hex.trim();
        if is_valid_hex_color(trimmed) {
            manifest.theme.accent_color = Some(trimmed.to_string());
        } else {
            log::warn!("Ignoring invalid [theme].accent_color '{trimmed}' in '{slug}'");
            manifest.theme.accent_color = None;
        }
    }

    Ok(manifest)
}

pub(super) fn read_custom_overrides(dir: &Path) -> CustomOverridesToml {
    fs::read_to_string(dir.join(CUSTOM_OVERRIDES_FILE))
        .ok()
        .and_then(|s| toml::from_str::<CustomOverridesToml>(&s).ok())
        .unwrap_or_default()
}

pub(super) fn write_custom_color_override(dir: &Path, color: Option<&str>) -> Result<()> {
    let custom_path = dir.join(CUSTOM_OVERRIDES_FILE);
    let mut overrides = read_custom_overrides(dir);
    overrides.color = color.map(str::to_string);

    if overrides.color.is_none() {
        if let Err(e) = fs::remove_file(&custom_path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(e)
                .with_context(|| format!("Failed to remove {}", custom_path.display()));
        }
        return Ok(());
    }

    let mut serialized = String::from("# User overrides — preserved across character package updates.\n");
    serialized.push_str(
        &toml::to_string_pretty(&overrides).context("Failed to serialize custom.toml")?,
    );
    atomic_write(&custom_path, serialized)
}

fn find_existing_rel(dir: &Path, candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .find(|rel| dir.join(rel).is_file())
        .map(|s| (*s).to_string())
}

/// Resolves the package's base (non-custom) avatar relative path inside `dir`, if any.
pub(super) fn resolve_package_base_avatar_rel(
    dir: &Path,
    manifest: Option<&CharacterManifest>,
) -> Option<String> {
    if let Some(ref rel) = manifest.and_then(|m| m.assets.avatar.as_deref())
        && dir.join(rel).is_file()
    {
        return Some((*rel).to_string());
    }
    find_existing_rel(dir, AUTO_AVATAR_CANDIDATES)
}

/// Resolves active `avatar` and `portrait` relative paths for `dir` (preferring `custom_*`
/// user overrides over `manifest.toml` `[assets]` and standard filenames).
pub(super) fn resolve_character_assets(
    dir: &Path,
    slug: &str,
    manifest: Option<&CharacterManifest>,
) -> ResolvedCharacterAssets {
    // Transparently upgrade unmodified v1 placeholder `AVATAR.png` files to the v2 bundled avatar
    // while leaving user-cropped avatars untouched.
    let legacy_avatar_path = dir.join("AVATAR.png");
    if let (Some(expected_v1_len), Some(v2_bytes)) =
        (legacy_v1_avatar_len(slug), bundled_default_avatar(slug))
        && fs::metadata(&legacy_avatar_path)
            .is_ok_and(|m| m.is_file() && m.len() == expected_v1_len)
        && fs::read(&legacy_avatar_path)
            .is_ok_and(|bytes| matches_legacy_v1_bundled_avatar(slug, &bytes))
    {
        let _ = atomic_write(&legacy_avatar_path, v2_bytes);
    }

    let mut avatar_rel_path = find_existing_rel(dir, &[CUSTOM_AVATAR_PNG, CUSTOM_AVATAR_WEBP])
        .or_else(|| resolve_package_base_avatar_rel(dir, manifest));

    // Ensure the three bundled characters always restore their default `AVATAR.png` if missing.
    if avatar_rel_path.is_none()
        && let Some(default_bytes) = bundled_default_avatar(slug)
        && atomic_write(&dir.join("AVATAR.png"), default_bytes).is_ok()
    {
        avatar_rel_path = Some("AVATAR.png".to_string());
    }

    let mut portrait_rel_path =
        find_existing_rel(dir, &[CUSTOM_PORTRAIT_WEBP, CUSTOM_PORTRAIT_PNG])
            .or_else(|| {
                manifest
                    .and_then(|m| m.assets.portrait.as_deref())
                    .filter(|rel| dir.join(rel).is_file())
                    .map(str::to_string)
            })
            .or_else(|| find_existing_rel(dir, AUTO_PORTRAIT_CANDIDATES));

    // Auto-seed bundled `portrait.webp` on existing installations when missing on disk.
    if portrait_rel_path.is_none()
        && let Some(default_portrait) = bundled_default_portrait(slug)
        && atomic_write(&dir.join("portrait.webp"), default_portrait).is_ok()
    {
        portrait_rel_path = Some("portrait.webp".to_string());
    }

    ResolvedCharacterAssets {
        avatar_rel_path,
        portrait_rel_path,
    }
}

/// Pure, zero-I/O resolver returning the active avatar `PathBuf` under `base_dir` for `info`.
///
/// Safe to call on the GTK main thread. Falls back to `"AVATAR.png"` when `info.has_avatar`
/// is `true` and `avatar_rel_path` was omitted (e.g. legacy wire payloads or unit tests).
#[must_use]
pub fn resolve_character_avatar_path_in(base_dir: &Path, info: &CharacterInfo) -> Option<PathBuf> {
    if !info.has_avatar {
        return None;
    }
    let rel = info.avatar_rel_path.as_deref().unwrap_or("AVATAR.png");
    let clean = validate_relative_asset_path(rel, true).ok()?;
    Some(base_dir.join(&info.config.name).join(clean))
}

/// Pure, zero-I/O resolver returning the active avatar `PathBuf` in the active config root.
#[must_use]
pub fn resolve_character_avatar_path(info: &CharacterInfo) -> Option<PathBuf> {
    let base_dir = super::get_characters_dir().ok()?;
    resolve_character_avatar_path_in(&base_dir, info)
}

/// Pure, zero-I/O resolver returning the active `16:9` portrait `PathBuf` under `base_dir` for `info`.
#[must_use]
pub fn resolve_character_portrait_path_in(
    base_dir: &Path,
    info: &CharacterInfo,
) -> Option<PathBuf> {
    if !info.has_portrait {
        return None;
    }
    let rel = info.portrait_rel_path.as_deref().unwrap_or("portrait.webp");
    let clean = validate_relative_asset_path(rel, true).ok()?;
    Some(base_dir.join(&info.config.name).join(clean))
}

/// Pure, zero-I/O resolver returning the active `16:9` portrait `PathBuf` in the active config root.
#[must_use]
pub fn resolve_character_portrait_path(info: &CharacterInfo) -> Option<PathBuf> {
    let base_dir = super::get_characters_dir().ok()?;
    resolve_character_portrait_path_in(&base_dir, info)
}

/// Refreshes `<characters_dir>/README.md` once per active config directory so clicking
/// "Open Characters Folder" always includes an up-to-date Characters v2 quickstart guide.
pub(super) fn sync_characters_readme_once(base_dir: &Path) {
    if let Ok(mut guard) = README_SYNCED_DIR.lock() {
        if guard.as_deref() == Some(base_dir) {
            return;
        }
        let readme_path = base_dir.join("README.md");
        if fs::read_to_string(&readme_path).ok().as_deref() != Some(SEEDED_CHARACTERS_README) {
            let _ = atomic_write(&readme_path, SEEDED_CHARACTERS_README);
        }
        *guard = Some(base_dir.to_path_buf());
    }
}

fn validate_manifest_on_disk(
    dir: &Path,
    slug: &str,
    manifest_path: &Path,
    warnings: &mut Vec<String>,
) -> std::result::Result<(u32, CharacterManifest), (u32, String)> {
    let raw_man = fs::read_to_string(manifest_path)
        .map_err(|e| (1, format!("{}: read error ({e})", manifest_path.display())))?;
    let man: CharacterManifest = toml::from_str(&raw_man)
        .map_err(|e| (1, format!("{}: {e}", manifest_path.display())))?;
    let ver = man.schema_version;
    if ver == 0 {
        return Err((
            ver,
            format!("{}: schema_version cannot be 0", manifest_path.display()),
        ));
    }

    for (label, opt_rel) in [
        ("[assets].avatar", man.assets.avatar.as_deref()),
        ("[assets].portrait", man.assets.portrait.as_deref()),
    ] {
        if let Some(rel) = opt_rel {
            let clean = validate_relative_asset_path(rel, true)
                .map_err(|e| (ver, format!("{}: {e}", manifest_path.display())))?;
            if !dir.join(&clean).is_file() {
                warnings.push(format!("{slug}: manifest {label} '{clean}' not found on disk"));
            }
        }
    }

    if let Some(ref hex) = man.theme.accent_color
        && !is_valid_hex_color(hex)
    {
        warnings.push(format!(
            "{slug}: invalid [theme].accent_color '{hex}' in manifest.toml"
        ));
    }

    Ok((ver, man))
}

/// Validates a character directory for `boxxy-terminal +doctor`.
///
/// Checks `CHARACTER.toml` and optional `manifest.toml` (schema version, relative asset paths,
/// missing declared asset files, and valid hex colors).
#[must_use]
pub fn validate_character_package_dir(dir: &Path) -> CharacterDirDiagnostic {
    let slug = dir
        .file_name()
        .map_or_else(|| dir.display().to_string(), |n| n.to_string_lossy().into_owned());
    let mut warnings = Vec::new();
    let mut schema_version = 1u32;

    let toml_path = dir.join("CHARACTER.toml");
    let raw_char = match fs::read_to_string(&toml_path) {
        Ok(s) => s,
        Err(e) => {
            return CharacterDirDiagnostic {
                slug,
                schema_version,
                has_portrait: false,
                warnings,
                error: Some(format!("{}: read error ({e})", toml_path.display())),
            };
        }
    };

    let parsed_char: CharacterToml = match toml::from_str(&raw_char) {
        Ok(c) => c,
        Err(e) => {
            return CharacterDirDiagnostic {
                slug,
                schema_version,
                has_portrait: false,
                warnings,
                error: Some(format!("{}: {e}", toml_path.display())),
            };
        }
    };

    if let Some(ref c) = parsed_char.color
        && !c.trim().is_empty()
        && !is_valid_hex_color(c)
    {
        warnings.push(format!("{slug}: invalid hex color '{c}' in CHARACTER.toml"));
    }

    let manifest_path = dir.join("manifest.toml");
    let parsed_manifest = if manifest_path.is_file() {
        match validate_manifest_on_disk(dir, &slug, &manifest_path, &mut warnings) {
            Ok((ver, man)) => {
                schema_version = ver;
                Some(man)
            }
            Err((ver, err)) => {
                return CharacterDirDiagnostic {
                    slug,
                    schema_version: ver,
                    has_portrait: false,
                    warnings,
                    error: Some(err),
                };
            }
        }
    } else {
        None
    };

    let resolved = resolve_character_assets(dir, &slug, parsed_manifest.as_ref());
    CharacterDirDiagnostic {
        slug,
        schema_version,
        has_portrait: resolved.portrait_rel_path.is_some(),
        warnings,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pathological_dotted_keys_and_malformed_toml_fail_cleanly_without_stack_overflow() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let pack_dir = tmp.path().join("untrusted-pack");
        fs::create_dir_all(&pack_dir).expect("create pack dir");

        // Deeply nested dotted keys (guarded by `toml 1.2.3` recursion cap) must return
        // a diagnostic parse error rather than overflowing the thread stack.
        let deep_dotted = format!(
            "{} = \"boom\"\n",
            (0..512).map(|i| format!("k{i}")).collect::<Vec<_>>().join(".")
        );
        fs::write(pack_dir.join("CHARACTER.toml"), deep_dotted).expect("write dotted toml");

        let diag = validate_character_package_dir(&pack_dir);
        assert!(
            diag.error.is_some(),
            "deeply nested dotted TOML keys must fail validation cleanly"
        );

        // Malformed deeply nested inline arrays/tables (`toml 1.2.2` guard) must also error cleanly.
        let malformed = format!("display_name = {}", "[".repeat(512));
        fs::write(pack_dir.join("CHARACTER.toml"), malformed).expect("write malformed toml");

        let diag_malformed = validate_character_package_dir(&pack_dir);
        assert!(diag_malformed.error.is_some());
    }
}
