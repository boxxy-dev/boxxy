use super::colors::generate_accent_override_css;
use super::community::resolve_app_theme;
use super::oklch::is_hex_dark;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::RefCell;

const PRIORITY_BOXXY_SKELETON: u32 = gtk::STYLE_PROVIDER_PRIORITY_USER;
const PRIORITY_BOXXY_TOKENS: u32 = gtk::STYLE_PROVIDER_PRIORITY_USER + 10;
const PRIORITY_BOXXY_ACCENT: u32 = gtk::STYLE_PROVIDER_PRIORITY_USER + 20;

/// Default Adwaita token colors for `@file` and `:character` input highlighting when
/// running in `AppThemeMode::DefaultGtk` (or before `apply_app_theme` is called).
const ADWAITA_FILE_MENTION_HEX: &str = "#62a0ea";
const ADWAITA_CHAR_MENTION_HEX: &str = "#c061cb";

#[derive(Debug, Clone, PartialEq, Eq)]
struct AppliedThemeState {
    use_boxxy_theme: bool,
    tokens_css: String,
    accent_css: Option<String>,
    color_scheme: adw::ColorScheme,
    mention_colors: (String, String),
}

thread_local! {
    static SKELETON_PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
    static TOKENS_PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
    static ACCENT_PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
    static LAST_APPLIED_STATE: RefCell<Option<AppliedThemeState>> = const { RefCell::new(None) };
}

/// Returns contrast-clamped `(file_mention_hex, char_mention_hex)` for non-CSS consumers
/// such as `GtkTextTag` syntax highlighting in the Claw input bar.
///
/// - When `AppThemeMode::Boxxy` is active: returns `(secondary_color, highlight_color)`
///   clamped against `card_bg` luminance.
/// - When `AppThemeMode::DefaultGtk` is active: returns stock Adwaita blue/purple.
#[must_use]
pub fn active_mention_colors() -> (String, String) {
    LAST_APPLIED_STATE.with(|cell| {
        if let Some(state) = cell.borrow().as_ref()
            && state.use_boxxy_theme
        {
            return state.mention_colors.clone();
        }
        (
            ADWAITA_FILE_MENTION_HEX.to_string(),
            ADWAITA_CHAR_MENTION_HEX.to_string(),
        )
    })
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
            mention_colors: (
                ADWAITA_FILE_MENTION_HEX.to_string(),
                ADWAITA_CHAR_MENTION_HEX.to_string(),
            ),
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
    let mention_colors = resolved.colors.readable_secondary_and_highlight();

    let target_state = AppliedThemeState {
        use_boxxy_theme: true,
        tokens_css: tokens_css.clone(),
        accent_css: accent_css.clone(),
        color_scheme: target_scheme,
        mention_colors,
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
