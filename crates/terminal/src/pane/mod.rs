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
use boxxy_claw_ui;
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
    pub claw_message_list: gtk::ListView,

    pub is_claw_active: Rc<Cell<bool>>,
    pub session_status: Rc<RefCell<AgentStatus>>,
    pub is_pinned: Rc<Cell<bool>>,
    is_web_search: Rc<Cell<bool>>,
    agent_name: Rc<RefCell<String>>,
    msg_bar: Rc<MsgBarComponent>,
    pub total_tokens: Rc<Cell<u64>>,
    msgbar_shortcut: gtk::Shortcut,
}

impl Drop for PaneInner {
    fn drop(&mut self) {
        // Release the character assignment in the daemon so the character
        // becomes available for other panes immediately after the tab closes.
        // TODO: when PTY persistence is stable, mirror it for claw sessions —
        // if `pty_persistence` is on, keep the session alive in the daemon
        // (skip end_claw_session) so the conversation can be resumed after the
        // UI restarts, the same way the shell process survives detach.
        let session_arc = self.session_id.clone();
        glib::spawn_future_local(async move {
            let sid = session_arc.lock().await.clone();
            if let Some(session_id) = sid {
                let agent = crate::get_agent().await;
                let _ = agent.end_claw_session(session_id).await;
            }
        });

        let Some(pid) = self.pid else { return };

        // Read the live setting here rather than snapshotting it at
        // spawn time — that way toggling persistence in Preferences
        // takes effect on the very next pane close, not just on panes
        // opened after the toggle.
        let persist = Settings::load().pty_persistence;

        if persist {
            log::info!(
                "PaneInner dropping (pid={}): pty_persistence on → detaching into daemon",
                pid
            );
            glib::spawn_future_local(async move {
                let agent = crate::get_agent().await;
                // Set the flag then detach. Order matters: detach()
                // only keeps the shell alive if persistence is already
                // on at decision time.
                let _ = agent.set_persistence(pid, true).await;
                match agent.detach(pid).await {
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
            // Non-persistent path: SIGTERM the process group so shells
            // and their children (mpv, etc.) are cleaned up even if they
            // ignore SIGHUP.
            log::info!("PaneInner dropping (pid={}): killing process group", pid);
            glib::spawn_future_local(async move {
                let agent = crate::get_agent().await;
                let _ = agent.signal_process_group(pid, 15).await;
            });
        }
    }
}

pub(super) struct PaneInner {
    pub terminal: TerminalWidget,
    pub(super) scrolled_window: gtk::ScrolledWindow,
    pub(super) _provider: gtk::CssProvider,
    pub(super) working_dir: Option<String>,
    pub id: String,
    pub(super) current_settings: Option<Settings>,
    pub(super) hide_scrollbars: bool,
    pub(super) is_dimmed: bool,
    pub(super) size_dismiss_source: Option<gtk::glib::SourceId>,
    pub(super) n_columns: i64,
    pub(super) n_rows: i64,
    pub(super) pid: Option<u32>,
    pub session_id: std::sync::Arc<tokio::sync::Mutex<Option<String>>>,
    pub(super) claw_indicator: Option<ClawIndicator>,
    pub(super) msg_bar: Rc<MsgBarComponent>,
    pub(super) callback: std::sync::Arc<dyn Fn(PaneOutput) + Send + Sync + 'static>,
}

impl std::fmt::Debug for TerminalPaneComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalPaneComponent").finish()
    }
}

impl TerminalPaneComponent {
    /// Explicitly release the Claw session in the daemon. Called from the
    /// tab-close path where we have a synchronous handle on the pane.
    /// Do NOT rely solely on PaneInner::drop for this — GTK closures hold
    /// Rc clones of inner, so drop may fire very late or not at all.
    pub fn end_session(&self) {
        let session_arc = self.inner.borrow().session_id.clone();
        glib::spawn_future_local(async move {
            let sid = session_arc.lock().await.clone();
            if let Some(session_id) = sid {
                let agent = crate::get_agent().await;
                let _ = agent.end_claw_session(session_id).await;
            }
        });
    }

