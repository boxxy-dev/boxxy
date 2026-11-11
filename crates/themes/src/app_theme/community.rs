use super::colors::{AppThemeColors, AppThemeDefinition, builtin_app_themes};
use super::oklch::{contrast_fg_for_hex, is_hex_dark, sanitize_hex_color};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Serialize)]
struct CommunityThemeToml {
    meta: Option<CommunityMetaToml>,
    colors: CommunityColorsToml,
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct CommunityMetaToml {
    name: Option<String>,
    author: Option<String>,
    description: Option<String>,
    mode: Option<String>,
    tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CommunityColorsToml {
    window_bg: String,
    window_fg: String,
    sidebar_bg: Option<String>,
    sidebar_fg: Option<String>,
    card_bg: Option<String>,
    card_fg: Option<String>,
    headerbar_bg: Option<String>,
    headerbar_fg: Option<String>,
    popover_bg: Option<String>,
    popover_fg: Option<String>,
    accent: String,
    accent_fg: Option<String>,
    secondary: String,
    secondary_fg: Option<String>,
    highlight: String,
    highlight_fg: Option<String>,
    ink_border: Option<String>,
    shadow: Option<String>,
}

const SEEDED_README: &str = r##"# Boxxy Community App Themes

Drop `.toml` theme files into this directory to customize Boxxy's application chrome!
Community themes strictly define color tokens (`#RRGGBB`) so they never break layout or padding.

## Theme Color Roles

Every Boxxy theme defines a 3-color identity triad alongside surface and ink tokens:
- `accent` *(Required)*: Primary interaction — selected tabs, selected sidebar rows, checked switches, primary action buttons, and shadowed hover surfaces.
- `secondary` *(Required)*: Supporting identity — category tag pills, status pills, the Claw `Okay` button, and `@file` input tags.
- `highlight` *(Required)*: Attention & discovery — `FRESH UPDATE` badges, Vibe Mode badge, live active interval laps, search matches, and `:character` mention tags.

## Minimal & Full Example (`my-theme.toml`)

```toml
[meta]
name = "Cotton Candy"
author = "boxxy-community"
description = "Soft pastels in baby pink, sky blue, and lemon candy"
mode = "light" # "light" or "dark"
tags = ["pastel", "pink", "cute"]

[colors]
# Required (5 fields)
window_bg = "#fff5f8"
window_fg = "#1e1b24"
accent    = "#ff7eb6"
secondary = "#7cc6fe"
highlight = "#ffe066"

# Optional surfaces, foregrounds & ink (derived automatically if omitted)
sidebar_bg   = "#fce8ef"
card_bg      = "#ffffff"
accent_fg    = "#1e1b24"
secondary_fg = "#1e1b24"
highlight_fg = "#1e1b24"
ink_border   = "#1e1b24"
shadow       = "#1e1b24"
```
"##;

pub(crate) const SEEDED_COTTON_CANDY: &str = r##"[meta]
name = "Cotton Candy"
author = "nukeop"
description = "Soft pastels in baby pink, sky blue, and lemon candy"
mode = "light"
tags = ["pastel", "pink", "cute"]

[colors]
window_bg = "#fff5f8"
window_fg = "#1e1b24"
sidebar_bg = "#fce8ef"
sidebar_fg = "#1e1b24"
card_bg = "#ffffff"
card_fg = "#1e1b24"
headerbar_bg = "#fce8ef"
headerbar_fg = "#1e1b24"
popover_bg = "#fce8ef"
popover_fg = "#1e1b24"
accent = "#ff7eb6"
accent_fg = "#1e1b24"
secondary = "#7cc6fe"
secondary_fg = "#1e1b24"
highlight = "#ffe066"
highlight_fg = "#1e1b24"
ink_border = "#1e1b24"
shadow = "#1e1b24"
"##;

pub(crate) const SEEDED_BAUHAUS: &str = r##"[meta]
name = "Bauhaus"
author = "nukeop"
description = "Hard corners, saturated primary triad, bold poster contrast."
mode = "light"
tags = ["bauhaus", "geometric", "poster", "primary-colors"]

[colors]
window_bg = "#f7f3e8"
window_fg = "#141413"
sidebar_bg = "#ede6d3"
sidebar_fg = "#141413"
card_bg = "#fffdf7"
card_fg = "#141413"
headerbar_bg = "#ede6d3"
headerbar_fg = "#141413"
popover_bg = "#ede6d3"
popover_fg = "#141413"
accent = "#e63946"
accent_fg = "#ffffff"
secondary = "#1d4ed8"
secondary_fg = "#ffffff"
highlight = "#f4b400"
highlight_fg = "#141413"
ink_border = "#141413"
shadow = "#141413"
"##;

static README_SYNCED_DIR: std::sync::LazyLock<parking_lot::Mutex<Option<PathBuf>>> =
    std::sync::LazyLock::new(|| parking_lot::Mutex::new(None));

/// Returns the user's community themes directory (`<config_dir>/themes`).
#[must_use]
pub fn get_app_themes_dir() -> PathBuf {
    boxxy_sys_utils::get_config_dir().join("themes")
}

/// Ensures `<config_dir>/themes` exists, refreshes `README.md` once per config dir so
/// existing users always have up-to-date documentation, and seeds starter community
/// themes (`cotton-candy.toml`, `bauhaus.toml`) when the directory is newly created.
pub fn ensure_app_themes_dir() -> PathBuf {
    let dir = get_app_themes_dir();
    let is_new = !dir.exists();
    let _ = fs::create_dir_all(&dir);

    if is_new {
        let _ = fs::write(dir.join("README.md"), SEEDED_README);
        let _ = fs::write(dir.join("cotton-candy.toml"), SEEDED_COTTON_CANDY);
        let _ = fs::write(dir.join("bauhaus.toml"), SEEDED_BAUHAUS);
        *README_SYNCED_DIR.lock() = Some(dir.clone());
    } else {
        let needs_readme_sync = {
            let guard = README_SYNCED_DIR.lock();
            guard.as_ref() != Some(&dir)
        };
        if needs_readme_sync {
            let readme_path = dir.join("README.md");
            let current = fs::read_to_string(&readme_path).unwrap_or_default();
            if current != SEEDED_README {
                let _ = fs::write(&readme_path, SEEDED_README);
            }
            *README_SYNCED_DIR.lock() = Some(dir.clone());
        }
    }

    dir
}

type CommunityFileMeta = (std::ffi::OsString, Option<std::time::SystemTime>, u64);
type CommunityDirStamp = (PathBuf, Vec<CommunityFileMeta>);
type CachedCommunityThemes = Option<(CommunityDirStamp, Vec<AppThemeDefinition>)>;

static COMMUNITY_CACHE: std::sync::LazyLock<parking_lot::Mutex<CachedCommunityThemes>> =
    std::sync::LazyLock::new(|| parking_lot::Mutex::new(None));

fn compute_dir_stamp(dir: &Path) -> CommunityDirStamp {
    let mut meta_list = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
            {
                let (mtime, len) = entry
                    .metadata()
                    .ok()
                    .map_or((None, 0), |m| (m.modified().ok(), m.len()));
                meta_list.push((entry.file_name(), mtime, len));
            }
        }
        meta_list.sort_by(|a, b| a.0.cmp(&b.0));
    }
    (dir.to_path_buf(), meta_list)
}

