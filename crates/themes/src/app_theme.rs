use gtk4 as gtk;
use libadwaita as adw;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

const PRIORITY_BOXXY_SKELETON: u32 = gtk::STYLE_PROVIDER_PRIORITY_USER;
const PRIORITY_BOXXY_TOKENS: u32 = gtk::STYLE_PROVIDER_PRIORITY_USER + 10;
const PRIORITY_BOXXY_ACCENT: u32 = gtk::STYLE_PROVIDER_PRIORITY_USER + 20;

/// Resolved color tokens for a Boxxy app theme (built-in or community).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppThemeColors {
    pub window_bg: String,
    pub window_fg: String,
    pub sidebar_bg: String,
    pub sidebar_fg: String,
    pub card_bg: String,
    pub card_fg: String,
    pub headerbar_bg: String,
    pub headerbar_fg: String,
    pub popover_bg: String,
    pub popover_fg: String,
    pub accent: String,
    pub accent_fg: String,
    pub ink_border: String,
    pub shadow: String,
}

impl AppThemeColors {
    /// Returns the 4 signature preview dots shown on theme cards (accent, window_bg, card_bg, ink_border).
    #[must_use]
    pub fn preview_dots(&self) -> [&str; 4] {
        [
            &self.accent,
            &self.window_bg,
            &self.card_bg,
            &self.ink_border,
        ]
    }

    /// Generates the `@define-color` CSS block for Layer 3 following Libadwaita conventions.
    #[must_use]
    pub fn to_gtk_css(&self, is_dark: bool) -> String {
        // Semantic status text/badges (`.diff-badge-created`, `.diff-stat-add`, `.diff-stat-del`,
        // `.model-status-warning`, etc.) render on top of `@card_bg_color`, so their foreground
        // contrast must follow the luminance of `card_bg` rather than `window_bg`.
        let is_card_dark = is_hex_dark(&self.card_bg);
        let (
            destructive_bg,
            destructive_fg,
            destructive_color,
            warning_bg,
            warning_fg,
            warning_color,
            success_bg,
            success_fg,
            success_color,
        ) = if is_card_dark {
            (
                "#c01c28",
                "#ffffff",
                "#ff7b63",
                "#e5a50a",
                "#111418",
                "#f8e45c",
                "#26a269",
                "#ffffff",
                "#8ff0a4",
            )
        } else {
            let _ = is_dark;
            (
                "#e01b24",
                "#ffffff",
                "#c01c28",
                "#e5a50a",
                "#111418",
                "#ae7b03",
                "#2ec27e",
                "#111418",
                "#1b8553",
            )
        };

        format!(
            r#"
@define-color window_bg_color {window_bg};
@define-color window_fg_color {window_fg};
@define-color view_bg_color {card_bg};
@define-color view_fg_color {card_fg};
@define-color headerbar_bg_color {headerbar_bg};
@define-color headerbar_backdrop_color {headerbar_bg};
@define-color headerbar_fg_color {headerbar_fg};
@define-color headerbar_border_color {ink_border};
@define-color sidebar_bg_color {sidebar_bg};
@define-color sidebar_fg_color {sidebar_fg};
@define-color sidebar_backdrop_color {sidebar_bg};
@define-color sidebar_border_color {ink_border};
@define-color secondary_sidebar_bg_color {sidebar_bg};
@define-color secondary_sidebar_fg_color {sidebar_fg};
@define-color secondary_sidebar_backdrop_color {sidebar_bg};
@define-color secondary_sidebar_border_color {ink_border};
@define-color card_bg_color {card_bg};
@define-color card_fg_color {card_fg};
@define-color dialog_bg_color {window_bg};
@define-color dialog_fg_color {window_fg};
@define-color popover_bg_color {popover_bg};
@define-color popover_fg_color {popover_fg};
@define-color accent_color {accent};
@define-color accent_bg_color {accent};
@define-color accent_fg_color {accent_fg};
@define-color destructive_bg_color {destructive_bg};
@define-color destructive_fg_color {destructive_fg};
@define-color destructive_color {destructive_color};
@define-color error_bg_color {destructive_bg};
@define-color error_fg_color {destructive_fg};
@define-color error_color {destructive_color};
@define-color warning_bg_color {warning_bg};
@define-color warning_fg_color {warning_fg};
@define-color warning_color {warning_color};
@define-color success_bg_color {success_bg};
@define-color success_fg_color {success_fg};
@define-color success_color {success_color};
@define-color boxxy_ink {ink_border};
@define-color boxxy_shadow {shadow};
@define-color boxxy_surface_hover mix(@card_bg_color, @accent_bg_color, 0.82);
"#,
            window_bg = self.window_bg,
            window_fg = self.window_fg,
            headerbar_bg = self.headerbar_bg,
            headerbar_fg = self.headerbar_fg,
            sidebar_bg = self.sidebar_bg,
            sidebar_fg = self.sidebar_fg,
            card_bg = self.card_bg,
            card_fg = self.card_fg,
            popover_bg = self.popover_bg,
            popover_fg = self.popover_fg,
            accent = self.accent,
            accent_fg = self.accent_fg,
            ink_border = self.ink_border,
            shadow = self.shadow,
        )
    }
}

