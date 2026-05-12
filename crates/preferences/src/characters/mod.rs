use crate::config::Settings;
use crate::user_profile;
use adw::prelude::*;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use libadwaita as adw;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

struct RowData {
    char_id: String,
    duties: String,
    title: String,
    row: adw::ActionRow,
}

pub fn setup_characters_page(
    builder: &gtk::Builder,
    settings_rc: Rc<RefCell<Settings>>,
    on_change: Rc<dyn Fn(Settings) + 'static>,
) -> Box<dyn Fn(&str) -> bool> {
    let page: adw::PreferencesPage = builder.object("page_characters").unwrap();

    // === User Profile group ===
    let user_group = adw::PreferencesGroup::new();
    user_group.set_title("You");

    let display_name_row = adw::EntryRow::new();
    display_name_row.set_title("What the model calls you");
    display_name_row.set_text(&settings_rc.borrow().user_profile.display_name);

    let user_avatar = adw::Avatar::new(42, None, true);
    user_avatar.set_margin_top(8);
    user_avatar.set_margin_bottom(8);
    user_profile::configure_avatar(
        &user_avatar,
        &user_profile::effective(&settings_rc.borrow()),
    );
    display_name_row.add_prefix(&user_avatar);

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    let avatar_clone = user_avatar.clone();
    display_name_row.connect_changed(move |entry| {
        let text = entry.text().to_string();
        let mut s = s_rc.borrow_mut();
        if s.user_profile.display_name != text {
            s.user_profile.display_name = text.clone();
            s.save();
            user_profile::configure_avatar(&avatar_clone, &user_profile::effective(&s));
            cb(s.clone());
        }
    });
    user_group.add(&display_name_row);

    // Upload & Color Actions
    let actions_row = adw::ActionRow::new();

    let upload_btn = gtk::Button::with_label("Upload Avatar…");
    upload_btn.set_valign(gtk::Align::Center);
    actions_row.add_prefix(&upload_btn);

    let remove_avatar_btn = gtk::Button::with_label("Remove Avatar");
    remove_avatar_btn.set_valign(gtk::Align::Center);
    remove_avatar_btn.add_css_class("flat");
    let avatar_exists = boxxy_assets::user::get_user_avatar_path()
        .as_ref()
        .is_some_and(|path| path.exists());
    remove_avatar_btn.set_visible(avatar_exists);
    actions_row.add_prefix(&remove_avatar_btn);

    let s_rc_upload = settings_rc.clone();
    let avatar_clone2 = user_avatar.clone();
    let remove_avatar_btn_upload = remove_avatar_btn.clone();
    upload_btn.connect_clicked(move |btn| {
        let window = btn.root().and_downcast::<gtk::Window>();
        let dialog = gtk::FileDialog::new();
        dialog.set_title("Select Avatar");
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Images"));
        filter.add_mime_type("image/png");
        filter.add_mime_type("image/jpeg");
        filter.add_mime_type("image/webp");
        let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        dialog.set_filters(Some(&filters));

        let s_rc_upload = s_rc_upload.clone();
        let avatar_clone2 = avatar_clone2.clone();
        let btn_clone = btn.clone();
        let remove_avatar_btn_upload = remove_avatar_btn_upload.clone();

        dialog.open(
            window.as_ref(),
            None::<&gtk::gio::Cancellable>,
            move |result| {
                if let Ok(file) = result {
                    if let Some(path) = file.path() {
                        let s_rc_upload = s_rc_upload.clone();
                        let avatar_clone2 = avatar_clone2.clone();
                        let btn_clone = btn_clone.clone();

                        glib::spawn_future_local(async move {
                            let bytes = match tokio::fs::read(&path).await {
                                Ok(b) => b,
                                Err(e) => {
                                    log::error!("Failed to read avatar file: {}", e);
                                    return;
                                }
                            };

                            let process_result = tokio::task::spawn_blocking(move || {
                                boxxy_assets::image::process_avatar(&bytes)
                            })
                            .await;

                            if let Ok(Ok(output)) = process_result {
                                if let Some(user_avatar_path) =
                                    boxxy_assets::user::get_user_avatar_path()
                                {
                                    if tokio::fs::write(&user_avatar_path, &output.png_bytes)
                                        .await
                                        .is_ok()
                                    {
                                        user_profile::configure_avatar(
                                            &avatar_clone2,
                                            &user_profile::effective(&s_rc_upload.borrow()),
                                        );
                                        remove_avatar_btn_upload.set_visible(true);

                                        if let Some(pref_win) = btn_clone
                                            .root()
                                            .and_downcast::<adw::PreferencesWindow>(
                                        ) {
                                            pref_win.add_toast(adw::Toast::new("Avatar updated"));
                                        }
                                    }
                                }
                            } else if let Some(pref_win) =
                                btn_clone.root().and_downcast::<adw::PreferencesWindow>()
                            {
                                pref_win.add_toast(adw::Toast::new("Failed to process avatar."));
                            }
                        });
                    }
                }
            },
        );
    });

    let s_rc_remove = settings_rc.clone();
    let avatar_remove_clone = user_avatar.clone();
    let remove_avatar_btn_clone = remove_avatar_btn.clone();
    remove_avatar_btn.connect_clicked(move |btn| {
        let s_rc_remove = s_rc_remove.clone();
        let avatar_remove_clone = avatar_remove_clone.clone();
        let remove_avatar_btn_clone = remove_avatar_btn_clone.clone();
        let btn_clone = btn.clone();

        glib::spawn_future_local(async move {
            if let Some(path) = boxxy_assets::user::get_user_avatar_path()
                && let Err(e) = tokio::fs::remove_file(&path).await
                && e.kind() != std::io::ErrorKind::NotFound
            {
                log::error!("Failed to remove user avatar: {}", e);
                if let Some(pref_win) = btn_clone.root().and_downcast::<adw::PreferencesWindow>() {
                    pref_win.add_toast(adw::Toast::new("Failed to remove avatar."));
                }
                return;
            }

            user_profile::configure_avatar(
                &avatar_remove_clone,
                &user_profile::effective(&s_rc_remove.borrow()),
            );
            remove_avatar_btn_clone.set_visible(false);

            if let Some(pref_win) = btn_clone.root().and_downcast::<adw::PreferencesWindow>() {
                pref_win.add_toast(adw::Toast::new("Avatar removed"));
            }
        });
    });

    let color_btn = gtk::ColorButton::new();
    color_btn.set_valign(gtk::Align::Center);
    user_profile::set_default_color_button(&color_btn, &settings_rc.borrow());

    let color_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    color_box.set_valign(gtk::Align::Center);
    color_box.append(&color_btn);
    actions_row.add_suffix(&color_box);

    let s_rc2 = settings_rc.clone();
    let cb2 = on_change.clone();
    let avatar_color_clone = user_avatar.clone();
    color_btn.connect_color_set(move |cb_btn| {
        let rgba = cb_btn.rgba();
        let hex = format!(
            "#{:02x}{:02x}{:02x}",
            (rgba.red() * 255.0) as u8,
            (rgba.green() * 255.0) as u8,
            (rgba.blue() * 255.0) as u8
        );
        let mut s = s_rc2.borrow_mut();
        if s.user_profile.color.as_deref() != Some(&hex) {
            s.user_profile.color = Some(hex);
            s.save();
            user_profile::configure_avatar(&avatar_color_clone, &user_profile::effective(&s));
            cb2(s.clone());
        }
    });

    user_group.add(&actions_row);
    page.add(&user_group);

    let chars_dir = boxxy_claw_protocol::character_loader::get_characters_dir().ok();

    // Initial fetch from cache
    let initial_catalog = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
    let characters = (**initial_catalog).clone();

    let rows = Rc::new(RefCell::new(Vec::new()));

    // === Characters list group ===
    let chars_group = adw::PreferencesGroup::new();
    chars_group.set_title("Available Characters");
    chars_group.set_description(Some(
        "Characters available for assignment to terminal panes. \
        Edits to CHARACTER.toml or AVATAR.png take effect immediately on the next interaction.\n\n\
        Note: If a character is removed, any of their past sessions \
        will automatically be reassigned to the first available character.",
    ));

    page.add(&chars_group);

    // Initial render
    render_characters(&chars_group, characters, rows.clone(), chars_dir.clone());

    // Setup reactivity: poll the CLAIMS_CACHE and CHARACTER_CACHE
    let mut last_catalog_json = serde_json::to_string(&*initial_catalog).unwrap_or_default();
    let rows_poll = rows.clone();
    let chars_group_poll = chars_group.clone();
    let chars_dir_poll = chars_dir.clone();

    glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
        let claims = boxxy_claw_protocol::characters::CLAIMS_CACHE.load();
        let catalog = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();

        let current_json = serde_json::to_string(&*catalog).unwrap_or_default();
        if current_json != last_catalog_json {
            last_catalog_json = current_json;
            render_characters(
                &chars_group_poll,
                (**catalog).clone(),
                rows_poll.clone(),
                chars_dir_poll.clone(),
            );
        }

        let current_rows = rows_poll.borrow();

        for row_data in current_rows.iter() {
            let active_claim = claims.iter().find(|c| c.character_id == row_data.char_id);

            row_data
                .row
                .set_subtitle(&glib::markup_escape_text(&row_data.duties));

            if active_claim.is_some() {
                row_data
                    .row
                    .set_title(&format!("{} (Active)", row_data.title));
                row_data.row.add_css_class("success");
            } else {
                row_data.row.set_title(&row_data.title);
                row_data.row.remove_css_class("success");
            }
        }

        glib::ControlFlow::Continue
    });

    // === Management group ===
    let manage_group = adw::PreferencesGroup::new();
    manage_group.set_title("Manage");
    // ... (rest of management group setup)

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

