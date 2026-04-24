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
    single_scroll: gtk::ScrolledWindow,
    history_scroll: gtk::ScrolledWindow,
    history_list: gtk::ListView,
    history_store: gio::ListStore,
    title_label: gtk::Label,
    action_label: gtk::Label,
    title_container: gtk::Box,
    diagnosis_viewer: StructuredViewer,
    command_view: gtk::TextView,
    template_entry: gtk::Entry,
    /// The merged input widget. Replaces the former `reply_entry`,
    /// `reply_btn`, and standalone `attachment_mgr`. Carries attachments,
    /// autocomplete, history navigation, and the 4 status toggles.
    msg_bar: Rc<MsgBarComponent>,
    accept_btn: gtk::Button,
    reject_btn: gtk::Button,
    ok_btn: gtk::Button,
    icon: gtk::Image,

    // File Write specific widgets
    #[allow(dead_code)]
    reject_file_btn: gtk::Button,
    #[allow(dead_code)]
    approve_file_btn: gtk::Button,
    inspect_btn: gtk::Button,
    command_frame: gtk::Frame,
    template_box: gtk::Box,
    file_action_box: gtk::Box,
    action_box: gtk::Box,
    current_proposal: Rc<RefCell<Proposal>>,
    current_mode: Rc<RefCell<OverlayMode>>,
    history_enabled: Rc<Cell<bool>>,
    /// Countdown that keeps pinning the history scroll to its bottom
    /// across multiple layout ticks — handles async row realization /
    /// multi-pass markdown measuring where `upper` grows over several
    /// frames after a new row is appended.
    history_sticky: Rc<Cell<u32>>,
}

