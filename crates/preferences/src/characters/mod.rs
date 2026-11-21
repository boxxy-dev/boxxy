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
    slug: String,
    has_avatar: Rc<Cell<bool>>,
    avatar_rel_path: Rc<RefCell<Option<String>>>,
    title: String,
    subtitle: RefCell<String>,
    display_name: RefCell<String>,
    color: Rc<RefCell<String>>,
    haystack: RefCell<String>,
    last_active: Cell<Option<bool>>,
    avatar: adw::Avatar,
    reset_color_btn: gtk::Button,
    set_swatch_color: Rc<dyn Fn(&str)>,
    row: adw::ActionRow,
}

fn format_row_subtitle(character: &boxxy_claw_protocol::characters::CharacterInfo) -> String {
    let is_bundled =
        boxxy_claw_protocol::character_loader::bundled_default_avatar(&character.config.name)
            .is_some();
    if !is_bundled && let Some(attr) = character.package_attribution() {
        format!("{}  ·  {attr}", character.config.duties)
    } else {
        character.config.duties.clone()
    }
}

fn format_row_tooltip(character: &boxxy_claw_protocol::characters::CharacterInfo) -> Option<String> {
    let man = character.manifest.as_ref()?;
    let desc = man.package.description.trim();
    let attr = character.package_attribution();
    match (!desc.is_empty(), attr) {
        (true, Some(a)) => Some(format!("{desc}\n{a}")),
        (true, None) => Some(desc.to_string()),
        (false, Some(a)) => Some(a),
        (false, None) => None,
    }
}

fn build_row_haystack(
    formatted_name: &str,
    character: &boxxy_claw_protocol::characters::CharacterInfo,
) -> String {
    let mut extra = String::new();
    if let Some(ref man) = character.manifest {
        extra.push(' ');
        extra.push_str(&man.package.description);
        for author in &man.package.authors {
            extra.push(' ');
            extra.push_str(author);
        }
        for tag in &man.package.tags {
            extra.push(' ');
            extra.push_str(tag);
        }
    }
    format!(
        "{} {} {} {}{}",
        formatted_name,
        character.config.display_name,
        character.config.name,
        character.config.duties,
        extra
    )
    .to_lowercase()
}

fn load_row_avatar_and_color_state_async(
    avatar: adw::Avatar,
    reset_color_btn: gtk::Button,
    chars_root: &std::path::Path,
    char_slug: String,
    avatar_path: Option<PathBuf>,
) {
    let profile_dir = chars_root.join(&char_slug);
    let has_avatar = avatar_path.is_some();
    glib::spawn_future_local(async move {
        let (img_bytes, is_custom_color) = boxxy_sys_utils::runtime()
            .spawn_blocking(move || {
                let bytes = avatar_path.and_then(|p| std::fs::read(p).ok());
                let custom_color = boxxy_claw_protocol::character_loader::has_custom_character_color(
                    &char_slug,
                    &profile_dir,
                );
                (bytes, custom_color)
            })
            .await
            .unwrap_or((None, false));

        if let Some(raw) = img_bytes {
            if let Some(texture) = boxxy_core_widgets::decode_texture_from_bytes(&raw) {
                avatar.set_custom_image(Some(&texture));
            }
        } else if !has_avatar {
            avatar.set_custom_image(None::<&gdk::Texture>);
        }
        reset_color_btn.set_visible(is_custom_color);
    });
}

fn sync_custom_color_btn_async(
    reset_color_btn: gtk::Button,
    chars_root: &std::path::Path,
    char_slug: String,
) {
    let profile_dir = chars_root.join(&char_slug);
    glib::spawn_future_local(async move {
        let is_custom_color = boxxy_sys_utils::runtime()
            .spawn_blocking(move || {
                boxxy_claw_protocol::character_loader::has_custom_character_color(
                    &char_slug,
                    &profile_dir,
                )
            })
            .await
            .unwrap_or(false);
        reset_color_btn.set_visible(is_custom_color);
    });
}