/// A built-in or community Boxxy App Theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppThemeDefinition {
    pub id: String,
    pub name: String,
    pub author: String,
    pub description: String,
    pub is_dark: bool,
    pub is_builtin: bool,
    pub tags: Vec<String>,
    pub colors: AppThemeColors,
}

/// Curated accent color presets for quick selection in Preferences → Themes.
pub const ACCENT_PRESETS: &[(&str, &str)] = &[
    ("Peanut Green", "#7ec868"),
    ("Boxxy Cyan", "#00acc1"),
    ("Ember Coral", "#e87a64"),
    ("Aurora Violet", "#a78bfa"),
    ("Bubblegum Pink", "#ff7eb6"),
    ("Solar Amber", "#f59e0b"),
];

/// Returns the two official built-in Boxxy themes ("Boxxy Dark" and "Boxxy Light").
#[must_use]
pub fn builtin_app_themes() -> Vec<AppThemeDefinition> {
    vec![
        AppThemeDefinition {
            id: "boxxy-dark".to_string(),
            name: "Boxxy Dark".to_string(),
            author: "Boxxy Team".to_string(),
            description: "Carbon dark surfaces with crisp ink borders and peanut green accents."
                .to_string(),
            is_dark: true,
            is_builtin: true,
            tags: vec!["dark".to_string(), "carbon".to_string(), "default".to_string()],
            colors: AppThemeColors {
                window_bg: "#1d1d20".to_string(),
                window_fg: "#f2f4f2".to_string(),
                sidebar_bg: "#2e2e32".to_string(),
                sidebar_fg: "#f2f4f2".to_string(),
                card_bg: "#26262a".to_string(),
                card_fg: "#f2f4f2".to_string(),
                headerbar_bg: "#1d1d20".to_string(),
                headerbar_fg: "#f2f4f2".to_string(),
                popover_bg: "#2e2e32".to_string(),
                popover_fg: "#f2f4f2".to_string(),
                accent: "#7ec868".to_string(),
                accent_fg: "#0d140a".to_string(),
                ink_border: "#050505".to_string(),
                shadow: "#050505".to_string(),
            },
        },
        AppThemeDefinition {
            id: "boxxy-light".to_string(),
            name: "Boxxy Light".to_string(),
            author: "Boxxy Team".to_string(),
            description: "Crisp comic-book ink outlines on warm mint paper with vibrant cyan pills."
                .to_string(),
            is_dark: false,
            is_builtin: true,
            tags: vec!["light".to_string(), "playful".to_string(), "clean".to_string()],
            colors: AppThemeColors {
                window_bg: "#ecfafb".to_string(),
                window_fg: "#111418".to_string(),
                sidebar_bg: "#ffffff".to_string(),
                sidebar_fg: "#111418".to_string(),
                card_bg: "#ffffff".to_string(),
                card_fg: "#111418".to_string(),
                headerbar_bg: "#ffffff".to_string(),
                headerbar_fg: "#111418".to_string(),
                popover_bg: "#ffffff".to_string(),
                popover_fg: "#111418".to_string(),
                accent: "#00acc1".to_string(),
                accent_fg: "#0d1117".to_string(),
                ink_border: "#111418".to_string(),
                shadow: "#111418".to_string(),
            },
        },
    ]
}

