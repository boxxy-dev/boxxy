use boxxy_preferences::Settings;
use boxxy_themes::Palette;
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gdk, gio, glib, pango};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::search_bar::SearchBarComponent;
use boxxy_claw_widget::MsgBarComponent;
use boxxy_vte::terminal::TerminalWidget;

use crate::is_flatpak;
use crate::{PaneInit, PaneOutput};

use boxxy_claw_protocol::*;
use boxxy_claw_widget::ClawIndicator;
use boxxy_claw_widget::{OverlayMode, TerminalOverlay};

mod app_menu;
mod claw;
mod claw_host;
mod events;
pub mod gestures;
pub(crate) mod preview;
mod session;
mod spawn;
mod ui;

pub(crate) use claw_host::PaneClawHost;

pub type PendingDiagnosis = Rc<RefCell<Option<(String, crate::TerminalProposal)>>>;
type PaneCallback = std::sync::Arc<dyn Fn(PaneOutput) + Send + Sync + 'static>;

/// GTK key names for characters that users commonly type literally in accelerator
/// strings (e.g. "/" instead of "slash"). `gtk_accelerator_parse` only accepts
/// symbolic names, so bare characters are normalized before parsing.
fn normalize_accel(accel: &str) -> String {
    let key_start = accel.rfind('>').map_or(0, |i| i + 1);
    let key = match &accel[key_start..] {
        "/" => "slash",
        "?" => "question",
        "=" => "equal",
        "+" => "plus",
        "-" => "minus",
        "_" => "underscore",
        "." => "period",
        "," => "comma",
        ";" => "semicolon",
        ":" => "colon",
        "'" => "apostrophe",
        "\"" => "quotedbl",
        "!" => "exclam",
        "@" => "at",
        "#" => "numbersign",
        "$" => "dollar",
        "%" => "percent",
        "^" => "asciicircum",
        "&" => "ampersand",
        "*" => "asterisk",
        "(" => "parenleft",
        ")" => "parenright",
        "[" => "bracketleft",
        "]" => "bracketright",
        "{" => "braceleft",
        "}" => "braceright",
        "|" => "bar",
        "\\" => "backslash",
        "`" => "grave",
        "~" => "asciitilde",
        other => other,
    };
    format!("{}{}", &accel[..key_start], key)
}

fn parse_accel_trigger(accel: &str) -> gtk::ShortcutTrigger {
    let normalised = normalize_accel(accel);
    gtk::ShortcutTrigger::parse_string(&normalised)
        .unwrap_or_else(|| gtk::ShortcutTrigger::parse_string("<Ctrl>slash").unwrap())
}

const fn is_actionable_claw_msg(msg: &ClawMessage) -> bool {
    matches!(
        msg,
        ClawMessage::ClawQuery { .. }
            | ClawMessage::UserMessage { .. }
            | ClawMessage::ResumeSession { .. }
            | ClawMessage::Initialize
            | ClawMessage::RequestLazyDiagnosis
    )
}

#[derive(Clone)]
pub struct TerminalPaneComponent {
    widget: gtk::Overlay,
    pub(super) inner: Rc<RefCell<PaneInner>>,
    _search_bar: Rc<SearchBarComponent>,
    pub(super) claw_popover: TerminalOverlay,
    pub(super) claw_indicator: ClawIndicator,

    pending_sleep_diagnosis: PendingDiagnosis,
    pub(super) claw_sender: async_channel::Sender<ClawMessage>,

    pub is_claw_active: Rc<Cell<bool>>,
    pub session_status: Rc<RefCell<AgentStatus>>,
    pub is_pinned: Rc<Cell<bool>>,
    pub(super) is_web_search: Rc<Cell<bool>>,
    pub(super) is_vibe: Rc<Cell<bool>>,
    pub(super) agent_name: Rc<RefCell<String>>,
    pub(super) msg_bar: Rc<MsgBarComponent>,
    pub session_usage: Rc<Cell<SessionUsage>>,
    pending_tasks: Rc<RefCell<Vec<ScheduledTask>>>,
    msgbar_shortcut: gtk::Shortcut,
}