    pub async fn end_session_sync(&self) {
        let sid = self.inner.borrow().session_id.lock().await.clone();
        if let Some(session_id) = sid {
            let agent = crate::get_agent().await;
            let _ = agent.end_claw_session(session_id).await;
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

        let provider = gtk::CssProvider::new();
        #[allow(deprecated)]
        terminal
            .style_context()
            .add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

        let (claw_sender, claw_rx_from_agent) = async_channel::unbounded();
        let (tx_to_agent, rx_from_ui) =
            async_channel::unbounded::<boxxy_claw_protocol::ClawMessage>();
        let session_id_arc: std::sync::Arc<tokio::sync::Mutex<Option<String>>> =
            std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let session_id_for_init = session_id_arc.clone();

        // Expose it to inner so claw_host can update it
        let session_id_for_inner = session_id_arc.clone();
        let claw_sender_clone = claw_sender.clone();
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
            let mut rx_from_ui = rx_from_ui;
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

        let (claw_message_list, claw_list_store) = boxxy_claw_ui::create_claw_message_list();
        let claw_rx = claw_rx_from_agent;
        let claw_sender = tx_to_agent;

        let is_claw_active = Rc::new(Cell::new(false));
        let session_status = Rc::new(RefCell::new(AgentStatus::Off));
        let is_pinned = Rc::new(Cell::new(false));
        let is_web_search = Rc::new(Cell::new(settings.web_search_on_by_default));
        let agent_name = Rc::new(RefCell::new(String::new()));
        let claw_indicator = ClawIndicator::new();
        let total_tokens = Rc::new(Cell::new(0));

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

            let is_claw_active_for_pin = is_claw_active.clone();
            let session_status_for_pin = session_status.clone();
            let is_pinned_for_pin = is_pinned.clone();
            let is_web_search_for_pin = is_web_search.clone();

            let is_claw_active_for_web_search = is_claw_active.clone();
            let session_status_for_web_search = session_status.clone();
            let is_pinned_for_web_search = is_pinned.clone();
            let is_web_search_for_web_search = is_web_search.clone();

            let tx_pin_toggle = claw_sender.clone();
            let tx_web_search_toggle = claw_sender.clone();
            let msg_bar = Rc::new(MsgBarComponent::new(
                move |(query, images)| {
                    if let Some(inner_arc) = inner_weak_for_msg
                        .borrow()
                        .as_ref()
                        .and_then(|w| w.upgrade())
                    {
                        let pane = inner_arc.borrow().terminal.clone();
                        pane.set_focusable(true);
                        pane.grab_focus();

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

                            // Lazily create the session on the first outgoing message,
                            // using whichever character the user pre-selected in the
                            // picker (defaulting to the first registry entry).
                            let pending = pending_char_for_msgbar.borrow().clone();
                            let pane_id = id_msg.clone();
                            let session_id_ref = inner_arc.borrow().session_id.clone();
                            let event_tx = event_sender_for_first_msg.clone();
                            let cwd = inner_arc.borrow().working_dir.clone().unwrap_or_default();

                            glib::spawn_future_local(async move {
                                let snapshot =
                                    pane.get_text_snapshot(100, 0).await.unwrap_or_default();
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

                                if let Ok((session_id, rx_events)) =
                                    agent.create_claw_session(pane_id, char_id).await
                                {
                                    if session_id.is_empty() {
                                        return;
                                    }
                                    *session_id_ref.lock().await = Some(session_id.clone());

                                    tokio::spawn(async move {
                                        while let Ok(event) = rx_events.recv().await {
                                            let _ = event_tx.send(event).await;
                                        }
                                    });

                                    let first_msg = if query.starts_with("/resume ") {
                                        let resume_id =
                                            query["/resume ".len()..].trim().to_string();
                                        if !resume_id.is_empty() {
                                            ClawMessage::ResumeSession {
                                                session_id: resume_id,
                                            }
                                        } else {
                                            ClawMessage::ClawQuery {
                                                query,
                                                snapshot,
                                                cwd,
                                                image_attachments: images,
                                            }
                                        }
                                    } else {
                                        ClawMessage::ClawQuery {
                                            query,
                                            snapshot,
                                            cwd,
                                            image_attachments: images,
                                        }
                                    };

                                    // New conversations need an explicit Initialize so the
                                    // session announces identity and emits the model-info
                                    // SystemMessage. ResumeSession has its own init path.
                                    if matches!(first_msg, ClawMessage::ClawQuery { .. }) {
                                        let _ = agent
                                            .post_claw_message(
                                                session_id.clone(),
                                                ClawMessage::Initialize,
                                            )
                                            .await;
                                    }
                                    let _ = agent.post_claw_message(session_id, first_msg).await;
                                }
                            });

                            return;
                        }

                        // Session already exists — send through the established pump.
                        let tx = tx_msg.clone();
                        let cwd = inner_arc.borrow().working_dir.clone().unwrap_or_default();

                        gtk::glib::spawn_future_local(async move {
                            if let Some(snapshot) = pane.get_text_snapshot(100, 0).await {
                                if query.starts_with("/resume ") {
                                    let session_id = query["/resume ".len()..].trim().to_string();
                                    if !session_id.is_empty() {
                                        let _ = tx
                                            .send(ClawMessage::ResumeSession { session_id })
                                            .await;
                                        return;
                                    }
                                }
                                let _ = tx
                                    .send(ClawMessage::ClawQuery {
                                        query,
                                        snapshot,
                                        cwd,
                                        image_attachments: images,
                                    })
                                    .await;
                            }
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
                        }
                        cb_toggle(PaneOutput::ClawStateChanged(
                            id_toggle.clone(),
                            active,
                            matches!(*session_status_for_active.borrow(), AgentStatus::Sleep),
                        ));
                        let tx = tx_claw_toggle.clone();
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
                _provider: provider,
                working_dir: init.working_dir,
                id: id.clone(),
                current_settings: None,
                hide_scrollbars: false,
                is_dimmed: false,
                size_dismiss_source: None,
                n_columns: 0,
                n_rows: 0,
                pid: None,
                session_id: session_id_for_inner,
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
            claw_list_store,
            msg_bar.clone(),
            callback.clone(),
            init.spawn_intent,
            total_tokens.clone(),
            is_pinned.clone(),
            is_web_search.clone(),
            session_status.clone(),
            agent_name.clone(),
            &claw_indicator,
            pending_char_for_overlay,
        );

        // Badge must be added AFTER the drawer overlay so it renders on top.
        widget.add_overlay(claw_indicator.badge());

        // Apply the initial overlay-history setting so the drawer renders
        // in the right mode before any user interaction.
        claw_popover.set_history_mode(settings.maintain_overlay_history);

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
            claw_message_list,
            is_claw_active,
            session_status,
            is_pinned,
            is_web_search,
            agent_name,
            msg_bar,
            total_tokens,
            msgbar_shortcut,
        }
    }

    pub fn widget(&self) -> &gtk::Overlay {
        &self.widget
    }

    pub fn get_total_tokens(&self) -> u64 {
        self.total_tokens.get()
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

    pub fn claw_history_widget(&self) -> gtk::ListView {
        self.claw_message_list.clone()
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
        let is_claw_active_spawn = self.is_claw_active.get();

        glib::spawn_future_local(async move {
            let settings = inner_rc
                .borrow()
                .current_settings
                .clone()
                .unwrap_or_else(Settings::load);
            let working_dir = inner_rc.borrow().working_dir.clone();

            let agent = crate::get_agent().await;

            let shell = match agent.get_preferred_shell().await {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("get_preferred_shell failed: {:#}, falling back", e);
                    crate::get_host_shell()
                }
            };
            let mut cmd = vec![shell];
            if settings.login_shell {
                cmd.push("--login".into());
            }

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
                        zbus::zvariant::OwnedFd::from(std::os::unix::io::OwnedFd::from_raw_fd(raw))
                    };

                    match agent.spawn_process(master_fd_for_agent, options).await {
                        Ok(pid) => {
                            {
                                let mut inner = inner_rc.borrow_mut();
                                inner.pid = Some(pid);
                            }

                            inner_rc.borrow().terminal.attach_pty(master_fd);

                            let _inner_weak_exit = Rc::downgrade(&inner_rc);
                            if let Ok(mut stream) = agent.proxy().receive_exited().await {
                                let id_for_exit = id.clone();
                                glib::spawn_future_local(async move {
                                    use futures_util::StreamExt;
                                    while let Some(signal) = stream.next().await {
                                        if let Ok(args) = signal.args()
                                            && args.pid == pid
                                        {
                                            if let Some(inner) = _inner_weak_exit.upgrade() {
                                                let cb = inner.borrow().callback.clone();
                                                cb(PaneOutput::Exited(
                                                    id_for_exit.clone(),
                                                    args.exit_code,
                                                ));
                                            }
                                            break;
                                        }
                                    }
                                });
                            }

                            let _inner_weak_fg = Rc::downgrade(&inner_rc);
                            let claw_tx = claw_sender.clone();
                            if let Ok(mut stream) =
                                agent.proxy().receive_foreground_process_changed().await
                            {
                                let id_for_fg = id.clone();
                                glib::spawn_future_local(async move {
                                    use futures_util::StreamExt;
                                    while let Some(signal) = stream.next().await {
                                        if let Ok(args) = signal.args()
                                            && args.pid == pid
                                        {
                                            // Update AI Engine immediately
                                            let _ = claw_tx
                                                .send(ClawMessage::ForegroundProcessChanged {
                                                    process_name: args.process_name.clone(),
                                                })
                                                .await;

                                            // Notify UI for overlays/indicators
                                            if let Some(inner) = _inner_weak_fg.upgrade() {
                                                let cb = inner.borrow().callback.clone();
                                                cb(PaneOutput::ForegroundProcessChanged(
                                                    id_for_fg.clone(),
                                                    args.process_name,
                                                ));
                                            }
                                        }
                                    }
                                });
                            }

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
        let tx = self.claw_sender.clone();
        glib::spawn_future_local(async move {
            let _ = tx.send(ClawMessage::SoftClearHistory).await;
        });
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

        inner.current_settings = Some(settings);
        if let Some(ind) = &inner.claw_indicator {
            ind.update_settings();
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
