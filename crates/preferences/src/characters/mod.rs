use adw::prelude::*;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use libadwaita as adw;

pub fn setup_characters_page(builder: &gtk::Builder) -> Box<dyn Fn(&str) -> bool> {
    let page: adw::PreferencesPage = builder.object("page_characters").unwrap();

    let chars_dir = boxxy_claw_protocol::character_loader::get_characters_dir().ok();
    let characters = boxxy_claw_protocol::character_loader::load_characters().unwrap_or_default();
    let mut rows = Vec::new();

    // === Characters list group ===
    let chars_group = adw::PreferencesGroup::new();
    chars_group.set_title("Available Characters");
    chars_group.set_description(Some(
        "Characters available for assignment to terminal panes. \
        Changes only take effect after restarting the Boxxy daemon.\n\n\
        Note: If a character is removed, any of their past sessions \
        will automatically be reassigned to the first available character.",
    ));

    if characters.is_empty() {
        let label = gtk::Label::new(Some(
            "No characters found. Open the characters folder to add one.",
        ));
        label.set_wrap(true);
        label.set_margin_top(8);
        label.set_margin_bottom(8);
        label.add_css_class("dim-label");
        chars_group.add(&label);
    } else {
        for character in &characters {
            let row = adw::ActionRow::new();
            row.set_title(&glib::markup_escape_text(&character.config.display_name));
            row.set_subtitle(&glib::markup_escape_text(&character.config.duties));

            // Avatar — adw::Avatar handles circular clipping automatically
            let avatar = adw::Avatar::new(52, Some(&character.config.display_name), true);
            if character.has_avatar {
                if let Some(dir) = &chars_dir {
                    let avatar_path = dir.join(&character.config.name).join("AVATAR.png");
                    if let Ok(texture) = gdk::Texture::from_filename(&avatar_path) {
                        avatar.set_custom_image(Some(&texture));
                    }
                }
            }
            avatar.set_margin_top(8);
            avatar.set_margin_bottom(8);
            row.add_prefix(&avatar);

            // Color swatch
            let swatch = make_color_dot(&character.config.color);
            row.add_suffix(&swatch);

            chars_group.add(&row);
            rows.push((
                character.config.id.clone(),
                character.config.duties.clone(),
                row,
            ));
        }
    }

    page.add(&chars_group);

    // Setup reactivity: poll the CLAIMS_CACHE to show which agents are busy
    let rows_clone = rows.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
        let claims = boxxy_claw_protocol::characters::CLAIMS_CACHE.load();

        for (char_id, duties, row) in &rows_clone {
            let active_claim = claims.iter().find(|c| c.character_id == *char_id);

            if let Some(claim) = active_claim {
                let status_text = match claim.holder_kind {
                    boxxy_claw_protocol::characters::HolderKind::Pane => {
                        let petname = if claim.petname.is_empty() {
                            String::new()
                        } else {
                            format!(" ({})", claim.petname)
                        };
                        format!("Active in pane {}{}", claim.holder_id, petname)
                    }
                };
                row.set_subtitle(&format!(
                    "<b>{}</b>",
                    glib::markup_escape_text(&status_text)
                ));
                row.set_use_markup(true);
                row.add_css_class("success");
            } else {
                row.set_subtitle(&glib::markup_escape_text(duties));
                row.set_use_markup(false);
                row.remove_css_class("success");
            }
        }

        glib::ControlFlow::Continue
    });

    // === Management group ===
    let manage_group = adw::PreferencesGroup::new();
    manage_group.set_title("Manage");

    // Open characters folder
    let open_row = adw::ActionRow::new();
    open_row.set_title("Open Characters Folder");
    open_row.set_subtitle("Browse and edit character files in your file manager");
    open_row.set_activatable(true);
    let arrow = gtk::Image::from_icon_name("folder-open-symbolic");
    arrow.set_valign(gtk::Align::Center);
    open_row.add_suffix(&arrow);

    let chars_dir_open = chars_dir.clone();
    open_row.connect_activated(move |row| {
        let Some(dir) = &chars_dir_open else { return };
        let _ = std::fs::create_dir_all(dir);
        let uri = format!("file://{}", dir.display());
        let _ =
            gtk::gio::AppInfo::launch_default_for_uri(&uri, None::<&gtk::gio::AppLaunchContext>);

        // Show restart notice
        if let Some(window) = row
            .root()
            .and_then(|r| r.downcast::<adw::PreferencesWindow>().ok())
        {
            let toast = adw::Toast::new("Restart Boxxy to apply character changes.");
            // Slight delay since the file manager takes focus away immediately
            toast.set_timeout(3);
            window.add_toast(toast);
        }
    });
    manage_group.add(&open_row);

    // Reset to defaults
    let reset_row = adw::ActionRow::new();
    reset_row.set_title("Reset to Defaults");
    reset_row.set_subtitle(
        "Restore the three bundled characters — all existing characters will be removed",
    );

    let reset_btn = gtk::Button::with_label("Reset…");
    reset_btn.set_valign(gtk::Align::Center);
    reset_btn.add_css_class("destructive-action");
    reset_row.add_suffix(&reset_btn);

    reset_btn.connect_clicked(move |btn| {
        show_reset_confirmation(btn);
    });

    manage_group.add(&reset_row);
    page.add(&manage_group);

    let chars_group_clone = chars_group.clone();
    let manage_group_clone = manage_group.clone();
    Box::new(move |query: &str| {
        let matches = query.is_empty()
            || "characters avatar personality agent niko levi kuro manage folder reset"
                .contains(query);
        chars_group_clone.set_visible(matches);
        manage_group_clone.set_visible(matches);
        matches
    })
}