impl Drop for PaneInner {
    fn drop(&mut self) {
        crate::unregister_live_pane(&self.id);
        // Close per-pane channels immediately so `spawn_dispatch` and the
        // UI-to-agent forwarder task exit right away.
        self.claw_event_tx.close();
        self.claw_msg_tx.close();
        self.process_subscriptions.clear();
        *self.claw_subscription.lock() = None;

        if self.already_handled_close {
            self.terminal.detach_pty();
            return;
        }

        // Read the live setting here rather than snapshotting at spawn time so
        // toggling persistence in Preferences takes effect on the very next close.
        let persist = Settings::load().pty_persistence && !self.force_terminate_on_close;

        let session_arc = self.session_id.clone();
        let pane_id = self.id.clone();
        glib::spawn_future_local(async move {
            let sid = session_arc.lock().await.take();
            let agent = crate::get_agent().await;
            if sid.is_some() {
                if persist {
                    let _ = agent.detach_holder(pane_id).await;
                } else {
                    let _ = agent.release_holder(pane_id).await;
                }
            } else if !persist {
                let _ = agent.unregister_workspace_pane(pane_id).await;
            }
        });

        let Some(pid) = self.pid else {
            self.terminal.detach_pty();
            return;
        };

        if persist {
            log::info!("PaneInner dropping (pid={pid}): pty_persistence on → detaching into daemon");
            let snapshot = self.export_snapshot_with_title();
            self.terminal.detach_pty();
            glib::spawn_future_local(async move {
                let agent = crate::get_agent().await;
                let _ = agent.set_persistence(pid, true).await;
                match agent.detach_with_snapshot(pid, snapshot).await {
                    Ok(1) => log::info!("pid={pid} detached into background"),
                    Ok(0) => log::info!(
                        "pid={pid} terminated (unexpectedly — persistence may be off on daemon)"
                    ),
                    Ok(3) => log::warn!(
                        "pid={pid} detached without buffer (daemon has no stored FD)"
                    ),
                    Ok(code) => log::debug!("pid={pid} detach returned code {code}"),
                    Err(e) => log::warn!("pid={pid} detach failed: {e}"),
                }
            });
        } else {
            log::info!("PaneInner dropping (pid={pid}): killing process group");
            self.terminal.detach_pty();
            glib::spawn_future_local(async move {
                let agent = crate::get_agent().await;
                let _ = agent.set_persistence(pid, false).await;
                let _ = agent.signal_process_group(pid, 15).await;
            });
        }
    }
}

pub(crate) struct PaneInner {
    pub terminal: TerminalWidget,
    pub(super) scrolled_window: gtk::ScrolledWindow,
    pub(super) working_dir: Option<String>,
    pub(super) last_title: Option<String>,
    pub(super) foreground_process: Option<String>,
    pub id: String,
    pub(super) layout_tag: String,
    pub(super) current_settings: Option<Settings>,
    pub(super) hide_scrollbars: bool,
    pub(super) is_dimmed: bool,
    pub(super) size_dismiss_source: Option<gtk::glib::SourceId>,
    pub(super) n_columns: i64,
    pub(super) n_rows: i64,
    pub(super) pid: Option<u32>,
    pub(super) reattach_pid: Option<u32>,
    pub(super) force_terminate_on_close: bool,
    pub(super) already_handled_close: bool,
    pub session_id: std::sync::Arc<tokio::sync::Mutex<Option<String>>>,
    pub(super) claw_event_tx: async_channel::Sender<boxxy_claw_protocol::ClawEngineEvent>,
    pub(super) claw_msg_tx: async_channel::Sender<boxxy_claw_protocol::ClawMessage>,
    pub(super) process_subscriptions: Vec<crate::agent_manager::SubscriptionGuard>,
    pub(super) claw_subscription:
        std::sync::Arc<parking_lot::Mutex<Option<crate::agent_manager::SubscriptionGuard>>>,
    pub(super) claw_indicator: Option<ClawIndicator>,
    pub(super) claw_popover: Option<TerminalOverlay>,
    pub(super) msg_bar: Rc<MsgBarComponent>,
    pub(super) callback: PaneCallback,
    pub(super) callback_slot: std::sync::Arc<parking_lot::RwLock<PaneCallback>>,
}

impl PaneInner {
    fn export_snapshot_with_title(&self) -> Vec<u8> {
        let mut snap = Vec::new();
        if let Some(ref title) = self.last_title
            && !title.is_empty()
        {
            snap.extend_from_slice(format!("\x1b]0;{title}\x07").as_bytes());
        }
        snap.extend_from_slice(&self.terminal.export_ansi_snapshot());
        snap
    }

    pub(super) fn mark_pty_exited(
        &mut self,
    ) -> Option<std::sync::Arc<tokio::sync::Mutex<Option<String>>>> {
        if self.already_handled_close {
            return None;
        }
        self.already_handled_close = true;
        self.pid = None;
        self.claw_event_tx.close();
        self.claw_msg_tx.close();
        self.process_subscriptions.clear();
        *self.claw_subscription.lock() = None;
        self.terminal.detach_pty();
        Some(self.session_id.clone())
    }
}

impl std::fmt::Debug for TerminalPaneComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalPaneComponent").finish()
    }
}

impl TerminalPaneComponent {
    pub fn set_force_terminate_on_close(&self, force: bool) {
        self.inner.borrow_mut().force_terminate_on_close = force;
    }

