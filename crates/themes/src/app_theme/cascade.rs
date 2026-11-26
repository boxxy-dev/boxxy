use super::colors::{
    adwaita_readable_secondary_and_highlight, generate_accent_override_css,
    generate_adwaita_tokens_css,
};
use super::community::resolve_app_theme;
use super::oklch::is_hex_dark;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::{Cell, RefCell};

const PRIORITY_BOXXY_SKELETON: u32 = gtk::STYLE_PROVIDER_PRIORITY_USER;
const PRIORITY_BOXXY_TOKENS: u32 = gtk::STYLE_PROVIDER_PRIORITY_USER + 10;
const PRIORITY_BOXXY_ACCENT: u32 = gtk::STYLE_PROVIDER_PRIORITY_USER + 20;

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
    static DARK_NOTIFY_INSTALLED: Cell<bool> = const { Cell::new(false) };
}

/// Returns contrast-clamped `(file_mention_hex, char_mention_hex)` for non-CSS consumers
/// such as `GtkTextTag` syntax highlighting in the Claw input bar.
///
/// Returns the active theme's `(secondary_color, highlight_color)` in both `AppThemeMode::Boxxy`
/// and `AppThemeMode::DefaultGtk` (Legacy Adwaita).
#[must_use]
pub fn active_mention_colors() -> (String, String) {
    LAST_APPLIED_STATE.with(|cell| {
        if let Some(state) = cell.borrow().as_ref() {
            return state.mention_colors.clone();
        }
        adwaita_readable_secondary_and_highlight(true)
    })
}

/// Applies or unloads the custom Boxxy App Theme cascade and updates `adw::StyleManager`.
///
/// - When `use_boxxy_theme` is `false` (Default GTK / Legacy Adwaita mode), unloads the Boxxy
///   skeleton and custom accent providers (so geometry stays native Adwaita and `@accent_*`
///   follows the OS system accent color), applies `fallback_scheme`, and loads Adwaita's
///   dark/light `secondary` and `highlight` token definitions into Layer 3 (`TOKENS_PROVIDER`).
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
    ensure_dark_notify_listener(&style_manager);

    if !use_boxxy_theme {
        if style_manager.color_scheme() != fallback_scheme {
            style_manager.set_color_scheme(fallback_scheme);
        }
        let is_dark = style_manager.is_dark();
        let tokens_css = generate_adwaita_tokens_css(is_dark);
        let mention_colors = adwaita_readable_secondary_and_highlight(is_dark);

        let target_state = AppliedThemeState {
            use_boxxy_theme: false,
            tokens_css: tokens_css.clone(),
            accent_css: None,
            color_scheme: fallback_scheme,
            mention_colors,
        };
        let already_applied =
            LAST_APPLIED_STATE.with(|cell| cell.borrow().as_ref() == Some(&target_state));
        if already_applied {
            return;
        }

        remove_provider(&SKELETON_PROVIDER);
        remove_provider(&ACCENT_PROVIDER);

        let tokens_changed = LAST_APPLIED_STATE.with(|cell| {
            cell.borrow()
                .as_ref()
                .is_none_or(|prev| prev.tokens_css != tokens_css)
        });
        if tokens_changed {
            set_string_provider(&TOKENS_PROVIDER, &tokens_css, PRIORITY_BOXXY_TOKENS);
        }

        set_display_card_dark(is_dark);
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
            .is_none_or(|prev| prev.tokens_css != tokens_css)
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

/// Keeps Legacy Adwaita's `secondary` and `highlight` CSS tokens and `"boxxy-card-is-dark"`
/// display property synchronized when the OS switches dark/light under `System Follow`.
fn ensure_dark_notify_listener(style_manager: &adw::StyleManager) {
    let already_installed = DARK_NOTIFY_INSTALLED.with(|installed| installed.replace(true));
    if already_installed {
        return;
    }

    style_manager.connect_dark_notify(|sm| {
        let should_sync_adwaita = LAST_APPLIED_STATE.with(|cell| {
            cell.borrow()
                .as_ref()
                .is_some_and(|state| !state.use_boxxy_theme)
        });
        if !should_sync_adwaita {
            return;
        }

        let is_dark = sm.is_dark();
        let tokens_css = generate_adwaita_tokens_css(is_dark);
        let mention_colors = adwaita_readable_secondary_and_highlight(is_dark);

        let changed = LAST_APPLIED_STATE.with(|cell| {
            let mut guard = cell.borrow_mut();
            let Some(state) = guard.as_mut() else {
                return false;
            };
            if state.tokens_css == tokens_css && state.mention_colors == mention_colors {
                return false;
            }
            state.tokens_css.clone_from(&tokens_css);
            state.mention_colors = mention_colors;
            true
        });

        if changed {
            set_string_provider(&TOKENS_PROVIDER, &tokens_css, PRIORITY_BOXXY_TOKENS);
            set_display_card_dark(is_dark);
        }
    });
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
