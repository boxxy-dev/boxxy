use boxxy_preferences::Settings;
use boxxy_themes::Palette;
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gdk, gio, glib, pango};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::search_bar::SearchBarComponent;
use boxxy_msgbar::MsgBarComponent;
use boxxy_vte::terminal::TerminalWidget;

use crate::is_flatpak;
use crate::{PaneInit, PaneOutput};

use crate::claw_indicator::ClawIndicator;
use crate::overlay::{OverlayMode, TerminalOverlay};

mod app_menu;
mod badge;
mod claw;
mod events;
mod gestures;
mod preview;
mod ui;

use badge::AgentBadge;

pub type PendingDiagnosis = Rc<RefCell<Option<(String, crate::TerminalProposal)>>>;

#[derive(Clone)]
pub struct TerminalPaneComponent {
    widget: gtk::Overlay,
    inner: Rc<RefCell<PaneInner>>,
    _search_bar: Rc<SearchBarComponent>,
    claw_popover: TerminalOverlay,
    claw_indicator: ClawIndicator,
    agent_badge: AgentBadge,
    pending_proactive_diagnosis: PendingDiagnosis,
    claw_sender: async_channel::Sender<boxxy_claw::engine::ClawMessage>,
    claw_message_list: gtk::ListBox,
    is_claw_active: Rc<Cell<bool>>,
    msg_bar: Rc<MsgBarComponent>,
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
    pub(super) agent_badge: AgentBadge,
    pub(super) callback: std::sync::Arc<dyn Fn(PaneOutput) + Send + Sync + 'static>,
}

impl std::fmt::Debug for TerminalPaneComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalPaneComponent").finish()
    }
}

impl TerminalPaneComponent {
    pub fn new<F: Fn(PaneOutput) + Send + Sync + 'static>(init: PaneInit, callback: F) -> Self {
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

        let (claw_session, claw_sender, claw_rx) = boxxy_claw::engine::ClawSession::new(id.clone());

        gtk::glib::spawn_future_local(async move {
            let agent = crate::get_agent().await;
            claw_session.start(agent.claw_proxy().clone());
        });

        let claw_message_list = boxxy_claw::ui::create_claw_message_list();

        let is_claw_active = Rc::new(Cell::new(false));
        let agent_badge = AgentBadge::new(&widget);