    pub async fn detach_on_close_sync(&self) {
        let (pid, snapshot) = {
            let mut inner = self.inner.borrow_mut();
            inner.already_handled_close = true;
            inner.claw_event_tx.close();
            inner.claw_msg_tx.close();
            inner.process_subscriptions.clear();
            *inner.claw_subscription.lock() = None;
            let snap = inner.export_snapshot_with_title();
            inner.terminal.detach_pty();
            (inner.pid, snap)
        };
        self.detach_session_sync().await;
        if let Some(pid) = pid {
            let agent = crate::get_agent().await;
            let _ = agent.set_persistence(pid, true).await;
            let _ = agent.detach_with_snapshot(pid, snapshot).await;
        }
    }

    pub async fn detach_session_sync(&self) {
        let (session_arc, pane_id) = {
            let inner = self.inner.borrow();
            (inner.session_id.clone(), inner.id.clone())
        };
        let sid = session_arc.lock().await.take();
        if sid.is_some() {
            let agent = crate::get_agent().await;
            let _ = agent.detach_holder(pane_id).await;
        }
    }

    pub async fn terminate_on_close_sync(&self) {
        let pid = {
            let mut inner = self.inner.borrow_mut();
            inner.already_handled_close = true;
            inner.force_terminate_on_close = true;
            inner.claw_event_tx.close();
            inner.claw_msg_tx.close();
            inner.process_subscriptions.clear();
            *inner.claw_subscription.lock() = None;
            inner.terminal.detach_pty();
            inner.pid
        };
        self.end_session_sync().await;
        if let Some(pid) = pid {
            let agent = crate::get_agent().await;
            let _ = agent.set_persistence(pid, false).await;
            let _ = agent.signal_process_group(pid, 15).await;
        }
    }

    pub fn end_session(&self) {
        let (session_arc, pane_id) = {
            let inner = self.inner.borrow();
            (inner.session_id.clone(), inner.id.clone())
        };
        glib::spawn_future_local(async move {
            let sid = session_arc.lock().await.take();
            let agent = crate::get_agent().await;
            if sid.is_some() {
                let _ = agent.release_holder(pane_id).await;
            } else {
                let _ = agent.unregister_workspace_pane(pane_id).await;
            }
        });
    }

    pub async fn end_session_sync(&self) {
        let (session_arc, pane_id) = {
            let inner = self.inner.borrow();
            (inner.session_id.clone(), inner.id.clone())
        };
        let sid = session_arc.lock().await.take();
        let agent = crate::get_agent().await;
        if sid.is_some() {
            let _ = agent.release_holder(pane_id).await;
        } else {
            let _ = agent.unregister_workspace_pane(pane_id).await;
        }
    }

    pub fn new<F: Fn(PaneOutput) + Send + Sync + 'static>(init: PaneInit, callback: F) -> Self {
        let settings = boxxy_preferences::Settings::load();
        let initial_cb: PaneCallback = std::sync::Arc::new(callback);
        let callback_slot = std::sync::Arc::new(parking_lot::RwLock::new(initial_cb));
        let slot_for_proxy = callback_slot.clone();
        let callback: PaneCallback = std::sync::Arc::new(move |output: PaneOutput| {
            let cb = slot_for_proxy.read().clone();
            cb(output);
        });
        let id = init.id;

        let (
            widget,
            terminal,
            scrolled_window,
            size_revealer,
            size_label,
            search_bar_rc,
            progress_bar,
            drop_overlay,
            drag_lane,
            drag_handle,
        ) = ui::build_ui();

        let (claw_event_tx, claw_rx_from_agent) = async_channel::unbounded();
        let (tx_to_agent, rx_from_ui) =
            async_channel::unbounded::<boxxy_claw_protocol::ClawMessage>();
        let session_id_arc: std::sync::Arc<tokio::sync::Mutex<Option<String>>> =
            std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let session_id_for_init = session_id_arc.clone();
        let session_id_for_inner = session_id_arc;

        let claw_sender_clone = claw_event_tx.clone();
        let claw_event_tx_for_inner = claw_event_tx;
        let event_sender_for_first_msg = claw_sender_clone.clone();

        let pending_character: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
        let pending_char_for_msgbar = pending_character.clone();
        let pending_char_for_overlay = pending_character;

        // Forward messages from the UI to the daemon's Claw session. If an
        // actionable message arrives while `create_claw_session_with_sender` is
        // still in flight over D-Bus, wait briefly for `session_id` to populate
        // instead of dropping the user's request.
        gtk::glib::spawn_future_local(async move {
            let agent = crate::get_agent().await;
            let session_id_ref = session_id_for_init;

            tokio::spawn(async move {
                while let Ok(msg) = rx_from_ui.recv().await {
                    let mut sid = session_id_ref.lock().await.clone();
                    if sid.is_none() && is_actionable_claw_msg(&msg) {
                        for _ in 0..40 {
                            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                            sid = session_id_ref.lock().await.clone();
                            if sid.is_some() {
                                break;
                            }
                        }
                    }
                    if let Some(session_id) = sid {
                        let _ = agent.post_claw_message(session_id, msg).await;
                    }
                }
            });
        });