// ---------------------------------------------------------------------------
// Community TOML Schema & Discovery
// ---------------------------------------------------------------------------

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
    ink_border: Option<String>,
    shadow: Option<String>,
}

/// Returns the user's community themes directory (`<config_dir>/themes`).
#[must_use]
pub fn get_app_themes_dir() -> PathBuf {
    boxxy_sys_utils::get_config_dir().join("themes")
}

/// Ensures `<config_dir>/themes` exists and seeds starter community themes + README on first creation.
pub fn ensure_app_themes_dir() -> PathBuf {
    let dir = get_app_themes_dir();
    let is_new = !dir.exists();
    let _ = fs::create_dir_all(&dir);

    if is_new {
        let readme = r##"# Boxxy Community App Themes

Drop `.toml` theme files into this directory to customize Boxxy's application chrome!
Community themes strictly define color tokens so they never break layout or padding.

## Minimal Theme Example (`my-theme.toml`)

```toml
[meta]
name = "Cotton Candy"
author = "boxxy-community"
description = "Soft pastels in baby pink and baby blue"
mode = "light" # "light" or "dark"
tags = ["pastel", "pink", "cute"]

[colors]
window_bg = "#fff5f8"
window_fg = "#1e1b24"
sidebar_bg = "#fce8ef"
card_bg = "#ffffff"
accent = "#ff7eb6"
ink_border = "#1e1b24"
shadow = "#1e1b24"
```
"##;
        let _ = fs::write(dir.join("README.md"), readme);

        let cotton_candy = r##"[meta]
name = "Cotton Candy"
author = "nukeop"
description = "Soft pastels in baby pink and baby blue"
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
ink_border = "#1e1b24"
shadow = "#1e1b24"
"##;
        let _ = fs::write(dir.join("cotton-candy.toml"), cotton_candy);

        let bauhaus = r##"[meta]
name = "Bauhaus"
author = "nukeop"
description = "Hard corners, saturated primary colors, bold poster contrast."
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
ink_border = "#141413"
shadow = "#141413"
"##;
        let _ = fs::write(dir.join("bauhaus.toml"), bauhaus);
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
/// Returns `None` if required colors (`window_bg`, `window_fg`, `accent`) are missing or invalid hex.
#[must_use]
pub fn parse_community_theme_toml(slug: &str, content: &str) -> Option<AppThemeDefinition> {
    let parsed: CommunityThemeToml = toml::from_str(content).ok()?;
    let meta = parsed.meta.unwrap_or_default();
    let c = parsed.colors;

    let window_bg = sanitize_hex_color(&c.window_bg)?;
    let window_fg = sanitize_hex_color(&c.window_fg)?;
    let accent = sanitize_hex_color(&c.accent)?;

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

/// Validates a hex color string (`#RRGGBB` or `#RRGGBBAA`), rejecting any CSS injection characters.
#[must_use]
pub fn sanitize_hex_color(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let hex = trimmed.strip_prefix('#')?;
    if (hex.len() == 6 || hex.len() == 8) && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(format!("#{}", hex.to_ascii_lowercase()))
    } else {
        None
    }
}

/// Computes a high-contrast foreground color (`#111418` or `#ffffff`) for a given `#RRGGBB` background.
#[must_use]
pub fn contrast_fg_for_hex(hex: &str) -> String {
    if is_hex_dark(hex) {
        "#ffffff".to_string()
    } else {
        "#111418".to_string()
    }
}

#[must_use]
pub fn is_hex_dark(hex: &str) -> bool {
    let clean = hex.trim().trim_start_matches('#');
    if clean.len() < 6 {
        return true;
    }
    let r = u8::from_str_radix(&clean[0..2], 16).unwrap_or(0) as f32 / 255.0;
    let g = u8::from_str_radix(&clean[2..4], 16).unwrap_or(0) as f32 / 255.0;
    let b = u8::from_str_radix(&clean[4..6], 16).unwrap_or(0) as f32 / 255.0;
    // Relative perceived luminance: below 0.38 requires light text; vibrant mid-tones
    // (like #00acc1 cyan or #e87a64 coral) pair with dark ink (#111418) for comic contrast.
    let luminance = 0.299 * r + 0.587 * g + 0.114 * b;
    luminance < 0.38
}

/// Generates the Layer 4 custom accent color override CSS block.
#[must_use]
pub fn generate_accent_override_css(accent_hex: &str) -> Option<String> {
    let safe_accent = sanitize_hex_color(accent_hex)?;
    let safe_fg = contrast_fg_for_hex(&safe_accent);
    Some(format!(
        r#"
@define-color accent_color {safe_accent};
@define-color accent_bg_color {safe_accent};
@define-color accent_fg_color {safe_fg};
"#
    ))
}

// ---------------------------------------------------------------------------
// GTK CssProvider Cascade Management
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct AppliedThemeState {
    use_boxxy_theme: bool,
    tokens_css: String,
    accent_css: Option<String>,
    color_scheme: adw::ColorScheme,
}

thread_local! {
    static SKELETON_PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
    static TOKENS_PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
    static ACCENT_PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
    static LAST_APPLIED_STATE: RefCell<Option<AppliedThemeState>> = const { RefCell::new(None) };
}

/// Applies or unloads the custom Boxxy App Theme cascade and updates `adw::StyleManager`.
///
/// - When `use_boxxy_theme` is `false` (Default GTK mode), all custom Boxxy theme providers
///   are removed from the display and `fallback_scheme` is applied to `adw::StyleManager`.
/// - When `use_boxxy_theme` is `true`, loads `boxxy_skeleton.css` (Layer 2), the theme's
///   color tokens (Layer 3), and the optional custom accent override (Layer 4), while
///   forcing `adw::ColorScheme::ForceDark` or `ForceLight` to match the theme.
/// - Deduplicates against `LAST_APPLIED_STATE` so unrelated settings saves incur zero
///   `CssProvider` rebuilds or GTK style invalidations.
pub fn apply_app_theme(
    use_boxxy_theme: bool,
    theme_id: &str,
    custom_accent_color: Option<&str>,
    fallback_scheme: adw::ColorScheme,
) {
    let style_manager = adw::StyleManager::default();

    if !use_boxxy_theme {
        let target_state = AppliedThemeState {
            use_boxxy_theme: false,
            tokens_css: String::new(),
            accent_css: None,
            color_scheme: fallback_scheme,
        };
        let already_applied =
            LAST_APPLIED_STATE.with(|cell| cell.borrow().as_ref() == Some(&target_state));
        if already_applied {
            return;
        }
        remove_provider(&SKELETON_PROVIDER);
        remove_provider(&TOKENS_PROVIDER);
        remove_provider(&ACCENT_PROVIDER);
        if style_manager.color_scheme() != fallback_scheme {
            style_manager.set_color_scheme(fallback_scheme);
        }
        set_display_card_dark(style_manager.is_dark());
        LAST_APPLIED_STATE.with(|cell| *cell.borrow_mut() = Some(target_state));
        return;
    }

    let resolved = resolve_app_theme(theme_id);
    let is_card_dark = is_hex_dark(&resolved.colors.card_bg);
    set_display_card_dark(is_card_dark);
    let target_scheme = if resolved.is_dark {
        adw::ColorScheme::ForceDark
    } else {
        adw::ColorScheme::ForceLight
    };
    let tokens_css = resolved.colors.to_gtk_css(resolved.is_dark);
    let accent_css = custom_accent_color.and_then(generate_accent_override_css);

    let target_state = AppliedThemeState {
        use_boxxy_theme: true,
        tokens_css: tokens_css.clone(),
        accent_css: accent_css.clone(),
        color_scheme: target_scheme,
    };
    let already_applied =
        LAST_APPLIED_STATE.with(|cell| cell.borrow().as_ref() == Some(&target_state));
    if already_applied {
        return;
    }

    if style_manager.color_scheme() != target_scheme {
        style_manager.set_color_scheme(target_scheme);
    }

    // Layer 2: Playful Boxxy Skeleton (loaded once and kept while Boxxy theme mode is active)
    ensure_skeleton_provider();

    // Layer 3: Theme Color Tokens (only reload provider if tokens_css changed)
    let tokens_changed = LAST_APPLIED_STATE.with(|cell| {
        cell.borrow()
            .as_ref()
            .is_none_or(|prev| !prev.use_boxxy_theme || prev.tokens_css != tokens_css)
    });
    if tokens_changed {
        set_string_provider(&TOKENS_PROVIDER, &tokens_css, PRIORITY_BOXXY_TOKENS);
    }

    // Layer 4: Optional User Accent Color Override
    let accent_changed = LAST_APPLIED_STATE.with(|cell| {
        cell.borrow()
            .as_ref()
            .is_none_or(|prev| !prev.use_boxxy_theme || prev.accent_css != accent_css)
    });
    if accent_changed {
        if let Some(ref css) = accent_css {
            set_string_provider(&ACCENT_PROVIDER, css, PRIORITY_BOXXY_ACCENT);
        } else {
            remove_provider(&ACCENT_PROVIDER);
        }
    }

    LAST_APPLIED_STATE.with(|cell| *cell.borrow_mut() = Some(target_state));
}

fn set_display_card_dark(is_card_dark: bool) {
    if let Some(display) = gtk::gdk::Display::default() {
        use glib::prelude::ObjectExt;
        unsafe {
            display.set_data("boxxy-card-is-dark", is_card_dark);
        }
    }
}

fn ensure_skeleton_provider() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    SKELETON_PROVIDER.with(|cell| {
        if cell.borrow().is_none() {
            let provider = gtk::CssProvider::new();
            provider.load_from_resource("/dev/boxxy/BoxxyTerminal/themes/boxxy_skeleton.css");
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                PRIORITY_BOXXY_SKELETON,
            );
            *cell.borrow_mut() = Some(provider);
        }
    });
}

