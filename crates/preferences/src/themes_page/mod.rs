use crate::config::{AppThemeMode, ColorScheme, Settings};
use adw::prelude::*;
use boxxy_themes::app_theme::{
    ACCENT_PRESETS, AppThemeDefinition, builtin_app_themes, discover_community_themes,
    ensure_app_themes_dir, resolve_app_theme, sanitize_hex_color,
};
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq, Eq)]
enum ThemeRowAction {
    BoxxyTheme(String),
    LegacyAdwaita(ColorScheme),
}

type RefreshCallbackCell = Rc<RefCell<Option<Rc<dyn Fn()>>>>;

fn open_themes_folder(widget: &impl IsA<gtk::Widget>) {
    let dir = ensure_app_themes_dir();
    let uri = format!("file://{}", dir.display());
    let _ = gtk::gio::AppInfo::launch_default_for_uri(&uri, None::<&gtk::gio::AppLaunchContext>);

    if let Some(overlay) = widget
        .ancestor(adw::ToastOverlay::static_type())
        .and_downcast::<adw::ToastOverlay>()
    {
        let toast = adw::Toast::new("Drop .toml theme files into the themes folder to load them.");
        toast.set_timeout(3);
        overlay.add_toast(toast);
    }
}

/// Creates a small static circular color dot widget for theme list rows and accent swatches.
fn make_static_color_dot(hex: &str, size: i32) -> gtk::Box {
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let class = format!("theme-preview-dot-{n}");

    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.set_size_request(size, size);
    dot.set_valign(gtk::Align::Center);
    dot.set_halign(gtk::Align::Center);
    dot.add_css_class("theme-preview-dot");
    dot.add_css_class(&class);

    let safe_hex = sanitize_hex_color(hex).unwrap_or_else(|| "#808080".to_string());
    let provider = gtk::CssProvider::new();
    let css = format!(
        ".{class} {{ \
            background-color: {safe_hex}; \
            min-width: {size}px; \
            min-height: {size}px; \
        }}"
    );
    provider.load_from_string(&css);

    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_USER + 50,
        );
        let provider_cleanup = provider;
        dot.connect_destroy(move |_| {
            if let Some(display) = gdk::Display::default() {
                gtk::style_context_remove_provider_for_display(&display, &provider_cleanup);
            }
        });
    }

    dot
}

/// Builds a standardized `AdwActionRow` with tag pills, 4 color preview dots, and an active checkmark.
fn build_theme_action_row(
    title: &str,
    subtitle: &str,
    tags: &[String],
    dots: &[&str; 4],
    is_selected: bool,
    on_activate: impl Fn() + 'static,
) -> (adw::ActionRow, gtk::Image) {
    let row = adw::ActionRow::new();
    row.set_title(&glib::markup_escape_text(title));
    row.set_subtitle(&glib::markup_escape_text(subtitle));
    row.set_activatable(true);

    let suffix_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    suffix_box.set_valign(gtk::Align::Center);

    for tag in tags.iter().take(3) {
        let pill = gtk::Label::new(Some(tag));
        pill.add_css_class("boxxy-theme-tag-pill");
        pill.set_valign(gtk::Align::Center);
        suffix_box.append(&pill);
    }

    let dots_box = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    dots_box.set_valign(gtk::Align::Center);
    dots_box.set_margin_start(4);
    for dot_hex in dots {
        dots_box.append(&make_static_color_dot(dot_hex, 14));
    }
    suffix_box.append(&dots_box);

    let check_img = gtk::Image::from_icon_name("boxxy-object-select-symbolic");
    check_img.set_valign(gtk::Align::Center);
    check_img.set_margin_start(6);
    check_img.set_visible(is_selected);
    suffix_box.append(&check_img);

    row.add_suffix(&suffix_box);
    row.connect_activated(move |_| {
        on_activate();
    });

    (row, check_img)
}