/// Updates existing character rows in-place when the ordered `(id, slug)` list is
/// unchanged (e.g. color change, prompt/duties edit, avatar toggle, or active-status transition),
/// avoiding tearing down `ActionRow`s and flashing/re-decoding `AVATAR.png` textures across the list.
fn try_patch_rows_in_place(
    existing: &[RowData],
    catalog: &[boxxy_claw_protocol::characters::CharacterInfo],
    chars_root: Option<&PathBuf>,
) -> bool {
    if existing.len() != catalog.len()
        || !existing
            .iter()
            .zip(catalog.iter())
            .all(|(r, c)| r.char_id == c.config.id && r.slug == c.config.name)
    {
        return false;
    }

    for (row_data, character) in existing.iter().zip(catalog.iter()) {
        let next_subtitle = format_row_subtitle(character);
        let subtitle_changed = if *row_data.subtitle.borrow() == next_subtitle {
            false
        } else {
            row_data.subtitle.borrow_mut().clone_from(&next_subtitle);
            row_data.row.set_subtitle(&next_subtitle);
            row_data
                .row
                .set_tooltip_text(format_row_tooltip(character).as_deref());
            true
        };
        let name_changed = if *row_data.display_name.borrow() == character.config.display_name {
            false
        } else {
            row_data
                .display_name
                .borrow_mut()
                .clone_from(&character.config.display_name);
            row_data
                .avatar
                .set_text(Some(&character.config.display_name));
            true
        };
        let haystack_dirty = subtitle_changed || name_changed;
        if *row_data.color.borrow() != character.config.color {
            row_data
                .color
                .borrow_mut()
                .clone_from(&character.config.color);
            (row_data.set_swatch_color)(&character.config.color);
            if let Some(dir) = chars_root {
                sync_custom_color_btn_async(
                    row_data.reset_color_btn.clone(),
                    dir,
                    character.config.name.clone(),
                );
            }
        }
        let avatar_changed = row_data.has_avatar.get() != character.has_avatar
            || *row_data.avatar_rel_path.borrow() != character.avatar_rel_path;
        if avatar_changed {
            row_data.has_avatar.set(character.has_avatar);
            row_data
                .avatar_rel_path
                .borrow_mut()
                .clone_from(&character.avatar_rel_path);
            if let Some(dir) = chars_root {
                let resolved_path =
                    boxxy_claw_protocol::character_loader::resolve_character_avatar_path_in(
                        dir, character,
                    );
                load_row_avatar_and_color_state_async(
                    row_data.avatar.clone(),
                    row_data.reset_color_btn.clone(),
                    dir,
                    character.config.name.clone(),
                    resolved_path,
                );
            } else if !character.has_avatar {
                row_data.avatar.set_custom_image(None::<&gdk::Texture>);
            }
        }
        if haystack_dirty {
            *row_data.haystack.borrow_mut() = build_row_haystack(&row_data.title, character);
        }
    }

    true
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
        "Edits to manifest.toml, CHARACTER.toml, avatar.png, or portrait.webp (16:9) take effect immediately. \
        You can drop community character packages into the characters folder or ask Boxxy directly to create new ones.\n\n\
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
                let patched =
                    try_patch_rows_in_place(&rows_poll.borrow(), &catalog, chars_dir_poll.as_ref());
                if !patched {
                    render_characters(
                        &chars_group,
                        &catalog,
                        &rows_poll,
                        &empty_poll,
                        chars_dir_poll.as_ref(),
                    );
                }
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
        bind_mapped_poller(
            &page,
            Duration::from_millis(500),
            move || on_map(),
            move || {
                on_tick();
            },
        );
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
            let m = query.is_empty()
                || static_chars_match
                || row_data.haystack.borrow().contains(query);
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
        let empty_row = adw::ActionRow::new();
        empty_row.set_use_markup(false);
        empty_row.set_title("No characters found");
        empty_row.set_subtitle("Open the characters folder below to add one.");
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
                c.next().map_or_else(String::new, |f| {
                    f.to_uppercase().collect::<String>() + c.as_str()
                })
            })
            .collect::<Vec<String>>()
            .join(" ");

        // Set `use_markup(false)` before `title` / `subtitle` (`ObjectBuilder` applies
        // properties alphabetically, which would parse `subtitle` as Pango XML before `use-markup`).
        let row_subtitle = format_row_subtitle(character);
        let row = adw::ActionRow::new();
        row.set_use_markup(false);
        row.set_title(&formatted_name);
        row.set_subtitle(&row_subtitle);
        row.set_tooltip_text(format_row_tooltip(character).as_deref());

        let avatar = adw::Avatar::new(52, Some(&character.config.display_name), true);
        avatar.set_valign(gtk::Align::Center);
        avatar.set_margin_top(6);
        avatar.set_margin_bottom(6);
        row.add_prefix(&avatar);

        let reset_color_btn = gtk::Button::builder()
            .icon_name("edit-undo-symbolic")
            .valign(gtk::Align::Center)
            .tooltip_text(format!("Reset {formatted_name}'s color to default"))
            .css_classes(["flat", "circular"])
            .focus_on_click(false)
            .visible(false)
            .build();

        if let Some(dir) = chars_root {
            let resolved_path =
                boxxy_claw_protocol::character_loader::resolve_character_avatar_path_in(
                    dir, character,
                );
            load_row_avatar_and_color_state_async(
                avatar.clone(),
                reset_color_btn.clone(),
                dir,
                character.config.name.clone(),
                resolved_path,
            );
        }

        let has_avatar_cell = Rc::new(Cell::new(character.has_avatar));
        let avatar_rel_cell = Rc::new(RefCell::new(character.avatar_rel_path.clone()));
        let row_color_cell = Rc::new(RefCell::new(character.config.color.clone()));

        // Editable color swatch (GPU-snapshotted ColorDot) + optional reset button
        let char_name_color = character.config.name.clone();
        let chars_dir_color = chars_root.cloned();
        let reset_btn_for_color = reset_color_btn.clone();
        let row_toast_for_color = row.clone();
        let row_color_update = row_color_cell.clone();
        let swatch_tooltip = format!("Click to change {formatted_name}'s color");
        let (swatch, set_swatch_color) =
            make_editable_color_dot(&character.config.color, &swatch_tooltip, move |new_hex| {
                let Some(base_dir) = chars_dir_color.clone() else {
                    return;
                };
                row_color_update.borrow_mut().clone_from(&new_hex);
                let char_name = char_name_color.clone();
                let reset_btn = reset_btn_for_color.clone();
                let toast_anchor = row_toast_for_color.clone();
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
                                boxxy_claw_protocol::character_loader::has_custom_character_color(
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
                            reset_btn.set_visible(is_custom);
                            if let Some(chars) = reloaded {
                                boxxy_claw_protocol::characters::update_catalog_preserving_claims(
                                    chars,
                                );
                            }
                        }
                        Ok(Err(e)) => {
                            log::error!("Failed to save character color: {e}");
                            show_toast(&toast_anchor, "Failed to save character color.");
                        }
                        Err(e) => {
                            log::error!("Character color task panicked: {e}");
                        }
                    }
                });
            });

        {
            let char_name_reset = character.config.name.clone();
            let chars_dir_reset = chars_root.cloned();
            let reset_btn_self = reset_color_btn.clone();
            let row_toast_for_reset = row.clone();
            let row_color_reset = row_color_cell.clone();
            let set_swatch_for_reset = set_swatch_color.clone();
            reset_color_btn.connect_clicked(move |_| {
                let Some(base_dir) = chars_dir_reset.clone() else {
                    return;
                };
                let char_name = char_name_reset.clone();
                let reset_btn = reset_btn_self.clone();
                let toast_anchor = row_toast_for_reset.clone();
                let row_color_reset = row_color_reset.clone();
                let set_swatch = set_swatch_for_reset.clone();

                glib::spawn_future_local(async move {
                    let outcome = boxxy_sys_utils::runtime()
                        .spawn_blocking(move || {
                            let profile_dir = base_dir.join(&char_name);
                            let restored_color =
                                boxxy_claw_protocol::character_loader::reset_character_color(
                                    &char_name,
                                    &profile_dir,
                                )
                                .map_err(|e| e.to_string())?;
                            let reloaded =
                                boxxy_claw_protocol::character_loader::load_characters().ok();
                            Ok::<_, String>((restored_color, reloaded))
                        })
                        .await;

                    match outcome {
                        Ok(Ok((restored_color, reloaded))) => {
                            if let Some(ref c) = restored_color {
                                row_color_reset.borrow_mut().clone_from(c);
                                set_swatch(c);
                            }
                            reset_btn.set_visible(false);
                            if let Some(chars) = reloaded {
                                boxxy_claw_protocol::characters::update_catalog_preserving_claims(
                                    chars,
                                );
                            }
                            show_toast(&toast_anchor, "Character color reset to default.");
                        }
                        Ok(Err(e)) => {
                            log::error!("Failed to reset character color: {e}");
                            show_toast(&toast_anchor, "Failed to reset character color.");
                        }
                        Err(e) => {
                            log::error!("Reset character color task panicked: {e}");
                        }
                    }
                });
            });
        }

        let suffix_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        suffix_box.set_valign(gtk::Align::Center);
        suffix_box.append(&reset_color_btn);
        suffix_box.append(&swatch);
        row.add_suffix(&suffix_box);

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

        let haystack = build_row_haystack(&formatted_name, character);

        chars_group.add(&row);
        new_rows.push(RowData {
            char_id: character.config.id.clone(),
            slug: character.config.name.clone(),
            has_avatar: has_avatar_cell,
            avatar_rel_path: avatar_rel_cell,
            title: formatted_name,
            subtitle: RefCell::new(row_subtitle),
            display_name: RefCell::new(character.config.display_name.clone()),
            color: row_color_cell,
            haystack: RefCell::new(haystack),
            last_active: Cell::new(None),
            avatar,
            reset_color_btn,
            set_swatch_color,
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
                        boxxy_claw_protocol::characters::update_catalog_preserving_claims(chars);
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