fn show_reset_confirmation(parent: &gtk::Button) {
    let dialog = adw::AlertDialog::new(
        Some("Reset to Default Characters?"),
        Some(
            "All existing character directories will be permanently removed and replaced \
            with the three bundled defaults (Niko, Levi and Kuro). \
            This cannot be undone.",
        ),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("reset", "Remove and Reset");
    dialog.set_response_appearance("reset", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let parent_clone = parent.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "reset" {
            show_final_confirmation(&parent_clone);
        }
    });

    dialog.present(Some(parent));
}

fn show_final_confirmation(parent: &gtk::Button) {
    let dialog = adw::AlertDialog::new(
        Some("Are You Sure?"),
        Some(
            "This will permanently delete all character directories and recreate the defaults. \
            This action cannot be undone.",
        ),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("confirm", "Yes, Reset to Defaults");
    dialog.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let parent_clone = parent.clone();
    dialog.connect_response(None, move |_, r| {
        if r == "confirm" {
            if let Err(e) = boxxy_claw_protocol::character_loader::reset_to_defaults() {
                log::error!("Failed to reset characters to defaults: {}", e);
            } else {
                if let Some(window) = parent_clone
                    .root()
                    .and_then(|r| r.downcast::<adw::PreferencesWindow>().ok())
                {
                    let toast = adw::Toast::new("Restart Boxxy to apply character changes.");
                    window.add_toast(toast);
                }
            }
        }
    });

    dialog.present(Some(parent));
}

fn make_color_dot(color_hex: &str) -> gtk::Button {
    // Validate: only allow characters safe for use in a CSS value.
    let safe_color: &str = if color_hex.chars().all(|c| c.is_ascii_hexdigit() || c == '#') {
        color_hex
    } else {
        "#808080"
    };

    // Unique class per dot to ensure styles don't bleed between rows.
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let class = format!("pref-color-dot-{n}");

    let dot = gtk::Button::new();
    dot.set_size_request(16, 16);
    dot.set_valign(gtk::Align::Center);
    dot.set_halign(gtk::Align::Center);
    dot.set_focusable(false);
    dot.set_sensitive(false);
    dot.add_css_class(&class);

    // We use a specific selector and override background-image to prevent
    // libadwaita themes from applying gradients or hover effects.
    let css = format!(
        "button.{class} {{ \
            background-color: {safe_color}; \
            background-image: none; \
            border-radius: 12px; \
            min-width: 16px; \
            min-height: 16px; \
            padding: 0; \
            border: none; \
            box-shadow: none; \
        }} \
        button.{class}:hover, button.{class}:active {{ \
            background-color: {safe_color}; \
            background-image: none; \
        }}"
    );
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&css);
    #[allow(deprecated)]
    dot.style_context()
        .add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

    dot
}
