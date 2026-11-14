//! Virtualized Claw conversation history `ListView` factory and row-append helpers.

use crate::character_style::{
    avatar_ring_color_class, is_safe_hex_color, load_cached_texture, load_character_avatar_texture,
    load_incognito_texture, prettify_character_name, resolve_character_display_name,
};
use crate::process_renderer::get_claw_viewer_registry;
use crate::row_object::{ClawRowObject, RowUpdateKind};
use crate::trace_widget::TurnTraceSlot;
use boxxy_claw_protocol::{PersistentClawRow, TurnTraceItem};
use boxxy_core_widgets::ObjectExtSafe;
use boxxy_preferences::{Settings, user_profile};
use boxxy_viewer::StructuredViewer;
use gtk::prelude::*;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

/// Builds the Claw per-pane virtualized history `ListView`.
///
/// Each row has a single stable widget tree (icon/avatar, title, working dots,
/// `TurnTraceSlot`, `StructuredViewer`, command label) set up once in `connect_setup`
/// and rebound per item in `connect_bind`. Widgets are stashed on the row's
/// outer box via `ObjectExtSafe` so long histories recycle a handful of live
/// widgets per visible slot rather than allocating per message.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn create_claw_message_list() -> (gtk::ListView, gio::ListStore) {
    boxxy_viewer::set_always_expand_code_blocks(Settings::load().always_show_code_blocks);

    let store = gio::ListStore::new::<ClawRowObject>();
    let store_for_factory = store.clone();
    let viewer_registry = get_claw_viewer_registry();
    let factory = gtk::SignalListItemFactory::new();

    factory.connect_setup(move |_, list_item| {
        let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        hbox.add_css_class("claw-virtual-row");
        hbox.add_css_class("claw-row");

        let left_col = gtk::Box::new(gtk::Orientation::Vertical, 0);
        left_col.set_valign(gtk::Align::Start);
        left_col.set_size_request(42, -1);
        hbox.append(&left_col);

        let icon = gtk::Image::new();
        icon.set_pixel_size(42);
        left_col.append(&icon);

        let avatar_frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        avatar_frame.set_halign(gtk::Align::Center);
        avatar_frame.set_valign(gtk::Align::Start);
        avatar_frame.add_css_class("claw-avatar-frame");
        avatar_frame.set_visible(false);

        let avatar = adw::Avatar::new(42, None, true);
        avatar_frame.append(&avatar);
        left_col.append(&avatar_frame);

        let active_ring_class: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 3);
        vbox.set_hexpand(true);
        hbox.append(&vbox);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.set_hexpand(true);
        header.set_valign(gtk::Align::Center);

        let title = gtk::Label::new(None);
        title.add_css_class("heading");
        title.set_halign(gtk::Align::Start);
        title.set_valign(gtk::Align::Center);
        header.append(&title);

        let working_dots = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        working_dots.add_css_class("claw-working-dots");
        working_dots.set_valign(gtk::Align::Center);
        working_dots.set_margin_start(2);
        working_dots.set_visible(false);
        for dot_cls in ["claw-dot-1", "claw-dot-2", "claw-dot-3"] {
            let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            dot.set_size_request(5, 5);
            dot.add_css_class("claw-working-dot");
            dot.add_css_class(dot_cls);
            working_dots.append(&dot);
        }
        header.append(&working_dots);

        let pane_lbl = gtk::Label::new(None);
        pane_lbl.add_css_class("caption");
        pane_lbl.add_css_class("dim-label");
        pane_lbl.set_valign(gtk::Align::Center);
        header.append(&pane_lbl);

        let copy_btn = gtk::Button::builder()
            .icon_name("boxxy-edit-copy-symbolic")
            .css_classes(["flat", "circular", "claw-row-copy-btn"])
            .tooltip_text("Copy prompt and response")
            .halign(gtk::Align::End)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .focus_on_click(false)
            .build();
        header.append(&copy_btn);

        vbox.append(&header);

        let thumbs_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        thumbs_box.set_halign(gtk::Align::Start);
        thumbs_box.set_margin_top(4);
        thumbs_box.set_margin_bottom(4);
        thumbs_box.set_visible(false);
        thumbs_box.add_css_class("user-attachment-thumbs");
        vbox.append(&thumbs_box);

        let trace_slot = TurnTraceSlot::new(viewer_registry.clone());
        vbox.append(trace_slot.widget());

        let viewer = StructuredViewer::new(viewer_registry.clone());
        vbox.append(viewer.widget());

        let cmd_label = gtk::Label::new(None);
        cmd_label.set_halign(gtk::Align::Start);
        cmd_label.set_wrap(true);
        cmd_label.set_selectable(true);
        cmd_label.add_css_class("monospace");
        cmd_label.add_css_class("dim-label");
        cmd_label.set_visible(false);
        vbox.append(&cmd_label);

        let typewriter_timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
        let copy_timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));

        {
            let store_for_copy = store_for_factory.clone();
            let item_weak = list_item.downgrade();
            let copy_timer_for_click = copy_timer.clone();
            copy_btn.connect_clicked(move |btn| {
                let Some(li) = item_weak.upgrade() else {
                    return;
                };
                let pos = li.position();
                if pos == gtk::INVALID_LIST_POSITION {
                    return;
                }
                let text = format_exchange_for_copy(&store_for_copy, pos);
                if text.is_empty() {
                    return;
                }
                if let Some(display) = gdk::Display::default() {
                    display.clipboard().set_text(&text);
                    btn.set_icon_name("boxxy-object-select-symbolic");
                    if let Some(prev) = copy_timer_for_click.borrow_mut().take() {
                        prev.remove();
                    }
                    let b = btn.clone();
                    let timer_slot = copy_timer_for_click.clone();
                    let sid = glib::timeout_add_local_once(
                        std::time::Duration::from_millis(1500),
                        move || {
                            let _ = timer_slot.borrow_mut().take();
                            b.set_icon_name("boxxy-edit-copy-symbolic");
                        },
                    );
                    *copy_timer_for_click.borrow_mut() = Some(sid);
                }
            });
        }

        hbox.set_safe_data("left_col", left_col);
        hbox.set_safe_data("header", header);
        hbox.set_safe_data("thumbs_box", thumbs_box);
        hbox.set_safe_data("icon", icon);
        hbox.set_safe_data("avatar_frame", avatar_frame);
        hbox.set_safe_data("avatar", avatar);
        hbox.set_safe_data("working_dots", working_dots);
        hbox.set_safe_data("active_ring_class", active_ring_class);
        hbox.set_safe_data("copy_btn", copy_btn);
        hbox.set_safe_data("copy_timer", copy_timer);
        hbox.set_safe_data("title", title);
        hbox.set_safe_data("pane_lbl", pane_lbl);
        hbox.set_safe_data("trace_slot", trace_slot);
        hbox.set_safe_data("viewer", viewer);
        hbox.set_safe_data("cmd_label", cmd_label);
        hbox.set_safe_data("typewriter_timer", typewriter_timer);

        list_item.set_child(Some(&hbox));
    });

    factory.connect_bind(move |_, list_item| {
        let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
        let hbox = list_item.child().and_downcast::<gtk::Box>().unwrap();

        let copy_btn = hbox.get_safe_data::<gtk::Button>("copy_btn").unwrap();
        let copy_timer = hbox
            .get_safe_data::<Rc<RefCell<Option<glib::SourceId>>>>("copy_timer")
            .unwrap();
        let typewriter_timer = hbox
            .get_safe_data::<Rc<RefCell<Option<glib::SourceId>>>>("typewriter_timer")
            .unwrap();

        if let Some(old_copy) = copy_timer.borrow_mut().take() {
            old_copy.remove();
        }
        if let Some(old_source) = typewriter_timer.borrow_mut().take() {
            old_source.remove();
        }

        let Some(obj) = list_item.item().and_downcast::<ClawRowObject>() else {
            return;
        };

        copy_btn.set_icon_name("boxxy-edit-copy-symbolic");

        bind_or_refresh_slot(&hbox, &obj);

        let hbox_weak = hbox.downgrade();
        let obj_weak = obj.downgrade();
        obj.bind_update_listener(Rc::new(move |kind| {
            let (Some(hbox_live), Some(obj_live)) = (hbox_weak.upgrade(), obj_weak.upgrade())
            else {
                return;
            };
            match kind {
                RowUpdateKind::ProseOnly => {
                    let Some(viewer_for_stream) =
                        hbox_live.get_safe_data::<StructuredViewer>("viewer")
                    else {
                        return;
                    };
                    let is_working = obj_live.is_working();
                    obj_live.with_row(|row| {
                        if let PersistentClawRow::Diagnosis { content, .. } = row {
                            if content.trim().is_empty() {
                                viewer_for_stream.park_for_recycle();
                                viewer_for_stream.widget().set_visible(false);
                            } else if is_working {
                                viewer_for_stream.set_streaming_content(content, false);
                                viewer_for_stream.widget().set_visible(true);
                            } else {
                                viewer_for_stream.set_content(content);
                                viewer_for_stream.widget().set_visible(true);
                            }
                        }
                    });
                }
                RowUpdateKind::Full => bind_or_refresh_slot(&hbox_live, &obj_live),
            }
        }));
    });

    // Parking viewer widgets into the slot's recycling pool on unbind cancels stale
    // background highlight/image tasks while keeping leaf labels & code blocks warm.
    factory.connect_unbind(move |_, list_item| {
        let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
        if let Some(obj) = list_item.item().and_downcast::<ClawRowObject>() {
            obj.clear_update_listener();
        }
        let hbox = list_item.child().and_downcast::<gtk::Box>().unwrap();
        if let Some(copy_timer) =
            hbox.get_safe_data::<Rc<RefCell<Option<glib::SourceId>>>>("copy_timer")
            && let Some(old_copy) = copy_timer.borrow_mut().take()
        {
            old_copy.remove();
        }
        if let Some(avatar_frame) = hbox.get_safe_data::<gtk::Box>("avatar_frame") {
            avatar_frame.remove_css_class("claw-avatar-typing");
        }
        if let Some(working_dots) = hbox.get_safe_data::<gtk::Box>("working_dots") {
            working_dots.set_visible(false);
        }
        if let Some(timer_slot) =
            hbox.get_safe_data::<Rc<RefCell<Option<glib::SourceId>>>>("typewriter_timer")
            && let Some(old_source) = timer_slot.borrow_mut().take()
        {
            old_source.remove();
        }
        if let Some(trace_slot) = hbox.get_safe_data::<TurnTraceSlot>("trace_slot") {
            trace_slot.reset_for_unbind();
        }
        if let Some(viewer) = hbox.get_safe_data::<StructuredViewer>("viewer") {
            viewer.park_for_recycle();
        }
    });

    let selection = gtk::NoSelection::new(Some(store.clone()));
    let list_view = gtk::ListView::new(Some(selection), Some(factory));
    list_view.set_show_separators(false);
    list_view.add_css_class("virtual-history");

    (list_view, store)
}

