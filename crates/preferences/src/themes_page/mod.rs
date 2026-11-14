use crate::config::{AppThemeMode, ColorScheme, Settings};
use crate::ui_utils::{
    PageHooks, VoidCallbackSlot, bind_mapped_poller, make_editable_color_dot, open_directory,
};
use adw::prelude::*;
use boxxy_themes::ColorDot;
use boxxy_themes::app_theme::{
    ACCENT_PRESETS, AppThemeDefinition, builtin_app_themes, discover_community_themes,
    ensure_app_themes_dir, resolve_app_theme,
};
use gtk4 as gtk;
use gtk4::glib;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
enum ThemeRowAction {
    BoxxyTheme(String),
    LegacyAdwaita(ColorScheme),
}

fn open_themes_folder(widget: &impl IsA<gtk::Widget>) {
    let widget_weak = widget.downgrade();
    glib::spawn_future_local(async move {
        let dir = boxxy_sys_utils::runtime()
            .spawn_blocking(ensure_app_themes_dir)
            .await
            .unwrap_or_else(|_| boxxy_sys_utils::get_config_dir().join("themes"));
        if let Some(w) = widget_weak.upgrade() {
            open_directory(
                &w,
                dir,
                Some("Drop .toml theme files into the themes folder to load them."),
            );
        }
    });
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
        dots_box.append(&ColorDot::new(dot_hex, 14));
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

#[allow(clippy::too_many_lines)]
pub(crate) fn setup_themes_page(builder: &gtk::Builder) -> PageHooks {
    let page: adw::PreferencesPage = builder.object("page_themes").unwrap();

    let static_rows: Rc<RefCell<Vec<(ThemeRowAction, gtk::Image)>>> =
        Rc::new(RefCell::new(Vec::new()));
    let refresh_selection_ui: VoidCallbackSlot = Rc::new(RefCell::new(None));
    let initial = Settings::load();

    // ── 1. Boxxy Official Themes Group ────────────────────────────────────────
    let official_group = adw::PreferencesGroup::new();
    official_group.set_title("Boxxy Official Themes");
    official_group.set_description(Some(
        "Official playful themes with crisp ink borders and tactile controls.",
    ));

    for theme in builtin_app_themes() {
        let is_selected = initial.app_theme_mode == AppThemeMode::Boxxy
            && initial.app_theme_id.eq_ignore_ascii_case(&theme.id);
        let dots = theme.colors.preview_dots();
        let theme_id = theme.id.clone();
        let refresh_ui = refresh_selection_ui.clone();

        let (row, check_img) = build_theme_action_row(
            &theme.name,
            &theme.description,
            &theme.tags,
            &dots,
            is_selected,
            move || {
                Settings::update(|s| {
                    s.app_theme_mode = AppThemeMode::Boxxy;
                    s.app_theme_id.clone_from(&theme_id);
                });
                if let Some(ref f) = *refresh_ui.borrow() {
                    f();
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
        dot_btn.set_child(Some(&ColorDot::new(preset_hex, 18)));

        dot_btn.connect_clicked(move |_| {
            Settings::update(|s| {
                s.custom_accent_color = Some(hex_owned.clone());
            });
            if let Some(ref f) = *refresh_ui.borrow() {
                f();
            }
        });
        accent_suffix_box.append(&dot_btn);
    }

    let initial_accent = initial
        .custom_accent_color
        .clone()
        .unwrap_or_else(|| resolve_app_theme(&initial.app_theme_id).colors.accent);
    let refresh_ui_custom = refresh_selection_ui.clone();
    let (custom_picker_btn, update_custom_picker_color) = make_editable_color_dot(
        &initial_accent,
        "Pick a custom accent color…",
        move |chosen_hex| {
            Settings::update(|s| {
                s.custom_accent_color = Some(chosen_hex.clone());
            });
            if let Some(ref f) = *refresh_ui_custom.borrow() {
                f();
            }
        },
    );
    accent_suffix_box.append(&custom_picker_btn);

    let refresh_ui_reset = refresh_selection_ui.clone();
    reset_accent_btn.connect_clicked(move |_| {
        Settings::update(|s| {
            s.custom_accent_color = None;
        });
        if let Some(ref f) = *refresh_ui_reset.borrow() {
            f();
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
    open_folder_row.connect_activated(open_themes_folder);
    community_group.add(&open_folder_row);
    page.add(&community_group);

    // ── 4. Legacy Adwaita Themes Group ────────────────────────────────────────
    let legacy_group = adw::PreferencesGroup::new();
    legacy_group.set_title("Legacy Adwaita Themes");
    legacy_group.set_description(Some(
        "You can still use standard Adwaita themes, though our main focus is on Boxxy themes.",
    ));

    let legacy_specs = [
        (
            "System Follow",
            "Follow your operating system's dark or light GTK preference.",
            vec!["system".to_string(), "adwaita".to_string()],
            ["#242424", "#3584e4", "#62a0ea", "#e5a50a"],
            ColorScheme::Default,
        ),
        (
            "System Dark",
            "Standard Libadwaita dark appearance.",
            vec!["dark".to_string(), "adwaita".to_string()],
            ["#1e1e1e", "#3584e4", "#62a0ea", "#f8e45c"],
            ColorScheme::Dark,
        ),
        (
            "System Light",
            "Standard Libadwaita light appearance.",
            vec!["light".to_string(), "adwaita".to_string()],
            ["#fafafa", "#3584e4", "#1c71d8", "#e5a50a"],
            ColorScheme::Light,
        ),
    ];

    for (title, subtitle, tags, dots, scheme) in legacy_specs {
        let is_selected =
            initial.app_theme_mode == AppThemeMode::DefaultGtk && initial.color_scheme == scheme;
        let refresh_ui = refresh_selection_ui.clone();

        let (row, check_img) = build_theme_action_row(
            title,
            subtitle,
            &tags,
            &dots,
            is_selected,
            move || {
                Settings::update(|s| {
                    s.app_theme_mode = AppThemeMode::DefaultGtk;
                    s.color_scheme = scheme;
                });
                if let Some(ref f) = *refresh_ui.borrow() {
                    f();
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
    let apply_selection_states: Rc<dyn Fn(&Settings)> = {
        let community_rows = community_rows.clone();
        let accent_group = accent_group.clone();

        Rc::new(move |s: &Settings| {
            let is_boxxy = s.app_theme_mode == AppThemeMode::Boxxy;

            for (action, check_img) in static_rows.borrow().iter() {
                let selected = match action {
                    ThemeRowAction::BoxxyTheme(id) => {
                        is_boxxy && id.eq_ignore_ascii_case(&s.app_theme_id)
                    }
                    ThemeRowAction::LegacyAdwaita(scheme) => !is_boxxy && s.color_scheme == *scheme,
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
                accent_row
                    .set_subtitle(&glib::markup_escape_text(&format!("Custom ({custom_hex})")));
                reset_accent_btn.set_sensitive(true);
            } else {
                accent_row.set_subtitle(&glib::markup_escape_text(&format!(
                    "{} default ({})",
                    active_theme.name, active_theme.colors.accent
                )));
                reset_accent_btn.set_sensitive(false);
            }
        })
    };

    let update_selection_states: Rc<dyn Fn()> = {
        let apply = apply_selection_states.clone();
        Rc::new(move || {
            apply(&Settings::load());
        })
    };
    *refresh_selection_ui.borrow_mut() = Some(update_selection_states.clone());
    apply_selection_states(&initial);

    // Asynchronously scan community themes off the GTK main thread
    let scan_in_flight = Rc::new(Cell::new(false));
    let trigger_community_scan: Rc<dyn Fn(bool)> = {
        let community_group = community_group.clone();
        let open_folder_row = open_folder_row;
        let community_rows = community_rows;
        let last_community_list = last_community_list.clone();
        let update_ui = update_selection_states;

        Rc::new(move |force: bool| {
            if scan_in_flight.get() {
                return;
            }
            scan_in_flight.set(true);

            let community_group = community_group.clone();
            let open_folder_row = open_folder_row.clone();
            let community_rows = community_rows.clone();
            let last_community_list = last_community_list.clone();
            let update_ui = update_ui.clone();
            let scan_in_flight = scan_in_flight.clone();

            glib::spawn_future_local(async move {
                let discovered = boxxy_sys_utils::runtime()
                    .spawn_blocking(discover_community_themes)
                    .await
                    .unwrap_or_default();
                scan_in_flight.set(false);

                let unchanged = !force
                    && last_community_list
                        .borrow()
                        .as_ref()
                        .is_some_and(|prev| prev == &discovered);
                if unchanged {
                    return;
                }
                let had_previous = last_community_list.borrow().is_some();
                *last_community_list.borrow_mut() = Some(discovered.clone());

                for (_, old_row, _) in community_rows.borrow_mut().drain(..) {
                    community_group.remove(&old_row);
                }
                community_group.remove(&open_folder_row);

                let current = Settings::load();
                for theme in discovered {
                    let subtitle = if theme.description.is_empty() {
                        format!("by {}", theme.author)
                    } else {
                        format!("{} · by {}", theme.description, theme.author)
                    };
                    let is_selected = current.app_theme_mode == AppThemeMode::Boxxy
                        && current.app_theme_id.eq_ignore_ascii_case(&theme.id);
                    let dots = theme.colors.preview_dots();
                    let theme_id = theme.id.clone();
                    let update_ui = update_ui.clone();

                    let (row, check_img) = build_theme_action_row(
                        &theme.name,
                        &subtitle,
                        &theme.tags,
                        &dots,
                        is_selected,
                        move || {
                            Settings::update(|s| {
                                s.app_theme_id.clone_from(&theme_id);
                                s.app_theme_mode = AppThemeMode::Boxxy;
                            });
                            update_ui();
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

                // If a community `.toml` changed during live polling while Boxxy theme mode
                // is active, notify all windows so edits apply immediately.
                if !force && had_previous && Settings::load().app_theme_mode == AppThemeMode::Boxxy
                {
                    Settings::notify_changed();
                }
            });
        })
    };

    // Kick off initial non-blocking discovery and single-instance 500ms poll while mapped
    trigger_community_scan(true);
    {
        let scan_map = trigger_community_scan.clone();
        let scan_tick = trigger_community_scan;
        bind_mapped_poller(
            &page,
            Duration::from_millis(500),
            move || scan_map(false),
            move || scan_tick(false),
        );
    }

    let sync = Box::new(move |s: &Settings| {
        apply_selection_states(s);
    });

    let filter = Box::new(move |query: &str| {
        if query.is_empty() {
            official_group.set_visible(true);
            accent_group.set_visible(true);
            community_group.set_visible(true);
            legacy_group.set_visible(true);
            return true;
        }

        let static_keywords = "theme themes boxxy dark light official legacy adwaita system follow gtk color scheme accent custom community toml cotton candy bauhaus";
        let community_match = last_community_list.borrow().as_ref().is_some_and(|list| {
            list.iter().any(|t| {
                t.name.to_lowercase().contains(query)
                    || t.id.to_lowercase().contains(query)
                    || t.author.to_lowercase().contains(query)
                    || t.description.to_lowercase().contains(query)
                    || t.tags.iter().any(|tag| tag.to_lowercase().contains(query))
            })
        });
        let matched = static_keywords.contains(query) || community_match;
        official_group.set_visible(matched);
        accent_group.set_visible(matched);
        community_group.set_visible(matched);
        legacy_group.set_visible(matched);
        matched
    });

    (sync, filter)
}