        let inner = Rc::new(RefCell::new(PaneInner {
            terminal: terminal.clone(),
            scrolled_window: scrolled_window.clone(),
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
            agent_badge: agent_badge.clone(),
            callback: callback.clone(),
        }));

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
        let msg_bar = {
            let tx_msg = claw_sender.clone();
            let inner_rc_for_msg = Rc::downgrade(&inner);
            let inner_rc_for_cancel = Rc::downgrade(&inner);
            let is_claw_active_for_msg = is_claw_active.clone();
            Rc::new(MsgBarComponent::new(
                move |(query, images)| {
                    if let Some(inner_arc) = inner_rc_for_msg.upgrade() {
                        let pane = inner_arc.borrow().terminal.clone();
                        pane.set_focusable(true);
                        pane.grab_focus();

                        if !is_claw_active_for_msg.get() {
                            return;
                        }

                        let tx = tx_msg.clone();
                        let cwd = inner_arc.borrow().working_dir.clone().unwrap_or_default();

                        gtk::glib::spawn_future_local(async move {
                            if let Some(snapshot) = pane.get_text_snapshot(100, 0).await {
                                let _ = tx
                                    .send(boxxy_claw::engine::ClawMessage::ClawQuery {
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
                    if let Some(inner_arc) = inner_rc_for_cancel.upgrade() {
                        let pane = inner_arc.borrow().terminal.clone();
                        pane.set_focusable(true);
                        pane.grab_focus();
                    }
                },
            ))
        };
        widget.add_overlay(&msg_bar.widget);

        let (claw_popover, claw_indicator, pending_proactive_diagnosis) = claw::setup_claw(
            &widget,
            &inner,
            id.clone(),
            claw_sender.clone(),
            claw_rx,
            claw_message_list.clone(),
            callback.clone(),
            init.spawn_intent,
        );

        // Focus toggle hotkey (Ctrl + `) and MsgBar hotkey (Ctrl + /)
        let focus_toggle_handler = gtk::EventControllerKey::new();
        focus_toggle_handler.set_propagation_phase(gtk::PropagationPhase::Capture);
        let terminal_clone = terminal.clone();
        let popover_clone = claw_popover.clone();
        let msg_bar_clone = msg_bar.clone();
        let is_claw_active_for_focus = is_claw_active.clone();
        focus_toggle_handler.connect_key_pressed(move |_, keyval, _keycode, state| {
            let is_ctrl = state.contains(gtk::gdk::ModifierType::CONTROL_MASK);

            if is_ctrl && keyval == gtk::gdk::Key::slash {
                if !is_claw_active_for_focus.get() {
                    return gtk::glib::Propagation::Proceed;
                }
                if let Some(rect) = terminal_clone.get_cursor_rect() {
                    terminal_clone.set_focusable(false); // Prevent clicks from stealing focus
                    msg_bar_clone.show_at_y(rect.y() as i32, rect.height() as i32);
                    // Use a micro-delay to let GTK process the visibility change
                    // before attempting to steal focus from the TerminalWidget.
                    let entry_clone = msg_bar_clone.entry.clone();
                    gtk::glib::spawn_future_local(async move {
                        entry_clone.grab_focus();
                    });
                }
                return gtk::glib::Propagation::Stop;
            }

            let is_grave = keyval == gtk::gdk::Key::dead_grave || keyval == gtk::gdk::Key::grave;

            if is_ctrl && is_grave
                && popover_clone.is_visible()
                    && let Some(root) = popover_clone.widget().root() {
                        let is_popover_focused = if let Some(focus) = root.focus() {
                            let focus_widget = focus.downcast_ref::<gtk::Widget>().unwrap();
                            focus_widget == popover_clone.widget().upcast_ref::<gtk::Widget>()
                                || focus_widget.is_ancestor(popover_clone.widget())
                        } else {
                            false
                        };

                        if is_popover_focused {
                            terminal_clone.grab_focus();
                        } else {
                            popover_clone.grab_reply_focus();
                        }
                        return gtk::glib::Propagation::Stop;
                    }
            gtk::glib::Propagation::Proceed
        });
        widget.add_controller(focus_toggle_handler);

        Self {
            widget,
            inner,
            _search_bar: search_bar_rc,
            claw_popover,
            claw_indicator,
            agent_badge,
            pending_proactive_diagnosis,
            claw_sender,
            claw_message_list,
            is_claw_active,
            msg_bar,
        }
    }

    pub fn widget(&self) -> &gtk::Overlay {
        &self.widget
    }

    pub fn claw_history_widget(&self) -> gtk::ListBox {
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
            .show(OverlayMode::Claw, title, message, proposal);
    }

    pub fn show_bookmark_proposal(
        &self,
        title: &str,
        message: &str,
        proposal: crate::TerminalProposal,
    ) {
        self.claw_popover
            .show(OverlayMode::Bookmark, title, message, proposal);
    }

    pub fn hide_claw_popover(&self) {
        self.claw_popover.hide();
    }

    pub fn show_lazy_error(&self) {
        self.claw_indicator.show_lazy_error();
    }

    pub fn show_diagnosis_ready(&self, diagnosis: String, proposal: crate::TerminalProposal) {
        *self.pending_proactive_diagnosis.borrow_mut() = Some((diagnosis, proposal));
        self.claw_indicator.show_diagnosis_ready();
    }

    pub fn set_agent_thinking(&self, thinking: bool) {
        if thinking {
            self.claw_indicator.show_thinking();
        } else {
            self.claw_indicator.hide();
        }
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

                    let options = boxxy_agent::ipc::SpawnOptions {
                        cwd: working_dir.unwrap_or_default(),
                        argv: cmd,
                        env,
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
                                                .send(
                                                    boxxy_claw::engine::ClawMessage::ForegroundProcessChanged {
                                                        process_name: args.process_name.clone(),
                                                    },
                                                )
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
        self.inner.borrow().terminal.grab_focus();
    }

    pub fn resize(&self) {}

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
        self.agent_badge.set_visible(active);

        // If turning ON, tell the session to Initialize
        let tx = self.claw_sender.clone();
        if active {
            glib::spawn_future_local(async move {
                let _ = tx.send(boxxy_claw::engine::ClawMessage::Initialize).await;
            });
        } else {
            glib::spawn_future_local(async move {
                let _ = tx.send(boxxy_claw::engine::ClawMessage::Deactivate).await;
            });
        }
    }

    pub fn update_diagnosis_mode(&self, mode: &boxxy_preferences::config::ClawAutoDiagnosisMode) {
        let _ = self
            .claw_sender
            .send_blocking(boxxy_claw::engine::ClawMessage::UpdateDiagnosisMode(*mode));
    }

    pub fn reload_claw(&self) {
        let tx = self.claw_sender.clone();
        gtk4::glib::spawn_future_local(async move {
            let _ = tx.send(boxxy_claw::engine::ClawMessage::Reload).await;
        });
    }

    pub fn open_in_files(&self) {
        let wd = self
            .working_dir()
            .unwrap_or_else(|| std::env::var("HOME").unwrap_or_default());
        let uri = if wd.starts_with("file://") {
            wd
        } else {
            format!("file://{}", wd)
        };
        let _ = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>);
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
        inner.agent_badge.update_settings();
    }

    pub fn set_dimmed(&self, dimmed: bool) {
        let mut inner = self.inner.borrow_mut();
        inner.is_dimmed = dimmed;

        // Instead of making the GTK widget transparent (which drops the alpha of the background color too),
        // we tell the VTE renderer to dim its text.
        inner.terminal.set_dimmed(dimmed);
    }
}