#[allow(clippy::too_many_lines)]
fn bind_or_refresh_slot(hbox: &gtk::Box, obj: &ClawRowObject) {
    let left_col = hbox.get_safe_data::<gtk::Box>("left_col").unwrap();
    let header = hbox.get_safe_data::<gtk::Box>("header").unwrap();
    let thumbs_box = hbox.get_safe_data::<gtk::Box>("thumbs_box").unwrap();
    let icon = hbox.get_safe_data::<gtk::Image>("icon").unwrap();
    let avatar_frame = hbox.get_safe_data::<gtk::Box>("avatar_frame").unwrap();
    let avatar = hbox.get_safe_data::<adw::Avatar>("avatar").unwrap();
    let working_dots = hbox.get_safe_data::<gtk::Box>("working_dots").unwrap();
    let active_ring_class = hbox
        .get_safe_data::<Rc<RefCell<String>>>("active_ring_class")
        .unwrap();
    let copy_btn = hbox.get_safe_data::<gtk::Button>("copy_btn").unwrap();
    let title = hbox.get_safe_data::<gtk::Label>("title").unwrap();
    let pane_lbl = hbox.get_safe_data::<gtk::Label>("pane_lbl").unwrap();
    let trace_slot = hbox.get_safe_data::<TurnTraceSlot>("trace_slot").unwrap();
    let viewer = hbox.get_safe_data::<StructuredViewer>("viewer").unwrap();
    let cmd_label = hbox.get_safe_data::<gtk::Label>("cmd_label").unwrap();
    let typewriter_timer = hbox
        .get_safe_data::<Rc<RefCell<Option<glib::SourceId>>>>("typewriter_timer")
        .unwrap();

    if let Some(old_source) = typewriter_timer.borrow_mut().take() {
        old_source.remove();
    }

    let is_working = obj.is_working();
    let always_show_code_blocks = boxxy_viewer::always_expand_code_blocks();

    // Reset per-row state that specific variants may mutate.
    left_col.set_visible(true);
    header.set_visible(true);
    copy_btn.set_visible(!is_working);
    while let Some(child) = thumbs_box.first_child() {
        thumbs_box.remove(&child);
    }
    thumbs_box.set_visible(false);
    icon.set_visible(true);
    avatar_frame.set_visible(false);
    avatar.set_visible(true);
    avatar.set_custom_image(None::<&gdk::Texture>);
    working_dots.set_visible(is_working);
    pane_lbl.set_visible(true);
    title.set_visible(true);
    title.remove_css_class("accent");
    icon.remove_css_class("accent");
    icon.remove_css_class("warning");
    icon.remove_css_class("error");
    hbox.remove_css_class("system-message");
    hbox.remove_css_class("dimmed");
    for cls in [
        "claw-row-system",
        "claw-row-diagnosis",
        "claw-row-user",
        "claw-row-suggested",
        "claw-row-process-list",
        "claw-row-tool-call",
        "claw-row-command",
        "claw-row-file-diff",
    ] {
        hbox.remove_css_class(cls);
    }

    let update_avatar_ring_color = |raw_color: &str| {
        let next_class = avatar_ring_color_class(raw_color);
        let mut current = active_ring_class.borrow_mut();
        if *current != next_class {
            if !current.is_empty() {
                avatar_frame.remove_css_class(&current);
                working_dots.remove_css_class(&current);
            }
            avatar_frame.add_css_class(&next_class);
            working_dots.add_css_class(&next_class);
            *current = next_class;
        }
    };

    let set_colored_title = |name: &str, raw_color: &str| {
        let escaped = glib::markup_escape_text(name);
        if is_safe_hex_color(raw_color) {
            title.set_markup(&format!(
                "<span foreground=\"{raw_color}\"><b>{escaped}</b></span>"
            ));
        } else {
            title.set_markup(&format!("<b>{escaped}</b>"));
        }
    };

    let set_agent_header = |char_id: &Option<String>,
                            fallback_name: &Option<String>,
                            default_icon: &str,
                            icon_class: Option<&str>| {
        let mut name_set = false;
        let cache = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
        let id_str = char_id.as_deref().unwrap_or_default();
        let fb_str = fallback_name.as_deref().unwrap_or_default();
        let fallback_names = [fb_str];

        if boxxy_claw_protocol::characters::is_incognito_character(id_str)
            || boxxy_claw_protocol::characters::is_incognito_character(fb_str)
        {
            let disp = boxxy_claw_protocol::characters::INCOGNITO_DISPLAY_NAME;
            let color = boxxy_claw_protocol::characters::INCOGNITO_COLOR;
            set_colored_title(disp, color);
            update_avatar_ring_color(color);
            avatar.set_text(Some(disp));
            avatar.set_show_initials(true);
            if let Some(texture) = load_incognito_texture() {
                avatar.set_custom_image(Some(&texture));
            }
            avatar_frame.set_visible(true);
            icon.set_visible(false);
            name_set = true;
        } else if (!id_str.is_empty() || !fb_str.is_empty())
            && let Some(info) = boxxy_claw_protocol::characters::find_character_exact(
                &cache,
                id_str,
                &fallback_names,
            )
        {
            let formatted_name = resolve_character_display_name(info);
            set_colored_title(&formatted_name, &info.config.color);
            update_avatar_ring_color(&info.config.color);
            avatar.set_text(Some(&formatted_name));
            avatar.set_show_initials(true);
            if let Some(texture) = load_character_avatar_texture(info) {
                avatar.set_custom_image(Some(&texture));
            }
            avatar_frame.set_visible(true);
            icon.set_visible(false);
            name_set = true;
        } else if !fb_str.trim().is_empty() {
            // Character was deleted from the catalog: preserve their original name
            // and revert to a standard Adwaita initials avatar.
            let formatted_name = prettify_character_name(fb_str);
            title.set_markup(&format!(
                "<b>{}</b>",
                glib::markup_escape_text(&formatted_name)
            ));
            update_avatar_ring_color("#808080");
            avatar.set_custom_image(None::<&gdk::Texture>);
            avatar.set_text(Some(&formatted_name));
            avatar.set_show_initials(true);
            avatar_frame.set_visible(true);
            icon.set_visible(false);
            name_set = true;
        }

        if !name_set {
            let name = fallback_name
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("Unknown Agent");
            title.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(name)));
            update_avatar_ring_color("#808080");
            avatar_frame.set_visible(false);
            icon.set_visible(true);
        }

        if is_working {
            avatar_frame.add_css_class("claw-avatar-typing");
        } else {
            avatar_frame.remove_css_class("claw-avatar-typing");
        }

        if icon.is_visible() {
            icon.set_icon_name(Some(default_icon));
            if let Some(cls) = icon_class {
                icon.add_css_class(cls);
            }
        }
    };

    let set_user_header = || {
        let profile = user_profile::current();

        icon.set_visible(false);
        user_profile::configure_avatar(&*avatar, &profile);
        update_avatar_ring_color(&profile.color);
        avatar_frame.remove_css_class("claw-avatar-typing");
        avatar_frame.set_visible(true);
        working_dots.set_visible(false);
        set_colored_title(&profile.display_name, &profile.color);
        pane_lbl.set_visible(false);
    };

    let diff_json_opt = obj.cached_file_diff_json();
    let should_typewriter = !is_working && obj.take_typewriter();

    obj.with_row(|row| match row {
        PersistentClawRow::SystemMessage { content, .. } => {
            left_col.set_visible(false);
            header.set_visible(false);
            copy_btn.set_visible(false);
            icon.set_visible(false);
            avatar_frame.set_visible(false);
            working_dots.set_visible(false);
            pane_lbl.set_visible(false);
            title.set_visible(false);
            trace_slot.widget().set_visible(false);
            hbox.add_css_class("system-message");
            hbox.add_css_class("claw-row-system");
            hbox.add_css_class("dimmed");

            viewer.set_content(content);
            viewer.widget().set_visible(true);
            cmd_label.set_visible(false);
        }
        PersistentClawRow::Diagnosis {
            agent_name,
            character_id,
            content,
            ..
        } => {
            set_agent_header(character_id, agent_name, "boxxyclaw", Some("accent"));
            pane_lbl.set_visible(false);
            hbox.add_css_class("claw-row-diagnosis");
            cmd_label.set_visible(false);

            trace_slot.sync(obj, always_show_code_blocks);

            if content.trim().is_empty() {
                viewer.park_for_recycle();
                viewer.widget().set_visible(false);
            } else if is_working {
                viewer.set_streaming_content(content, false);
                viewer.widget().set_visible(true);
            } else if should_typewriter {
                viewer.widget().set_visible(true);
                let char_ends: Vec<usize> = content
                    .char_indices()
                    .map(|(i, c)| i + c.len_utf8())
                    .collect();
                if char_ends.len() <= 2 {
                    viewer.set_content(content);
                } else {
                    let step = (char_ends.len() / 120).clamp(2, 24);
                    let content_arc: std::sync::Arc<str> = content.as_str().into();
                    avatar_frame.add_css_class("claw-avatar-typing");
                    viewer.set_streaming_content(&content_arc[..char_ends[1]], false);
                    let idx = Cell::new(1usize);
                    let viewer_for_tick = viewer.clone();
                    let timer_slot = typewriter_timer.clone();
                    let avatar_frame_for_tick = avatar_frame.clone();
                    let source_id =
                        glib::timeout_add_local(std::time::Duration::from_millis(22), move || {
                            let next_idx = (idx.get() + step).min(char_ends.len() - 1);
                            idx.set(next_idx);
                            let byte_end = char_ends[next_idx];
                            let is_done = next_idx + 1 >= char_ends.len();
                            viewer_for_tick
                                .set_streaming_content(&content_arc[..byte_end], is_done);
                            if is_done {
                                avatar_frame_for_tick.remove_css_class("claw-avatar-typing");
                                let _ = timer_slot.borrow_mut().take();
                                glib::ControlFlow::Break
                            } else {
                                glib::ControlFlow::Continue
                            }
                        });
                    *typewriter_timer.borrow_mut() = Some(source_id);
                }
            } else {
                viewer.set_content(content);
                viewer.widget().set_visible(true);
            }
        }
        PersistentClawRow::User {
            content, images, ..
        } => {
            set_user_header();
            trace_slot.widget().set_visible(false);
            hbox.add_css_class("claw-row-user");

            for img_path in images {
                if let Some(texture) = load_cached_texture(Path::new(img_path)) {
                    let thumb_frame = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                    thumb_frame.set_overflow(gtk::Overflow::Hidden);
                    thumb_frame.add_css_class("user-attachment-thumb");

                    let img = gtk::Image::new();
                    img.set_paintable(Some(&texture));
                    img.set_pixel_size(56);
                    thumb_frame.append(&img);
                    thumbs_box.append(&thumb_frame);
                }
            }
            thumbs_box.set_visible(thumbs_box.first_child().is_some());

            viewer.set_content(content);
            viewer.widget().set_visible(!content.is_empty());
            cmd_label.set_visible(false);
        }
        PersistentClawRow::Suggested {
            agent_name,
            character_id,
            diagnosis,
            command,
            ..
        } => {
            set_agent_header(
                character_id,
                agent_name,
                "boxxy-dialog-warning-symbolic",
                Some("warning"),
            );
            pane_lbl.set_visible(false);
            hbox.add_css_class("claw-row-suggested");

            trace_slot.sync(obj, always_show_code_blocks);

            if diagnosis.is_empty() {
                viewer.park_for_recycle();
                viewer.widget().set_visible(false);
            } else {
                viewer.set_content(diagnosis);
                viewer.widget().set_visible(true);
            }

            cmd_label.set_label(command);
            cmd_label.set_visible(!command.is_empty());
        }
        PersistentClawRow::ProcessList {
            agent_name,
            character_id,
            result_json,
            ..
        } => {
            set_agent_header(character_id, agent_name, "boxxyclaw", Some("accent"));
            pane_lbl.set_visible(false);
            trace_slot.widget().set_visible(false);
            hbox.add_css_class("claw-row-process-list");

            viewer.park_for_recycle();
            viewer.append_custom_block("list_processes", result_json);
            viewer.widget().set_visible(true);
            cmd_label.set_visible(false);
        }
        PersistentClawRow::ToolCall {
            agent_name,
            character_id,
            tool_name,
            ..
        } => {
            set_agent_header(
                character_id,
                agent_name,
                "boxxy-build-circle-symbolic",
                Some("accent"),
            );
            title.set_label(&format!("Used tool: {tool_name}"));
            pane_lbl.set_visible(false);
            trace_slot.widget().set_visible(false);
            hbox.add_css_class("claw-row-tool-call");

            viewer.park_for_recycle();
            viewer.widget().set_visible(false);
            cmd_label.set_visible(false);
        }
        PersistentClawRow::Command { command, exit_code } => {
            set_user_header();
            trace_slot.widget().set_visible(false);

            let status = if *exit_code == 0 {
                "Command Execution"
            } else {
                "Command Failed"
            };
            pane_lbl.set_label(status);
            pane_lbl.set_visible(true);

            hbox.add_css_class("claw-row-command");

            viewer.set_content(command);
            viewer.widget().set_visible(true);
            cmd_label.set_visible(false);
        }
        PersistentClawRow::FileDiff {
            agent_name,
            character_id,
            auto_approved,
            ..
        } => {
            set_agent_header(character_id, agent_name, "boxxyclaw", Some("accent"));
            pane_lbl.set_visible(false);
            trace_slot.widget().set_visible(false);
            hbox.add_css_class("claw-row-file-diff");

            viewer.park_for_recycle();
            if let Some(ref diff_json) = diff_json_opt {
                let schema = if always_show_code_blocks {
                    "file_diff"
                } else if *auto_approved {
                    "file_diff_compact"
                } else {
                    "file_diff_collapsed"
                };
                viewer.append_custom_block(schema, diff_json);
            }
            viewer.widget().set_visible(true);
            cmd_label.set_visible(false);
        }
    });
}

