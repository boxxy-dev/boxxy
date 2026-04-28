use crate::claw_host::ClawHost;
use crate::msgbar::MsgBarComponent;
use crate::proposal::Proposal;
use boxxy_claw_protocol::ClawMessage;
use boxxy_viewer::StructuredViewer;
use gtk::prelude::*;
use gtk4 as gtk;
use gtk4::gio;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OverlayMode {
    Claw,
    Bookmark,
}

#[derive(Clone)]
pub struct TerminalOverlay {
    revealer: gtk::Revealer,
    indicator_slot: gtk::Box,
    character_selector_box: gtk::Box,
    single_scroll: gtk::ScrolledWindow,
    history_scroll: gtk::ScrolledWindow,
    history_list: gtk::ListView,
    history_store: gio::ListStore,
    diagnosis_viewer: StructuredViewer,
    command_view: gtk::TextView,
    template_entry: gtk::Entry,
    msg_bar: Rc<MsgBarComponent>,
    accept_btn: gtk::Button,
    reject_btn: gtk::Button,
    ok_btn: gtk::Button,
    approve_file_btn: gtk::Button,
    inspect_btn: gtk::Button,
    command_frame: gtk::Frame,
    template_box: gtk::Box,
    file_action_box: gtk::Box,
    action_box: gtk::Box,
    current_proposal: Rc<RefCell<Proposal>>,
    current_mode: Rc<RefCell<OverlayMode>>,
    is_thinking: Rc<Cell<bool>>,
    active_agent: Rc<RefCell<String>>,
    history_enabled: Rc<Cell<bool>>,
    history_sticky: Rc<Cell<bool>>,
    is_auto_scrolling: Rc<Cell<bool>>,
    /// Character name pre-selected in the picker before any session starts.
    selected_character: Rc<RefCell<String>>,
    host: Rc<dyn ClawHost>,
}

