use crate::config::Settings;
use crate::ui_utils::{
    Debouncer, PageHooks, SearchRow, bind_mapped_poller, make_editable_color_dot, open_directory,
    show_toast,
};
use crate::user_profile;
use adw::prelude::*;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

struct RowData {
    char_id: String,
    title: String,
    haystack: String,
    last_active: Cell<Option<bool>>,
    row: adw::ActionRow,
}

#[allow(clippy::too_many_lines)]
pub(crate) fn setup_characters_page(builder: &gtk::Builder) -> PageHooks {
    let page: adw::PreferencesPage = builder.object("page_characters").unwrap();

    let is_syncing = Rc::new(Cell::new(false));
    let initial = Settings::load();

    // === User Profile group ===
    let user_group = adw::PreferencesGroup::new();
    user_group.set_title("You");

    let display_name_row = adw::EntryRow::new();
    display_name_row.set_title("What the model calls you");
    display_name_row.set_text(&initial.user_profile.display_name);

    let user_avatar = boxxy_core_widgets::EditableAvatar::new(52, None, true);
    user_avatar.widget().set_margin_top(6);
    user_avatar.widget().set_margin_bottom(6);
    user_profile::configure_avatar(user_avatar.avatar(), &user_profile::effective(&initial));
    display_name_row.add_prefix(user_avatar.widget());

    let name_debouncer = Debouncer::new();
    name_debouncer.bind_flush_lifecycle(&display_name_row);
    {
        let is_syncing = is_syncing.clone();
        let avatar_clone = user_avatar.avatar().clone();
        let debouncer = name_debouncer.clone();
        display_name_row.connect_changed(move |entry| {
            if is_syncing.get() {
                return;
            }
            let text = entry.text().to_string();
            let avatar_inner = avatar_clone.clone();
            debouncer.schedule(Duration::from_millis(250), move || {
                let updated = Settings::update(|s| {
                    s.user_profile.display_name.clone_from(&text);
                });
                user_profile::configure_avatar(&avatar_inner, &user_profile::effective(&updated));
            });
        });
    }
    display_name_row.connect_entry_activated(move |_| name_debouncer.flush());

    user_avatar.set_remove_tooltip("Remove custom avatar");
    user_avatar.set_removable(false);
    {
        let ua_init = user_avatar.clone();
        glib::spawn_future_local(async move {
            let exists = tokio::fs::try_exists(boxxy_assets::user::get_user_avatar_path())
                .await
                .unwrap_or(false);
            ua_init.set_removable(exists);
        });
    }

    {
        let user_avatar_upload = user_avatar.clone();
        let avatar_btn_toast = user_avatar.widget().clone();
        user_avatar.connect_avatar_cropped("Crop Your Avatar", move |result| match result {
            Ok(output) => {
                let user_avatar_upload = user_avatar_upload.clone();
                let avatar_btn_toast = avatar_btn_toast.clone();

                glib::spawn_future_local(async move {
                    let user_dir = boxxy_assets::user::get_user_dir();
                    let user_avatar_path = user_dir.join("AVATAR.png");
                    if tokio::fs::create_dir_all(&user_dir).await.is_ok()
                        && tokio::fs::write(&user_avatar_path, &output.png_bytes)
                            .await
                            .is_ok()
                    {
                        user_profile::invalidate_avatar_cache();
                        user_profile::configure_avatar(
                            user_avatar_upload.avatar(),
                            &user_profile::current(),
                        );
                        user_avatar_upload.set_removable(true);
                        Settings::notify_changed();
                        show_toast(&avatar_btn_toast, "Avatar updated");
                    } else {
                        show_toast(&avatar_btn_toast, "Failed to save avatar.");
                    }
                });
            }
            Err(err) => {
                show_toast(
                    &avatar_btn_toast,
                    &format!("Failed to process avatar: {err}"),
                );
            }
        });
    }

    {
        let user_avatar_remove = user_avatar.clone();
        user_avatar.connect_avatar_removed(move || {
            let user_avatar_remove = user_avatar_remove.clone();

            glib::spawn_future_local(async move {
                let path = boxxy_assets::user::get_user_avatar_path();
                if let Err(e) = tokio::fs::remove_file(&path).await
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    log::error!("Failed to remove user avatar: {e}");
                    show_toast(user_avatar_remove.widget(), "Failed to remove avatar.");
                    return;
                }

                user_profile::invalidate_avatar_cache();
                user_profile::configure_avatar(
                    user_avatar_remove.avatar(),
                    &user_profile::current(),
                );
                user_avatar_remove.set_removable(false);
                Settings::notify_changed();

                show_toast(user_avatar_remove.widget(), "Avatar removed");
            });
        });
    }

    let initial_user_color = user_profile::effective(&initial).color;
    let (color_btn, set_user_color_dot) =
        make_editable_color_dot(&initial_user_color, "Profile Color", move |hex| {
            Settings::update(|s| {
                s.user_profile.color = Some(hex.clone());
            });
        });
    display_name_row.add_suffix(&color_btn);

    user_group.add(&display_name_row);
    page.add(&user_group);

    let chars_dir = boxxy_claw_protocol::character_loader::get_characters_dir().ok();

    let initial_catalog = boxxy_claw_protocol::characters::CHARACTER_CACHE.load_full();
    let rows: Rc<RefCell<Vec<RowData>>> = Rc::new(RefCell::new(Vec::new()));
    let empty_placeholder: Rc<RefCell<Option<adw::ActionRow>>> = Rc::new(RefCell::new(None));

    // === Characters list group ===
    let chars_group = adw::PreferencesGroup::new();
    chars_group.set_title("Available Characters");
    chars_group.set_description(Some(
        "Edits to CHARACTER.toml or AVATAR.png take effect immediately on the next interaction. \
        You can also ask Boxxy directly to create new characters or remove existing ones.\n\n\
        Note: If a character is removed, any of their past sessions \
        will automatically be reassigned to the first available character.",
    ));
    page.add(&chars_group);

    render_characters(
        &chars_group,
        &initial_catalog,
        &rows,
        &empty_placeholder,
        chars_dir.as_ref(),
    );

    // Refresh active claim badges & catalog changes strictly while the Characters page is mapped
    let last_catalog = Rc::new(RefCell::new(initial_catalog));
    let sync_claims_and_catalog: Rc<dyn Fn()> = {
        let chars_group = chars_group.clone();
        let rows_poll = rows.clone();
        let empty_poll = empty_placeholder;
        let chars_dir_poll = chars_dir.clone();

        Rc::new(move || {
            let claims = boxxy_claw_protocol::characters::CLAIMS_CACHE.load();
            let catalog = boxxy_claw_protocol::characters::CHARACTER_CACHE.load_full();

            let catalog_changed = {
                let prev = last_catalog.borrow();
                !Arc::ptr_eq(&catalog, &prev) && *catalog != **prev
            };
            if catalog_changed {
                *last_catalog.borrow_mut() = Arc::clone(&catalog);
                render_characters(
                    &chars_group,
                    &catalog,
                    &rows_poll,
                    &empty_poll,
                    chars_dir_poll.as_ref(),
                );
            }

            for row_data in rows_poll.borrow().iter() {
                let is_active = claims.iter().any(|c| c.character_id == row_data.char_id);
                if row_data.last_active.get() != Some(is_active) {
                    row_data.last_active.set(Some(is_active));
                    if is_active {
                        row_data
                            .row
                            .set_title(&format!("{} (Active)", row_data.title));
                        row_data.row.add_css_class("success");
                    } else {
                        row_data.row.set_title(&row_data.title);
                        row_data.row.remove_css_class("success");
                    }
                }
            }
        })
    };
    sync_claims_and_catalog();
    {
        let on_map = sync_claims_and_catalog.clone();
        let on_tick = sync_claims_and_catalog;
        bind_mapped_poller(&page, Duration::from_millis(500), move || on_map(), move || {
            on_tick();
        });
    }

    // === Link to Toolbox & Permissions (Autonomous Turn Limit) ===
    let turns_link_group = adw::PreferencesGroup::new();
    let turns_link_label = gtk::Label::new(None);
    turns_link_label.set_use_markup(true);
    turns_link_label.set_markup(
        "<a href=\"toolbox\"><span underline=\"none\">Set Characters Autonomous Turns →</span></a>",
    );
    turns_link_label.set_halign(gtk::Align::Start);
    turns_link_label.add_css_class("boxxy-link");

    let category_list: gtk::ListBox = builder.object("category_list").unwrap();
    let nav_toolbox: gtk::ListBoxRow = builder.object("nav_toolbox").unwrap();
    let max_roundtable_turns_spin: adw::SpinRow =
        builder.object("max_roundtable_turns_spin").unwrap();
    turns_link_label.connect_activate_link(move |_, _| {
        category_list.select_row(Some(&nav_toolbox));
        max_roundtable_turns_spin.grab_focus();
        glib::Propagation::Stop
    });

    turns_link_group.add(&turns_link_label);
    page.add(&turns_link_group);

    // === Management group ===
    let manage_group = adw::PreferencesGroup::new();
    manage_group.set_title("Manage");

    let open_row = adw::ActionRow::new();
    open_row.set_title("Open Characters Folder");
    open_row.set_subtitle("Browse and edit character files in your file manager");
    open_row.set_activatable(true);
    let arrow = gtk::Image::from_icon_name("folder-open-symbolic");
    arrow.set_valign(gtk::Align::Center);
    open_row.add_suffix(&arrow);

    let chars_dir_open = chars_dir;
    open_row.connect_activated(move |row| {
        if let Some(ref dir) = chars_dir_open {
            open_directory(
                row,
                dir.clone(),
                Some("Changes in the characters folder apply automatically."),
            );
        }
    });
    manage_group.add(&open_row);

    let reset_row = adw::ActionRow::new();
    reset_row.set_title("Reset to Defaults");
    reset_row.set_subtitle(
        "Restore the three bundled characters — all existing characters will be removed",
    );

    let reset_btn = gtk::Button::with_label("Reset…");
    reset_btn.set_valign(gtk::Align::Center);
    reset_btn.add_css_class("destructive-action");
    reset_row.add_suffix(&reset_btn);
    reset_btn.connect_clicked(show_reset_confirmation);

    manage_group.add(&reset_row);
    page.add(&manage_group);

    let s_user = SearchRow::new(
        &display_name_row,
        "you user profile display name what the model calls you avatar color",
    );

    let display_name_sync = display_name_row;
    let user_avatar_sync = user_avatar;
    let sync = Box::new(move |s: &Settings| {
        is_syncing.set(true);
        if !display_name_sync.has_focus()
            && display_name_sync.focus_child().is_none()
            && display_name_sync.text().as_str() != s.user_profile.display_name
        {
            display_name_sync.set_text(&s.user_profile.display_name);
        }
        let eff = user_profile::effective(s);
        user_profile::configure_avatar(user_avatar_sync.avatar(), &eff);
        set_user_color_dot(&eff.color);
        is_syncing.set(false);
    });

    let filter = Box::new(move |query: &str| {
        let user_vis = s_user.apply(query);
        user_group.set_visible(user_vis);

        let static_chars_match = query.is_empty()
            || "characters avatar personality agent niko levi kuro manage folder reset autonomous turn limit"
                .contains(query);
        let mut any_char_vis = static_chars_match;
        for row_data in rows.borrow().iter() {
            let m = query.is_empty() || static_chars_match || row_data.haystack.contains(query);
            row_data.row.set_visible(m);
            if m {
                any_char_vis = true;
            }
        }
        chars_group.set_visible(any_char_vis);
        turns_link_group.set_visible(static_chars_match);
        manage_group.set_visible(static_chars_match);

        user_vis || any_char_vis || static_chars_match
    });

    (sync, filter)
}