/// Applies the `always_show_code_blocks` policy to `boxxy-viewer` and refreshes all live
/// `ClawRowObject` rows in `store` so inline diffs and fenced code blocks update immediately.
pub fn apply_code_blocks_visibility_policy(store: &gio::ListStore, always_expand: bool) {
    boxxy_viewer::set_always_expand_code_blocks(always_expand);
    let n = store.n_items();
    for i in 0..n {
        if let Some(obj) = store.item(i).and_downcast::<ClawRowObject>() {
            obj.notify_external_refresh();
        }
    }
}

fn format_row_speaker_and_body(row: &PersistentClawRow) -> Option<(String, String)> {
    let resolve_agent_display = |char_id: &Option<String>, fallback: &Option<String>| -> String {
        let cache = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
        let id_str = char_id.as_deref().unwrap_or_default();
        let fb_str = fallback.as_deref().unwrap_or_default();
        if boxxy_claw_protocol::characters::is_incognito_character(id_str)
            || boxxy_claw_protocol::characters::is_incognito_character(fb_str)
        {
            return boxxy_claw_protocol::characters::INCOGNITO_DISPLAY_NAME.to_string();
        }
        let fallback_names = [fb_str];
        if (!id_str.is_empty() || !fb_str.is_empty())
            && let Some(info) = boxxy_claw_protocol::characters::find_character_exact(
                &cache,
                id_str,
                &fallback_names,
            )
        {
            return resolve_character_display_name(info);
        }
        if !fb_str.trim().is_empty() {
            return prettify_character_name(fb_str);
        }
        "Agent".to_string()
    };

    match row {
        PersistentClawRow::User { content, .. } => {
            let profile = user_profile::current();
            Some((profile.display_name, content.clone()))
        }
        PersistentClawRow::Diagnosis {
            agent_name,
            character_id,
            content,
            trace,
            ..
        } => {
            let speaker = resolve_agent_display(character_id, agent_name);
            if content.trim().is_empty() {
                let edits: Vec<String> = trace
                    .iter()
                    .filter_map(|t| match t {
                        TurnTraceItem::Edit { diff, .. } => Some(format!(
                            "File: `{}` (+{} -{})",
                            diff.path, diff.stats.added_lines, diff.stats.removed_lines
                        )),
                        _ => None,
                    })
                    .collect();
                if edits.is_empty() {
                    None
                } else {
                    Some((speaker, edits.join("\n")))
                }
            } else {
                Some((speaker, content.clone()))
            }
        }
        PersistentClawRow::Suggested {
            agent_name,
            character_id,
            diagnosis,
            command,
            ..
        } => {
            let speaker = resolve_agent_display(character_id, agent_name);
            let body = if diagnosis.trim().is_empty() {
                format!("```bash\n{command}\n```")
            } else if command.trim().is_empty() {
                diagnosis.clone()
            } else {
                format!("{diagnosis}\n\n```bash\n{command}\n```")
            };
            Some((speaker, body))
        }
        PersistentClawRow::Command { command, .. } => {
            let profile = user_profile::current();
            Some((profile.display_name, command.clone()))
        }
        PersistentClawRow::FileDiff {
            agent_name,
            character_id,
            diff,
            ..
        } => {
            let speaker = resolve_agent_display(character_id, agent_name);
            let body = format!(
                "File: `{}` (+{} -{})",
                diff.path, diff.stats.added_lines, diff.stats.removed_lines
            );
            Some((speaker, body))
        }
        PersistentClawRow::SystemMessage { .. }
        | PersistentClawRow::ProcessList { .. }
        | PersistentClawRow::ToolCall { .. } => None,
    }
}