        let claw_rx = claw_rx_from_agent;
        let claw_sender = tx_to_agent;

        let is_claw_active = Rc::new(Cell::new(false));
        let session_status = Rc::new(RefCell::new(AgentStatus::Off));
        let is_pinned = Rc::new(Cell::new(false));
        let is_web_search = Rc::new(Cell::new(settings.web_search_on_by_default));
        let is_vibe = Rc::new(Cell::new(false));
        let agent_name = Rc::new(RefCell::new(String::new()));
        let claw_indicator = ClawIndicator::new();
        let session_usage = Rc::new(Cell::new(SessionUsage::default()));
        let pending_tasks: Rc<RefCell<Vec<ScheduledTask>>> = Rc::new(RefCell::new(Vec::new()));

        let inner_weak_ref: session::DeferredInnerWeak = Rc::new(RefCell::new(None));
        let msg_bar = session::build_msg_bar(
            &id,
            &claw_sender,
            event_sender_for_first_msg,
            &callback,
            &inner_weak_ref,
            pending_char_for_msgbar,
            &is_claw_active,
            &session_status,
            &is_pinned,
            &is_web_search,
            &is_vibe,
            &agent_name,
            &session_usage,
            &pending_tasks,
        );

        msg_bar.set_web_search_visible(settings.enable_web_search);
        msg_bar.set_working_dir(init.working_dir.clone());

        let inner = Rc::new(RefCell::new(PaneInner {
            terminal: terminal.clone(),
            scrolled_window,
            working_dir: init.working_dir,
            last_title: None,
            foreground_process: None,
            id: id.clone(),
            layout_tag: init.layout_tag.unwrap_or_else(|| id.clone()),
            current_settings: None,
            hide_scrollbars: false,
            is_dimmed: false,
            size_dismiss_source: None,
            n_columns: 0,
            n_rows: 0,
            pid: None,
            reattach_pid: init.reattach_pid,
            force_terminate_on_close: false,
            already_handled_close: false,
            session_id: session_id_for_inner,
            claw_event_tx: claw_event_tx_for_inner,
            claw_msg_tx: claw_sender.clone(),
            process_subscriptions: Vec::new(),
            claw_subscription: std::sync::Arc::new(parking_lot::Mutex::new(None)),
            claw_indicator: None,
            claw_popover: None,
            msg_bar: msg_bar.clone(),
            callback: callback.clone(),
            callback_slot,
        }));

        *inner_weak_ref.borrow_mut() = Some(Rc::downgrade(&inner));
        crate::register_live_pane(id.clone(), Rc::downgrade(&inner));

        gestures::setup_gestures(
            &terminal,
            &inner,
            &search_bar_rc,
            callback.clone(),
            id.clone(),
        );
        preview::setup_preview(&terminal, &inner);

        events::wire_terminal_events(
            &terminal,
            &inner,
            &progress_bar,
            &is_claw_active,
            &claw_sender,
            callback.clone(),
            id.clone(),
        );

        let (claw_popover, pending_sleep_diagnosis) = claw::setup_claw(
            &widget,
            &inner,
            id.clone(),
            claw_sender.clone(),
            claw_sender_clone,
            claw_rx,
            msg_bar.clone(),
            callback.clone(),
            init.spawn_intent,
            is_claw_active.clone(),
            session_usage.clone(),
            is_pinned.clone(),
            is_web_search.clone(),
            is_vibe.clone(),
            session_status.clone(),
            agent_name.clone(),
            &claw_indicator,
            pending_char_for_overlay,
            pending_tasks.clone(),
        );

        // Badge must be added AFTER the drawer overlay so it renders on top.
        widget.add_overlay(claw_indicator.badge());

        claw_popover.set_bottom_gap(settings.overlay_bottom_gap);
        claw_indicator.update_settings(settings.hide_character_badge);

        inner.borrow_mut().claw_indicator = Some(claw_indicator.clone());
        inner.borrow_mut().claw_popover = Some(claw_popover.clone());

        gestures::setup_split_drag_and_drop(
            &widget,
            &terminal,
            &drop_overlay,
            &drag_lane,
            &drag_handle,
            &search_bar_rc,
            &claw_popover,
            callback,
            id,
        );

