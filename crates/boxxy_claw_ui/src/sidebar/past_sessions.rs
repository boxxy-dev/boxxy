//! Past Conversations resume list rendered inside the Claw sidebar on blank panes.

use crate::character_style::{
    format_relative_age, load_cached_texture, prettify_character_name,
    resolve_character_display_name,
};
use boxxy_core_widgets::ObjectExtSafe;
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub(super) type RenderedSessionSig = (
    Vec<(
        String,
        String,
        Option<String>,
        Option<String>,
        bool,
        i64,
        Option<String>,
    )>,
    Vec<(String, String, bool)>,
);

pub(super) type ResumeSessionCallback =
    Rc<RefCell<Option<Rc<dyn Fn(String, Option<String>) + 'static>>>>;

#[derive(Clone)]
pub(super) struct PastSessionsCtx {
    pub(super) sessions_box: gtk::Box,
    pub(super) sessions_list: gtk::ListBox,
    pub(super) hero_box: gtk::Box,
    pub(super) hero_icon: gtk::Image,
    pub(super) is_new_session: Rc<Cell<bool>>,
    pub(super) fetch_seq: Rc<Cell<u64>>,
    pub(super) rendered_sessions_sig: Rc<RefCell<RenderedSessionSig>>,
    pub(super) active_options_popover: Rc<RefCell<Option<gtk::Popover>>>,
    pub(super) on_resume_session: ResumeSessionCallback,
}