fn set_string_provider(
    slot: &'static std::thread::LocalKey<RefCell<Option<gtk::CssProvider>>>,
    css: &str,
    priority: u32,
) {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    slot.with(|cell| {
        if let Some(old) = cell.borrow().as_ref() {
            gtk::style_context_remove_provider_for_display(&display, old);
        }
        let provider = gtk::CssProvider::new();
        provider.load_from_string(css);
        gtk::style_context_add_provider_for_display(&display, &provider, priority);
        *cell.borrow_mut() = Some(provider);
    });
}

fn remove_provider(slot: &'static std::thread::LocalKey<RefCell<Option<gtk::CssProvider>>>) {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    slot.with(|cell| {
        if let Some(old) = cell.borrow_mut().take() {
            gtk::style_context_remove_provider_for_display(&display, &old);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::prelude::*;

    #[test]
    fn test_sanitize_hex_color_valid_and_invalid() {
        assert_eq!(sanitize_hex_color("#00ACC1"), Some("#00acc1".to_string()));
        assert_eq!(sanitize_hex_color(" #ff7eb6 "), Some("#ff7eb6".to_string()));
        assert_eq!(
            sanitize_hex_color("#112233ff"),
            Some("#112233ff".to_string())
        );

        // Reject CSS injection attempts or malformed hex strings
        assert_eq!(sanitize_hex_color("red; } * { padding: 999px"), None);
        assert_eq!(sanitize_hex_color("#12345"), None);
        assert_eq!(sanitize_hex_color("#12345g"), None);
        assert_eq!(sanitize_hex_color("00acc1"), None);
    }

    #[test]
    fn test_parse_minimal_community_theme_derives_defaults() {
        let toml_src = r##"
[meta]
name = "Synthetic Pastel"
author = "tester"
description = "Minimal synthetic test theme"
mode = "light"
tags = ["pastel", "test"]

[colors]
window_bg = "#fff5f8"
window_fg = "#1e1b24"
accent = "#ff7eb6"
"##;

        let parsed = parse_community_theme_toml("synthetic-pastel", toml_src)
            .expect("minimal community theme should parse");
        assert_eq!(parsed.id, "synthetic-pastel");
        assert_eq!(parsed.name, "Synthetic Pastel");
        assert_eq!(parsed.author, "tester");
        assert!(!parsed.is_dark);
        assert!(!parsed.is_builtin);
        assert_eq!(parsed.colors.window_bg, "#fff5f8");
        assert_eq!(parsed.colors.sidebar_bg, "#fff5f8");
        assert_eq!(parsed.colors.card_bg, "#fff5f8");
        assert_eq!(parsed.colors.popover_bg, "#fff5f8");
        assert_eq!(parsed.colors.accent, "#ff7eb6");
        assert_eq!(parsed.colors.ink_border, "#111418");
    }

    #[test]
    fn test_parse_community_theme_rejects_css_injection() {
        let malicious = r##"
[meta]
name = "Bad Theme"

[colors]
window_bg = "#ffffff; @import url('https://example.com/evil.css');"
window_fg = "#000000"
accent = "#00acc1"
"##;
        assert!(parse_community_theme_toml("bad-theme", malicious).is_none());
    }

    #[test]
    fn test_discover_community_themes_in_tempdir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let valid_toml = r##"
[meta]
name = "Sunset Glow"
author = "synth"
mode = "dark"

[colors]
window_bg = "#1a1423"
window_fg = "#f8f0fb"
accent = "#f77f00"
"##;
        fs::write(dir.path().join("sunset-glow.toml"), valid_toml).unwrap();
        fs::write(dir.path().join("ignored.txt"), "not a toml file").unwrap();

        let themes = discover_community_themes_in(dir.path());
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].id, "sunset-glow");
        assert_eq!(themes[0].name, "Sunset Glow");
        assert!(themes[0].is_dark);
    }

    #[test]
    fn test_accent_override_css_generation() {
        let css = generate_accent_override_css("#00acc1").expect("valid hex");
        assert!(css.contains("@define-color accent_color #00acc1;"));
        assert!(css.contains("@define-color accent_bg_color #00acc1;"));
        assert!(css.contains("@define-color accent_fg_color #111418;"));
    }

    #[test]
    fn test_boxxy_skeleton_imports_all_exist_and_registered() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let themes_dir = repo_root.join("resources/themes");
        let skeleton_path = themes_dir.join("boxxy_skeleton.css");
        let skeleton_src =
            fs::read_to_string(&skeleton_path).expect("resources/themes/boxxy_skeleton.css exists");

        let mut imported_paths = Vec::new();
        for line in skeleton_src.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("@import \"")
                && let Some(rel_path) = rest.strip_suffix("\";")
            {
                imported_paths.push(rel_path.to_string());
            }
        }

        assert!(
            !imported_paths.is_empty(),
            "boxxy_skeleton.css must declare @import modules"
        );

        let gresource_xml = fs::read_to_string(repo_root.join("resources/resources.gresource.xml"))
            .expect("resources/resources.gresource.xml exists");

        for rel in &imported_paths {
            let module_path = themes_dir.join(rel);
            let content = fs::read_to_string(&module_path)
                .unwrap_or_else(|_| panic!("imported CSS module must exist: {}", module_path.display()));
            assert!(
                !content.trim().is_empty(),
                "imported CSS module must not be empty: {}",
                module_path.display()
            );
            let expected_alias = format!("alias=\"themes/{rel}\"");
            assert!(
                gresource_xml.contains(&expected_alias),
                "resources.gresource.xml must register imported module: {expected_alias}"
            );
        }
    }

    #[test]
    fn test_gtk_css_provider_parses_style_and_modular_skeleton_without_errors() {
        if gtk::init().is_err() {
            return;
        }

        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let style_path = repo_root.join("resources/style.css");
        let skeleton_path = repo_root.join("resources/themes/boxxy_skeleton.css");

        let errors: std::rc::Rc<RefCell<Vec<String>>> = std::rc::Rc::new(RefCell::new(Vec::new()));

        for css_path in [&style_path, &skeleton_path] {
            let provider = gtk::CssProvider::new();
            let err_sink = errors.clone();
            provider.connect_parsing_error(move |_prov, section, err| {
                let file_name = section
                    .file()
                    .and_then(|f| f.path())
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<inline>".to_string());
                let line = section.start_location().lines() + 1;
                err_sink
                    .borrow_mut()
                    .push(format!("{file_name}:{line}: {err}"));
            });
            provider.load_from_path(css_path);
        }

        let collected = errors.borrow();
        assert!(
            collected.is_empty(),
            "GTK CSS parser reported errors:\n{}",
            collected.join("\n")
        );
    }
}