impl TerminalOverlay {
    pub fn new(
        indicator_widget: &gtk::Widget,
        msg_bar: Rc<MsgBarComponent>,
        host: Rc<dyn ClawHost>,
        pending_character: Rc<RefCell<String>>,
    ) -> Self {
        let builder = gtk::Builder::from_resource("/dev/boxxy/BoxxyTerminal/ui/claw_overlay.ui");

        let revealer: gtk::Revealer = builder.object("root_revealer").unwrap();
        let indicator_slot: gtk::Box = builder.object("indicator_slot").unwrap();
        let character_selector_box: gtk::Box = builder.object("character_selector_box").unwrap();

        indicator_slot.append(indicator_widget);
        let single_scroll: gtk::ScrolledWindow = builder.object("single_scroll").unwrap();
        let history_scroll: gtk::ScrolledWindow = builder.object("history_scroll").unwrap();

        let command_view: gtk::TextView = builder.object("command_view").unwrap();
        let template_entry: gtk::Entry = builder.object("template_entry").unwrap();

        let accept_btn: gtk::Button = builder.object("accept_btn").unwrap();
        let reject_btn: gtk::Button = builder.object("reject_btn").unwrap();
        let ok_btn: gtk::Button = builder.object("ok_btn").unwrap();

        let reject_file_btn: gtk::Button = builder.object("reject_file_btn").unwrap();
        let approve_file_btn: gtk::Button = builder.object("approve_file_btn").unwrap();
        let inspect_btn: gtk::Button = builder.object("inspect_btn").unwrap();

        let command_frame: gtk::Frame = builder.object("command_frame").unwrap();
        let template_box: gtk::Box = builder.object("template_box").unwrap();
        let file_action_box: gtk::Box = builder.object("file_action_box").unwrap();
        let action_box: gtk::Box = builder.object("action_box").unwrap();

        let diagnosis_container: gtk::Box = builder.object("diagnosis_container").unwrap();
        let diagnosis_viewer = StructuredViewer::new(boxxy_claw_ui::get_claw_viewer_registry());
        diagnosis_container.append(diagnosis_viewer.widget());

        // Embed the merged msgbar into the drawer's bottom area. The
        // msgbar owns attachments, autocomplete, history nav, Ctrl+V
        // paste, and the 4 status toggles — one manager per drawer, one
        // drawer per pane. The send button is appended *next* to the bar
        // (not inside it) so the bar can render as a single rounded field
        // and the send icon floats alongside without a background.
        let msgbar_slot: gtk::Box = builder.object("msgbar_slot").unwrap();
        msg_bar.widget.set_hexpand(true);
        msgbar_slot.append(&msg_bar.widget);
        msgbar_slot.append(&msg_bar.send_btn);
        msg_bar.set_embedded(true);

        // Build the virtualized history list (Claude-Code-style scrollable log).
        // Uses the same factory + backing store as the sidebar so a huge
        // conversation stays O(visible_rows) in memory.
        //
        // We set the list as the direct child of the ScrolledWindow so that
        // they share a single Adjustment. This ensures scroll_to(FOCUS) correctly
        // drives the visible viewport.
        let (history_list, history_store) = boxxy_claw_ui::create_claw_message_list();
        history_scroll.set_child(Some(&history_list));

        // Improved auto-scroll logic (Fractal-style).
        // 1. On items_changed: arm a "sticky to bottom" flag for 1200ms.
        // 2. On value_changed: if the user scrolls up manually, clear the sticky flag.
        // 3. On notify::upper: if sticky and we were at the bottom, snap to latest.
        let history_sticky = Rc::new(Cell::new(false));
        let is_auto_scrolling = Rc::new(Cell::new(false));
        let prev_upper = Rc::new(Cell::new(0.0));

        let sticky = history_sticky.clone();
        let list_items = history_list.clone();
        let is_auto_items = is_auto_scrolling.clone();
        history_store.connect_items_changed(move |s, _, _, _| {
            let n = s.n_items();
            if n == 0 {
                return;
            }
            let was_idle = !sticky.get();
            sticky.set(true);

            let lv = list_items.clone();
            let is_auto = is_auto_items.clone();
            gtk::glib::idle_add_local_once(move || {
                is_auto.set(true);
                lv.scroll_to(n - 1, gtk::ListScrollFlags::FOCUS, None);
                gtk::glib::idle_add_local_once(move || {
                    is_auto.set(false);
                });
            });

            if was_idle {
                let sticky_timer = sticky.clone();
                gtk::glib::timeout_add_local(std::time::Duration::from_millis(1200), move || {
                    sticky_timer.set(false);
                    gtk::glib::ControlFlow::Break
                });
            }
        });

        let adj = history_scroll.vadjustment();
        let sticky_val = history_sticky.clone();
        let is_auto = is_auto_scrolling.clone();
        let prev_val = Rc::new(Cell::new(adj.value()));
        adj.connect_value_changed(move |a| {
            let current_val = a.value();
            let old_val = prev_val.replace(current_val);
            if is_auto.get() {
                return;
            }

            let at_bottom = (current_val + a.page_size() - a.upper()).abs() < 60.0;

            // If the user scrolls up away from the bottom, clear the sticky flag
            // so we don't yank them back down.
            if current_val < old_val - 1.0 {
                if !at_bottom {
                    sticky_val.set(false);
                }
            } else if at_bottom {
                // If they scroll back down to the true bottom, re-arm auto-scroll.
                sticky_val.set(true);
            }
        });

        let sticky_upper = history_sticky.clone();
        let prev_u = prev_upper.clone();
        let is_auto_upper = is_auto_scrolling.clone();
        let list_upper = history_list.clone();
        adj.connect_notify_local(Some("upper"), move |a, _| {
            let new_upper = a.upper();
            let old_upper = prev_u.replace(new_upper);

            if !sticky_upper.get() || new_upper <= old_upper {
                return;
            }

            // Generous tolerance (60px) to account for padding/margins.
            let at_bottom = a.value() + a.page_size() >= old_upper - 60.0;
            if at_bottom {
                is_auto_upper.set(true);
                let n = list_upper.model().map(|m| m.n_items()).unwrap_or(0);
                if n > 0 {
                    list_upper.scroll_to(n - 1, gtk::ListScrollFlags::FOCUS, None);
                }
                a.set_value(new_upper - a.page_size());
                let is_auto_idle = is_auto_upper.clone();
                gtk::glib::idle_add_local_once(move || {
                    is_auto_idle.set(false);
                });
            }
        });

        // Host adapter owns all pane-side interactions: focus grab, byte
        // injection, script execution (with the tempfile trick),
        // ClawMessage dispatch, sidebar focus. Every click handler below
        // is ~2 lines of `host.xyz()` now that the terminal-specific
        // logic lives behind the trait.
        //
        // Event-masking fix: the Revealer's `crossfade` transition keeps
        // its child *allocated at full size with opacity 0* while hidden
        // — which means the drawer would silently eat mouse events even
        // when invisible, blocking terminal text selection. Toggling
        // `can_target` alongside the reveal state makes the revealer
        // transparent to pointer events whenever it's hidden. The initial
        // `can_target=false` handles the "never shown yet" case on pane
        // creation.
        revealer.set_can_target(false);
        let host_vis = host.clone();
        let revealer_for_target = revealer.clone();
        revealer.connect_reveal_child_notify(move |rev| {
            let revealed = rev.reveals_child();
            host_vis.set_focusable(!revealed);
            revealer_for_target.set_can_target(revealed);
        });

        let current_proposal = Rc::new(RefCell::new(Proposal::None));
        let current_mode = Rc::new(RefCell::new(OverlayMode::Claw));
        let is_thinking = Rc::new(Cell::new(false));
        let active_agent = Rc::new(RefCell::new(String::new()));
        let history_enabled = Rc::new(Cell::new(false));

        // Common "dismiss the drawer + return focus to the host" tail,
        // used by Reject, Ok, and the two file-action buttons. The
        // 50ms delay mirrors the pre-refactor behavior — it lets the
        // Revealer's fade-out start before focus grabs, so GTK doesn't
        // steal focus from the still-animating widget.
        let dismiss_and_refocus = {
            let host = host.clone();
            let revealer = revealer.clone();
            let command_frame = command_frame.clone();
            let template_box = template_box.clone();
            let file_action_box = file_action_box.clone();
            let action_box = action_box.clone();

            Rc::new(move || {
                revealer.set_reveal_child(false);
                // Robust hiding: Ensure that if the drawer receives a new event while fading out,
                // the stale proposal buttons are already hidden.
                command_frame.set_visible(false);
                template_box.set_visible(false);
                file_action_box.set_visible(false);
                action_box.set_visible(false);

                let host = host.clone();
                gtk4::glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
                    host.grab_focus();
                    gtk4::glib::ControlFlow::Break
                });
            })
        };

        // Reject / Ok: in Claw mode send CancelPending so the agent
        // stops waiting; in Bookmark mode it's purely a UI dismiss.
        let host_reject = host.clone();
        let cm_reject = current_mode.clone();
        let dismiss_reject = dismiss_and_refocus.clone();
        reject_btn.connect_clicked(move |_| {
            if *cm_reject.borrow() == OverlayMode::Claw {
                host_reject.send_claw(ClawMessage::CancelPending);
            }
            dismiss_reject();
        });

        let host_ok = host.clone();
        let cm_ok = current_mode.clone();
        let dismiss_ok = dismiss_and_refocus.clone();
        ok_btn.connect_clicked(move |_| {
            if *cm_ok.borrow() == OverlayMode::Claw {
                host_ok.send_claw(ClawMessage::CancelPending);
            }
            dismiss_ok();
        });

        // Approve / Reject for file / clipboard / kill-process proposals.
        // We pattern-match on current_proposal to pick the right reply
        // message type — same logic as before, just inlined here now
        // that the trait hides the channel.
        let make_file_reply = |proposal: &Proposal, approved: bool| -> ClawMessage {
            match proposal {
                Proposal::FileWrite { .. } => ClawMessage::FileWriteReply { approved },
                Proposal::FileDelete { .. } => ClawMessage::FileDeleteReply { approved },
                Proposal::KillProcess { .. } => ClawMessage::KillProcessReply { approved },
                Proposal::GetClipboard => ClawMessage::GetClipboardReply { approved },
                Proposal::SetClipboard(_) => ClawMessage::SetClipboardReply { approved },
                _ => ClawMessage::FileWriteReply { approved },
            }
        };

        let host_approve = host.clone();
        let cp_approve = current_proposal.clone();
        let dismiss_approve = dismiss_and_refocus.clone();
        approve_file_btn.connect_clicked(move |_| {
            let msg = make_file_reply(&cp_approve.borrow(), true);
            host_approve.send_claw(msg);
            dismiss_approve();
        });

        let host_reject_file = host.clone();
        let cp_reject = current_proposal.clone();
        let dismiss_reject_file = dismiss_and_refocus.clone();
        reject_file_btn.connect_clicked(move |_| {
            let msg = make_file_reply(&cp_reject.borrow(), false);
            host_reject_file.send_claw(msg);
            dismiss_reject_file();
        });

        // Inspect — route the user to the sidebar-side log.
        let host_inspect = host.clone();
        inspect_btn.connect_clicked(move |_| {
            host_inspect.focus_sidebar();
        });

        // Accept: Command proposals inject the (possibly-edited) buffer
        // text; Bookmark proposals expand placeholders from
        // template_entry and hand the expanded script to the host's
        // `execute_script`, which on the terminal side writes it to an
        // ephemeral file under the bookmarks-runs cache and injects the
        // path. That filesystem logic now lives in `PaneClawHost` so the
        // widget stays IO-free.
        let host_accept = host.clone();
        let cmd_view_clone = command_view.clone();
        let current_proposal_for_accept = current_proposal.clone();
        let template_entry_clone = template_entry.clone();
        let dismiss_accept = dismiss_and_refocus.clone();
        accept_btn.connect_clicked(move |_| {
            let proposal = current_proposal_for_accept.borrow().clone();
            match proposal {
                Proposal::Bookmark {
                    filename,
                    script,
                    placeholders,
                } => {
                    let input_str = template_entry_clone.text().to_string();
                    let values: Vec<String> =
                        input_str.split(',').map(|s| s.trim().to_string()).collect();
                    let mut expanded = script;
                    for (i, name) in placeholders.iter().enumerate() {
                        if let Some(val) = values.get(i) {
                            let pattern = format!("{{{{{{{}}}}}}}", name);
                            expanded = expanded.replace(&pattern, val);
                        }
                    }
                    host_accept.execute_script(&filename, expanded);
                }
                _ => {
                    let buffer = cmd_view_clone.buffer();
                    let start = buffer.start_iter();
                    let end = buffer.end_iter();
                    let cmd = buffer.text(&start, &end, false).to_string();
                    host_accept.inject_line(cmd);
                }
            }
            dismiss_accept();
        });

        let accept_btn_clone = accept_btn.clone();
        template_entry.connect_activate(move |_| {
            if accept_btn_clone.is_visible() && accept_btn_clone.is_sensitive() {
                accept_btn_clone.emit_clicked();
            }
        });

        // Esc == Okay. Installed on the revealer (an ancestor of every
        // focusable widget inside the drawer) at Capture phase so it
        // fires *before* the msgbar's own Escape controller — we want
        // a global "close the drawer" semantic, not the msgbar's
        // "clear input text" default. We only fire when the Okay
        // button is visible; if a pending proposal (accept/reject,
        // approve-file, …) is on screen, Esc falls through so the
        // user has to explicitly decide rather than silently skip.
        let ok_btn_for_esc = ok_btn.clone();
        let esc_controller = gtk::EventControllerKey::new();
        esc_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        esc_controller.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape && ok_btn_for_esc.is_visible() {
                ok_btn_for_esc.emit_clicked();
                return gtk::glib::Propagation::Stop;
            }
            gtk::glib::Propagation::Proceed
        });
        revealer.add_controller(esc_controller);

        let s = Self {
            revealer,
            indicator_slot,
            character_selector_box,
            single_scroll,
            history_scroll,
            diagnosis_viewer,
            command_view,
            template_entry,
            msg_bar,
            accept_btn,
            reject_btn,
            ok_btn,
            approve_file_btn,
            inspect_btn,
            command_frame,
            template_box,
            file_action_box,
            action_box,
            current_proposal,
            current_mode,
            is_thinking,
            active_agent,
            history_enabled,
            history_sticky,
            is_auto_scrolling,
            selected_character: pending_character,
            history_list,
            history_store,
            host,
        };
        s
    }

    /// Returns the per-pane history store for the overlay. The pane wires
    /// `ClawEngineEvent` messages into this store (in parallel with the
    /// sidebar store) when `maintain_overlay_history` is on.
    pub fn history_store(&self) -> gio::ListStore {
        self.history_store.clone()
    }

    /// Toggle between the single-message view (latest diagnosis only) and the
    /// full scrollable history.
    pub fn set_history_mode(&self, enabled: bool) {
        self.history_enabled.set(enabled);
        self.single_scroll.set_visible(!enabled);
        self.history_scroll.set_visible(enabled);
    }

    pub fn history_mode(&self) -> bool {
        self.history_enabled.get()
    }

    /// Called whenever the parent pane is resized. Caps the scroll window height
    /// so the popover never overflows the visible terminal area minus the gap.
    pub fn update_pane_height(&self, pane_height: i32) {
        // 80px bottom gap + ~12px top padding.
        const V_PAD: i32 = 92;
        let effective = (pane_height - V_PAD).max(100);
        self.single_scroll.set_max_content_height(effective);
        self.history_scroll.set_max_content_height(effective);
    }

    pub fn widget(&self) -> &gtk::Revealer {
        &self.revealer
    }

    pub fn set_active_agent(&self, agent_name: &str) {
        *self.active_agent.borrow_mut() = agent_name.to_string();
        self.sync_action_state();
    }

    pub fn set_thinking(&self, thinking: bool) {
        self.is_thinking.set(thinking);
        self.sync_action_state();
    }

    pub fn refresh_character_selector(&self, current_agent: &str) {
        if !current_agent.is_empty() {
            *self.active_agent.borrow_mut() = current_agent.to_string();
        }

        self.sync_action_state();

        // If the picker isn't visible, don't waste time rebuilding the buttons.
        if !self.character_selector_box.is_visible() {
            return;
        }

        while let Some(child) = self.character_selector_box.first_child() {
            self.character_selector_box.remove(&child);
        }

        let claims = boxxy_claw_protocol::characters::CLAIMS_CACHE.load();
        let host_id = self.host.host_id();

        // Returns true if the character with the given UUID is Active in another holder.
        let is_taken = |id: &str| -> bool {
            claims
                .iter()
                .any(|claim| claim.character_id == id && claim.holder_id != host_id)
        };

        // Auto-default (or auto-correct): ensure `pending` always points to a
        // character UUID that isn't claimed by another holder.
        let registry = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
        {
            let mut pending = self.selected_character.borrow_mut();
            if pending.is_empty() || is_taken(&pending) {
                // Pick the first registry entry that isn't taken elsewhere.
                let first_free = registry.iter().find(|info| !is_taken(&info.config.id));
                if let Some(info) = first_free {
                    *pending = info.config.id.clone();
                } else if pending.is_empty() {
                    // All characters are in use — fall back to first so the
                    // picker is never completely blank.
                    if let Some(first) = registry.first() {
                        *pending = first.config.id.clone();
                    }
                }
                // If every character is taken and pending already has a value,
                // leave it as-is (the button will be insensitive anyway).
            }
        }
        let pending = self.selected_character.borrow().clone();

        for info in registry.iter() {
            let btn = gtk::Button::new();
            let inner_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            inner_box.set_margin_start(4);
            inner_box.set_margin_end(4);
            inner_box.set_margin_top(1);
            inner_box.set_margin_bottom(1);

            let img = gtk::Image::new();
            if info.has_avatar {
                if let Ok(dir) = boxxy_claw_protocol::character_loader::get_characters_dir() {
                    let avatar_path = dir.join(&info.config.name).join("AVATAR.png");
                    if let Ok(texture) = gtk::gdk::Texture::from_filename(&avatar_path) {
                        img.set_paintable(Some(&texture));
                        img.set_pixel_size(20);
                        img.add_css_class("avatar-icon");
                    }
                }
            }
            if img.paintable().is_none() {
                img.set_icon_name(Some("boxxy-boxxyclaw-symbolic"));
                img.set_pixel_size(16);
            }
            inner_box.append(&img);

            let label = gtk::Label::new(Some(&info.config.display_name.to_uppercase()));
            inner_box.append(&label);

            // In the pre-selection phase the local `pending` is the sole
            // source of truth for which character is highlighted. Registry
            // Active status only tells us whether a character is in use in
            // *another* pane (and should therefore be dimmed).
            let is_current = info.config.id == pending;
            let is_in_use = is_taken(&info.config.id);

            if is_current {
                let check_icon = gtk::Image::from_icon_name("boxxy-object-select-2-symbolic");
                check_icon.set_pixel_size(16);
                inner_box.append(&check_icon);
            }

            btn.set_child(Some(&inner_box));
            btn.add_css_class("character-btn");

            if is_in_use {
                btn.set_sensitive(false);
                btn.add_css_class("in-use");
            }
            if is_current {
                btn.add_css_class("selected-character");
            }

            let class_name = format!("char-btn-{}", info.config.name);
            btn.add_css_class(&class_name);

            let css = format!(
                ".{} {{ background-color: {}; color: white; }}\n\
                 .{} *:not(image) {{ color: white; }}\n\
                 .{}:hover {{ filter: brightness(1.1); transform: scale(1.02); }}\n",
                class_name, info.config.color, class_name, class_name
            );
            let provider = gtk::CssProvider::new();
            #[allow(deprecated)]
            provider.load_from_string(&css);
            #[allow(deprecated)]
            btn.style_context()
                .add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

            // Clicking only updates the local pre-selection so the checkmark
            // moves immediately. The actual session is created lazily when the
            // user sends their first message.
            let selected_rc = self.selected_character.clone();
            let overlay_clone = self.clone();
            let char_id = info.config.id.clone();
            btn.connect_clicked(move |_| {
                *selected_rc.borrow_mut() = char_id.clone();
                overlay_clone.refresh_character_selector("");
            });

            self.character_selector_box.append(&btn);
        }
    }

    pub fn show(
        &self,
        mode: OverlayMode,
        title: &str,
        _action: Option<&str>,
        diagnosis: &str,
        proposal: Proposal,
    ) {
        self.refresh_character_selector(title);

        self.diagnosis_viewer.set_content(diagnosis);
        self.msg_bar.entry.set_text("");
        self.template_entry.set_text("");
        *self.current_proposal.borrow_mut() = proposal.clone();
        *self.current_mode.borrow_mut() = mode;
        self.is_thinking.set(false);

        self.sync_action_state();

        match proposal {
            Proposal::Command(cmd) => {
                self.command_view.buffer().set_text(&cmd);
                self.command_view.set_editable(mode == OverlayMode::Claw);
            }
            Proposal::Bookmark {
                script,
                placeholders,
                ..
            } => {
                let mut display_cmd = script.lines().take(15).collect::<Vec<_>>().join("\n");
                if script.lines().count() > 15 {
                    display_cmd.push_str("\n\n... (truncated for preview)");
                }
                self.command_view.buffer().set_text(&display_cmd);
                self.command_view.set_editable(false);
                self.template_entry
                    .set_placeholder_text(Some(&placeholders.join(", ")));
            }
            _ => {}
        }

        self.revealer.set_reveal_child(true);
        self.scroll_to_latest();

        let ok_btn = self.ok_btn.clone();
        let accept_btn = self.accept_btn.clone();
        let approve_file_btn = self.approve_file_btn.clone();
        let template_box = self.template_box.clone();
        let template_entry = self.template_entry.clone();
        let msg_bar = self.msg_bar.clone();

        // The OK button is only a dismiss affordance — it doesn't need
        // default focus, because Esc already closes the drawer and
        // the user's mental model is "keep typing". Focus goes to the
        // input in Claw mode, or to the action the user must decide
        // on (Accept / Approve-file / template variables).
        let _ = ok_btn; // retained for clarity
        gtk4::glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            if accept_btn.is_visible() {
                accept_btn.grab_focus();
            } else if approve_file_btn.is_visible() {
                approve_file_btn.grab_focus();
            } else if template_box.is_visible() {
                template_entry.grab_focus();
            } else if mode == OverlayMode::Claw {
                msg_bar.entry.grab_focus();
            }
            gtk4::glib::ControlFlow::Break
        });
    }

    pub fn show_chat_only(&self, agent_name: &str) {
        self.refresh_character_selector(agent_name);

        self.diagnosis_viewer.set_content("");
        self.msg_bar.entry.set_text("");
        self.template_entry.set_text("");
        *self.current_proposal.borrow_mut() = Proposal::None;
        *self.current_mode.borrow_mut() = OverlayMode::Claw;
        self.is_thinking.set(false);

        self.sync_action_state();

        self.revealer.set_reveal_child(true);
    }

    pub fn show_input_only(&self, agent_name: &str) {
        if !self.revealer.reveals_child() {
            // Drawer is closed → present the "chat only" shell so the user
            // gets a clean prompt aimed at this pane's agent.
            self.show_chat_only(agent_name);
        }

        // Ensure action buttons are synced (shows "Okay" if no proposal, etc.)
        self.set_thinking(false);

        // Auto-scroll to the newest row so reopening lands on the latest
        // message (history mode) or the bottom of the scroll (single mode).
        self.scroll_to_latest();

        // Defer focus grab a tick so GTK has realized the revealed widgets.
        let msg_bar = self.msg_bar.clone();
        gtk4::glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            msg_bar.entry.grab_focus();
            gtk4::glib::ControlFlow::Break
        });
    }

    /// Force the visible scroll (history or single) to its bottom edge.
    /// Used on reveal so the user always lands on the latest content.
    fn scroll_to_latest(&self) {
        if self.history_enabled.get() {
            let n = self.history_store.n_items();
            if n == 0 {
                return;
            }
            // Arm the same 1200ms sticky window used on items_changed.
            let was_idle = !self.history_sticky.get();
            self.history_sticky.set(true);
            if was_idle {
                let sticky_timer = self.history_sticky.clone();
                gtk::glib::timeout_add_local(std::time::Duration::from_millis(1200), move || {
                    sticky_timer.set(false);
                    gtk::glib::ControlFlow::Break
                });
            }

            let adj = self.history_scroll.vadjustment();
            let list = self.history_list.clone();
            let is_auto = self.is_auto_scrolling.clone();
            gtk::glib::idle_add_local_once(move || {
                is_auto.set(true);
                list.scroll_to(n - 1, gtk::ListScrollFlags::FOCUS, None);
                // Also snap the adjustment directly; for realized rows this
                // is redundant but for unrealized rows it helps the initial jump.
                adj.set_value(adj.upper() - adj.page_size());
                gtk::glib::idle_add_local_once(move || {
                    is_auto.set(false);
                });
            });
        } else {
            let adj = self.single_scroll.vadjustment();
            gtk::glib::idle_add_local_once(move || {
                adj.set_value(adj.upper() - adj.page_size());
            });
        }
    }

    pub fn hide(&self) {
        self.revealer.set_reveal_child(false);
        // Robust hiding: clear the proposal state and sync visibility
        // so that stale buttons aren't briefly visible during fade-out
        // or the next time the drawer opens.
        *self.current_proposal.borrow_mut() = Proposal::None;
        self.is_thinking.set(false);
        self.sync_action_state();
    }

    pub fn grab_input_focus(&self) {
        if *self.current_mode.borrow() == OverlayMode::Claw {
            self.msg_bar.entry.grab_focus();
        } else if self.template_box.is_visible() {
            self.template_entry.grab_focus();
        }
    }

    pub fn set_indicator_slot_visible(&self, visible: bool) {
        self.indicator_slot.set_visible(visible);
    }

    pub fn is_visible(&self) -> bool {
        self.revealer.reveals_child()
    }

    pub fn sync_action_state(&self) {
        let mode = *self.current_mode.borrow();
        let proposal = self.current_proposal.borrow().clone();
        let is_thinking = self.is_thinking.get();
        let has_active_agent = !self.active_agent.borrow().is_empty();

        // 1. Hide everything by default to prevent stale buttons
        self.command_frame.set_visible(false);
        self.action_box.set_visible(false);
        self.accept_btn.set_visible(false);
        self.reject_btn.set_visible(false);
        self.ok_btn.set_visible(false);
        self.file_action_box.set_visible(false);
        self.template_box.set_visible(false);

        // 2. Base components based on mode
        self.msg_bar.widget.set_visible(mode == OverlayMode::Claw);
        self.inspect_btn.set_visible(mode == OverlayMode::Claw);

        // 3. Character Selector Box logic
        // Only visible if in Claw mode, NOT thinking, and no active agent is set yet.
        let show_picker = mode == OverlayMode::Claw && !is_thinking && !has_active_agent;
        self.character_selector_box.set_visible(show_picker);

        // If the picker shouldn't be shown, we don't need to poll the registry.
        if !show_picker {}

        // 4. If the agent is actively thinking, we show no actions.
        if is_thinking {
            return;
        }

        // 5. Show actions based strictly on the current proposal state
        match proposal {
            Proposal::Command(_) => {
                self.command_frame.set_visible(true);
                self.action_box.set_visible(true);
                self.accept_btn.set_visible(true);
                self.reject_btn.set_visible(true);
            }
            Proposal::Bookmark { .. } => {
                self.command_frame.set_visible(true);
                self.action_box.set_visible(true);
                self.accept_btn.set_visible(true);
                self.reject_btn.set_visible(true);
                self.template_box.set_visible(true);
            }
            Proposal::FileWrite { .. }
            | Proposal::FileDelete { .. }
            | Proposal::KillProcess { .. }
            | Proposal::GetClipboard
            | Proposal::SetClipboard(_) => {
                self.file_action_box.set_visible(true);
            }
            Proposal::None => {
                // Idle state: just Okay and Inspect (handled above)
                self.action_box.set_visible(true);
                self.ok_btn.set_visible(true);
            }
        }
    }

    pub fn current_proposal(&self) -> Proposal {
        self.current_proposal.borrow().clone()
    }
}