fn render_characters(
    chars_group: &adw::PreferencesGroup,
    characters: Vec<boxxy_claw_protocol::characters::CharacterInfo>,
    rows: Rc<RefCell<Vec<RowData>>>,
    chars_dir: Option<PathBuf>,
) {
    // Clear the group first. Instead of trying to clear the group's internal children directly
    // which causes GTK critical errors, we will just remove the specific rows we added.
    let current_rows = rows.borrow();
    for row_data in current_rows.iter() {
        chars_group.remove(&row_data.row);
    }
    drop(current_rows);

    let mut new_rows = Vec::new();

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
        for character in characters.iter() {
            let row = adw::ActionRow::new();

            // Format name: "niko-una" -> "Niko Una"
            let formatted_name = character
                .config
                .name
                .split('-')
                .map(|word| {
                    let mut c = word.chars();
                    match c.next() {
                        None => String::new(),
                        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    }
                })
                .collect::<Vec<String>>()
                .join(" ");

            row.set_title(&glib::markup_escape_text(&formatted_name));
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

            // Drag indicator
            let drag_icon = gtk::Image::from_icon_name("list-drag-handle-symbolic");
            drag_icon.set_valign(gtk::Align::Center);
            drag_icon.add_css_class("dim-label");
            drag_icon.set_margin_end(6);
            row.add_prefix(&drag_icon);

            // Drag and Drop
            let drag_source = gtk::DragSource::new();
            drag_source.set_actions(gdk::DragAction::MOVE);
            let id_str = character.config.id.clone();
            drag_source.connect_prepare(move |_, _, _| {
                Some(gdk::ContentProvider::for_value(&id_str.to_value()))
            });
            row.add_controller(drag_source);

            let drop_target = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
            let target_id = character.config.id.clone();
            let characters_dnd = characters.clone();
            let rows_dnd = rows.clone();
            let chars_group_dnd = chars_group.clone();
            let chars_dir_dnd = chars_dir.clone();
            drop_target.connect_drop(move |_, value, _, _| {
                if let Ok(source_id_str) = value.get::<String>() {
                    if source_id_str != target_id {
                        let mut chars = characters_dnd.clone();
                        let source_idx_opt =
                            chars.iter().position(|c| c.config.id == source_id_str);
                        let target_idx_opt = chars.iter().position(|c| c.config.id == target_id);

                        if let (Some(source_idx), Some(target_idx)) =
                            (source_idx_opt, target_idx_opt)
                        {
                            let item = chars.remove(source_idx);
                            chars.insert(target_idx, item);

                            let order: Vec<String> =
                                chars.iter().map(|c| c.config.name.clone()).collect();
                            let _ =
                                boxxy_claw_protocol::character_loader::save_character_order(order);
                            render_characters(
                                &chars_group_dnd,
                                chars,
                                rows_dnd.clone(),
                                chars_dir_dnd.clone(),
                            );
                        }
                    }
                    return true;
                }
                false
            });
            row.add_controller(drop_target);

            // Color swatch
            let swatch = make_color_dot(&character.config.color);
            row.add_suffix(&swatch);

            // Upload Avatar Action
            let upload_char_btn = gtk::Button::from_icon_name("document-edit-symbolic");
            upload_char_btn.set_valign(gtk::Align::Center);
            upload_char_btn.add_css_class("flat");
            upload_char_btn.set_tooltip_text(Some("Upload Avatar"));
            row.add_suffix(&upload_char_btn);

            let char_name = character.config.name.clone();
            let chars_dir_upload = chars_dir.clone();
            upload_char_btn.connect_clicked(move |btn| {
                let window = btn.root().and_downcast::<gtk::Window>();
                let dialog = gtk::FileDialog::new();
                dialog.set_title("Select Character Avatar");
                let filter = gtk::FileFilter::new();
                filter.set_name(Some("Images"));
                filter.add_mime_type("image/png");
                filter.add_mime_type("image/jpeg");
                filter.add_mime_type("image/webp");
                let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
                filters.append(&filter);
                dialog.set_filters(Some(&filters));

                let char_name = char_name.clone();
                let chars_dir_upload = chars_dir_upload.clone();
                let btn_clone = btn.clone();

                dialog.open(
                    window.as_ref(),
                    None::<&gtk::gio::Cancellable>,
                    move |result| {
                        if let Ok(file) = result {
                            if let Some(path) = file.path() {
                                let char_name = char_name.clone();
                                let chars_dir_upload = chars_dir_upload.clone();
                                let btn_clone = btn_clone.clone();

                                glib::spawn_future_local(async move {
                                    let bytes = match tokio::fs::read(&path).await {
                                        Ok(b) => b,
                                        Err(e) => {
                                            log::error!("Failed to read avatar file: {}", e);
                                            return;
                                        }
                                    };

                                    let process_result = tokio::task::spawn_blocking(move || {
                                        boxxy_assets::image::process_avatar(&bytes)
                                    })
                                    .await;

                                    if let Ok(Ok(output)) = process_result {
                                        if let Some(base_dir) = &chars_dir_upload {
                                            let char_dir = base_dir.join(&char_name);
                                            let avatar_path = char_dir.join("AVATAR.png");

                                            if tokio::fs::write(&avatar_path, &output.png_bytes)
                                                .await
                                                .is_ok()
                                            {
                                                if let Some(pref_win) = btn_clone
                                                    .root()
                                                    .and_downcast::<adw::PreferencesWindow>(
                                                ) {
                                                    pref_win.add_toast(adw::Toast::new(
                                                        "Avatar updated.",
                                                    ));
                                                }
                                            }
                                        }
                                    } else if let Some(pref_win) =
                                        btn_clone.root().and_downcast::<adw::PreferencesWindow>()
                                    {
                                        pref_win.add_toast(adw::Toast::new(
                                            "Failed to process avatar.",
                                        ));
                                    }
                                });
                            }
                        }
                    },
                );
            });

            chars_group.add(&row);
            new_rows.push(RowData {
                char_id: character.config.id.clone(),
                duties: character.config.duties.clone(),
                title: formatted_name,
                row,
            });
        }
    }

    *rows.borrow_mut() = new_rows;
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