/// Discovers and parses all valid `.toml` community themes in `<config_dir>/themes`.
/// Uses an mtime/size metadata cache so repeated calls on unchanged directories do zero file reads.
#[must_use]
pub fn discover_community_themes() -> Vec<AppThemeDefinition> {
    let dir = ensure_app_themes_dir();
    let stamp = compute_dir_stamp(&dir);
    {
        let guard = COMMUNITY_CACHE.lock();
        if let Some((cached_stamp, cached_themes)) = guard.as_ref()
            && cached_stamp == &stamp
        {
            return cached_themes.clone();
        }
    }
    let parsed = discover_community_themes_in(&dir);
    *COMMUNITY_CACHE.lock() = Some((stamp, parsed.clone()));
    parsed
}

/// Pure directory scanner for community `.toml` themes (used by runtime and unit tests).
#[must_use]
pub fn discover_community_themes_in(dir: &Path) -> Vec<AppThemeDefinition> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
        })
        .collect();

    files.sort_by_key(|e| e.file_name());

    let builtin_ids: Vec<String> = builtin_app_themes().into_iter().map(|t| t.id).collect();
    let mut themes = Vec::new();

    for entry in files {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let slug = stem.trim().to_lowercase().replace(' ', "-");
        if slug.is_empty() || builtin_ids.contains(&slug) {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        if let Some(theme) = parse_community_theme_toml(&slug, &content) {
            themes.push(theme);
        }
    }

    themes
}