/// Formats the full exchange (user prompt + agent response) surrounding `pos`
/// in `store` so clicking Copy on either the prompt or the response copies both.
#[must_use]
pub fn format_exchange_for_copy(store: &gio::ListStore, pos: u32) -> String {
    let n = store.n_items();
    if pos >= n {
        return String::new();
    }

    let is_user_at = |idx: u32| -> Option<bool> {
        store
            .item(idx)
            .and_downcast::<ClawRowObject>()
            .map(|o| o.with_row(|r| matches!(r, PersistentClawRow::User { .. })))
    };

    let Some(clicked_is_user) = is_user_at(pos) else {
        return String::new();
    };

    let mut start_idx = pos;
    if !clicked_is_user {
        let mut cursor = pos;
        while cursor > 0 {
            cursor -= 1;
            if is_user_at(cursor) == Some(true) {
                start_idx = cursor;
                break;
            }
        }
    }

    let mut sections = Vec::new();
    for idx in start_idx..n {
        let Some(obj) = store.item(idx).and_downcast::<ClawRowObject>() else {
            continue;
        };
        if idx > start_idx && obj.with_row(|r| matches!(r, PersistentClawRow::User { .. })) {
            break;
        }
        if let Some((speaker, body)) = obj.with_row(format_row_speaker_and_body)
            && !body.trim().is_empty()
        {
            sections.push(format!("{speaker}:\n{}", body.trim()));
        }
    }

    sections.join("\n\n")
}