        // Single pass-through resize observer driving both the `cols × rows` OSD
        // and the Claw drawer's live max-height cap.
        let inner_clone_resize = Rc::downgrade(&inner);
        let size_revealer_weak = size_revealer.downgrade();
        let size_label_weak = size_label.downgrade();
        let terminal_weak = terminal.downgrade();
        let resize_detector = gtk::DrawingArea::new();
        resize_detector.set_can_target(false);
        resize_detector.connect_resize(move |_, _w, h| {
            let Some(inner_rc) = inner_clone_resize.upgrade() else {
                return;
            };
            let popover_opt = inner_rc.borrow().claw_popover.clone();
            if let Some(pop) = popover_opt {
                pop.update_pane_height(h);
            }

            let Some(term) = terminal_weak.upgrade() else {
                return;
            };
            let cols = term.column_count() as i64;
            let rows = term.row_count() as i64;

            let mut inner = inner_rc.borrow_mut();
            if cols != inner.n_columns || rows != inner.n_rows {
                inner.n_columns = cols;
                inner.n_rows = rows;

                if cols > 0
                    && rows > 0
                    && let (Some(lbl), Some(rev)) =
                        (size_label_weak.upgrade(), size_revealer_weak.upgrade())
                {
                    lbl.set_label(&format!("{cols} × {rows}"));
                    rev.set_reveal_child(true);

                    if let Some(source) = inner.size_dismiss_source.take() {
                        source.remove();
                    }

                    let revealer_weak = rev.downgrade();
                    let inner_weak = Rc::downgrade(&inner_rc);
                    let source = glib::timeout_add_local(
                        std::time::Duration::from_millis(1000),
                        move || {
                            if let Some(r) = revealer_weak.upgrade() {
                                r.set_reveal_child(false);
                            }
                            if let Some(i) = inner_weak.upgrade() {
                                i.borrow_mut().size_dismiss_source = None;
                            }
                            glib::ControlFlow::Break
                        },
                    );

                    inner.size_dismiss_source = Some(source);
                }
            }
        });
        widget.add_overlay(&resize_detector);

        // Ctrl+/ opens the claw drawer (if closed) and focuses the merged msgbar entry.
        let shortcut_controller = gtk::ShortcutController::new();
        shortcut_controller.set_propagation_phase(gtk::PropagationPhase::Capture);

        let inner_weak_sc = Rc::downgrade(&inner);
        let session_status_sc = session_status.clone();
        let is_pinned_sc = is_pinned.clone();
        let is_web_search_sc = is_web_search.clone();
        let is_vibe_sc = is_vibe.clone();
        let agent_name_sc = agent_name.clone();
        let action = gtk::CallbackAction::new(move |_, _| {
            if let Some(inner_rc) = inner_weak_sc.upgrade() {
                let (term, mb, pop_opt) = {
                    let b = inner_rc.borrow();
                    (
                        b.terminal.clone(),
                        b.msg_bar.clone(),
                        b.claw_popover.clone(),
                    )
                };
                term.set_focusable(false);
                mb.update_ui(
                    session_status_sc.borrow().clone(),
                    is_pinned_sc.get(),
                    is_web_search_sc.get(),
                    is_vibe_sc.get(),
                );
                if let Some(pop) = pop_opt {
                    let aname = agent_name_sc.borrow().clone();
                    pop.show_input_only(&aname);
                }
            }
            gtk::glib::Propagation::Stop
        });

        let initial_trigger = parse_accel_trigger(&settings.claw_msgbar_shortcut);
        let msgbar_shortcut = gtk::Shortcut::builder()
            .trigger(&initial_trigger)
            .action(&action)
            .build();
        shortcut_controller.add_shortcut(msgbar_shortcut.clone());
        widget.add_controller(shortcut_controller);