/// Parses a single community `.toml` string into an `AppThemeDefinition`.
/// Returns `None` if required colors (`window_bg`, `window_fg`, `accent`, `secondary`, `highlight`)
/// are missing or invalid hex.
#[must_use]
pub fn parse_community_theme_toml(slug: &str, content: &str) -> Option<AppThemeDefinition> {
    let parsed: CommunityThemeToml = toml::from_str(content).ok()?;
    let meta = parsed.meta.unwrap_or_default();
    let c = parsed.colors;

    let window_bg = sanitize_hex_color(&c.window_bg)?;
    let window_fg = sanitize_hex_color(&c.window_fg)?;
    let accent = sanitize_hex_color(&c.accent)?;
    let secondary = sanitize_hex_color(&c.secondary)?;
    let highlight = sanitize_hex_color(&c.highlight)?;

    let is_dark = match meta.mode.as_deref().map(str::trim) {
        Some(m) if m.eq_ignore_ascii_case("light") => false,
        Some(m) if m.eq_ignore_ascii_case("dark") => true,
        _ => is_hex_dark(&window_bg),
    };

    let sidebar_bg = c
        .sidebar_bg
        .as_deref()
        .and_then(sanitize_hex_color)
        .unwrap_or_else(|| window_bg.clone());
    let sidebar_fg = c
        .sidebar_fg
        .as_deref()
        .and_then(sanitize_hex_color)
        .unwrap_or_else(|| window_fg.clone());

    let card_bg = c
        .card_bg
        .as_deref()
        .and_then(sanitize_hex_color)
        .unwrap_or_else(|| window_bg.clone());
    let card_fg = c
        .card_fg
        .as_deref()
        .and_then(sanitize_hex_color)
        .unwrap_or_else(|| window_fg.clone());

    let headerbar_bg = c
        .headerbar_bg
        .as_deref()
        .and_then(sanitize_hex_color)
        .unwrap_or_else(|| sidebar_bg.clone());
    let headerbar_fg = c
        .headerbar_fg
        .as_deref()
        .and_then(sanitize_hex_color)
        .unwrap_or_else(|| sidebar_fg.clone());

    let popover_bg = c
        .popover_bg
        .as_deref()
        .and_then(sanitize_hex_color)
        .unwrap_or_else(|| sidebar_bg.clone());
    let popover_fg = c
        .popover_fg
        .as_deref()
        .and_then(sanitize_hex_color)
        .unwrap_or_else(|| sidebar_fg.clone());

    let accent_fg = c
        .accent_fg
        .as_deref()
        .and_then(sanitize_hex_color)
        .unwrap_or_else(|| contrast_fg_for_hex(&accent));
    let secondary_fg = c
        .secondary_fg
        .as_deref()
        .and_then(sanitize_hex_color)
        .unwrap_or_else(|| contrast_fg_for_hex(&secondary));
    let highlight_fg = c
        .highlight_fg
        .as_deref()
        .and_then(sanitize_hex_color)
        .unwrap_or_else(|| contrast_fg_for_hex(&highlight));

    let default_ink = if is_dark { "#07080c" } else { "#111418" };
    let ink_border = c
        .ink_border
        .as_deref()
        .and_then(sanitize_hex_color)
        .unwrap_or_else(|| default_ink.to_string());
    let shadow = c
        .shadow
        .as_deref()
        .and_then(sanitize_hex_color)
        .unwrap_or_else(|| ink_border.clone());

    let name = meta
        .name
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| slug.replace('-', " "));
    let author = meta
        .author
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "community".to_string());
    let description = meta
        .description
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let tags = meta
        .tags
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .take(6)
        .collect();

    Some(AppThemeDefinition {
        id: slug.to_string(),
        name,
        author,
        description,
        is_dark,
        is_builtin: false,
        tags,
        colors: AppThemeColors {
            window_bg,
            window_fg,
            sidebar_bg,
            sidebar_fg,
            card_bg,
            card_fg,
            headerbar_bg,
            headerbar_fg,
            popover_bg,
            popover_fg,
            accent,
            accent_fg,
            secondary,
            secondary_fg,
            highlight,
            highlight_fg,
            ink_border,
            shadow,
        },
    })
}

/// Resolves a theme by ID across built-in themes and community themes, falling back to `boxxy-dark`.
#[must_use]
pub fn resolve_app_theme(theme_id: &str) -> AppThemeDefinition {
    let target = theme_id.trim();
    let builtins = builtin_app_themes();
    if let Some(found) = builtins
        .iter()
        .find(|t| t.id.eq_ignore_ascii_case(target) || t.name.eq_ignore_ascii_case(target))
    {
        return found.clone();
    }

    let community = discover_community_themes();
    if let Some(found) = community
        .into_iter()
        .find(|t| t.id.eq_ignore_ascii_case(target) || t.name.eq_ignore_ascii_case(target))
    {
        return found;
    }

    builtins
        .into_iter()
        .next()
        .expect("builtin_app_themes is non-empty")
}