// ---------------------------------------------------------------------------
// Row-append helpers (live working & read-only history rows)
// ---------------------------------------------------------------------------

#[must_use]
pub fn add_working_turn_row(
    store: &gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    character_id: Option<String>,
) -> ClawRowObject {
    let obj = ClawRowObject::new_working(pane_id, agent_name, character_id);
    store.append(&obj);
    obj
}

pub fn add_system_message_row(store: &gio::ListStore, pane_id: String, content: &str) {
    store.append(&ClawRowObject::new(PersistentClawRow::SystemMessage {
        pane_id,
        content: content.to_string(),
    }));
}

pub fn add_diagnosis_row(
    store: &gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    character_id: Option<String>,
    content: &str,
) {
    add_diagnosis_row_with_trace(
        store,
        pane_id,
        agent_name,
        character_id,
        content,
        Vec::new(),
        false,
    );
}

pub fn add_diagnosis_row_with_trace(
    store: &gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    character_id: Option<String>,
    content: &str,
    trace: Vec<TurnTraceItem>,
    typewriter: bool,
) {
    store.append(&ClawRowObject::new_with_typewriter(
        PersistentClawRow::Diagnosis {
            pane_id,
            agent_name,
            character_id,
            content: content.to_string(),
            trace,
            usage: None,
        },
        typewriter,
    ));
}