pub fn setup_themes_page(
    builder: &gtk::Builder,
    settings_rc: Rc<RefCell<Settings>>,
    on_change: Rc<dyn Fn(Settings) + 'static>,
) -> Box<dyn Fn(&str) -> bool> {
    let page: adw::PreferencesPage = builder.object("page_themes").unwrap();

    let static_rows: Rc<RefCell<Vec<(ThemeRowAction, gtk::Image)>>> =
        Rc::new(RefCell::new(Vec::new()));
    let refresh_selection_ui: RefreshCallbackCell = Rc::new(RefCell::new(None));

    // ── 1. Boxxy Official Themes Group ────────────────────────────────────────
    let official_group = adw::PreferencesGroup::new();
    official_group.set_title("Boxxy Official Themes");
    official_group.set_description(Some(
        "Official playful themes with crisp ink borders and tactile controls.",
    ));

    for theme in builtin_app_themes() {
        let is_selected = {
            let s = settings_rc.borrow();
            s.app_theme_mode == AppThemeMode::Boxxy
                && s.app_theme_id.eq_ignore_ascii_case(&theme.id)
        };
        let dots = theme.colors.preview_dots();
        let theme_id = theme.id.clone();
        let s_rc = settings_rc.clone();
        let cb = on_change.clone();
        let refresh_ui = refresh_selection_ui.clone();

        let (row, check_img) = build_theme_action_row(
            &theme.name,
            &theme.description,
            &theme.tags,
            &dots,
            is_selected,
            move || {
                let mut s = s_rc.borrow_mut();
                if s.app_theme_mode != AppThemeMode::Boxxy || s.app_theme_id != theme_id {
                    s.app_theme_mode = AppThemeMode::Boxxy;
                    s.app_theme_id = theme_id.clone();
                    s.save();
                    let snapshot = s.clone();
                    drop(s);
                    if let Some(ref f) = *refresh_ui.borrow() {
                        f();
                    }
                    cb(snapshot);
                }
            },
        );

        official_group.add(&row);
        static_rows
            .borrow_mut()
            .push((ThemeRowAction::BoxxyTheme(theme.id), check_img));
    }
    page.add(&official_group);

    // ── 2. Accent Color Group ─────────────────────────────────────────────────
    let accent_group = adw::PreferencesGroup::new();
    accent_group.set_title("Accent Color");
    accent_group.set_description(Some(
        "Override the highlight color for active buttons, switches, and sidebar pills.",
    ));

    let accent_row = adw::ActionRow::new();
    accent_row.set_title("Accent Color");

    let accent_suffix_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    accent_suffix_box.set_valign(gtk::Align::Center);

    let reset_accent_btn = gtk::Button::with_label("Default");
    reset_accent_btn.add_css_class("flat");
    reset_accent_btn.set_valign(gtk::Align::Center);
    reset_accent_btn.set_tooltip_text(Some("Use the active theme's built-in accent color"));

    for &(preset_name, preset_hex) in ACCENT_PRESETS {
        let s_rc = settings_rc.clone();
        let cb = on_change.clone();
        let refresh_ui = refresh_selection_ui.clone();
        let hex_owned = preset_hex.to_string();

        let dot_btn = gtk::Button::new();
        dot_btn.set_size_request(22, 22);
        dot_btn.set_valign(gtk::Align::Center);
        dot_btn.set_halign(gtk::Align::Center);
        dot_btn.set_focus_on_click(false);
        dot_btn.set_cursor_from_name(Some("pointer"));
        dot_btn.set_tooltip_text(Some(preset_name));
        dot_btn.add_css_class("flat");
        dot_btn.add_css_class("circular");
        dot_btn.set_child(Some(&make_static_color_dot(preset_hex, 18)));

        dot_btn.connect_clicked(move |_| {
            let mut s = s_rc.borrow_mut();
            if s.custom_accent_color.as_deref() != Some(hex_owned.as_str()) {
                s.custom_accent_color = Some(hex_owned.clone());
                s.save();
                let snapshot = s.clone();
                drop(s);
                if let Some(ref f) = *refresh_ui.borrow() {
                    f();
                }
                cb(snapshot);
            }
        });
        accent_suffix_box.append(&dot_btn);
    }

    let initial_accent = settings_rc
        .borrow()
        .custom_accent_color
        .clone()
        .unwrap_or_else(|| {
            resolve_app_theme(&settings_rc.borrow().app_theme_id)
                .colors
                .accent
        });
    let s_rc_custom = settings_rc.clone();
    let cb_custom = on_change.clone();
    let refresh_ui_custom = refresh_selection_ui.clone();
    let (custom_picker_btn, update_custom_picker_color) =
        crate::characters::make_editable_color_dot(
            &initial_accent,
            "Pick a custom accent color…",
            move |chosen_hex| {
                let mut s = s_rc_custom.borrow_mut();
                if s.custom_accent_color.as_deref() != Some(chosen_hex.as_str()) {
                    s.custom_accent_color = Some(chosen_hex);
                    s.save();
                    let snapshot = s.clone();
                    drop(s);
                    if let Some(ref f) = *refresh_ui_custom.borrow() {
                        f();
                    }
                    cb_custom(snapshot);
                }
            },
        );
    accent_suffix_box.append(&custom_picker_btn);

    let s_rc_reset = settings_rc.clone();
    let cb_reset = on_change.clone();
    let refresh_ui_reset = refresh_selection_ui.clone();
    reset_accent_btn.connect_clicked(move |_| {
        let mut s = s_rc_reset.borrow_mut();
        if s.custom_accent_color.is_some() {
            s.custom_accent_color = None;
            s.save();
            let snapshot = s.clone();
            drop(s);
            if let Some(ref f) = *refresh_ui_reset.borrow() {
                f();
            }
            cb_reset(snapshot);
        }
    });
    accent_suffix_box.append(&reset_accent_btn);

    accent_row.add_suffix(&accent_suffix_box);
    accent_group.add(&accent_row);
    page.add(&accent_group);

    // ── 3. Community Themes Group ─────────────────────────────────────────────
    let community_group = adw::PreferencesGroup::new();
    community_group.set_title("Community Themes");
    community_group.set_description(Some(
        "Color-only TOML themes loaded from your Boxxy configuration directory.",
    ));

    let open_folder_row = adw::ActionRow::new();
    open_folder_row.set_title("Open Themes Folder");
    open_folder_row.set_subtitle("Add or edit .toml community themes in your file manager");
    open_folder_row.set_activatable(true);
    let folder_icon = gtk::Image::from_icon_name("boxxy-folder-open-symbolic");
    folder_icon.set_valign(gtk::Align::Center);
    open_folder_row.add_suffix(&folder_icon);
    open_folder_row.connect_activated(|row| {
        open_themes_folder(row);
    });
    community_group.add(&open_folder_row);
    page.add(&community_group);

    // ── 4. Legacy Adwaita Themes Group ────────────────────────────────────────
    let legacy_group = adw::PreferencesGroup::new();
    legacy_group.set_title("Legacy Adwaita Themes");
    legacy_group.set_description(Some(
        "Note: Support for Legacy Adwaita themes may be dropped in a future release.",
    ));

    let legacy_specs = [
        (
            "System Follow",
            "Follow your operating system's dark or light GTK preference.",
            vec!["system".to_string(), "adwaita".to_string()],
            ["#3584e4", "#fafafa", "#242424", "#1e1e1e"],
            ColorScheme::Default,
        ),
        (
            "System Dark",
            "Standard Libadwaita dark appearance.",
            vec!["dark".to_string(), "adwaita".to_string()],
            ["#3584e4", "#1e1e1e", "#2d2d2d", "#111111"],
            ColorScheme::Dark,
        ),
        (
            "System Light",
            "Standard Libadwaita light appearance.",
            vec!["light".to_string(), "adwaita".to_string()],
            ["#3584e4", "#fafafa", "#ffffff", "#c0bfbc"],
            ColorScheme::Light,
        ),
    ];

    for (title, subtitle, tags, dots, scheme) in legacy_specs {
        let is_selected = {
            let s = settings_rc.borrow();
            s.app_theme_mode == AppThemeMode::DefaultGtk && s.color_scheme == scheme
        };
        let s_rc = settings_rc.clone();
        let cb = on_change.clone();
        let refresh_ui = refresh_selection_ui.clone();

        let (row, check_img) = build_theme_action_row(
            title,
            subtitle,
            &tags,
            &dots,
            is_selected,
            move || {
                let mut s = s_rc.borrow_mut();
                if s.app_theme_mode != AppThemeMode::DefaultGtk || s.color_scheme != scheme {
                    s.app_theme_mode = AppThemeMode::DefaultGtk;
                    s.color_scheme = scheme;
                    s.save();
                    let snapshot = s.clone();
                    drop(s);
                    if let Some(ref f) = *refresh_ui.borrow() {
                        f();
                    }
                    cb(snapshot);
                }
            },
        );

        legacy_group.add(&row);
        static_rows
            .borrow_mut()
            .push((ThemeRowAction::LegacyAdwaita(scheme), check_img));
    }
    page.add(&legacy_group);

    let community_rows: Rc<RefCell<Vec<(String, adw::ActionRow, gtk::Image)>>> =
        Rc::new(RefCell::new(Vec::new()));
    let last_community_list: Rc<RefCell<Option<Vec<AppThemeDefinition>>>> =
        Rc::new(RefCell::new(None));

    // Synchronize active checkmarks and subtitles across Official, Accent, Community, and Legacy rows
    let update_selection_states: Rc<dyn Fn()> = {
        let settings_rc = settings_rc.clone();
        let static_rows = static_rows.clone();
        let community_rows = community_rows.clone();
        let accent_row = accent_row.clone();
        let accent_group = accent_group.clone();
        let reset_accent_btn = reset_accent_btn.clone();

        Rc::new(move || {
            let s = settings_rc.borrow();
            let is_boxxy = s.app_theme_mode == AppThemeMode::Boxxy;

            for (action, check_img) in static_rows.borrow().iter() {
                let selected = match action {
                    ThemeRowAction::BoxxyTheme(id) => {
                        is_boxxy && id.eq_ignore_ascii_case(&s.app_theme_id)
                    }
                    ThemeRowAction::LegacyAdwaita(scheme) => {
                        !is_boxxy && s.color_scheme == *scheme
                    }
                };
                check_img.set_visible(selected);
            }

            for (id, _row, check_img) in community_rows.borrow().iter() {
                let selected = is_boxxy && id.eq_ignore_ascii_case(&s.app_theme_id);
                check_img.set_visible(selected);
            }

            accent_group.set_sensitive(is_boxxy);
            let active_theme = resolve_app_theme(&s.app_theme_id);
            let effective_accent = s
                .custom_accent_color
                .as_deref()
                .unwrap_or(&active_theme.colors.accent);
            update_custom_picker_color(effective_accent);

            if !is_boxxy {
                accent_row.set_subtitle("Select a Boxxy or Community theme to customize accent");
                reset_accent_btn.set_sensitive(false);
            } else if let Some(ref custom_hex) = s.custom_accent_color {
                accent_row.set_subtitle(&format!("Custom ({custom_hex})"));
                reset_accent_btn.set_sensitive(true);
            } else {
                accent_row.set_subtitle(&format!(
                    "{} default ({})",
                    active_theme.name, active_theme.colors.accent
                ));
                reset_accent_btn.set_sensitive(false);
            }
        })
    };
    *refresh_selection_ui.borrow_mut() = Some(update_selection_states.clone());

    // Populate / refresh Community Themes list
    let rebuild_community_rows: Rc<dyn Fn(bool)> = {
        let community_group = community_group.clone();
        let open_folder_row = open_folder_row.clone();
        let community_rows = community_rows.clone();
        let last_community_list = last_community_list.clone();
        let settings_rc = settings_rc.clone();
        let on_change = on_change.clone();
        let update_ui = update_selection_states.clone();

        Rc::new(move |force: bool| {
            let discovered = discover_community_themes();
            if !force
                && last_community_list
                    .borrow()
                    .as_ref()
                    .is_some_and(|prev| prev == &discovered)
            {
                return;
            }
            *last_community_list.borrow_mut() = Some(discovered.clone());

            for (_, old_row, _) in community_rows.borrow_mut().drain(..) {
                community_group.remove(&old_row);
            }
            community_group.remove(&open_folder_row);

            for theme in discovered {
                let subtitle = if theme.description.is_empty() {
                    format!("by {}", theme.author)
                } else {
                    format!("{} · by {}", theme.description, theme.author)
                };
                let is_selected = {
                    let s = settings_rc.borrow();
                    s.app_theme_mode == AppThemeMode::Boxxy
                        && s.app_theme_id.eq_ignore_ascii_case(&theme.id)
                };
                let dots = theme.colors.preview_dots();
                let theme_id = theme.id.clone();
                let s_rc = settings_rc.clone();
                let cb = on_change.clone();
                let update_ui = update_ui.clone();

                let (row, check_img) = build_theme_action_row(
                    &theme.name,
                    &subtitle,
                    &theme.tags,
                    &dots,
                    is_selected,
                    move || {
                        let mut s = s_rc.borrow_mut();
                        if s.app_theme_id != theme_id || s.app_theme_mode != AppThemeMode::Boxxy {
                            s.app_theme_id = theme_id.clone();
                            s.app_theme_mode = AppThemeMode::Boxxy;
                            s.save();
                            let snapshot = s.clone();
                            drop(s);
                            update_ui();
                            cb(snapshot);
                        }
                    },
                );

                community_group.add(&row);
                community_rows
                    .borrow_mut()
                    .push((theme.id.clone(), row, check_img));
            }

            // Always keep "Open Themes Folder" as the final row in Community Themes
            community_group.add(&open_folder_row);

            update_ui();

            // If this was a live hot-reload tick (`!force`) and the user's active theme is in
            // the community list, trigger `on_change` so any `.toml` edits apply immediately.
            if !force {
                let snapshot = settings_rc.borrow().clone();
                if snapshot.app_theme_mode == AppThemeMode::Boxxy {
                    on_change(snapshot);
                }
            }
        })
    };

    rebuild_community_rows(true);

    // Poll community themes folder every 500ms while the Themes page is mapped
    {
        let rebuild = rebuild_community_rows.clone();
        let page_weak = page.downgrade();
        page.connect_map(move |_| {
            rebuild(false);
            let rebuild_tick = rebuild.clone();
            let page_weak_tick = page_weak.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
                let Some(p) = page_weak_tick.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                if !p.is_mapped() {
                    return glib::ControlFlow::Break;
                }
                rebuild_tick(false);
                glib::ControlFlow::Continue
            });
        });
    }

    let official_group_filter = official_group;
    let accent_group_filter = accent_group;
    let community_group_filter = community_group;
    let legacy_group_filter = legacy_group;

    Box::new(move |query: &str| {
        if query.is_empty() {
            official_group_filter.set_visible(true);
            accent_group_filter.set_visible(true);
            community_group_filter.set_visible(true);
            legacy_group_filter.set_visible(true);
            return true;
        }

        let keywords = "theme themes boxxy dark light official legacy adwaita system follow gtk color scheme accent custom community toml cotton candy bauhaus";
        let matched = keywords.contains(query);
        official_group_filter.set_visible(matched);
        accent_group_filter.set_visible(matched);
        community_group_filter.set_visible(matched);
        legacy_group_filter.set_visible(matched);
        matched
    })
}
