use super::oklch::{
    contrast_fg_for_hex, is_hex_dark, readable_color_on_surface, sanitize_hex_color,
};

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
    pub secondary: String,
    pub secondary_fg: String,
    pub highlight: String,
    pub highlight_fg: String,
    pub ink_border: String,
    pub shadow: String,
}

impl AppThemeColors {
    /// Returns the 4 signature preview swatches shown on theme rows:
    /// `[window_bg, accent, secondary, highlight]` — immediately communicating
    /// surface luminance alongside the theme's 3-color identity triad.
    #[must_use]
    pub fn preview_dots(&self) -> [&str; 4] {
        [
            &self.window_bg,
            &self.accent,
            &self.secondary,
            &self.highlight,
        ]
    }

    /// Computes contrast-clamped `(secondary_color, highlight_color)` suitable for
    /// standalone text or `GtkTextTag` token highlighting on top of `card_bg`.
    #[must_use]
    pub fn readable_secondary_and_highlight(&self) -> (String, String) {
        let is_card_dark = is_hex_dark(&self.card_bg);
        (
            readable_color_on_surface(&self.secondary, is_card_dark),
            readable_color_on_surface(&self.highlight, is_card_dark),
        )
    }

    /// Generates the `@define-color` CSS block for Layer 3 following Libadwaita conventions.
    #[must_use]
    pub fn to_gtk_css(&self, is_dark: bool) -> String {
        // Semantic status text/badges and standalone secondary/highlight text render on top of
        // `@card_bg_color`, so their foreground contrast follows `is_hex_dark(card_bg)` rather
        // than `window_bg` (supporting hybrid themes).
        let is_card_dark = is_hex_dark(&self.card_bg);
        let (secondary_readable, highlight_readable) = self.readable_secondary_and_highlight();

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
@define-color dialog_bg_color {popover_bg};
@define-color dialog_fg_color {popover_fg};
@define-color popover_bg_color {popover_bg};
@define-color popover_fg_color {popover_fg};
@define-color accent_color {accent};
@define-color accent_bg_color {accent};
@define-color accent_fg_color {accent_fg};
@define-color boxxy_secondary_bg_color {secondary};
@define-color boxxy_secondary_fg_color {secondary_fg};
@define-color boxxy_secondary_color {secondary_readable};
@define-color boxxy_secondary_subtle alpha(@boxxy_secondary_bg_color, 0.16);
@define-color boxxy_highlight_bg_color {highlight};
@define-color boxxy_highlight_fg_color {highlight_fg};
@define-color boxxy_highlight_color {highlight_readable};
@define-color boxxy_highlight_subtle alpha(@boxxy_highlight_bg_color, 0.22);
@define-color boxxy_search_match_bg alpha(@boxxy_highlight_bg_color, 0.32);
@define-color boxxy_search_match_fg @card_fg_color;
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
            secondary = self.secondary,
            secondary_fg = self.secondary_fg,
            highlight = self.highlight,
            highlight_fg = self.highlight_fg,
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
            description:
                "Carbon dark surfaces with peanut green accents, sky cyan tags, and warm solar highlights."
                    .to_string(),
            is_dark: true,
            is_builtin: true,
            tags: vec![
                "dark".to_string(),
                "carbon".to_string(),
                "default".to_string(),
            ],
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
                secondary: "#7dc4e4".to_string(),
                secondary_fg: "#0b1620".to_string(),
                highlight: "#f5b971".to_string(),
                highlight_fg: "#201407".to_string(),
                ink_border: "#050505".to_string(),
                shadow: "#050505".to_string(),
            },
        },
        AppThemeDefinition {
            id: "boxxy-light".to_string(),
            name: "Boxxy Light".to_string(),
            author: "Boxxy Team".to_string(),
            description:
                "Crisp comic-book ink outlines on warm mint paper with cyan, iris lavender, and tangerine."
                    .to_string(),
            is_dark: false,
            is_builtin: true,
            tags: vec![
                "light".to_string(),
                "playful".to_string(),
                "clean".to_string(),
            ],
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
                secondary: "#a594f9".to_string(),
                secondary_fg: "#111418".to_string(),
                highlight: "#ff9f43".to_string(),
                highlight_fg: "#111418".to_string(),
                ink_border: "#111418".to_string(),
                shadow: "#111418".to_string(),
            },
        },
    ]
}

/// Generates the Layer 4 custom accent color override CSS block.
///
/// Intentionally overrides only `@accent_*` tokens so a user's personal accent choice
/// does not clobber a theme's curated `secondary` and `highlight` roles.
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