pub fn add_user_row(store: &gio::ListStore, pane_id: String, content: &str, images: Vec<String>) {
    store.append(&ClawRowObject::new(PersistentClawRow::User {
        pane_id,
        content: content.to_string(),
        images,
    }));
}

pub fn add_suggested_row_with_trace(
    store: &gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    character_id: Option<String>,
    diagnosis: &str,
    command: &str,
    trace: Vec<TurnTraceItem>,
) {
    store.append(&ClawRowObject::new(PersistentClawRow::Suggested {
        pane_id,
        agent_name,
        character_id,
        diagnosis: diagnosis.to_string(),
        command: command.to_string(),
        trace,
        usage: None,
    }));
}

pub fn add_approval_row(
    store: &gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    character_id: Option<String>,
    command: &str,
) {
    add_diagnosis_row(
        store,
        pane_id,
        agent_name,
        character_id,
        &format!("Proposed command:\n```bash\n{command}\n```"),
    );
}

pub fn add_file_delete_approval_row(
    store: &gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    character_id: Option<String>,
    path: &str,
) {
    add_diagnosis_row(
        store,
        pane_id,
        agent_name,
        character_id,
        &format!("Proposed file deletion: `{path}`"),
    );
}

pub fn add_kill_process_approval_row(
    store: &gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    character_id: Option<String>,
    pid: u32,
    process_name: &str,
) {
    add_diagnosis_row(
        store,
        pane_id,
        agent_name,
        character_id,
        &format!("Proposed killing process: {process_name} (PID: {pid})"),
    );
}