#[allow(clippy::too_many_lines)]
pub(super) fn populate_past_sessions(ctx: PastSessionsCtx, force: bool) {
    if !force
        && ctx
            .active_options_popover
            .borrow()
            .as_ref()
            .is_some_and(gtk::prelude::WidgetExt::is_visible)
    {
        return;
    }

    let seq = ctx.fetch_seq.get().wrapping_add(1);
    ctx.fetch_seq.set(seq);

    glib::spawn_future_local(async move {
        let fetched = boxxy_sys_utils::runtime()
            .spawn(async move {
                let db = boxxy_db::Db::new().await?;
                let store = boxxy_db::store::Store::new(db.pool());
                store.get_recent_active_sessions(10).await
            })
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();

        // Drop stale out-of-order DB query completions if a newer refresh was started.
        if ctx.fetch_seq.get() != seq {
            return;
        }

        if !force
            && ctx
                .active_options_popover
                .borrow()
                .as_ref()
                .is_some_and(gtk::prelude::WidgetExt::is_visible)
        {
            return;
        }

        if !ctx.is_new_session.get() {
            *ctx.rendered_sessions_sig.borrow_mut() = (Vec::new(), Vec::new());
            while let Some(row) = ctx.sessions_list.row_at_index(0) {
                ctx.sessions_list.remove(&row);
            }
            ctx.sessions_box.set_visible(false);
            return;
        }

        if fetched.is_empty() {
            *ctx.rendered_sessions_sig.borrow_mut() = (Vec::new(), Vec::new());
            while let Some(row) = ctx.sessions_list.row_at_index(0) {
                ctx.sessions_list.remove(&row);
            }
            ctx.sessions_box.set_visible(false);
            ctx.hero_box.set_visible(true);
            ctx.hero_box.set_vexpand(true);
            ctx.hero_box.set_valign(gtk::Align::Center);
            ctx.hero_icon.set_pixel_size(72);
            return;
        }

        ctx.hero_box.set_visible(false);
        ctx.sessions_box.set_visible(true);

        let registry = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
        let claims = boxxy_claw_protocol::characters::CLAIMS_CACHE.load();

        let new_sessions_sig: Vec<_> = fetched
            .iter()
            .map(|s| {
                (
                    s.id.clone(),
                    s.character_id.clone(),
                    s.agent_name.clone(),
                    s.title.clone(),
                    s.pinned,
                    s.message_count,
                    s.updated_at.clone(),
                )
            })
            .collect();
        let new_claims_sig: Vec<(String, String, bool)> = claims
            .iter()
            .map(|c| {
                (
                    c.character_id.clone(),
                    c.session_id.clone(),
                    c.owner_bus_name.is_empty(),
                )
            })
            .collect();
        let new_sig = (new_sessions_sig, new_claims_sig);

        if !force
            && *ctx.rendered_sessions_sig.borrow() == new_sig
            && ctx.sessions_list.row_at_index(0).is_some()
        {
            return;
        }
        *ctx.rendered_sessions_sig.borrow_mut() = new_sig;

        if let Some(prev_pop) = ctx.active_options_popover.borrow_mut().take() {
            prev_pop.popdown();
        }
        while let Some(row) = ctx.sessions_list.row_at_index(0) {
            ctx.sessions_list.remove(&row);
        }

        let chars_dir = boxxy_claw_protocol::character_loader::get_characters_dir().ok();

        for session in fetched {
            let session_id = session.id.clone();
            let is_pinned = session.pinned;

            let mut title = session
                .title
                .clone()
                .unwrap_or_else(|| "Untitled Session".to_string());
            if title.trim().is_empty() {
                title = "Untitled Session".to_string();
            }

            let raw_agent = session.agent_name.clone().unwrap_or_default();
            let fallback_names = [
                session.character_display_name.as_str(),
                raw_agent.as_str(),
                session.name.as_str(),
            ];
            let char_info = boxxy_claw_protocol::characters::resolve_character(
                &registry,
                &session.character_id,
                &fallback_names,
            );

            let active_claim =
                char_info.and_then(|info| claims.iter().find(|c| c.character_id == info.config.id));
            let is_active_session = active_claim.is_some_and(|c| c.session_id == session_id);
            let is_in_background = active_claim.is_some_and(|c| c.owner_bus_name.is_empty());
            let is_disabled_row = active_claim.is_some() && !is_active_session;

            let char_display = char_info.map_or_else(
                || {
                    if !session.character_display_name.is_empty() {
                        prettify_character_name(&session.character_display_name)
                    } else if !raw_agent.is_empty() {
                        prettify_character_name(&raw_agent)
                    } else {
                        "AI Agent".to_string()
                    }
                },
                resolve_character_display_name,
            );

            let age = session.updated_at.as_deref().map_or_else(
                || "recent".to_string(),
                |ts| format_relative_age(ts, "recent"),
            );

            let msg_word = if session.message_count == 1 {
                "msg"
            } else {
                "msgs"
            };
            let base_meta = format!(
                "{} • {} {} • {}",
                char_display, session.message_count, msg_word, age
            );
            let subtitle_text = if is_disabled_row && !is_active_session {
                if is_in_background {
                    format!("In Background • {base_meta}")
                } else {
                    format!("In Use • {base_meta}")
                }
            } else {
                base_meta
            };

            let row = gtk::ListBoxRow::new();
            row.add_css_class("past-session-row");
            row.add_css_class("activatable");
            row.set_safe_data("session_id", session_id.clone());
            if is_disabled_row {
                row.set_activatable(false);
                row.set_focusable(false);
            }

            let outer_hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            outer_hbox.set_margin_top(6);
            outer_hbox.set_margin_bottom(6);
            outer_hbox.set_margin_start(10);
            outer_hbox.set_margin_end(6);
            outer_hbox.set_hexpand(true);

            let resume_btn_content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            resume_btn_content.set_hexpand(true);
            if is_active_session {
                resume_btn_content.set_tooltip_text(Some(if is_in_background {
                    "Reattach active background session"
                } else {
                    "Focus active session"
                }));
            } else if is_disabled_row {
                resume_btn_content.set_tooltip_text(Some(if is_in_background {
                    "Character is already running in the background"
                } else {
                    "Character is currently in use"
                }));
            }

            let avatar = adw::Avatar::new(28, Some(&char_display), true);
            avatar.set_valign(gtk::Align::Center);
            if let Some(info) = char_info
                && info.has_avatar
                && let Some(ref dir) = chars_dir
            {
                let avatar_path = dir.join(&info.config.name).join("AVATAR.png");
                if let Some(texture) = load_cached_texture(&avatar_path) {
                    avatar.set_custom_image(Some(&texture));
                }
            }
            resume_btn_content.append(&avatar);

            let text_vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
            text_vbox.set_hexpand(true);
            text_vbox.set_valign(gtk::Align::Center);
            if is_disabled_row {
                text_vbox.set_opacity(0.5);
            }

            let title_hbox = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            title_hbox.set_hexpand(true);
            let title_lbl = gtk::Label::builder()
                .label(&title)
                .css_classes(["caption", "heading"])
                .halign(gtk::Align::Start)
                .hexpand(true)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            title_hbox.append(&title_lbl);
            text_vbox.append(&title_hbox);

            let sub_hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            sub_hbox.set_halign(gtk::Align::Start);
            let sub_lbl = gtk::Label::builder()
                .label(&subtitle_text)
                .css_classes(["caption", "dim-label"])
                .halign(gtk::Align::Start)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            sub_hbox.append(&sub_lbl);
            if is_pinned {
                let pin_icon = gtk::Image::builder()
                    .icon_name("boxxy-view-pin-symbolic")
                    .pixel_size(14)
                    .css_classes(["accent"])
                    .valign(gtk::Align::Center)
                    .build();
                sub_hbox.append(&pin_icon);
            }
            if is_active_session {
                let bolt_icon = gtk::Image::builder()
                    .icon_name("boxxy-vibe-symbolic")
                    .pixel_size(14)
                    .css_classes(["success"])
                    .valign(gtk::Align::Center)
                    .build();
                sub_hbox.append(&bolt_icon);
            }
            text_vbox.append(&sub_hbox);

            resume_btn_content.append(&text_vbox);
            outer_hbox.append(&resume_btn_content);

            let options_btn = gtk::Button::builder()
                .icon_name("view-more-symbolic")
                .css_classes(["flat", "circular", "past-session-options-btn"])
                .valign(gtk::Align::Center)
                .can_focus(false)
                .tooltip_text("Conversation Options")
                .build();

            let popover = gtk::Popover::builder()
                .has_arrow(false)
                .autohide(true)
                .position(gtk::PositionType::Bottom)
                .build();
            popover.add_css_class("menu");
            popover.add_css_class("past-session-popover");
            popover.set_parent(&options_btn);

            {
                let btn_weak = options_btn.downgrade();
                popover.connect_visible_notify(move |pop| {
                    if let Some(btn) = btn_weak.upgrade() {
                        let row = btn
                            .ancestor(gtk::ListBoxRow::static_type())
                            .and_downcast::<gtk::ListBoxRow>();
                        if pop.is_visible() {
                            btn.add_css_class("menu-open");
                            if let Some(r) = row {
                                r.add_css_class("menu-open");
                            }
                        } else {
                            btn.remove_css_class("menu-open");
                            if let Some(r) = row {
                                r.remove_css_class("menu-open");
                            }
                        }
                    }
                });
            }

            let popover_cleanup = popover.clone();
            options_btn.connect_destroy(move |_| {
                popover_cleanup.unparent();
            });

            {
                let pop_toggle = popover.clone();
                let active_pop_slot = ctx.active_options_popover.clone();
                options_btn.connect_clicked(move |_| {
                    if pop_toggle.is_visible() {
                        pop_toggle.popdown();
                    } else {
                        if let Some(prev) = active_pop_slot.borrow_mut().replace(pop_toggle.clone())
                            && prev != pop_toggle
                        {
                            prev.popdown();
                        }
                        pop_toggle.popup();
                    }
                });
            }

            let menu_vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
            menu_vbox.set_margin_top(4);
            menu_vbox.set_margin_bottom(4);
            menu_vbox.set_margin_start(4);
            menu_vbox.set_margin_end(4);

            let pin_hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            pin_hbox.append(&gtk::Image::from_icon_name("boxxy-view-pin-symbolic"));
            pin_hbox.append(
                &gtk::Label::builder()
                    .label(if is_pinned {
                        "Unpin Conversation"
                    } else {
                        "Pin Conversation"
                    })
                    .halign(gtk::Align::Start)
                    .xalign(0.0)
                    .build(),
            );
            let pin_btn = gtk::Button::builder()
                .css_classes(["flat"])
                .child(&pin_hbox)
                .build();

            let del_hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            del_hbox.append(&gtk::Image::from_icon_name("boxxy-user-trash-symbolic"));
            del_hbox.append(
                &gtk::Label::builder()
                    .label("Delete Conversation")
                    .halign(gtk::Align::Start)
                    .xalign(0.0)
                    .build(),
            );
            let del_btn = gtk::Button::builder()
                .css_classes(["flat", "error"])
                .child(&del_hbox)
                .sensitive(!is_active_session)
                .build();

            {
                let pop_weak = popover.downgrade();
                let sid_pin = session_id.clone();
                let ctx_for_pin = ctx.clone();
                pin_btn.connect_clicked(move |_| {
                    if let Some(p) = pop_weak.upgrade() {
                        p.popdown();
                    }
                    let sid = sid_pin.clone();
                    let next_pinned = !is_pinned;
                    let ctx_refresh = ctx_for_pin.clone();
                    glib::spawn_future_local(async move {
                        let _ = boxxy_sys_utils::runtime()
                            .spawn(async move {
                                if let Ok(db) = boxxy_db::Db::new().await {
                                    let store = boxxy_db::store::Store::new(db.pool());
                                    let _ = store.set_session_pinned(&sid, next_pinned).await;
                                }
                            })
                            .await;
                        populate_past_sessions(ctx_refresh, true);
                    });
                });
            }

            {
                let pop_weak = popover.downgrade();
                let sid_del = session_id.clone();
                let ctx_for_del = ctx.clone();
                del_btn.connect_clicked(move |_| {
                    if let Some(p) = pop_weak.upgrade() {
                        p.popdown();
                    }
                    let sid = sid_del.clone();
                    let ctx_refresh = ctx_for_del.clone();
                    glib::spawn_future_local(async move {
                        let _ = boxxy_sys_utils::runtime()
                            .spawn(async move {
                                if let Ok(db) = boxxy_db::Db::new().await {
                                    let store = boxxy_db::store::Store::new(db.pool());
                                    let _ = store.delete_session(&sid).await;
                                }
                            })
                            .await;
                        populate_past_sessions(ctx_refresh, true);
                    });
                });
            }

            menu_vbox.append(&pin_btn);

            if !is_active_session {
                let current_char_id =
                    char_info.map_or(session.character_id.as_str(), |c| c.config.id.as_str());
                let available_others: Vec<_> =
                    boxxy_claw_protocol::characters::find_available_characters_in(
                        &registry, &claims, None,
                    )
                    .into_iter()
                    .filter(|c| c.config.id != current_char_id)
                    .collect();

                if !available_others.is_empty() {
                    let continue_hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                    continue_hbox.append(&gtk::Image::from_icon_name("boxxy-chat-symbolic"));
                    continue_hbox.append(
                        &gtk::Label::builder()
                            .label("Continue with…")
                            .halign(gtk::Align::Start)
                            .hexpand(true)
                            .xalign(0.0)
                            .build(),
                    );
                    let chevron = gtk::Image::from_icon_name("boxxy-down-symbolic");
                    chevron.add_css_class("dim-label");
                    continue_hbox.append(&chevron);

                    let continue_btn = gtk::Button::builder()
                        .css_classes(["flat"])
                        .child(&continue_hbox)
                        .build();

                    let continue_revealer = gtk::Revealer::builder()
                        .transition_type(gtk::RevealerTransitionType::SlideDown)
                        .transition_duration(150)
                        .reveal_child(false)
                        .build();

                    let chars_vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
                    chars_vbox.set_margin_start(14);
                    chars_vbox.set_margin_top(2);
                    chars_vbox.set_margin_bottom(2);

                    for alt_char in available_others {
                        let alt_display = resolve_character_display_name(alt_char);

                        let alt_hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                        let alt_avatar = adw::Avatar::new(18, Some(&alt_display), true);
                        alt_avatar.set_valign(gtk::Align::Center);
                        if alt_char.has_avatar
                            && let Some(ref dir) = chars_dir
                        {
                            let avatar_path = dir.join(&alt_char.config.name).join("AVATAR.png");
                            if let Some(texture) = load_cached_texture(&avatar_path) {
                                alt_avatar.set_custom_image(Some(&texture));
                            }
                        }
                        alt_hbox.append(&alt_avatar);
                        alt_hbox.append(
                            &gtk::Label::builder()
                                .label(&alt_display)
                                .halign(gtk::Align::Start)
                                .xalign(0.0)
                                .build(),
                        );

                        let alt_btn = gtk::Button::builder()
                            .css_classes(["flat"])
                            .child(&alt_hbox)
                            .build();

                        let pop_weak = popover.downgrade();
                        let sid_alt = session_id.clone();
                        let target_cid = alt_char.config.id.clone();
                        let cb_alt = ctx.on_resume_session.clone();
                        alt_btn.connect_clicked(move |_| {
                            if let Some(p) = pop_weak.upgrade() {
                                p.popdown();
                            }
                            if let Some(cb) = cb_alt.borrow().as_ref().cloned() {
                                cb(sid_alt.clone(), Some(target_cid.clone()));
                            }
                        });
                        chars_vbox.append(&alt_btn);
                    }

                    continue_revealer.set_child(Some(&chars_vbox));

                    {
                        let rev_toggle = continue_revealer.clone();
                        continue_btn.connect_clicked(move |_| {
                            rev_toggle.set_reveal_child(!rev_toggle.reveals_child());
                        });
                    }
                    {
                        let rev_weak = continue_revealer.downgrade();
                        popover.connect_visible_notify(move |pop| {
                            if !pop.is_visible()
                                && let Some(rev) = rev_weak.upgrade()
                            {
                                rev.set_reveal_child(false);
                            }
                        });
                    }

                    menu_vbox.append(&continue_btn);
                    menu_vbox.append(&continue_revealer);
                }
            }

            menu_vbox.append(&del_btn);
            popover.set_child(Some(&menu_vbox));

            outer_hbox.append(&options_btn);

            row.set_child(Some(&outer_hbox));
            ctx.sessions_list.append(&row);
        }
    });
}