        Self {
            widget,
            inner,
            _search_bar: search_bar_rc,
            claw_popover,
            claw_indicator,
            pending_sleep_diagnosis,
            claw_sender,
            is_claw_active,
            session_status,
            is_pinned,
            is_web_search,
            is_vibe,
            agent_name,
            msg_bar,
            session_usage,
            pending_tasks,
            msgbar_shortcut,
        }
    }

    pub fn widget(&self) -> &gtk::Overlay {
        &self.widget
    }

    pub fn set_callback<F: Fn(PaneOutput) + Send + Sync + 'static>(&self, callback: F) {
        let slot = self.inner.borrow().callback_slot.clone();
        *slot.write() = std::sync::Arc::new(callback);
    }

    pub fn set_layout_tag(&self, layout_tag: String) {
        let pid = {
            let mut inner = self.inner.borrow_mut();
            inner.layout_tag = layout_tag.clone();
            inner.pid
        };
        if let Some(pid) = pid {
            glib::spawn_future_local(async move {
                let agent = crate::get_agent().await;
                let _ = agent.set_pane_id(pid, layout_tag).await;
            });
        }
    }

    pub fn last_title(&self) -> Option<String> {
        self.inner.borrow().last_title.clone()
    }

    pub fn foreground_process(&self) -> Option<String> {
        self.inner.borrow().foreground_process.clone()
    }

    pub fn is_alt_screen(&self) -> bool {
        self.inner.borrow().terminal.is_alt_screen()
    }

    pub fn get_session_usage(&self) -> SessionUsage {
        self.session_usage.get()
    }

    pub fn get_pending_tasks(&self) -> Vec<ScheduledTask> {
        self.pending_tasks.borrow().clone()
    }

    pub fn has_pending_tasks(&self) -> bool {
        self.pending_tasks
            .borrow()
            .iter()
            .any(|t| t.status == TaskStatus::Pending)
    }

    pub fn reveal_claw_chat(&self) {
        let aname = self.agent_name.borrow().clone();
        if !aname.is_empty() && !self.claw_popover.is_visible() {
            self.claw_popover.show_chat_only(&aname);
        }
    }

    pub fn is_claw_drawer_visible(&self) -> bool {
        self.claw_popover.is_visible()
    }

    pub fn send_claw_message(&self, msg: ClawMessage) {
        let tx = self.claw_sender.clone();
        glib::spawn_future_local(async move {
            let _ = tx.send(msg).await;
        });
    }

    pub fn get_total_tokens(&self) -> u64 {
        self.session_usage.get().total_tokens
    }

    pub fn is_pinned(&self) -> bool {
        self.is_pinned.get()
    }

    pub fn is_web_search(&self) -> bool {
        self.is_web_search.get()
    }

    pub fn is_vibe(&self) -> bool {
        self.is_vibe.get()
    }

    pub fn agent_name(&self) -> String {
        self.agent_name.borrow().clone()
    }

    pub fn send_text(&self, text: &str) {
        self.inner
            .borrow()
            .terminal
            .write_all(text.as_bytes().to_vec());
    }

    pub fn has_selection(&self) -> bool {
        self.inner.borrow().terminal.has_selection()
    }

    pub fn show_claw_popover(&self, title: &str, message: &str, proposal: crate::TerminalProposal) {
        self.claw_indicator.hide();
        self.claw_popover
            .show(OverlayMode::Claw, title, None, message, proposal.into());
    }

    pub fn show_bookmark_proposal(
        &self,
        title: &str,
        message: &str,
        proposal: crate::TerminalProposal,
    ) {
        self.claw_popover
            .show(OverlayMode::Bookmark, title, None, message, proposal.into());
    }

    pub fn hide_claw_popover(&self) {
        self.claw_popover.hide();
    }

    pub fn show_lazy_error(&self) {
        self.claw_indicator.show_lazy_error();
    }

    pub fn show_diagnosis_ready(&self, diagnosis: String, proposal: crate::TerminalProposal) {
        *self.pending_sleep_diagnosis.borrow_mut() = Some((diagnosis, proposal));
        self.claw_indicator.show_diagnosis_ready();
    }

    pub fn terminal(&self) -> TerminalWidget {
        self.inner.borrow().terminal.clone()
    }

    pub fn working_dir(&self) -> Option<String> {
        self.inner.borrow().working_dir.clone()
    }

    pub fn id(&self) -> String {
        self.inner.borrow().id.clone()
    }

    pub fn get_pid(&self) -> Option<u32> {
        self.inner.borrow().pid
    }

    pub async fn get_text_snapshot(&self, max_lines: usize, offset_lines: usize) -> Option<String> {
        let terminal = self.terminal();
        terminal.get_text_snapshot(max_lines, offset_lines).await
    }

    pub fn inject_text(&self, text: &str) {
        self.inner
            .borrow()
            .terminal
            .write_all(text.as_bytes().to_vec());
    }

    pub fn cancel_task(&self, task_id: uuid::Uuid) {
        self.pending_tasks.borrow_mut().retain(|t| t.id != task_id);
        let has_remaining = self.has_pending_tasks();
        if let Some(ind) = &self.inner.borrow().claw_indicator {
            ind.set_has_tasks(has_remaining);
        }
        let _ = self
            .claw_sender
            .send_blocking(ClawMessage::CancelTask { task_id });
    }

    pub fn toggle_pause_task(&self, task_id: uuid::Uuid) {
        if let Some(task) = self
            .pending_tasks
            .borrow_mut()
            .iter_mut()
            .find(|t| t.id == task_id)
        {
            task.toggle_pause_now();
        }
        let _ = self
            .claw_sender
            .send_blocking(ClawMessage::TogglePauseTask { task_id });
    }

    pub fn cancel_all_tasks(&self) {
        self.pending_tasks.borrow_mut().clear();
        if let Some(ind) = &self.inner.borrow().claw_indicator {
            ind.set_has_tasks(false);
        }
        let _ = self.claw_sender.send_blocking(ClawMessage::CancelAllTasks);
    }

    pub fn write_all(&self, data: Vec<u8>) {
        self.inner.borrow().terminal.write_all(data);
    }

    pub fn copy(&self) {
        self.inner.borrow().terminal.copy_clipboard();
    }

    pub fn select_all(&self) {
        self.inner.borrow().terminal.select_all();
    }

    pub fn toggle_search(&self) {
        if !self._search_bar.is_visible() {
            self._search_bar.reveal();
        } else {
            self._search_bar.hide();
        }
    }

    pub fn copy_last_command_output(&self) {
        self.inner.borrow().terminal.copy_last_command_output();
    }

    pub fn open_scrollback_in_editor(&self) {
        let term = self.inner.borrow().terminal.clone();
        gestures::export_and_open_scrollback(&term);
    }

    pub fn clear_scrollback(&self) {
        self.inner.borrow().terminal.clear_scrollback();
    }

    pub fn reset_terminal(&self) {
        self.inner.borrow().terminal.reset_terminal();
    }

    pub fn inject_keystrokes(&self, keys: &str) {
        log::debug!(
            "Injecting keystrokes into pane {}: {:?}",
            self.inner.borrow().id,
            keys
        );
        let unescaped = keys
            .replace("\\u001b", "\x1b")
            .replace("\\e", "\x1b")
            .replace("\\r", "\r")
            .replace("\\n", "\n")
            .replace("\\t", "\t")
            .replace("\\x03", "\x03")
            .replace("\\x04", "\x04");

        self.inner
            .borrow()
            .terminal
            .write_all(unescaped.into_bytes());
    }

    pub fn paste(&self) {
        self.inner.borrow().terminal.paste_clipboard();
    }

    pub fn grab_focus(&self) {
        if self.claw_popover.is_visible() {
            self.claw_popover.grab_input_focus();
        } else {
            self.inner.borrow().terminal.grab_focus();
        }
    }

    pub fn is_claw_active(&self) -> bool {
        self.is_claw_active.get()
    }

    pub fn is_sleep(&self) -> bool {
        matches!(*self.session_status.borrow(), AgentStatus::Sleep)
    }

    pub fn set_session_status(&self, status: AgentStatus) {
        *self.session_status.borrow_mut() = status.clone();
        self.msg_bar.set_status(status.clone());
        self.claw_indicator.set_mode(status);
    }

    pub fn reload_claw(&self) {
        let tx = self.claw_sender.clone();
        glib::spawn_future_local(async move {
            let _ = tx.send(ClawMessage::Reload).await;
        });
    }

    pub fn soft_clear_claw_history(&self) {
        self.claw_popover.clear_visible_history();
    }

    pub fn notify_settings_invalidated(&self) {
        let _ = self.claw_sender.try_send(ClawMessage::SettingsInvalidated);
    }

    pub fn clear_claw_history(&self) {
        // Handled strictly in the sidebar UI to preserve persisted database history.
    }

    pub fn open_in_files(&self) {
        let wd = self
            .working_dir()
            .unwrap_or_else(|| std::env::var("HOME").unwrap_or_default());
        let uri = if wd.starts_with("file://") {
            wd.clone()
        } else {
            format!("file://{wd}")
        };
        let path = wd.strip_prefix("file://").unwrap_or(&wd).to_string();

        gtk4::glib::spawn_future_local(async move {
            if is_flatpak() {
                let open_res = boxxy_sys_utils::runtime()
                    .spawn_blocking({
                        let p = path.clone();
                        move || std::fs::File::open(p)
                    })
                    .await;
                match open_res {
                    Ok(Ok(dir)) => {
                        use std::os::fd::AsFd;
                        if let Err(e) = ashpd::desktop::open_uri::OpenDirectoryRequest::default()
                            .send(&dir.as_fd())
                            .await
                        {
                            log::warn!("Failed to open directory via ashpd: {e}");
                        }
                    }
                    _ => log::warn!("Failed to open directory for ashpd: {path}"),
                }
            } else {
                let _ = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>);
            }
        });
    }

    pub fn update_settings(&self, settings: Settings, palette_opt: Option<Palette>) {
        let mut inner = self.inner.borrow_mut();

        let mut needs_font = true;
        let mut needs_padding = true;
        let mut needs_cell = true;
        let mut needs_cursor_shape = true;
        let mut needs_palette = true;
        let mut needs_cursor_color = true;
        let mut needs_show_grid = true;
        let mut needs_invert_scroll = true;
        let mut needs_frosted_glass = true;
        let mut needs_cursor_blink = true;
        let mut needs_mouse_autohide = true;
        if let Some(ref p) = inner.current_settings {
            if p.enable_web_search != settings.enable_web_search {
                self.msg_bar
                    .set_web_search_visible(settings.enable_web_search);

                if !settings.enable_web_search {
                    self.is_web_search.set(false);
                    if self.is_claw_active.get() {
                        let _ = self
                            .claw_sender
                            .try_send(ClawMessage::ToggleWebSearch(false));
                    }
                    self.msg_bar.update_ui(
                        self.session_status.borrow().clone(),
                        self.is_pinned.get(),
                        false,
                        self.is_vibe.get(),
                    );
                } else if self.is_claw_active.get() {
                    let _ = self
                        .claw_sender
                        .try_send(ClawMessage::ToggleWebSearch(self.is_web_search.get()));
                }
            }

            needs_font = p.font_name != settings.font_name || p.font_size != settings.font_size;
            needs_padding = p.padding != settings.padding;
            needs_cell = (p.cell_height_scale - settings.cell_height_scale).abs() > 1e-6
                || (p.cell_width_scale - settings.cell_width_scale).abs() > 1e-6;
            needs_cursor_shape = p.cursor_shape != settings.cursor_shape;
            needs_cursor_blink = p.cursor_blinking != settings.cursor_blinking;
            needs_mouse_autohide = p.mouse_autohide != settings.mouse_autohide;
            needs_palette =
                p.theme != settings.theme || (p.opacity - settings.opacity).abs() > 1e-4;
            needs_cursor_color = p.cursor_color_override != settings.cursor_color_override
                || p.cursor_color != settings.cursor_color;
            needs_show_grid = p.show_vte_grid != settings.show_vte_grid;
            needs_invert_scroll = p.invert_scroll != settings.invert_scroll;
            let prev_effective_frosted = p.frosted_glass && p.opacity < 1.0;
            let next_effective_frosted = settings.frosted_glass && settings.opacity < 1.0;
            needs_frosted_glass = prev_effective_frosted != next_effective_frosted;
        }

        self.claw_popover
            .set_bottom_gap(settings.overlay_bottom_gap);
        self.msgbar_shortcut
            .set_trigger(Some(parse_accel_trigger(&settings.claw_msgbar_shortcut)));

        inner.hide_scrollbars = settings.hide_scrollbars;
        inner
            .scrolled_window
            .set_vscrollbar_policy(if settings.hide_scrollbars {
                gtk::PolicyType::Never
            } else {
                gtk::PolicyType::Always
            });

        if needs_show_grid {
            inner.terminal.set_show_grid(settings.show_vte_grid);
        }

        if needs_invert_scroll {
            inner.terminal.set_invert_scroll(settings.invert_scroll);
        }

        if needs_frosted_glass {
            inner
                .terminal
                .set_frosted_glass(settings.frosted_glass && settings.opacity < 1.0);
        }

        if needs_font {
            let font_desc = pango::FontDescription::from_string(&format!(
                "{} {}",
                settings.font_name, settings.font_size
            ));
            inner.terminal.set_font(Some(&font_desc));
            self.msg_bar.apply_font(&font_desc);
        }

        if needs_padding {
            inner.terminal.set_padding(settings.padding as f32);
        }

        if needs_cell {
            inner
                .terminal
                .set_cell_height_scale(settings.cell_height_scale);
            inner
                .terminal
                .set_cell_width_scale(settings.cell_width_scale);
        }

        if needs_cursor_shape {
            inner
                .terminal
                .set_cursor_shape(match settings.cursor_shape {
                    boxxy_preferences::CursorShape::Block => {
                        boxxy_vte::terminal::CursorShape::Block
                    }
                    boxxy_preferences::CursorShape::IBeam => boxxy_vte::terminal::CursorShape::Beam,
                    boxxy_preferences::CursorShape::Underline => {
                        boxxy_vte::terminal::CursorShape::Underline
                    }
                });
        }

        if needs_cursor_blink {
            inner
                .terminal
                .set_cursor_blink_mode(settings.cursor_blinking);
        }

        if needs_mouse_autohide {
            inner.terminal.set_mouse_autohide(settings.mouse_autohide);
        }

        if needs_palette && let Some(palette) = palette_opt {
            let (fg, mut bg, colors) = palette.to_vte_colors();
            bg.set_alpha(settings.opacity as f32);
            let palette_refs: Vec<&gtk::gdk::RGBA> = colors.iter().collect();
            inner
                .terminal
                .set_colors(Some(&fg), Some(&bg), &palette_refs);
            needs_cursor_color = true;
        }

        if needs_cursor_color {
            if settings.cursor_color_override {
                if let Ok(rgba) = gdk::RGBA::parse(&settings.cursor_color) {
                    inner.terminal.set_color_cursor(Some(&rgba));
                }
            } else {
                let theme_cursor = palette_opt.and_then(|p| gdk::RGBA::parse(p.cursor).ok());
                inner.terminal.set_color_cursor(theme_cursor.as_ref());
            }
        }

        let hide_badge = settings.hide_character_badge;
        inner.current_settings = Some(settings);
        if let Some(ind) = &inner.claw_indicator {
            ind.update_settings(hide_badge);
        }
    }

    pub fn set_dimmed(&self, dimmed: bool) {
        let mut inner = self.inner.borrow_mut();
        inner.is_dimmed = dimmed;
        inner.terminal.set_dimmed(dimmed);
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_accel;

    #[test]
    fn normalize_accel_translates_literal_punctuation() {
        assert_eq!(normalize_accel("<Ctrl>/"), "<Ctrl>slash");
        assert_eq!(normalize_accel("<Ctrl><Shift>?"), "<Ctrl><Shift>question");
        assert_eq!(normalize_accel("<Alt>Return"), "<Alt>Return");
    }
}