pub fn add_clipboard_get_approval_row(
    store: &gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    character_id: Option<String>,
) {
    add_diagnosis_row(
        store,
        pane_id,
        agent_name,
        character_id,
        "Proposed reading from clipboard.",
    );
}

pub fn add_clipboard_set_approval_row(
    store: &gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    character_id: Option<String>,
    text: &str,
) {
    add_diagnosis_row(
        store,
        pane_id,
        agent_name,
        character_id,
        &format!("Proposed writing to clipboard:\n```\n{text}\n```"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_exchange_for_copy_pairs_prompt_and_response() {
        let store = gio::ListStore::new::<ClawRowObject>();
        add_user_row(
            &store,
            "pane-1".to_string(),
            "Explain the parser module.",
            Vec::new(),
        );
        add_system_message_row(&store, "pane-1".to_string(), "Claw: Model A");
        add_diagnosis_row(
            &store,
            "pane-1".to_string(),
            Some("Niko Una".to_string()),
            None,
            "The parser converts source tokens into an AST.",
        );
        add_user_row(
            &store,
            "pane-1".to_string(),
            "What about errors?",
            Vec::new(),
        );
        add_diagnosis_row(
            &store,
            "pane-1".to_string(),
            Some("Niko Una".to_string()),
            None,
            "Errors are collected into a diagnostic list.",
        );

        let user_name = user_profile::current().display_name;
        let expected_first = format!(
            "{user_name}:\nExplain the parser module.\n\nNiko Una:\nThe parser converts source tokens into an AST."
        );

        assert_eq!(format_exchange_for_copy(&store, 0), expected_first);
        assert_eq!(format_exchange_for_copy(&store, 2), expected_first);

        let expected_second = format!(
            "{user_name}:\nWhat about errors?\n\nNiko Una:\nErrors are collected into a diagnostic list."
        );
        assert_eq!(format_exchange_for_copy(&store, 3), expected_second);
        assert_eq!(format_exchange_for_copy(&store, 4), expected_second);
    }
}