impl TerminalOverlay {
    pub fn new(
        indicator_widget: &gtk::Widget,
        msg_bar: Rc<MsgBarComponent>,
        host: Rc<dyn ClawHost>,
    ) -> Self {
        let builder = gtk::Builder::from_resource("/dev/boxxy/BoxxyTerminal/ui/claw_overlay.ui");

        let revealer: gtk::Revealer = builder.object("root_revealer").unwrap();
        let single_scroll: gtk::ScrolledWindow = builder.object("single_scroll").unwrap();
        let history_scroll: gtk::ScrolledWindow = builder.object("history_scroll").unwrap();
        let history_container: gtk::Box = builder.object("history_container").unwrap();
        let title_label: gtk::Label = builder.object("title_label").unwrap();
        let action_label: gtk::Label = builder.object("action_label").unwrap();
        let title_container: gtk::Box = builder.object("title_container").unwrap();
        let header_box: gtk::Box = builder.object("header_box").unwrap();

        header_box.append(indicator_widget);

        let command_view: gtk::TextView = builder.object("command_view").unwrap();
        let template_entry: gtk::Entry = builder.object("template_entry").unwrap();

        let accept_btn: gtk::Button = builder.object("accept_btn").unwrap();
        let reject_btn: gtk::Button = builder.object("reject_btn").unwrap();
        let ok_btn: gtk::Button = builder.object("ok_btn").unwrap();
        let icon: gtk::Image = builder.object("icon").unwrap();

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
        let (history_list, history_store) = boxxy_claw_ui::create_claw_message_list();
        history_container.append(&history_list);

        // Auto-scroll to the newest row on every change. Chat-style UX
        // (Claude Code, Slack, Discord) always lands on the latest message
        // rather than preserving the scroll — so we drop the "only if the
        // user is at the bottom" gate the sidebar uses.
        //
        // Layout timing is the tricky part: when `items_changed` fires,
        // the new ClawRow has been appended to the store but the ListView
        // hasn't measured it yet, so `adj.upper()` is stale. Markdown
        // rendering inside rows also re-measures across several frames.
        // We handle this by arming a "sticky to bottom" counter on
        // items_changed and re-snapping the adjustment each time the
        // upper bound changes, for up to N follow-up frames.
        // GTK4 ListView has a known virtualization bug (GNOME/gtk#2971)
        // where scrolling to a newly-appended row lands on the bottom of
        // the *realized* portion, not the true bottom — rows below the
        // current viewport haven't been measured yet, so `adj.upper()` is
        // underestimated and `scroll_to(FOCUS)` returns before the layout
        // has settled. The GNOME Discourse thread on this recommends
        // switching to ColumnView entirely; the pragmatic workaround in
        // chat apps is to repeatedly re-pin the bottom for ~10 frames
        // while the ListView realizes rows + markdown re-measures its
        // content. We drive that retry loop via a `Cell<u32>` counter.
        let history_sticky = Rc::new(Cell::new(0u32));

        let sticky_items = history_sticky.clone();
        let adj_sched = history_scroll.vadjustment();
        let list_sched = history_list.clone();
        history_store.connect_items_changed(move |s, _, _, _| {
            let n = s.n_items();
            if n == 0 {
                return;
            }
            // Arm the retry loop — 10 frames @ ~16ms each covers the
            // worst case (tall markdown row, remote image fetches, font
            // fallback, etc.).
            let first_arm = sticky_items.get() == 0;
            sticky_items.set(10);
            if !first_arm {
                // A retry loop is already running; let it keep pinning.
                return;
            }
            let adj = adj_sched.clone();
            let list = list_sched.clone();
            let sticky = sticky_items.clone();
            gtk::glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
                let n_now = list.model().map(|m| m.n_items()).unwrap_or(0);
                if n_now > 0 {
                    // Ask the ListView to bring the last row into view —
                    // this forces row realization which the adjustment
                    // snap alone cannot trigger.
                    list.scroll_to(n_now - 1, gtk::ListScrollFlags::FOCUS, None);
                    // Then pin the adjustment; on frames where scroll_to
                    // already settled, this is a no-op.
                    adj.set_value(adj.upper() - adj.page_size());
                }
                let remaining = sticky.get();
                if remaining <= 1 {
                    sticky.set(0);
                    gtk::glib::ControlFlow::Break
                } else {
                    sticky.set(remaining - 1);
                    gtk::glib::ControlFlow::Continue
                }
            });
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

        TerminalOverlay {
            revealer,
            single_scroll,
            history_scroll,
            history_list,
            history_store,
            title_label,
            action_label,
            title_container,
            diagnosis_viewer,
            command_view,
            template_entry,
            msg_bar,
            accept_btn,
            reject_btn,
            ok_btn,
            icon,
            reject_file_btn,
            approve_file_btn,
            inspect_btn,
            command_frame,
            template_box,
            file_action_box,
            action_box,
            current_proposal,
            current_mode,
            history_enabled,
            history_sticky,
        }
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

    /// Hide the proposal/action widgets so the drawer presents only a
    /// chat-style input while the agent is thinking. Called by the pane
    /// right after the user submits a reply through the embedded msgbar.
    pub fn enter_waiting_state(&self) {
        self.command_frame.set_visible(false);
        self.action_box.set_visible(false);
        self.accept_btn.set_visible(false);
        self.reject_btn.set_visible(false);
        self.ok_btn.set_visible(false);
        self.file_action_box.set_visible(false);
        self.template_box.set_visible(false);
        // The msgbar stays visible by design — it's the input.
    }

    pub fn show(
        &self,
        mode: OverlayMode,
        title: &str,
        action: Option<&str>,
        diagnosis: &str,
        proposal: Proposal,
    ) {
        self.title_label.set_label(title);
        self.diagnosis_viewer.set_content(diagnosis);
        self.msg_bar.entry.set_text("");
        self.template_entry.set_text("");
        *self.current_proposal.borrow_mut() = proposal.clone();
        *self.current_mode.borrow_mut() = mode;

        // Reset visibility
        self.command_frame.set_visible(false);
        self.action_box.set_visible(false);
        self.accept_btn.set_visible(false);
        self.reject_btn.set_visible(false);
        self.ok_btn.set_visible(false);
        self.file_action_box.set_visible(false);
        // Msgbar is visible in Claw mode, hidden in Bookmark mode (no agent input needed).
        self.msg_bar.widget.set_visible(mode == OverlayMode::Claw);
        self.template_box.set_visible(false);
        self.inspect_btn.set_visible(mode == OverlayMode::Claw);

        match mode {
            OverlayMode::Claw => {
                self.icon.set_visible(true);
                self.icon.set_icon_name(Some("boxxy-boxxyclaw-symbolic"));
                self.title_label.add_css_class("heading");

                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                title.hash(&mut hasher);
                let hash = hasher.finish();

                let r = (hash & 0xFF) as u8 % 150 + 50;
                let g = ((hash >> 8) & 0xFF) as u8 % 150 + 50;
                let b = ((hash >> 16) & 0xFF) as u8 % 150 + 50;
                let color = format!("rgb({}, {}, {})", r, g, b);

                let css = format!(
                    ".overlay-badge {{ background-color: {}; color: white; border-radius: 12px; padding: 4px 10px; font-weight: bold; font-size: 0.8rem; box-shadow: 0 2px 4px rgba(0,0,0,0.2); }}",
                    color
                );
                let provider = gtk::CssProvider::new();
                #[allow(deprecated)]
                provider.load_from_string(&css);
                #[allow(deprecated)]
                self.title_label
                    .style_context()
                    .add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
                self.title_label.add_css_class("overlay-badge");

                if let Some(act) = action {
                    self.action_label.set_label(act);
                    self.action_label.set_visible(true);

                    let action_css = ".action-badge { background-color: rgba(255, 255, 255, 0.1); color: @window_fg_color; border: 1px solid rgba(255,255,255,0.1); border-radius: 12px; padding: 4px 10px; font-weight: bold; font-size: 0.8rem; margin-left: 6px; }";
                    let action_provider = gtk::CssProvider::new();
                    #[allow(deprecated)]
                    action_provider.load_from_string(&action_css);
                    #[allow(deprecated)]
                    self.action_label
                        .style_context()
                        .add_provider(&action_provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

                    self.action_label.add_css_class("action-badge");
                } else {
                    self.action_label.set_visible(false);
                }
            }
            OverlayMode::Bookmark => {
                self.icon.set_visible(true);
                self.icon
                    .set_icon_name(Some("boxxy-user-bookmarks-symbolic"));
                self.title_label.add_css_class("heading");
                self.action_label.set_visible(false);
            }
        }

        match proposal {
            Proposal::Command(cmd) => {
                self.command_view.buffer().set_text(&cmd);
                self.command_view.set_editable(mode == OverlayMode::Claw);
                self.command_frame.set_visible(true);
                self.action_box.set_visible(true);
                self.accept_btn.set_visible(true);
                self.reject_btn.set_visible(true);
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
                self.command_frame.set_visible(true);
                self.action_box.set_visible(true);
                self.accept_btn.set_visible(true);
                self.reject_btn.set_visible(true);
                self.template_box.set_visible(true);
                self.template_entry
                    .set_placeholder_text(Some(&placeholders.join(", ")));
            }
            Proposal::FileWrite { .. }
            | Proposal::FileDelete { .. }
            | Proposal::KillProcess { .. }
            | Proposal::GetClipboard
            | Proposal::SetClipboard(_) => {
                self.file_action_box.set_visible(true);
            }
            Proposal::None => {
                self.action_box.set_visible(true);
                self.ok_btn.set_visible(true);
            }
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
        self.title_label.set_label(agent_name);
        self.diagnosis_viewer.set_content("");
        self.msg_bar.entry.set_text("");
        self.template_entry.set_text("");
        *self.current_proposal.borrow_mut() = Proposal::None;
        *self.current_mode.borrow_mut() = OverlayMode::Claw;

        self.command_frame.set_visible(false);
        self.action_box.set_visible(false);
        self.accept_btn.set_visible(false);
        self.reject_btn.set_visible(false);
        self.ok_btn.set_visible(false);
        self.file_action_box.set_visible(false);
        self.template_box.set_visible(false);

        self.msg_bar.widget.set_visible(true);
        self.inspect_btn.set_visible(true);

        self.icon.set_visible(true);
        self.icon.set_icon_name(Some("boxxy-boxxyclaw-symbolic"));
        self.title_label.add_css_class("heading");

        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        agent_name.hash(&mut hasher);
        let hash = hasher.finish();

        let r = (hash & 0xFF) as u8 % 150 + 50;
        let g = ((hash >> 8) & 0xFF) as u8 % 150 + 50;
        let b = ((hash >> 16) & 0xFF) as u8 % 150 + 50;
        let color = format!("rgb({}, {}, {})", r, g, b);

        let css = format!(
            ".overlay-badge {{ background-color: {}; color: white; border-radius: 12px; padding: 4px 10px; font-weight: bold; font-size: 0.8rem; box-shadow: 0 2px 4px rgba(0,0,0,0.2); }}",
            color
        );
        let provider = gtk::CssProvider::new();
        #[allow(deprecated)]
        provider.load_from_string(&css);
        #[allow(deprecated)]
        self.title_label
            .style_context()
            .add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        self.title_label.add_css_class("overlay-badge");

        self.action_label.set_visible(false);
        self.revealer.set_reveal_child(true);
    }

    /// Entry point for the Ctrl+/ shortcut. Opens the drawer if closed,
    /// and always focuses the merged msgbar entry. Never hides — the agent
    /// may be waiting for a message and a silent dismiss would strand the
    /// user without an input surface.
    pub fn show_input_only(&self, agent_name: &str) {
        if !self.revealer.reveals_child() {
            // Drawer is closed → present the "chat only" shell so the user
            // gets a clean prompt aimed at this pane's agent.
            self.show_chat_only(agent_name);
        }
        // Surface the Okay button (and the inspect/bug icon) so the user
        // has an explicit, smooth way to dismiss the drawer — otherwise
        // the only escape is the ESC key, which doesn't close in embedded
        // mode. Only when there's no pending proposal, so we don't clobber
        // an Accept/Reject choice the user must still make.
        if matches!(*self.current_proposal.borrow(), Proposal::None) {
            self.action_box.set_visible(true);
            self.ok_btn.set_visible(true);
            self.inspect_btn.set_visible(true);
        }

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
            // Arm the same retry loop used on items_changed — opening an
            // already-populated drawer exercises the same virtualization
            // bug (rows aren't realized until they enter the viewport).
            let first_arm = self.history_sticky.get() == 0;
            self.history_sticky.set(10);
            if first_arm {
                let adj = self.history_scroll.vadjustment();
                let list = self.history_list.clone();
                let sticky = self.history_sticky.clone();
                gtk::glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
                    let n_now = list.model().map(|m| m.n_items()).unwrap_or(0);
                    if n_now > 0 {
                        list.scroll_to(n_now - 1, gtk::ListScrollFlags::FOCUS, None);
                        adj.set_value(adj.upper() - adj.page_size());
                    }
                    let remaining = sticky.get();
                    if remaining <= 1 {
                        sticky.set(0);
                        gtk::glib::ControlFlow::Break
                    } else {
                        sticky.set(remaining - 1);
                        gtk::glib::ControlFlow::Continue
                    }
                });
            }
        } else {
            let adj = self.single_scroll.vadjustment();
            gtk::glib::idle_add_local_once(move || {
                adj.set_value(adj.upper() - adj.page_size());
            });
        }
    }

    pub fn hide(&self) {
        self.revealer.set_reveal_child(false);
        // Robust hiding: explicitly hide all proposal-specific action containers
        // to ensure we don't accidentally leave stale buttons visible if the drawer
        // re-opens later.
        self.command_frame.set_visible(false);
        self.template_box.set_visible(false);
        self.file_action_box.set_visible(false);
        self.action_box.set_visible(false);
    }

    pub fn grab_input_focus(&self) {
        if *self.current_mode.borrow() == OverlayMode::Claw {
            self.msg_bar.entry.grab_focus();
        } else if self.template_box.is_visible() {
            self.template_entry.grab_focus();
        }
    }

    pub fn is_visible(&self) -> bool {
        self.revealer.reveals_child()
    }

    pub fn current_proposal(&self) -> Proposal {
        self.current_proposal.borrow().clone()
    }
}