#[allow(clippy::too_many_lines)]
fn render_characters(
    chars_group: &adw::PreferencesGroup,
    characters: &[boxxy_claw_protocol::characters::CharacterInfo],
    rows: &Rc<RefCell<Vec<RowData>>>,
    empty_placeholder: &Rc<RefCell<Option<adw::ActionRow>>>,
    chars_root: Option<&PathBuf>,
) {
    for row_data in rows.borrow_mut().drain(..) {
        chars_group.remove(&row_data.row);
    }
    if let Some(old_empty) = empty_placeholder.borrow_mut().take() {
        chars_group.remove(&old_empty);
    }

    if characters.is_empty() {
        let empty_row = adw::ActionRow::builder()
            .use_markup(false)
            .title("No characters found")
            .subtitle("Open the characters folder below to add one.")
            .build();
        chars_group.add(&empty_row);
        *empty_placeholder.borrow_mut() = Some(empty_row);
        return;
    }

    let mut new_rows = Vec::with_capacity(characters.len());

    for character in characters {
        let formatted_name = character
            .config
            .name
            .split('-')
            .map(|word| {
                let mut c = word.chars();
                c.next()
                    .map_or_else(String::new, |f| f.to_uppercase().collect::<String>() + c.as_str())
            })
            .collect::<Vec<String>>()
            .join(" ");

        let row = adw::ActionRow::builder()
            .use_markup(false)
            .title(&formatted_name)
            .subtitle(&character.config.duties)
            .build();

        let editable_avatar = boxxy_core_widgets::EditableAvatar::new(
            52,
            Some(&character.config.display_name),
            true,
        );
        let is_bundled = boxxy_claw_protocol::character_loader::bundled_default_avatar(
            &character.config.name,
        )
        .is_some();
        editable_avatar.set_remove_tooltip(if is_bundled {
            "Reset to default avatar"
        } else {
            "Remove custom avatar"
        });

        if let Some(dir) = chars_root {
            let profile_dir = dir.join(&character.config.name);
            let avatar_path = profile_dir.join("AVATAR.png");
            let has_avatar = character.has_avatar;
            let char_slug = character.config.name.clone();
            let ea_load = editable_avatar.clone();
            glib::spawn_future_local(async move {
                let (png_bytes, is_custom) = boxxy_sys_utils::runtime()
                    .spawn_blocking(move || {
                        let bytes = if has_avatar {
                            std::fs::read(&avatar_path).ok()
                        } else {
                            None
                        };
                        let custom =
                            boxxy_claw_protocol::character_loader::has_custom_character_avatar(
                                &char_slug,
                                &profile_dir,
                            );
                        (bytes, custom)
                    })
                    .await
                    .unwrap_or((None, false));

                if let Some(raw) = png_bytes {
                    let gbytes = glib::Bytes::from_owned(raw);
                    if let Ok(texture) = gdk::Texture::from_bytes(&gbytes) {
                        ea_load.avatar().set_custom_image(Some(&texture));
                    }
                }
                ea_load.set_removable(is_custom);
            });
        }
        editable_avatar.widget().set_margin_top(4);
        editable_avatar.widget().set_margin_bottom(4);
        row.add_prefix(editable_avatar.widget());

        {
            let char_name = character.config.name.clone();
            let chars_dir_upload = chars_root.cloned();
            let editable_avatar_upload = editable_avatar.clone();
            let btn_toast = editable_avatar.widget().clone();
            let dialog_title = format!("Crop Avatar — {formatted_name}");
            editable_avatar.connect_avatar_cropped(&dialog_title, move |result| match result {
                Ok(output) => {
                    let char_name = char_name.clone();
                    let chars_dir_upload = chars_dir_upload.clone();
                    let editable_avatar_upload = editable_avatar_upload.clone();
                    let btn_toast = btn_toast.clone();

                    glib::spawn_future_local(async move {
                        let Some(base_dir) = chars_dir_upload else {
                            return;
                        };
                        let profile_dir = base_dir.join(&char_name);
                        let avatar_path = profile_dir.join("AVATAR.png");

                        if tokio::fs::write(&avatar_path, &output.png_bytes)
                            .await
                            .is_ok()
                        {
                            let bytes = glib::Bytes::from(&output.png_bytes);
                            if let Ok(texture) = gdk::Texture::from_bytes(&bytes) {
                                editable_avatar_upload
                                    .avatar()
                                    .set_custom_image(Some(&texture));
                            }
                            let is_custom = boxxy_sys_utils::runtime()
                                .spawn_blocking(move || {
                                    boxxy_claw_protocol::character_loader::has_custom_character_avatar(
                                        &char_name,
                                        &profile_dir,
                                    )
                                })
                                .await
                                .unwrap_or(true);
                            editable_avatar_upload.set_removable(is_custom);
                            show_toast(&btn_toast, "Avatar updated.");
                        } else {
                            show_toast(&btn_toast, "Failed to save avatar.");
                        }
                    });
                }
                Err(err) => {
                    show_toast(&btn_toast, &format!("Failed to process avatar: {err}"));
                }
            });
        }

        // Editable color swatch (GPU-snapshotted ColorDot)
        let char_name_color = character.config.name.clone();
        let chars_dir_color = chars_root.cloned();
        let editable_avatar_color = editable_avatar.clone();
        let swatch_tooltip = format!("Click to change {formatted_name}'s color");
        let (swatch, set_swatch_color) =
            make_editable_color_dot(&character.config.color, &swatch_tooltip, move |new_hex| {
                let Some(base_dir) = chars_dir_color.clone() else {
                    return;
                };
                let char_name = char_name_color.clone();
                let ea = editable_avatar_color.clone();
                glib::spawn_future_local(async move {
                    let res = boxxy_sys_utils::runtime()
                        .spawn_blocking(move || {
                            let profile_dir = base_dir.join(&char_name);
                            boxxy_claw_protocol::character_loader::save_character_color(
                                &profile_dir,
                                &new_hex,
                            )
                            .map_err(|e| e.to_string())?;
                            let is_custom =
                                boxxy_claw_protocol::character_loader::has_custom_character_avatar(
                                    &char_name,
                                    &profile_dir,
                                );
                            let reloaded =
                                boxxy_claw_protocol::character_loader::load_characters().ok();
                            Ok::<_, String>((is_custom, reloaded))
                        })
                        .await;

                    match res {
                        Ok(Ok((is_custom, reloaded))) => {
                            ea.set_removable(is_custom);
                            if let Some(chars) = reloaded {
                                boxxy_claw_protocol::characters::CHARACTER_CACHE
                                    .store(Arc::new(chars));
                            }
                        }
                        Ok(Err(e)) => {
                            log::error!("Failed to save character color: {e}");
                            show_toast(ea.widget(), "Failed to save character color.");
                        }
                        Err(e) => {
                            log::error!("Character color task panicked: {e}");
                        }
                    }
                });
            });
        row.add_suffix(&swatch);

        {
            let char_name_reset = character.config.name.clone();
            let chars_dir_reset = chars_root.cloned();
            let editable_avatar_reset = editable_avatar.clone();
            editable_avatar.connect_avatar_removed(move || {
                let Some(base_dir) = chars_dir_reset.clone() else {
                    return;
                };
                let char_name = char_name_reset.clone();
                let ea = editable_avatar_reset.clone();
                let set_swatch = set_swatch_color.clone();

                glib::spawn_future_local(async move {
                    let outcome = boxxy_sys_utils::runtime()
                        .spawn_blocking(move || {
                            let profile_dir = base_dir.join(&char_name);
                            let restored_color =
                                boxxy_claw_protocol::character_loader::character_default_color(
                                    &char_name,
                                    &profile_dir,
                                );
                            let bytes_opt =
                                boxxy_claw_protocol::character_loader::reset_character_avatar(
                                    &char_name,
                                    &profile_dir,
                                )
                                .map_err(|e| e.to_string())?;
                            let reloaded =
                                boxxy_claw_protocol::character_loader::load_characters().ok();
                            Ok::<_, String>((bytes_opt, restored_color, reloaded))
                        })
                        .await;

                    match outcome {
                        Ok(Ok((Some(default_bytes), restored_color, reloaded))) => {
                            let bytes = glib::Bytes::from_static(default_bytes);
                            if let Ok(texture) = gdk::Texture::from_bytes(&bytes) {
                                ea.avatar().set_custom_image(Some(&texture));
                            }
                            if let Some(ref c) = restored_color {
                                set_swatch(c);
                            }
                            ea.set_removable(false);
                            if let Some(chars) = reloaded {
                                boxxy_claw_protocol::characters::CHARACTER_CACHE
                                    .store(Arc::new(chars));
                            }
                            show_toast(ea.widget(), "Avatar and color reset to default.");
                        }
                        Ok(Ok((None, restored_color, reloaded))) => {
                            ea.avatar().set_custom_image(None::<&gdk::Texture>);
                            if let Some(ref c) = restored_color {
                                set_swatch(c);
                            }
                            ea.set_removable(false);
                            if let Some(chars) = reloaded {
                                boxxy_claw_protocol::characters::CHARACTER_CACHE
                                    .store(Arc::new(chars));
                            }
                            show_toast(ea.widget(), "Custom avatar and color reset.");
                        }
                        Ok(Err(e)) => {
                            log::error!("Failed to reset character avatar: {e}");
                            show_toast(ea.widget(), "Failed to reset avatar.");
                        }
                        Err(e) => {
                            log::error!("Reset character avatar task panicked: {e}");
                        }
                    }
                });
            });
        }

        // Drag indicator
        let drag_icon = gtk::Image::from_icon_name("list-drag-handle-symbolic");
        drag_icon.set_valign(gtk::Align::Center);
        drag_icon.add_css_class("dim-label");
        drag_icon.set_margin_end(6);
        row.add_prefix(&drag_icon);

        // Drag and Drop reordering
        let drag_source = gtk::DragSource::new();
        drag_source.set_actions(gdk::DragAction::MOVE);
        let id_str = character.config.id.clone();
        drag_source.connect_prepare(move |_, _, _| {
            Some(gdk::ContentProvider::for_value(&id_str.to_value()))
        });
        row.add_controller(drag_source);

        let drop_target = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
        let target_id = character.config.id.clone();
        let characters_dnd = characters.to_vec();
        let rows_dnd = rows.clone();
        let empty_dnd = empty_placeholder.clone();
        let chars_group_dnd = chars_group.clone();
        let chars_dir_dnd = chars_root.cloned();
        drop_target.connect_drop(move |_, value, _, _| {
            if let Ok(source_id_str) = value.get::<String>() {
                if source_id_str != target_id {
                    let mut chars = characters_dnd.clone();
                    let source_idx_opt = chars.iter().position(|c| c.config.id == source_id_str);
                    let target_idx_opt = chars.iter().position(|c| c.config.id == target_id);

                    if let (Some(source_idx), Some(target_idx)) = (source_idx_opt, target_idx_opt) {
                        let item = chars.remove(source_idx);
                        chars.insert(target_idx, item);

                        let order: Vec<String> =
                            chars.iter().map(|c| c.config.name.clone()).collect();
                        boxxy_sys_utils::runtime().spawn_blocking(move || {
                            let _ =
                                boxxy_claw_protocol::character_loader::save_character_order(order);
                        });
                        render_characters(
                            &chars_group_dnd,
                            &chars,
                            &rows_dnd,
                            &empty_dnd,
                            chars_dir_dnd.as_ref(),
                        );
                    }
                }
                return true;
            }
            false
        });
        row.add_controller(drop_target);

        let haystack = format!(
            "{} {} {} {}",
            formatted_name,
            character.config.display_name,
            character.config.name,
            character.config.duties
        )
        .to_lowercase();

        chars_group.add(&row);
        new_rows.push(RowData {
            char_id: character.config.id.clone(),
            title: formatted_name,
            haystack,
            last_active: Cell::new(None),
            row,
        });
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

    let parent_weak = parent.downgrade();
    dialog.connect_response(None, move |_, r| {
        if r == "confirm" {
            let parent_weak = parent_weak.clone();
            glib::spawn_future_local(async move {
                let res = boxxy_sys_utils::runtime()
                    .spawn_blocking(|| {
                        boxxy_claw_protocol::character_loader::reset_to_defaults()
                            .map_err(|e| e.to_string())?;
                        boxxy_claw_protocol::character_loader::load_characters()
                            .map_err(|e| e.to_string())
                    })
                    .await;

                match res {
                    Ok(Ok(chars)) => {
                        boxxy_claw_protocol::characters::CHARACTER_CACHE
                            .store(Arc::new(chars));
                        if let Some(btn) = parent_weak.upgrade() {
                            show_toast(&btn, "Characters reset to defaults.");
                        }
                    }
                    Ok(Err(e)) => {
                        log::error!("Failed to reset characters to defaults: {e}");
                        if let Some(btn) = parent_weak.upgrade() {
                            show_toast(&btn, "Failed to reset characters.");
                        }
                    }
                    Err(e) => {
                        log::error!("Reset characters task panicked: {e}");
                    }
                }
            });
        }
    });

    dialog.present(Some(parent));
}
