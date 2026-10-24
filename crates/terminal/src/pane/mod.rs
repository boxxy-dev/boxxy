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
mod gestures;
mod preview;
mod ui;

pub(crate) use claw_host::PaneClawHost;

pub type PendingDiagnosis = Rc<RefCell<Option<(String, crate::TerminalProposal)>>>;

/// GTK key names for characters that users commonly type literally in accelerator
/// strings (e.g. "/" instead of "slash").  `gtk_accelerator_parse` only accepts
/// the symbolic names, so bare characters must be normalised before parsing.
fn normalize_accel(accel: &str) -> String {
    let key_start = accel.rfind('>').map(|i| i + 1).unwrap_or(0);
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

#[derive(Clone)]
pub struct TerminalPaneComponent {
    widget: gtk::Overlay,
    inner: Rc<RefCell<PaneInner>>,
    _search_bar: Rc<SearchBarComponent>,
    claw_popover: TerminalOverlay,
    claw_indicator: ClawIndicator,

    pending_sleep_diagnosis: PendingDiagnosis,
    claw_sender: async_channel::Sender<ClawMessage>,

    pub is_claw_active: Rc<Cell<bool>>,
    pub session_status: Rc<RefCell<AgentStatus>>,
    pub is_pinned: Rc<Cell<bool>>,
    is_web_search: Rc<Cell<bool>>,
    agent_name: Rc<RefCell<String>>,
    msg_bar: Rc<MsgBarComponent>,
    pub session_usage: Rc<Cell<SessionUsage>>,
    pending_tasks: Rc<RefCell<Vec<ScheduledTask>>>,
    msgbar_shortcut: gtk::Shortcut,
}

impl Drop for PaneInner {
    fn drop(&mut self) {
        // Close per-pane channels immediately so `spawn_dispatch` (which holds
        // strong references to `PaneClawHost`, `TerminalOverlay`, and `MsgBarComponent`)
        // and the UI-to-agent forwarder task exit right away.
        self.claw_event_tx.close();
        self.claw_msg_tx.close();
        self.process_subscriptions.clear();
        if let Ok(mut sub) = self.claw_subscription.lock() {
            *sub = None;
        }

        if self.already_handled_close {
            self.terminal.detach_pty();
            return;
        }

        // Read the live setting here rather than snapshotting it at
        // spawn time — that way toggling persistence in Preferences
        // takes effect on the very next pane close, not just on panes
        // opened after the toggle. Explicitly closing a single split or
        // idle tab sets `force_terminate_on_close = true` so it doesn't
        // linger in the background.
        let persist = Settings::load().pty_persistence && !self.force_terminate_on_close;

        let session_arc = self.session_id.clone();
        let pane_id = self.id.clone();
        glib::spawn_future_local(async move {
            let sid = session_arc.lock().await.take();
            if sid.is_some() {
                let agent = crate::get_agent().await;
                if persist {
                    let _ = agent.detach_holder(pane_id).await;
                } else {
                    let _ = agent.release_holder(pane_id).await;
                }
            }
        });

        let Some(pid) = self.pid else {
            self.terminal.detach_pty();
            return;
        };

        if persist {
            log::info!(
                "PaneInner dropping (pid={}): pty_persistence on → detaching into daemon",
                pid
            );
            let snapshot = self.export_snapshot_with_title();
            self.terminal.detach_pty();
            glib::spawn_future_local(async move {
                let agent = crate::get_agent().await;
                // Set the flag then detach. Order matters: detach()
                // only keeps the shell alive if persistence is already
                // on at decision time.
                let _ = agent.set_persistence(pid, true).await;
                match agent.detach_with_snapshot(pid, snapshot).await {
                    Ok(1) => log::info!("pid={} detached into background", pid),
                    Ok(0) => log::info!(
                        "pid={} terminated (unexpectedly — persistence may be off on daemon)",
                        pid
                    ),
                    Ok(3) => log::warn!(
                        "pid={} detached without buffer (daemon has no stored FD — shell will block when kernel PTY buffer fills)",
                        pid
                    ),
                    Ok(code) => log::debug!("pid={} detach returned code {}", pid, code),
                    Err(e) => log::warn!("pid={} detach failed: {}", pid, e),
                }
            });
        } else {
            // Non-persistent path: disable persistence on the daemon and
            // SIGTERM the process group so shells and their children are cleaned up.
            log::info!("PaneInner dropping (pid={}): killing process group", pid);
            self.terminal.detach_pty();
            glib::spawn_future_local(async move {
                let agent = crate::get_agent().await;
                let _ = agent.set_persistence(pid, false).await;
                let _ = agent.signal_process_group(pid, 15).await;
            });
        }
    }
}

pub(super) struct PaneInner {
    pub terminal: TerminalWidget,
    pub(super) scrolled_window: gtk::ScrolledWindow,
    pub(super) working_dir: Option<String>,
    pub(super) last_title: Option<String>,
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
        std::sync::Arc<std::sync::Mutex<Option<crate::agent_manager::SubscriptionGuard>>>,
    pub(super) claw_indicator: Option<ClawIndicator>,
    pub(super) msg_bar: Rc<MsgBarComponent>,
    pub(super) callback: std::sync::Arc<dyn Fn(PaneOutput) + Send + Sync + 'static>,
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
        if let Ok(mut sub) = self.claw_subscription.lock() {
            *sub = None;
        }
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
            // Close per-pane channels and drop the router subscription BEFORE
            // calling `detach_session_sync` so the closing pane cannot consume
            // events meant for a future reattached pane, and GTK widget drops
            // cannot later treat this pane as an unhandled close.
            inner.claw_event_tx.close();
            inner.claw_msg_tx.close();
            inner.process_subscriptions.clear();
            if let Ok(mut sub) = inner.claw_subscription.lock() {
                *sub = None;
            }
            let snap = inner.export_snapshot_with_title();
            // Shut down this pane's EventLoop and close its master_fd BEFORE
            // handing the session to the daemon's background reader.
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
            if let Ok(mut sub) = inner.claw_subscription.lock() {
                *sub = None;
            }
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

    /// Explicitly release the Claw session in the daemon. Called from the
    /// tab-close path where we have a synchronous handle on the pane.
    /// Do NOT rely solely on PaneInner::drop for this — GTK closures hold
    /// Rc clones of inner, so drop may fire very late or not at all.
    pub fn end_session(&self) {
        let (session_arc, pane_id) = {
            let inner = self.inner.borrow();
            (inner.session_id.clone(), inner.id.clone())
        };
        glib::spawn_future_local(async move {
            let sid = session_arc.lock().await.take();
            if sid.is_some() {
                let agent = crate::get_agent().await;
                let _ = agent.release_holder(pane_id).await;
            }
        });
    }

    pub async fn end_session_sync(&self) {
        let (session_arc, pane_id) = {
            let inner = self.inner.borrow();
            (inner.session_id.clone(), inner.id.clone())
        };
        let sid = session_arc.lock().await.take();
        if sid.is_some() {
            let agent = crate::get_agent().await;
            let _ = agent.release_holder(pane_id).await;
        }
    }

    pub fn new<F: Fn(PaneOutput) + Send + Sync + 'static>(init: PaneInit, callback: F) -> Self {
        let settings = boxxy_preferences::Settings::load();
        let callback: std::sync::Arc<dyn Fn(PaneOutput) + Send + Sync + 'static> =
            std::sync::Arc::new(callback);
        let id = init.id;

        let (
            widget,
            terminal,
            scrolled_window,
            size_revealer,
            size_label,
            search_bar_rc,
            progress_bar,
        ) = ui::build_ui();

        let (claw_event_tx, claw_rx_from_agent) = async_channel::unbounded();
        let (tx_to_agent, rx_from_ui) =
            async_channel::unbounded::<boxxy_claw_protocol::ClawMessage>();
        let session_id_arc: std::sync::Arc<tokio::sync::Mutex<Option<String>>> =
            std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let session_id_for_init = session_id_arc.clone();

        // Expose it to inner so claw_host can update it
        let session_id_for_inner = session_id_arc.clone();
        let claw_sender_clone = claw_event_tx.clone();
        let claw_event_tx_for_inner = claw_event_tx;
        // Clone for the first-message path (event forwarding from new session)
        let event_sender_for_first_msg = claw_sender_clone.clone();

        // Shared pre-selection state: the overlay populates this when the user
        // clicks a character button; the msgbar reads it when creating the
        // session on the first outgoing message.
        let pending_character: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
        let pending_char_for_msgbar = pending_character.clone();
        let pending_char_for_overlay = pending_character.clone();

        // Do not auto-spawn the Claw session. The user will select a character via the Overlay first.
        // The overlay will start in "Selection Mode" because there is no identity yet.
        gtk::glib::spawn_future_local(async move {
            let agent = crate::get_agent().await;

            // Forward messages from UI to agent. Wait until session_id is populated.
            let rx_from_ui = rx_from_ui;
            let session_id_ref = session_id_for_init.clone();

            tokio::spawn(async move {
                while let Ok(msg) = rx_from_ui.recv().await {
                    let sid: Option<String> = {
                        let lock = session_id_ref.lock().await;
                        lock.clone()
                    };
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
        let agent_name = Rc::new(RefCell::new(String::new()));
        let claw_indicator = ClawIndicator::new();
        let session_usage = Rc::new(Cell::new(SessionUsage::default()));
        let pending_tasks: Rc<RefCell<Vec<ScheduledTask>>> = Rc::new(RefCell::new(Vec::new()));

        let (msg_bar, inner) = {
            let tx_msg = claw_sender.clone();
            let tx_claw_toggle = claw_sender.clone();
            let tx_sleep_toggle = claw_sender.clone();
            let cb_msg = callback.clone();
            let id_msg = id.clone();
            let cb_toggle = callback.clone();
            let id_toggle = id.clone();
            let cb_sleep = callback.clone();
            let id_sleep = id.clone();
            let cb_pin = callback.clone();
            let id_pin = id.clone();
            let cb_web_search = callback.clone();
            let id_web_search = id.clone();

            // We need a weak ref for the msg_bar callbacks, but inner isn't created yet.
            // We'll use a RefCell<Option<Weak<RefCell<PaneInner>>>> that we fill later.
            let inner_weak_ref: Rc<RefCell<Option<std::rc::Weak<RefCell<PaneInner>>>>> =
                Rc::new(RefCell::new(None));
            let inner_weak_for_msg = inner_weak_ref.clone();
            let inner_weak_for_cancel = inner_weak_ref.clone();
            let inner_weak_for_active = inner_weak_ref.clone();
            let inner_weak_for_sleep = inner_weak_ref.clone();
            let inner_weak_for_pin = inner_weak_ref.clone();
            let inner_weak_for_web_search = inner_weak_ref.clone();

            let is_claw_active_for_msg = is_claw_active.clone();
            let session_status_for_msg = session_status.clone();
            let is_pinned_for_msg = is_pinned.clone();
            let is_web_search_for_msg = is_web_search.clone();

            let is_claw_active_for_active = is_claw_active.clone();
            let session_status_for_active = session_status.clone();
            let is_pinned_for_active = is_pinned.clone();
            let is_web_search_for_active = is_web_search.clone();

            let is_claw_active_for_sleep = is_claw_active.clone();
            let session_status_for_sleep = session_status.clone();
            let is_pinned_for_sleep = is_pinned.clone();
            let is_web_search_for_sleep = is_web_search.clone();

            let session_status_for_pin = session_status.clone();
            let is_pinned_for_pin = is_pinned.clone();
            let is_web_search_for_pin = is_web_search.clone();

            let session_status_for_web_search = session_status.clone();
            let is_pinned_for_web_search = is_pinned.clone();
            let is_web_search_for_web_search = is_web_search.clone();

            let tx_pin_toggle = claw_sender.clone();
            let tx_web_search_toggle = claw_sender.clone();
            let pending_char_for_toggle = pending_char_for_msgbar.clone();
            let event_sender_for_toggle = event_sender_for_first_msg.clone();
            let msg_bar = Rc::new(MsgBarComponent::new(
                move |(query, images)| {
                    if let Some(inner_arc) = inner_weak_for_msg
                        .borrow()
                        .as_ref()
                        .and_then(|w| w.upgrade())
                    {
                        let pane = inner_arc.borrow().terminal.clone();
                        let is_resume_cmd = query
                            .strip_prefix("/resume ")
                            .map(str::trim)
                            .is_some_and(|s| !s.is_empty());
                        if is_resume_cmd {
                            inner_arc.borrow().msg_bar.entry.grab_focus();
                        }

                        if !is_claw_active_for_msg.get() {
                            is_claw_active_for_msg.set(true);
                            if let Some(ind) = &inner_arc.borrow().claw_indicator {
                                ind.set_visible(true);
                            }
                            inner_arc.borrow().msg_bar.update_ui(
                                session_status_for_msg.borrow().clone(),
                                is_pinned_for_msg.get(),
                                is_web_search_for_msg.get(),
                            );
                            cb_msg(PaneOutput::ClawStateChanged(
                                id_msg.clone(),
                                true,
                                matches!(*session_status_for_msg.borrow(), AgentStatus::Sleep),
                            ));
                        }

                        let pending_rc = pending_char_for_msgbar.clone();
                        let pane_id = id_msg.clone();
                        let session_id_ref = inner_arc.borrow().session_id.clone();
                        let claw_sub_ref = inner_arc.borrow().claw_subscription.clone();
                        let event_tx = event_sender_for_first_msg.clone();
                        let cwd = inner_arc.borrow().working_dir.clone().unwrap_or_default();
                        let tx = tx_msg.clone();

                        glib::spawn_future_local(async move {
                            let snapshot =
                                pane.get_text_snapshot(100, 0).await.unwrap_or_default();
                            let has_session = session_id_ref.lock().await.is_some();

                            let resume_id_opt = query
                                .strip_prefix("/resume ")
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                                .map(str::to_string);

                            // If `/resume <id>` targets a session that is currently running in the
                            // background (or active in another open pane), reattach or focus it
                            // instead of creating a conflicting session.
                            if let Some(ref resume_id) = resume_id_opt {
                                let existing_claim = boxxy_claw_protocol::characters::CLAIMS_CACHE
                                    .load()
                                    .iter()
                                    .find(|c| c.session_id == *resume_id && c.holder_id != pane_id)
                                    .cloned();
                                if let Some(claim) = existing_claim {
                                    let agent = crate::get_agent().await;
                                    if claim.owner_bus_name.is_empty() {
                                        if let Ok(detached) =
                                            agent.list_detached_sessions_detailed().await
                                            && let Some(s) = detached.into_iter().find(|s| {
                                                s.session_id == *resume_id
                                                    || s.character_id == claim.character_id
                                                    || s.pane_id == claim.holder_id
                                            })
                                        {
                                            let _ = crate::TERMINAL_EVENT_BUS.send(
                                                crate::TerminalEvent {
                                                    id: String::new(),
                                                    kind: crate::TerminalEventKind::RestoreDetached(
                                                        s.pid, s.cwd,
                                                    ),
                                                },
                                            );
                                            return;
                                        }
                                    } else {
                                        let _ = agent.request_focus_pane(claim.holder_id).await;
                                        return;
                                    }
                                }
                            }

                            if !has_session {
                                let agent = crate::get_agent().await;

                                // If resuming a past session, claim the session's original character
                                // (falling back to the next/first available character if it was removed)
                                // rather than the pre-selected default character in the picker.
                                let resolved_resume_char_id =
                                    if let Some(ref resume_id) = resume_id_opt {
                                        boxxy_claw_widget::msgbar::autocomplete::resolve_session_character_id(
                                            resume_id,
                                        )
                                        .await
                                    } else {
                                        None
                                    };

                                let char_id = if let Some(resume_char_id) = resolved_resume_char_id
                                {
                                    *pending_rc.borrow_mut() = resume_char_id.clone();
                                    resume_char_id
                                } else {
                                    let pending = pending_rc.borrow().clone();
                                    if pending.is_empty() {
                                        boxxy_claw_protocol::characters::CHARACTER_CACHE
                                            .load()
                                            .first()
                                            .map(|c| c.config.id.clone())
                                            .unwrap_or_default()
                                    } else {
                                        pending
                                    }
                                };

                                if let Ok((session_id, sub_guard)) = agent
                                    .create_claw_session_with_sender(
                                        pane_id,
                                        char_id,
                                        event_tx.clone(),
                                    )
                                    .await
                                {
                                    if session_id.is_empty() {
                                        return;
                                    }
                                    if let Some(resume_id) = resume_id_opt {
                                        let resume_guard = agent
                                            .subscribe_claw_session(resume_id.clone(), event_tx);
                                        if let Ok(mut guard_slot) = claw_sub_ref.lock() {
                                            *guard_slot = Some(resume_guard);
                                        }
                                        *session_id_ref.lock().await = Some(resume_id.clone());
                                        let _ = agent
                                            .post_claw_message(
                                                session_id,
                                                ClawMessage::ResumeSession {
                                                    session_id: resume_id,
                                                },
                                            )
                                            .await;
                                    } else {
                                        if let Ok(mut guard_slot) = claw_sub_ref.lock() {
                                            *guard_slot = Some(sub_guard);
                                        }
                                        *session_id_ref.lock().await = Some(session_id.clone());
                                        let _ = agent
                                            .post_claw_message(
                                                session_id.clone(),
                                                ClawMessage::Initialize,
                                            )
                                            .await;
                                        let _ = agent
                                            .post_claw_message(
                                                session_id,
                                                ClawMessage::ClawQuery {
                                                    query,
                                                    snapshot,
                                                    cwd,
                                                    image_attachments: images,
                                                },
                                            )
                                            .await;
                                    }
                                }
                                return;
                            }

                            // Session already exists — send through the established pump.
                            if let Some(resume_id) = resume_id_opt {
                                let agent = crate::get_agent().await;
                                let resume_guard =
                                    agent.subscribe_claw_session(resume_id.clone(), event_tx);
                                if let Ok(mut guard_slot) = claw_sub_ref.lock() {
                                    *guard_slot = Some(resume_guard);
                                }
                                let _ = tx
                                    .send(ClawMessage::ResumeSession {
                                        session_id: resume_id.clone(),
                                    })
                                    .await;
                                *session_id_ref.lock().await = Some(resume_id);
                                return;
                            }
                            let _ = tx
                                .send(ClawMessage::ClawQuery {
                                    query,
                                    snapshot,
                                    cwd,
                                    image_attachments: images,
                                })
                                .await;
                        });
                    }
                },
                move || {
                    if let Some(inner_arc) = inner_weak_for_cancel
                        .borrow()
                        .as_ref()
                        .and_then(|w| w.upgrade())
                    {
                        let pane = inner_arc.borrow().terminal.clone();
                        pane.set_focusable(true);
                        pane.grab_focus();
                    }
                },
                move |active| {
                    if is_claw_active_for_active.get() != active {
                        is_claw_active_for_active.set(active);
                        let mut session_refs = None;
                        if let Some(inner_arc) = inner_weak_for_active
                            .borrow()
                            .as_ref()
                            .and_then(|w| w.upgrade())
                        {
                            let mut status = session_status_for_active.borrow().clone();
                            if active && status == AgentStatus::Off {
                                status = AgentStatus::Waiting;
                                *session_status_for_active.borrow_mut() = status.clone();
                            } else if !active {
                                status = AgentStatus::Off;
                                *session_status_for_active.borrow_mut() = status.clone();
                            }

                            inner_arc.borrow().msg_bar.update_ui(
                                status.clone(),
                                is_pinned_for_active.get(),
                                is_web_search_for_active.get(),
                            );
                            if let Some(ind) = &inner_arc.borrow().claw_indicator {
                                ind.set_visible(active);
                            }
                            session_refs = Some((
                                inner_arc.borrow().session_id.clone(),
                                inner_arc.borrow().claw_subscription.clone(),
                            ));
                        }
                        cb_toggle(PaneOutput::ClawStateChanged(
                            id_toggle.clone(),
                            active,
                            matches!(*session_status_for_active.borrow(), AgentStatus::Sleep),
                        ));
                        let tx = tx_claw_toggle.clone();
                        if active {
                            let pane_id = id_toggle.clone();
                            let pending = pending_char_for_toggle.borrow().clone();
                            let event_tx = event_sender_for_toggle.clone();
                            glib::spawn_future_local(async move {
                                if let Some((session_id_ref, claw_sub_ref)) = session_refs
                                    && session_id_ref.lock().await.is_none()
                                {
                                    let agent = crate::get_agent().await;
                                    let char_id = if pending.is_empty() {
                                        boxxy_claw_protocol::characters::CHARACTER_CACHE
                                            .load()
                                            .first()
                                            .map(|c| c.config.id.clone())
                                            .unwrap_or_default()
                                    } else {
                                        pending
                                    };
                                    if let Ok((sid, sub_guard)) = agent
                                        .create_claw_session_with_sender(pane_id, char_id, event_tx)
                                        .await
                                        && !sid.is_empty()
                                    {
                                        if let Ok(mut guard_slot) = claw_sub_ref.lock() {
                                            *guard_slot = Some(sub_guard);
                                        }
                                        *session_id_ref.lock().await = Some(sid.clone());
                                        let _ = agent
                                            .post_claw_message(sid, ClawMessage::Initialize)
                                            .await;
                                        return;
                                    }
                                }
                                let _ = tx.send(ClawMessage::Initialize).await;
                            });
                        } else {
                            glib::spawn_future_local(async move {
                                let _ = tx.send(ClawMessage::Deactivate).await;
                            });
                        }
                    }
                },
                move |sleep| {
                    let currently_sleeping =
                        matches!(*session_status_for_sleep.borrow(), AgentStatus::Sleep);
                    if currently_sleeping != sleep {
                        let new_status = if sleep {
                            AgentStatus::Sleep
                        } else {
                            AgentStatus::Waiting
                        };
                        *session_status_for_sleep.borrow_mut() = new_status.clone();

                        if let Some(inner_arc) = inner_weak_for_sleep
                            .borrow()
                            .as_ref()
                            .and_then(|w| w.upgrade())
                        {
                            inner_arc.borrow().msg_bar.update_ui(
                                new_status,
                                is_pinned_for_sleep.get(),
                                is_web_search_for_sleep.get(),
                            );
                        }
                        cb_sleep(PaneOutput::ClawStateChanged(
                            id_sleep.clone(),
                            is_claw_active_for_sleep.get(),
                            sleep,
                        ));
                        let tx = tx_sleep_toggle.clone();
                        glib::spawn_future_local(async move {
                            let _ = tx.send(ClawMessage::SleepToggle(sleep)).await;
                        });
                    }
                },
                move |pinned| {
                    if is_pinned_for_pin.get() != pinned {
                        is_pinned_for_pin.set(pinned);
                        if let Some(inner_arc) = inner_weak_for_pin
                            .borrow()
                            .as_ref()
                            .and_then(|w| w.upgrade())
                        {
                            inner_arc.borrow().msg_bar.update_ui(
                                session_status_for_pin.borrow().clone(),
                                pinned,
                                is_web_search_for_pin.get(),
                            );
                        }
                        cb_pin(PaneOutput::ClawEvent(
                            id_pin.clone(),
                            ClawEngineEvent::PinStatusChanged(pinned),
                        ));
                        let tx = tx_pin_toggle.clone();
                        glib::spawn_future_local(async move {
                            let _ = tx.send(ClawMessage::TogglePin(pinned)).await;
                        });
                    }
                },
                move |enabled| {
                    if is_web_search_for_web_search.get() != enabled {
                        is_web_search_for_web_search.set(enabled);
                        if let Some(inner_arc) = inner_weak_for_web_search
                            .borrow()
                            .as_ref()
                            .and_then(|w| w.upgrade())
                        {
                            inner_arc.borrow().msg_bar.update_ui(
                                session_status_for_web_search.borrow().clone(),
                                is_pinned_for_web_search.get(),
                                enabled,
                            );
                        }
                        cb_web_search(PaneOutput::ClawEvent(
                            id_web_search.clone(),
                            ClawEngineEvent::WebSearchStatusChanged(enabled),
                        ));
                        let tx = tx_web_search_toggle.clone();
                        glib::spawn_future_local(async move {
                            let _ = tx.send(ClawMessage::ToggleWebSearch(enabled)).await;
                        });
                    }
                },
            ));

            msg_bar.set_web_search_visible(settings.enable_web_search);

            let inner = Rc::new(RefCell::new(PaneInner {
                terminal: terminal.clone(),
                scrolled_window,
                working_dir: init.working_dir,
                last_title: None,
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
                claw_subscription: std::sync::Arc::new(std::sync::Mutex::new(None)),
                claw_indicator: None,
                msg_bar: msg_bar.clone(),
                callback: callback.clone(),
            }));

            *inner_weak_ref.borrow_mut() = Some(Rc::downgrade(&inner));
            (msg_bar, inner)
        };

        gestures::setup_gestures(&terminal, &search_bar_rc, callback.clone(), id.clone());
        preview::setup_preview(&terminal, &inner);

        let inner_clone_resize = Rc::downgrade(&inner);
        let size_revealer_clone = size_revealer.clone();
        let size_label_clone = size_label.clone();
        let terminal_clone = terminal.clone();

        let resize_detector = gtk::DrawingArea::new();
        resize_detector.set_can_target(false);
        resize_detector.connect_resize(move |_, _, _| {
            if let Some(inner_rc) = inner_clone_resize.upgrade() {
                let mut inner = inner_rc.borrow_mut();
                let cols = terminal_clone.column_count() as i64;
                let rows = terminal_clone.row_count() as i64;

                if cols != inner.n_columns || rows != inner.n_rows {
                    inner.n_columns = cols;
                    inner.n_rows = rows;

                    if cols > 0 && rows > 0 {
                        size_label_clone.set_label(&format!("{} × {}", cols, rows));
                        size_revealer_clone.set_reveal_child(true);

                        if let Some(source) = inner.size_dismiss_source.take() {
                            source.remove();
                        }

                        let revealer_weak = size_revealer_clone.downgrade();
                        let inner_weak = Rc::downgrade(&inner_rc);
                        let source = glib::timeout_add_local(
                            std::time::Duration::from_millis(1000),
                            move || {
                                if let Some(rev) = revealer_weak.upgrade() {
                                    rev.set_reveal_child(false);
                                }
                                if let Some(inner) = inner_weak.upgrade() {
                                    inner.borrow_mut().size_dismiss_source = None;
                                }
                                glib::ControlFlow::Break
                            },
                        );

                        inner.size_dismiss_source = Some(source);
                    }
                }
            }
        });
        widget.add_overlay(&resize_detector);

        events::wire_terminal_events(
            &terminal,
            &inner,
            &progress_bar,
            &is_claw_active,
            &claw_sender,
            callback.clone(),
            id.clone(),
        );
        // Msgbar is embedded inside the claw overlay drawer — not a pane
        // overlay anymore. setup_claw receives it and appends it into the
        // drawer's msgbar_slot.

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
            session_usage.clone(),
            is_pinned.clone(),
            is_web_search.clone(),
            session_status.clone(),
            agent_name.clone(),
            &claw_indicator,
            pending_char_for_overlay,
            pending_tasks.clone(),
        );

        // Badge must be added AFTER the drawer overlay so it renders on top.
        widget.add_overlay(claw_indicator.badge());

        // Apply the initial overlay-history setting so the drawer renders
        // in the right mode before any user interaction.
        claw_popover.set_history_mode(settings.maintain_overlay_history);
        claw_indicator.update_settings(settings.hide_character_badge);

        inner.borrow_mut().claw_indicator = Some(claw_indicator.clone());

        // Keep popover height capped to the live pane height.
        let claw_popover_for_resize = claw_popover.clone();
        let height_detector = gtk::DrawingArea::new();
        height_detector.set_can_target(false);
        height_detector.connect_resize(move |_, _w, h| {
            claw_popover_for_resize.update_pane_height(h);
        });
        widget.add_overlay(&height_detector);

        // Ctrl+/ opens the claw drawer (if closed) and focuses the merged
        // msgbar entry. Never hides the drawer: the agent may be waiting
        // for a message and silently dismissing the only input surface
        // would strand the user.
        let shortcut_controller = gtk::ShortcutController::new();
        shortcut_controller.set_propagation_phase(gtk::PropagationPhase::Capture);

        let msg_bar_sc = msg_bar.clone();
        let terminal_sc = terminal.clone();
        let session_status_sc = session_status.clone();
        let is_pinned_sc = is_pinned.clone();
        let is_web_search_sc = is_web_search.clone();
        let agent_name_sc = agent_name.clone();
        let claw_popover_sc = claw_popover.clone();
        let action = gtk::CallbackAction::new(move |_, _| {
            terminal_sc.set_focusable(false);
            // Refresh toggle visuals from live state before presenting.
            msg_bar_sc.update_ui(
                session_status_sc.borrow().clone(),
                is_pinned_sc.get(),
                is_web_search_sc.get(),
            );
            let aname = agent_name_sc.borrow().clone();
            claw_popover_sc.show_input_only(&aname);
            gtk::glib::Propagation::Stop
        });

        let initial_trigger = parse_accel_trigger(&Settings::load().claw_msgbar_shortcut);
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

    pub fn resume_session(&self, resume_id: &str) {
        let resume_id = resume_id.trim().to_string();
        if resume_id.is_empty() {
            return;
        }

        let (pane_id, session_id_ref, claw_sub_ref, event_tx) = {
            let inner = self.inner.borrow();
            (
                inner.id.clone(),
                inner.session_id.clone(),
                inner.claw_subscription.clone(),
                inner.claw_event_tx.clone(),
            )
        };

        // Check if `resume_id` is already an active background or open session
        // BEFORE mutating this pane's Claw state so reattaching/focusing doesn't
        // leave the current empty pane in a half-activated state.
        let existing_claim = boxxy_claw_protocol::characters::CLAIMS_CACHE
            .load()
            .iter()
            .find(|c| c.session_id == resume_id && c.holder_id != pane_id)
            .cloned();
        if let Some(claim) = existing_claim {
            glib::spawn_future_local(async move {
                let agent = crate::get_agent().await;
                if claim.owner_bus_name.is_empty() {
                    if let Ok(detached) = agent.list_detached_sessions_detailed().await
                        && let Some(s) = detached.into_iter().find(|s| {
                            s.session_id == resume_id
                                || s.character_id == claim.character_id
                                || s.pane_id == claim.holder_id
                        })
                    {
                        let _ = crate::TERMINAL_EVENT_BUS.send(crate::TerminalEvent {
                            id: String::new(),
                            kind: crate::TerminalEventKind::RestoreDetached(s.pid, s.cwd),
                        });
                    }
                } else {
                    let _ = agent.request_focus_pane(claim.holder_id).await;
                }
            });
            return;
        }

        if !self.is_claw_active.get() {
            self.is_claw_active.set(true);
            let mut status = self.session_status.borrow().clone();
            if status == AgentStatus::Off {
                status = AgentStatus::Waiting;
                *self.session_status.borrow_mut() = status.clone();
            }
            self.claw_indicator.set_visible(true);
            self.msg_bar.update_ui(
                status.clone(),
                self.is_pinned.get(),
                self.is_web_search.get(),
            );
            let (cb, pane_id) = {
                let inner = self.inner.borrow();
                (inner.callback.clone(), inner.id.clone())
            };
            cb(PaneOutput::ClawStateChanged(
                pane_id,
                true,
                matches!(status, AgentStatus::Sleep),
            ));
        }

        let tx = self.claw_sender.clone();
        let claw_popover = self.claw_popover.clone();
        let agent_name_rc = self.agent_name.clone();
        let msg_bar = self.msg_bar.clone();
        let terminal = self.terminal();

        glib::spawn_future_local(async move {
            let has_session = session_id_ref.lock().await.is_some();
            if !has_session {
                let agent = crate::get_agent().await;
                let char_id =
                    boxxy_claw_widget::msgbar::autocomplete::resolve_session_character_id(
                        &resume_id,
                    )
                    .await
                    .unwrap_or_else(|| {
                        boxxy_claw_protocol::characters::CHARACTER_CACHE
                            .load()
                            .first()
                            .map(|c| c.config.id.clone())
                            .unwrap_or_default()
                    });

                if let Ok((session_id, _sub_guard)) = agent
                    .create_claw_session_with_sender(pane_id, char_id, event_tx.clone())
                    .await
                {
                    if session_id.is_empty() {
                        return;
                    }
                    let resume_guard = agent.subscribe_claw_session(resume_id.clone(), event_tx);
                    if let Ok(mut guard_slot) = claw_sub_ref.lock() {
                        *guard_slot = Some(resume_guard);
                    }
                    *session_id_ref.lock().await = Some(resume_id.clone());
                    let _ = agent
                        .post_claw_message(
                            session_id,
                            ClawMessage::ResumeSession {
                                session_id: resume_id,
                            },
                        )
                        .await;
                }
            } else {
                let agent = crate::get_agent().await;
                let resume_guard = agent.subscribe_claw_session(resume_id.clone(), event_tx);
                if let Ok(mut guard_slot) = claw_sub_ref.lock() {
                    *guard_slot = Some(resume_guard);
                }
                let _ = tx
                    .send(ClawMessage::ResumeSession {
                        session_id: resume_id.clone(),
                    })
                    .await;
                *session_id_ref.lock().await = Some(resume_id);
            }

            glib::timeout_add_local_once(std::time::Duration::from_millis(80), move || {
                terminal.set_focusable(false);
                let aname = agent_name_rc.borrow().clone();
                if !aname.is_empty() {
                    claw_popover.show_input_only(&aname);
                } else {
                    claw_popover.show_chat_only("Agent");
                }
                msg_bar.entry.grab_focus();
            });
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
        let terminal = self.terminal().clone();
        terminal.get_text_snapshot(max_lines, offset_lines).await
    }

    pub fn inject_text(&self, text: &str) {
        self.inner
            .borrow()
            .terminal
            .write_all(text.as_bytes().to_vec());
    }

    pub fn cancel_task(&self, task_id: uuid::Uuid) {
        let _ = self
            .claw_sender
            .send_blocking(ClawMessage::CancelTask { task_id });
    }

    pub fn write_all(&self, data: Vec<u8>) {
        self.inner.borrow().terminal.write_all(data);
    }

    pub fn spawn(&self) {
        let inner_rc = self.inner.clone();
        let id = self.id();
        let claw_sender = self.claw_sender.clone();
        let is_claw_active_cell = self.is_claw_active.clone();
        let session_status_cell = self.session_status.clone();
        let is_pinned_cell = self.is_pinned.clone();
        let is_web_search_cell = self.is_web_search.clone();
        let claw_popover_reattach = self.claw_popover.clone();
        let agent_name_reattach = self.agent_name.clone();
        let is_claw_active_spawn = self.is_claw_active.get();

        glib::spawn_future_local(async move {
            let (reattach_pid, layout_tag) = {
                let mut inner = inner_rc.borrow_mut();
                (inner.reattach_pid.take(), inner.layout_tag.clone())
            };
            let settings = inner_rc
                .borrow()
                .current_settings
                .clone()
                .unwrap_or_else(|| (*Settings::load()).clone());
            let working_dir = inner_rc.borrow().working_dir.clone();

            let agent = crate::get_agent().await;

            // Reattach path: restore buffered output and attach existing PTY master FD.
            if let Some(pid) = reattach_pid {
                match agent.reattach(pid).await {
                    Ok((replay_bytes, master_fd)) => {
                        {
                            let mut inner = inner_rc.borrow_mut();
                            inner.pid = Some(pid);
                        }
                        let _ = agent.set_pane_id(pid, layout_tag).await;

                        // If this detached PTY had a backgrounded ClawSession, rebind it
                        // to this newly created pane so the character badge, conversation
                        // history, and pending reminders are restored immediately.
                        let (event_tx, session_id_ref, claw_sub_ref) = {
                            let inner = inner_rc.borrow();
                            (
                                inner.claw_event_tx.clone(),
                                inner.session_id.clone(),
                                inner.claw_subscription.clone(),
                            )
                        };
                        let mut reattached_claw = false;
                        if let Ok(Some((sid, sub_guard))) = agent
                            .reattach_holder_by_pid_with_sender(pid, id.clone(), event_tx)
                            .await
                        {
                            reattached_claw = true;
                            if let Ok(mut guard_slot) = claw_sub_ref.lock() {
                                *guard_slot = Some(sub_guard);
                            }
                            *session_id_ref.lock().await = Some(sid);
                            is_claw_active_cell.set(true);
                            *session_status_cell.borrow_mut() = AgentStatus::Waiting;
                            {
                                let inner = inner_rc.borrow();
                                if let Some(ind) = &inner.claw_indicator {
                                    ind.set_visible(true);
                                }
                                inner.msg_bar.update_ui(
                                    AgentStatus::Waiting,
                                    is_pinned_cell.get(),
                                    is_web_search_cell.get(),
                                );
                                (inner.callback)(PaneOutput::ClawStateChanged(
                                    id.clone(),
                                    true,
                                    false,
                                ));
                            }
                            let pop = claw_popover_reattach.clone();
                            let aname = agent_name_reattach.clone();
                            glib::timeout_add_local_once(
                                std::time::Duration::from_millis(60),
                                move || {
                                    let name = aname.borrow().clone();
                                    if !name.is_empty() && !pop.is_visible() {
                                        pop.show_chat_only(&name);
                                    }
                                },
                            );
                        }

                        let has_osc_title = replay_bytes
                            .windows(4)
                            .any(|w| w == b"\x1b]0;" || w == b"\x1b]2;");

                        inner_rc
                            .borrow()
                            .terminal
                            .attach_pty_with_replay(master_fd, replay_bytes);

                        if let Ok(cwd) = agent.get_cwd(pid).await
                            && !cwd.is_empty()
                        {
                            inner_rc.borrow_mut().working_dir = Some(cwd.clone());
                            let cb = inner_rc.borrow().callback.clone();
                            cb(PaneOutput::DirectoryChanged(id.clone(), cwd.clone()));

                            if !has_osc_title {
                                let pretty_cwd = if let Ok(home) = std::env::var("HOME")
                                    && !home.is_empty()
                                    && cwd.starts_with(&home)
                                {
                                    format!("~{}", &cwd[home.len()..])
                                } else {
                                    cwd
                                };
                                inner_rc.borrow_mut().last_title = Some(pretty_cwd.clone());
                                cb(PaneOutput::TitleChanged(id.clone(), pretty_cwd));
                            }
                        }

                        let (exit_rx, exit_guard) = agent.subscribe_process_exit(pid).await;
                        let (fg_rx, fg_guard) = agent.subscribe_foreground_process(pid);
                        {
                            let mut inner = inner_rc.borrow_mut();
                            inner.process_subscriptions.push(exit_guard);
                            inner.process_subscriptions.push(fg_guard);
                        }

                        let inner_weak_exit = Rc::downgrade(&inner_rc);
                        let id_for_exit = id.clone();
                        let active_for_exit = is_claw_active_cell.clone();
                        glib::spawn_future_local(async move {
                            if let Ok(exit_code) = exit_rx.recv().await
                                && let Some(inner) = inner_weak_exit.upgrade()
                            {
                                active_for_exit.set(false);
                                let (session_opt, cb) = {
                                    let mut inner_mut = inner.borrow_mut();
                                    (inner_mut.mark_pty_exited(), inner_mut.callback.clone())
                                };
                                let Some(session_arc) = session_opt else {
                                    return;
                                };
                                let pane_id = id_for_exit.clone();
                                glib::spawn_future_local(async move {
                                    let sid = session_arc.lock().await.take();
                                    if sid.is_some() {
                                        let agent = crate::get_agent().await;
                                        let _ = agent.release_holder(pane_id).await;
                                    }
                                });
                                cb(PaneOutput::Exited(id_for_exit, exit_code));
                            }
                        });

                        let inner_weak_fg = Rc::downgrade(&inner_rc);
                        let claw_tx = claw_sender.clone();
                        let id_for_fg = id.clone();
                        glib::spawn_future_local(async move {
                            while let Ok(process_name) = fg_rx.recv().await {
                                let Some(inner) = inner_weak_fg.upgrade() else {
                                    break;
                                };
                                let _ = claw_tx
                                    .send(ClawMessage::ForegroundProcessChanged {
                                        process_name: process_name.clone(),
                                    })
                                    .await;
                                let cb = inner.borrow().callback.clone();
                                cb(PaneOutput::ForegroundProcessChanged(
                                    id_for_fg.clone(),
                                    process_name,
                                ));
                            }
                        });

                        if is_claw_active_spawn || reattached_claw {
                            let _ = agent.set_foreground_tracking(pid, true).await;
                        }
                        return;
                    }
                    Err(e) => {
                        log::warn!(
                            "Failed to reattach to detached pid={}: {:#}; spawning fresh shell",
                            pid,
                            e
                        );
                    }
                }
            }

            let shell = match agent.get_preferred_shell().await {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("get_preferred_shell failed: {:#}, falling back", e);
                    crate::get_host_shell()
                }
            };

            let cmd = if settings.use_custom_command && !settings.custom_command.trim().is_empty() {
                match glib::shell_parse_argv(&settings.custom_command) {
                    Ok(argv) if !argv.is_empty() => argv
                        .into_iter()
                        .map(|os| os.to_string_lossy().into_owned())
                        .collect(),
                    _ => vec![
                        "/bin/sh".into(),
                        "-c".into(),
                        settings.custom_command.clone(),
                    ],
                }
            } else {
                let mut c = vec![shell];
                if settings.login_shell {
                    c.push("--login".into());
                }
                c
            };

            match agent.create_pty().await {
                Ok(master_fd) => {
                    let mut env: Vec<(String, String)> = if is_flatpak() {
                        Vec::new()
                    } else {
                        std::env::vars().collect()
                    };
                    env.push(("TERM".to_string(), "xterm-256color".to_string()));
                    env.push(("COLORTERM".to_string(), "truecolor".to_string()));
                    env.push(("TERM_PROGRAM".to_string(), "Boxxy".to_string()));
                    env.push((
                        "TERM_PROGRAM_VERSION".to_string(),
                        env!("CARGO_PKG_VERSION").to_string(),
                    ));

                    env.push(("VTE_VERSION".to_string(), "7600".to_string()));

                    let (cols, rows) = {
                        let inner = inner_rc.borrow();
                        let c = if inner.n_columns > 0 {
                            inner.n_columns as u16
                        } else {
                            80
                        };
                        let r = if inner.n_rows > 0 {
                            inner.n_rows as u16
                        } else {
                            24
                        };
                        (c, r)
                    };

                    let options = boxxy_agent::ipc::pty::SpawnOptions {
                        cwd: working_dir.unwrap_or_default(),
                        argv: cmd,
                        env,
                        cols,
                        rows,
                        pane_id: id.clone(),
                    };

                    use std::os::unix::io::{AsRawFd, FromRawFd};
                    let master_fd_for_agent = unsafe {
                        let raw = libc::dup(master_fd.as_raw_fd());
                        zbus::OwnedFd::from(std::os::unix::io::OwnedFd::from_raw_fd(raw))
                    };

                    match agent.spawn_process(master_fd_for_agent, options).await {
                        Ok(pid) => {
                            {
                                let mut inner = inner_rc.borrow_mut();
                                inner.pid = Some(pid);
                            }
                            let _ = agent.set_pane_id(pid, layout_tag).await;

                            inner_rc.borrow().terminal.attach_pty(master_fd);

                            let (exit_rx, exit_guard) = agent.subscribe_process_exit(pid).await;
                            let (fg_rx, fg_guard) = agent.subscribe_foreground_process(pid);
                            {
                                let mut inner = inner_rc.borrow_mut();
                                inner.process_subscriptions.push(exit_guard);
                                inner.process_subscriptions.push(fg_guard);
                            }

                            let inner_weak_exit = Rc::downgrade(&inner_rc);
                            let id_for_exit = id.clone();
                            let active_for_exit = is_claw_active_cell.clone();
                            glib::spawn_future_local(async move {
                                if let Ok(exit_code) = exit_rx.recv().await
                                    && let Some(inner) = inner_weak_exit.upgrade()
                                {
                                    active_for_exit.set(false);
                                    let (session_opt, cb) = {
                                        let mut inner_mut = inner.borrow_mut();
                                        (inner_mut.mark_pty_exited(), inner_mut.callback.clone())
                                    };
                                    let Some(session_arc) = session_opt else {
                                        return;
                                    };
                                    let pane_id = id_for_exit.clone();
                                    glib::spawn_future_local(async move {
                                        let sid = session_arc.lock().await.take();
                                        if sid.is_some() {
                                            let agent = crate::get_agent().await;
                                            let _ = agent.release_holder(pane_id).await;
                                        }
                                    });
                                    cb(PaneOutput::Exited(id_for_exit, exit_code));
                                }
                            });

                            let inner_weak_fg = Rc::downgrade(&inner_rc);
                            let claw_tx = claw_sender.clone();
                            let id_for_fg = id.clone();
                            glib::spawn_future_local(async move {
                                while let Ok(process_name) = fg_rx.recv().await {
                                    let Some(inner) = inner_weak_fg.upgrade() else {
                                        break;
                                    };
                                    // Update AI Engine immediately
                                    let _ = claw_tx
                                        .send(ClawMessage::ForegroundProcessChanged {
                                            process_name: process_name.clone(),
                                        })
                                        .await;

                                    // Notify UI for overlays/indicators
                                    let cb = inner.borrow().callback.clone();
                                    cb(PaneOutput::ForegroundProcessChanged(
                                        id_for_fg.clone(),
                                        process_name,
                                    ));
                                }
                            });

                            // CWD tracking is now handled entirely via OSC 7 events
                            // registered in events::setup_terminal_events.

                            // Initialize tracking state based on current claw activity
                            if is_claw_active_spawn {
                                let _ = agent.set_foreground_tracking(pid, true).await;
                            }
                        }
                        Err(e) => log::error!("Failed to spawn process via agent: {:#}", e),
                    }
                }
                Err(e) => log::error!("Failed to create PTY via agent: {:#}", e),
            }
        });
    }

    pub fn copy(&self) {
        self.inner.borrow().terminal.copy_clipboard();
    }

    pub fn inject_keystrokes(&self, keys: &str) {
        log::debug!(
            "Injecting keystrokes into pane {}: {:?}",
            self.inner.borrow().id,
            keys
        );
        let mut unescaped = keys.to_string();
        // Fallback for LLMs that literally output "\u001b" text instead of JSON escapes
        unescaped = unescaped.replace("\\u001b", "\x1b");
        unescaped = unescaped.replace("\\e", "\x1b");
        unescaped = unescaped.replace("\\r", "\r");
        unescaped = unescaped.replace("\\n", "\n");
        unescaped = unescaped.replace("\\t", "\t");
        unescaped = unescaped.replace("\\x03", "\x03");
        unescaped = unescaped.replace("\\x04", "\x04");

        log::debug!("Unescaped bytes: {:?}", unescaped.as_bytes());
        self.inner
            .borrow()
            .terminal
            .write_all(unescaped.into_bytes());
    }

    pub fn paste(&self) {
        self.inner.borrow().terminal.paste_clipboard();
    }

    pub fn grab_focus(&self) {
        // With the msgbar embedded in the drawer, "input is active" maps
        // to "drawer is visible" — the msgbar's own is_active flag stays
        // latched on in embedded mode and can't be used as the signal.
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

    pub fn set_claw_active(&self, active: bool) {
        if self.is_claw_active.get() == active {
            return;
        }

        self.is_claw_active.set(active);

        let pid = self.inner.borrow().pid;
        if let Some(pid) = pid {
            glib::spawn_future_local(async move {
                let agent = crate::get_agent().await;
                let _ = agent.set_foreground_tracking(pid, active).await;
            });
        }

        // Update badge visibility
        self.claw_indicator.set_visible(active);

        let status = if active {
            AgentStatus::Waiting
        } else {
            AgentStatus::Off
        };
        *self.session_status.borrow_mut() = status.clone();

        // Sync MsgBar toggle state
        self.msg_bar
            .update_ui(status, self.is_pinned.get(), self.is_web_search.get());

        // If turning ON, tell the session to Initialize
        let tx = self.claw_sender.clone();
        if active {
            glib::spawn_future_local(async move {
                let _ = tx.send(ClawMessage::Initialize).await;
            });
        } else {
            glib::spawn_future_local(async move {
                let _ = tx.send(ClawMessage::Deactivate).await;
            });
        }
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
        // No-op: clearing is now handled strictly in the sidebar UI
        // to keep the database/resume history intact as requested.
    }

    pub fn open_in_files(&self) {
        let wd = self
            .working_dir()
            .unwrap_or_else(|| std::env::var("HOME").unwrap_or_default());
        let uri = if wd.starts_with("file://") {
            wd.clone()
        } else {
            format!("file://{}", wd)
        };

        let path = wd.strip_prefix("file://").unwrap_or(&wd).to_string();

        gtk4::glib::spawn_future_local(async move {
            if is_flatpak() {
                if let Ok(dir) = std::fs::File::open(&path) {
                    use std::os::fd::AsFd;
                    let req = ashpd::desktop::open_uri::OpenDirectoryRequest::default()
                        .send(&dir.as_fd())
                        .await;
                    if let Err(e) = req {
                        eprintln!("Failed to open directory via ashpd: {}", e);
                    }
                } else {
                    eprintln!("Failed to open directory for ashpd: {}", path);
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
        if let Some(ref p) = inner.current_settings {
            if p.enable_web_search != settings.enable_web_search {
                self.msg_bar
                    .set_web_search_visible(settings.enable_web_search);

                if !settings.enable_web_search {
                    // Force disable if globally disallowed
                    self.is_web_search.set(false);
                    let _ = self
                        .claw_sender
                        .send_blocking(ClawMessage::ToggleWebSearch(false));
                    self.msg_bar.update_ui(
                        self.session_status.borrow().clone(),
                        self.is_pinned.get(),
                        false,
                    );
                } else {
                    // If enabled, send current local state to ensure it's synced
                    let _ = self
                        .claw_sender
                        .send_blocking(ClawMessage::ToggleWebSearch(self.is_web_search.get()));
                }
            }

            needs_font = p.font_name != settings.font_name || p.font_size != settings.font_size;

            needs_padding = p.padding != settings.padding;
            needs_cell = (p.cell_height_scale - settings.cell_height_scale).abs() > 1e-6
                || (p.cell_width_scale - settings.cell_width_scale).abs() > 1e-6;
            needs_cursor_shape = p.cursor_shape != settings.cursor_shape;
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
            .set_history_mode(settings.maintain_overlay_history);

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
            // Msgbar uses the default app font (chat chrome, not shell);
            // apply_font is a no-op but we leave the call here so any
            // future per-msgbar font setting can flow through.
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
            inner
                .terminal
                .set_cursor_blink_mode(settings.cursor_blinking);
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

        // Instead of making the GTK widget transparent (which drops the alpha of the background color too),
        // we tell the VTE renderer to dim its text.
        inner.terminal.set_dimmed(dimmed);
    }
}
