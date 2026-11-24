//! Ephemeral `/btw` side-question button and `AdwDialog` controller (`OpenCode`-aligned).
//!
//! Flow:
//! 1. Clicking the action-row **`BTW`** button (or pressing the shortcut / bare `/btw`) opens
//!    `BtwDialog` on its **Ask** page.
//! 2. Submitting (`Enter` or **Ask**) **closes the dialog immediately** so the user keeps an
//!    unobstructed view of the terminal/drawer while the button spins (`BTW…`).
//! 3. Clicking **`BTW…`** while in-flight peeks at live reasoning & streaming Markdown (with an
//!    explicit **Cancel Question** button, while `Esc` / **Hide** returns to background mode).
//! 4. When `BtwComplete` arrives, the Answer dialog pops open automatically if this pane still
//!    owns focus (and the user isn't mid-typing in `msg_bar`), or highlights **`● BTW Ready`**
//!    without stealing keyboard focus across tabs/splits.

use crate::claw_host::ClawHost;
use crate::msgbar::{MsgBarComponent, autocomplete::FileCompletionProvider, file_index};
use boxxy_claw_protocol::ClawMessage;
use boxxy_claw_ui::{
    load_character_avatar_texture, prettify_character_name, resolve_character_display_name,
};
use boxxy_viewer::StructuredViewer;
use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::rc::Rc;
use uuid::Uuid;

#[derive(Debug, Clone)]
struct BtwExchange {
    question: String,
    answer: String,
    error: Option<String>,
    agent_name: String,
    character_id: String,
    model_label: String,
    duration_ms: u64,
    prior_exchange: Option<(String, String)>,
}

#[derive(Debug, Clone)]
enum BtwStatus {
    Idle,
    Loading {
        request_id: Uuid,
        question: String,
        prior_exchange: Option<(String, String)>,
        active_status: String,
        streamed_text: String,
    },
    UnreadReady,
}

struct OpenBtwDialog {
    dialog: adw::Dialog,
    view_stack: gtk::Stack,
    header_avatar: adw::Avatar,
    header_character_label: gtk::Label,
    header_meta_label: gtk::Label,
    last_answer_btn: gtk::Button,
    followup_box: gtk::Box,
    followup_label: gtk::Label,
    ask_entry: gtk::TextView,
    question_display_label: gtk::Label,
    loading_status_box: gtk::Box,
    loading_spinner_svg: gtk::Svg,
    loading_spinner_playing: Rc<Cell<bool>>,
    loading_status_label: gtk::Label,
    viewer: StructuredViewer,
    abort_btw_btn: gtk::Button,
    followup_btn: gtk::Button,
    new_question_btn: gtk::Button,
    retry_btn: gtk::Button,
    copy_answer_btn: gtk::Button,
    copy_btn_icon: gtk::Image,
    copy_btn_label: gtk::Label,
    done_btn: gtk::Button,
    followup_context: Rc<RefCell<Option<(String, String)>>>,
    copy_reset_timer: Rc<RefCell<Option<gtk::glib::SourceId>>>,
    _autocomplete: Rc<boxxy_core_widgets::autocomplete::AutocompleteController>,
}

struct BtwInner {
    anchor_revealer: gtk::Revealer,
    btn: gtk::Button,
    btn_spinner_img: gtk::Image,
    btn_spinner_svg: gtk::Svg,
    btn_spinner_playing: Rc<Cell<bool>>,
    btn_dot_lbl: gtk::Label,
    btn_text_lbl: gtk::Label,
    status: RefCell<BtwStatus>,
    last_exchange: RefCell<Option<BtwExchange>>,
    open_dialog: RefCell<Option<Rc<OpenBtwDialog>>>,
    host: Rc<dyn ClawHost>,
    msg_bar: Rc<MsgBarComponent>,
    active_agent: Rc<RefCell<String>>,
    selected_character: Rc<RefCell<String>>,
    on_state_changed: RefCell<Option<Rc<dyn Fn()>>>,
}

#[derive(Clone)]
pub struct BtwController {
    inner: Rc<BtwInner>,
}

fn is_spinner_surface_animating(widget: &gtk::Image) -> bool {
    if !widget.is_mapped() {
        return false;
    }
    if let Some(win) = widget
        .root()
        .and_then(|r| r.downcast::<gtk::Window>().ok())
    {
        return !win.is_suspended();
    }
    true
}

fn build_managed_gpa_spinner(pixel_size: i32) -> (gtk::Image, gtk::Svg, Rc<Cell<bool>>) {
    let svg = gtk::Svg::from_resource("/dev/boxxy/BoxxyTerminal/icons/boxxy-spinner.gpa");
    svg.pause();
    let playing = Rc::new(Cell::new(false));
    let img = gtk::Image::builder()
        .paintable(&svg)
        .pixel_size(pixel_size)
        .visible(false)
        .build();
    img.add_css_class("claw-spinner");

    let win_slot: Rc<RefCell<Option<(gtk::Window, gtk::glib::SignalHandlerId)>>> =
        Rc::new(RefCell::new(None));

    img.connect_realize({
        let svg = svg.clone();
        let playing = playing.clone();
        let win_slot = win_slot.clone();
        move |widget| {
            if let Some((old_win, id)) = win_slot.borrow_mut().take() {
                old_win.disconnect(id);
            }
            if let Some(clock) = widget.frame_clock() {
                svg.set_frame_clock(&clock);
            }
            if let Some(win) = widget
                .root()
                .and_then(|r| r.downcast::<gtk::Window>().ok())
            {
                let img_weak = widget.downgrade();
                let svg_w = svg.clone();
                let playing_w = playing.clone();
                let id = win.connect_suspended_notify(move |_| {
                    if let Some(w) = img_weak.upgrade() {
                        if playing_w.get() && is_spinner_surface_animating(&w) {
                            svg_w.play();
                        } else {
                            svg_w.pause();
                        }
                    }
                });
                *win_slot.borrow_mut() = Some((win, id));
            }
        }
    });
    img.connect_unrealize({
        let svg = svg.clone();
        move |_| {
            svg.pause();
            if let Some((win, id)) = win_slot.borrow_mut().take() {
                win.disconnect(id);
            }
        }
    });
    img.connect_map({
        let svg = svg.clone();
        let playing = playing.clone();
        move |widget| {
            if let Some(clock) = widget.frame_clock() {
                svg.set_frame_clock(&clock);
            }
            if playing.get() && is_spinner_surface_animating(widget) {
                svg.play();
            } else {
                svg.pause();
            }
        }
    });
    img.connect_unmap({
        let svg = svg.clone();
        move |_| {
            svg.pause();
        }
    });

    (img, svg, playing)
}

fn set_spinner_active(
    img: &gtk::Image,
    svg: &gtk::Svg,
    playing: &Cell<bool>,
    active: bool,
) {
    playing.set(active);
    img.set_visible(active);
    if active && is_spinner_surface_animating(img) {
        if let Some(clock) = img.frame_clock() {
            svg.set_frame_clock(&clock);
        }
        svg.play();
    } else {
        svg.pause();
    }
}

fn format_duration_meta(model_label: &str, duration_ms: u64) -> String {
    let mut parts: Vec<String> = Vec::new();
    let trimmed_model = model_label.trim();
    if !trimmed_model.is_empty() {
        parts.push(trimmed_model.to_string());
    }
    if duration_ms > 0 {
        if duration_ms < 10_000 {
            let whole = duration_ms / 1000;
            let tenths = (duration_ms % 1000) / 100;
            parts.push(format!("{whole}.{tenths}s"));
        } else {
            let secs = (duration_ms + 500) / 1000;
            if secs < 60 {
                parts.push(format!("{secs}s"));
            } else {
                parts.push(format!("{}m {}s", secs / 60, secs % 60));
            }
        }
    }
    if parts.is_empty() {
        "Side question".to_string()
    } else {
        parts.join(" · ")
    }
}

impl BtwController {
    #[must_use]
    pub fn new(
        anchor_revealer: gtk::Revealer,
        host: Rc<dyn ClawHost>,
        msg_bar: Rc<MsgBarComponent>,
        active_agent: Rc<RefCell<String>>,
        selected_character: Rc<RefCell<String>>,
    ) -> Self {
        let (btn_spinner_img, btn_spinner_svg, btn_spinner_playing) = build_managed_gpa_spinner(16);

        let btn_dot_lbl = gtk::Label::builder()
            .label("●")
            .visible(false)
            .valign(gtk::Align::Center)
            .build();

        let btn_text_lbl = gtk::Label::builder()
            .label("BTW")
            .valign(gtk::Align::Center)
            .css_classes(["claw-btw-btn-label"])
            .build();

        let btn_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        btn_box.set_valign(gtk::Align::Center);
        btn_box.append(&btn_spinner_img);
        btn_box.append(&btn_dot_lbl);
        btn_box.append(&btn_text_lbl);

        let btn = gtk::Button::builder()
            .child(&btn_box)
            .valign(gtk::Align::Center)
            .tooltip_text("Ask a quick side question without interrupting (/btw)")
            .css_classes(["claw-btw-btn"])
            .build();

        let ctrl = Self {
            inner: Rc::new(BtwInner {
                anchor_revealer,
                btn: btn.clone(),
                btn_spinner_img,
                btn_spinner_svg,
                btn_spinner_playing,
                btn_dot_lbl,
                btn_text_lbl,
                status: RefCell::new(BtwStatus::Idle),
                last_exchange: RefCell::new(None),
                open_dialog: RefCell::new(None),
                host,
                msg_bar,
                active_agent,
                selected_character,
                on_state_changed: RefCell::new(None),
            }),
        };

        let ctrl_click = ctrl.clone();
        btn.connect_clicked(move |_| {
            ctrl_click.handle_button_clicked();
        });

        ctrl
    }

    #[must_use]
    pub fn widget(&self) -> &gtk::Button {
        &self.inner.btn
    }

    pub fn connect_state_changed<F: Fn() + 'static>(&self, f: F) {
        *self.inner.on_state_changed.borrow_mut() = Some(Rc::new(f));
    }

    /// True when a `/btw` request is currently loading or holding an unread answer,
    /// keeping the `BTW` pill visible in the action bar even after the main turn finishes.
    #[must_use]
    pub fn is_active_or_unread(&self) -> bool {
        !matches!(*self.inner.status.borrow(), BtwStatus::Idle)
    }

    /// Clears cached `/btw` state when the session is cleared, exited, or replaced via `/resume`.
    pub fn reset_session(&self) {
        if let BtwStatus::Loading { request_id, .. } = *self.inner.status.borrow() {
            self.inner.host.send_claw(ClawMessage::CancelBtw {
                request_id: Some(request_id),
            });
        }
        *self.inner.last_exchange.borrow_mut() = None;
        if let Some(open) = self.inner.open_dialog.borrow_mut().take() {
            open.dialog.close();
        }
        self.set_status(&BtwStatus::Idle);
    }

    /// Opens the `/btw` dialog (showing an unread answer or in-flight peek if one is active,
    /// otherwise opening the Ask prompt).
    pub fn handle_button_clicked(&self) {
        let current_status = self.inner.status.borrow().clone();
        match current_status {
            BtwStatus::UnreadReady => {
                self.set_status(&BtwStatus::Idle);
                if let Some(ex) = self.inner.last_exchange.borrow().clone() {
                    self.present_answer_view(&ex);
                } else {
                    self.present_ask_view(None);
                }
            }
            BtwStatus::Loading {
                question,
                active_status,
                streamed_text,
                ..
            } => {
                self.present_loading_peek(&question, &active_status, &streamed_text);
            }
            BtwStatus::Idle => {
                self.present_ask_view(None);
            }
        }
    }

    /// Opens the `/btw` Ask prompt directly (invoked by bare `/btw` or shortcut).
    pub fn open_ask_dialog(&self) {
        if matches!(
            *self.inner.status.borrow(),
            BtwStatus::UnreadReady | BtwStatus::Loading { .. }
        ) {
            self.handle_button_clicked();
        } else {
            self.present_ask_view(None);
        }
    }

    /// Dispatches a `/btw` side question in the background and switches the button to `BTW…`.
    pub fn submit_question(&self, raw_question: &str, prior_exchange: Option<(String, String)>) {
        let question = raw_question.trim().to_string();
        if question.is_empty() {
            return;
        }

        // Cancel any earlier in-flight `/btw` request before starting the new one.
        if let BtwStatus::Loading {
            request_id: old_id, ..
        } = *self.inner.status.borrow()
        {
            self.inner.host.send_claw(ClawMessage::CancelBtw {
                request_id: Some(old_id),
            });
        }

        let request_id = Uuid::new_v4();
        self.set_status(&BtwStatus::Loading {
            request_id,
            question: question.clone(),
            prior_exchange: prior_exchange.clone(),
            active_status: "Thinking…".to_string(),
            streamed_text: String::new(),
        });

        let host = self.inner.host.clone();
        let cwd = host.working_dir().unwrap_or_default();
        gtk::glib::spawn_future_local(async move {
            let snapshot = host.snapshot(100, 0).await.unwrap_or_default();
            host.send_claw(ClawMessage::BtwQuery {
                request_id,
                question,
                prior_exchange,
                snapshot,
                cwd,
            });
        });
    }

    pub fn on_progress(
        &self,
        request_id: Uuid,
        agent_name: &str,
        character_id: &str,
        active_status: Option<&str>,
        text_delta: Option<&str>,
    ) {
        let mut status_borrow = self.inner.status.borrow_mut();
        let BtwStatus::Loading {
            request_id: current_id,
            ref question,
            active_status: ref mut cur_status,
            ref mut streamed_text,
            ..
        } = *status_borrow
        else {
            return;
        };
        if current_id != request_id {
            return;
        }

        if let Some(st) = active_status.filter(|s| !s.is_empty()) {
            cur_status.clear();
            cur_status.push_str(st);
        }
        if let Some(delta) = text_delta {
            streamed_text.clear();
            streamed_text.push_str(delta);
        }

        let mut tip = format!("Asking: {}", single_line_preview(question, 72));
        if !cur_status.is_empty() {
            let _ = write!(tip, "\n{cur_status}");
        }
        self.inner.btn.set_tooltip_text(Some(&tip));

        let status_copy = cur_status.clone();
        let text_copy = streamed_text.clone();
        drop(status_borrow);

        if let Some(open) = self.inner.open_dialog.borrow().clone()
            && open.view_stack.visible_child_name().as_deref() == Some("answer")
        {
            Self::apply_header_identity(&open, agent_name, character_id, "Working…");
            open.loading_status_label.set_label(&status_copy);
            if !text_copy.is_empty() {
                open.viewer.set_streaming_content(&text_copy, false);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn on_complete(
        &self,
        request_id: Uuid,
        agent_name: &str,
        character_id: String,
        question: String,
        answer: String,
        model_label: String,
        duration_ms: u64,
        error: Option<String>,
        pane_owns_focus: bool,
        drawer_visible: bool,
    ) {
        let prior_exchange = {
            let status_borrow = self.inner.status.borrow();
            match &*status_borrow {
                BtwStatus::Loading {
                    request_id: current_id,
                    prior_exchange,
                    ..
                } if *current_id == request_id => prior_exchange.clone(),
                _ => return,
            }
        };

        let exchange = BtwExchange {
            question,
            answer,
            error,
            agent_name: agent_name.to_string(),
            character_id,
            model_label,
            duration_ms,
            prior_exchange,
        };
        *self.inner.last_exchange.borrow_mut() = Some(exchange.clone());

        // Case 1: Dialog is already open on screen (e.g. user clicked `BTW…` to peek) -> update in-place.
        if let Some(open) = self.inner.open_dialog.borrow().clone() {
            self.set_status(&BtwStatus::Idle);
            Self::populate_answer_dialog(&open, &exchange);
            return;
        }

        // Case 2: User is still on this pane and not actively mid-sentence in `msg_bar` -> auto-present!
        let user_typing_in_msgbar = self.inner.msg_bar.entry.has_focus()
            && self.inner.msg_bar.entry.is_sensitive()
            && self.inner.msg_bar.entry.buffer().char_count() > 0;
        let window_active = self
            .inner
            .anchor_revealer
            .root()
            .and_then(|r| r.downcast::<gtk::Window>().ok())
            .is_some_and(|w| w.is_active());
        let another_dialog_open = self
            .inner
            .anchor_revealer
            .root()
            .and_then(|r| r.downcast::<adw::ApplicationWindow>().ok())
            .and_then(|w| w.visible_dialog())
            .is_some();

        if window_active && pane_owns_focus && !user_typing_in_msgbar && !another_dialog_open {
            self.set_status(&BtwStatus::Idle);
            self.present_answer_view(&exchange);
        } else {
            self.set_status(&BtwStatus::UnreadReady);
            if !drawer_visible {
                let who = if agent_name.trim().is_empty() {
                    "Claw"
                } else {
                    agent_name.trim()
                };
                self.inner.host.notify(format!(
                    "{who} answered your /btw question — open Claw (Ctrl+/) to view"
                ));
            }
        }
    }

    fn set_status(&self, next: &BtwStatus) {
        *self.inner.status.borrow_mut() = next.clone();
        self.inner
            .msg_bar
            .btw_busy
            .set(matches!(next, BtwStatus::Loading { .. }));
        self.inner.btn.remove_css_class("is-loading");
        self.inner.btn.remove_css_class("is-ready");

        match next {
            BtwStatus::Idle => {
                set_spinner_active(
                    &self.inner.btn_spinner_img,
                    &self.inner.btn_spinner_svg,
                    &self.inner.btn_spinner_playing,
                    false,
                );
                self.inner.btn_dot_lbl.set_visible(false);
                self.inner.btn_text_lbl.set_label("BTW");
                self.inner
                    .btn
                    .set_tooltip_text(Some("Ask a quick side question without interrupting (/btw)"));
            }
            BtwStatus::Loading {
                question,
                active_status,
                ..
            } => {
                self.inner.btn.add_css_class("is-loading");
                set_spinner_active(
                    &self.inner.btn_spinner_img,
                    &self.inner.btn_spinner_svg,
                    &self.inner.btn_spinner_playing,
                    true,
                );
                self.inner.btn_dot_lbl.set_visible(false);
                self.inner.btn_text_lbl.set_label("BTW…");
                let tip = format!(
                    "Asking: {}\n{} (Click to peek or cancel)",
                    single_line_preview(question, 72),
                    active_status
                );
                self.inner.btn.set_tooltip_text(Some(&tip));
            }
            BtwStatus::UnreadReady => {
                self.inner.btn.add_css_class("is-ready");
                set_spinner_active(
                    &self.inner.btn_spinner_img,
                    &self.inner.btn_spinner_svg,
                    &self.inner.btn_spinner_playing,
                    false,
                );
                self.inner.btn_dot_lbl.set_visible(true);
                self.inner.btn_text_lbl.set_label("BTW Ready");
                self.inner
                    .btn
                    .set_tooltip_text(Some("Side question answered — click to read"));
            }
        }

        let cb = self.inner.on_state_changed.borrow().clone();
        if let Some(cb) = cb {
            cb();
        }
    }

    fn resolve_current_speaker(&self) -> (String, String) {
        let cid = self
            .inner
            .msg_bar
            .character_id
            .borrow()
            .clone()
            .unwrap_or_else(|| self.inner.selected_character.borrow().clone());
        let name = self.inner.active_agent.borrow().clone();
        (name, cid)
    }

    fn apply_header_identity(
        open: &OpenBtwDialog,
        agent_name: &str,
        character_id: &str,
        meta_text: &str,
    ) {
        let catalog = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
        let fallback_names = [agent_name];
        let info_opt =
            boxxy_claw_protocol::characters::resolve_character(&catalog, character_id, &fallback_names);

        let display_name = if boxxy_claw_protocol::characters::is_incognito_character(character_id) {
            "Inco Gnito".to_string()
        } else if let Some(info) = info_opt {
            resolve_character_display_name(info)
        } else if !agent_name.trim().is_empty() {
            prettify_character_name(agent_name)
        } else {
            "Boxxy-Claw".to_string()
        };

        open.header_character_label.set_label(&display_name);
        open.header_avatar.set_text(Some(&display_name));
        open.header_meta_label.set_label(meta_text);

        if let Some(info) = info_opt
            && let Some(tex) = load_character_avatar_texture(info)
        {
            open.header_avatar.set_custom_image(Some(&tex));
        } else {
            open.header_avatar.set_custom_image(gtk::gdk::Paintable::NONE);
        }
    }

    #[allow(clippy::too_many_lines)]
    fn ensure_dialog(&self) -> Option<Rc<OpenBtwDialog>> {
        if let Some(existing) = self.inner.open_dialog.borrow().clone() {
            return Some(existing);
        }

        let root_widget = self.inner.anchor_revealer.root()?;
        adw::Avatar::ensure_type();
        let builder =
            gtk::Builder::from_resource("/dev/boxxy/BoxxyTerminal/ui/claw_btw_dialog.ui");

        let dialog: adw::Dialog = builder.object("btw_dialog")?;
        let view_stack: gtk::Stack = builder.object("view_stack")?;
        let header_avatar: adw::Avatar = builder.object("header_avatar")?;
        let header_character_label: gtk::Label = builder.object("header_character_label")?;
        let header_meta_label: gtk::Label = builder.object("header_meta_label")?;
        let last_answer_btn: gtk::Button = builder.object("last_answer_btn")?;
        let followup_box: gtk::Box = builder.object("followup_box")?;
        let followup_label: gtk::Label = builder.object("followup_label")?;
        let clear_followup_btn: gtk::Button = builder.object("clear_followup_btn")?;
        let ask_overlay: gtk::Overlay = builder.object("ask_overlay")?;
        let ask_entry: gtk::TextView = builder.object("ask_entry")?;
        let ask_cancel_btn: gtk::Button = builder.object("ask_cancel_btn")?;
        let ask_submit_btn: gtk::Button = builder.object("ask_submit_btn")?;
        let question_display_label: gtk::Label = builder.object("question_display_label")?;
        let loading_status_box: gtk::Box = builder.object("loading_status_box")?;
        let loading_spinner_slot: gtk::Box = builder.object("loading_spinner_slot")?;
        let loading_status_label: gtk::Label = builder.object("loading_status_label")?;
        let answer_viewer_slot: gtk::Box = builder.object("answer_viewer_slot")?;
        let abort_btw_btn: gtk::Button = builder.object("abort_btw_btn")?;
        let followup_btn: gtk::Button = builder.object("followup_btn")?;
        let new_question_btn: gtk::Button = builder.object("new_question_btn")?;
        let retry_btn: gtk::Button = builder.object("retry_btn")?;
        let copy_answer_btn: gtk::Button = builder.object("copy_answer_btn")?;
        let copy_btn_icon: gtk::Image = builder.object("copy_btn_icon")?;
        let copy_btn_label: gtk::Label = builder.object("copy_btn_label")?;
        let done_btn: gtk::Button = builder.object("done_btn")?;

        let (loading_img, loading_spinner_svg, loading_spinner_playing) =
            build_managed_gpa_spinner(18);
        loading_spinner_slot.append(&loading_img);
        loading_img.set_visible(true);

        let placeholder_lbl = gtk::Label::builder()
            .label("Ask anything about this session… (@file to attach context)")
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Start)
            .xalign(0.0)
            .margin_top(12)
            .margin_start(14)
            .margin_end(14)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .can_target(false)
            .can_focus(false)
            .css_classes(["dim-label"])
            .build();
        ask_overlay.add_overlay(&placeholder_lbl);

        let submit_for_buf = ask_submit_btn.clone();
        let placeholder_for_buf = placeholder_lbl;
        ask_entry.buffer().connect_changed(move |buf| {
            let text = buf.text(&buf.start_iter(), &buf.end_iter(), false);
            placeholder_for_buf.set_visible(buf.char_count() == 0);
            submit_for_buf.set_sensitive(!text.trim().is_empty());
        });

        let working_dir_rc = Rc::new(RefCell::new(self.inner.host.working_dir()));
        let cwd_for_frecency = working_dir_rc.clone();
        let autocomplete = boxxy_core_widgets::autocomplete::AutocompleteController::new(
            &ask_entry,
            vec![Box::new(FileCompletionProvider::new(
                working_dir_rc,
                Some(ask_entry.downgrade()),
            ))],
            Some(Box::new(move |replacement| {
                if replacement.starts_with('@') && !replacement.ends_with('/') {
                    let cwd = cwd_for_frecency.borrow().clone().unwrap_or_default();
                    file_index::record_file_frecency(&cwd, &replacement);
                }
            })),
        );

        let viewer = StructuredViewer::new(boxxy_claw_ui::get_claw_viewer_registry());
        answer_viewer_slot.append(viewer.widget());

        let followup_context: Rc<RefCell<Option<(String, String)>>> = Rc::new(RefCell::new(None));
        let copy_reset_timer: Rc<RefCell<Option<gtk::glib::SourceId>>> =
            Rc::new(RefCell::new(None));

        let open = Rc::new(OpenBtwDialog {
            dialog: dialog.clone(),
            view_stack: view_stack.clone(),
            header_avatar,
            header_character_label,
            header_meta_label,
            last_answer_btn: last_answer_btn.clone(),
            followup_box: followup_box.clone(),
            followup_label,
            ask_entry: ask_entry.clone(),
            question_display_label,
            loading_status_box,
            loading_spinner_svg,
            loading_spinner_playing,
            loading_status_label,
            viewer,
            abort_btw_btn: abort_btw_btn.clone(),
            followup_btn: followup_btn.clone(),
            new_question_btn: new_question_btn.clone(),
            retry_btn: retry_btn.clone(),
            copy_answer_btn: copy_answer_btn.clone(),
            copy_btn_icon,
            copy_btn_label,
            done_btn: done_btn.clone(),
            followup_context: followup_context.clone(),
            copy_reset_timer: copy_reset_timer.clone(),
            _autocomplete: autocomplete.clone(),
        });

        // Clean up dialog slot & timers when closed (`Esc`, backdrop click, or Close button)
        {
            let inner_weak = Rc::downgrade(&self.inner);
            let timer_slot = copy_reset_timer;
            let spin_svg = open.loading_spinner_svg.clone();
            let spin_playing = open.loading_spinner_playing.clone();
            let viewer_for_close = open.viewer.clone();
            dialog.connect_closed(move |_| {
                spin_playing.set(false);
                spin_svg.pause();
                if let Some(tid) = timer_slot.borrow_mut().take() {
                    tid.remove();
                }
                viewer_for_close.park_for_recycle();
                if let Some(inner) = inner_weak.upgrade() {
                    *inner.open_dialog.borrow_mut() = None;
                }
            });
        }

        // Close buttons (`AdwHeaderBar` natively handles the top-right window close button)
        {
            let dlg = dialog.clone();
            ask_cancel_btn.connect_clicked(move |_| {
                dlg.close();
            });
        }
        {
            let dlg = dialog.clone();
            done_btn.connect_clicked(move |_| {
                dlg.close();
            });
        }

        // Clear follow-up context chip
        {
            let f_box = followup_box;
            let f_ctx = followup_context.clone();
            let entry_focus = ask_entry.clone();
            clear_followup_btn.connect_clicked(move |_| {
                *f_ctx.borrow_mut() = None;
                f_box.set_visible(false);
                entry_focus.grab_focus();
            });
        }

        // Submit action (closes dialog immediately and runs in background!)
        let submit_fn: Rc<dyn Fn()> = {
            let ctrl = self.clone();
            let entry = ask_entry.clone();
            let f_ctx = followup_context;
            let dlg = dialog.clone();
            Rc::new(move || {
                let buf = entry.buffer();
                let raw = buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string();
                if raw.trim().is_empty() {
                    return;
                }
                let prior = f_ctx.borrow_mut().take();
                dlg.close();
                ctrl.submit_question(&raw, prior);
            })
        };

        {
            let submit_click = submit_fn.clone();
            ask_submit_btn.connect_clicked(move |_| {
                submit_click();
            });
        }

        // Enter submits, Shift+Enter inserts newline, Escape closes autocomplete popup first
        {
            let key_ctrl = gtk::EventControllerKey::new();
            key_ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
            let submit_enter = submit_fn;
            let ac = autocomplete;
            key_ctrl.connect_key_pressed(move |_, key, _, state| {
                if ac.handle_key_pressed_with_state(key, state) == gtk::glib::Propagation::Stop {
                    return gtk::glib::Propagation::Stop;
                }
                let is_shift = state.contains(gtk::gdk::ModifierType::SHIFT_MASK);
                if (key == gtk::gdk::Key::Return || key == gtk::gdk::Key::KP_Enter) && !is_shift {
                    submit_enter();
                    return gtk::glib::Propagation::Stop;
                }
                gtk::glib::Propagation::Proceed
            });
            ask_entry.add_controller(key_ctrl);
        }

        // View Last Answer button
        {
            let ctrl = self.clone();
            let open_weak = Rc::downgrade(&open);
            last_answer_btn.connect_clicked(move |_| {
                if let Some(ex) = ctrl.inner.last_exchange.borrow().clone()
                    && let Some(op) = open_weak.upgrade()
                {
                    Self::populate_answer_dialog(&op, &ex);
                }
            });
        }

        // Cancel Question button (explicitly aborts the background `/btw` request)
        {
            let ctrl = self.clone();
            let dlg = dialog.clone();
            abort_btw_btn.connect_clicked(move |_| {
                if let BtwStatus::Loading { request_id, .. } = *ctrl.inner.status.borrow() {
                    ctrl.inner.host.send_claw(ClawMessage::CancelBtw {
                        request_id: Some(request_id),
                    });
                }
                ctrl.set_status(&BtwStatus::Idle);
                dlg.close();
            });
        }

        // Ask Follow-up button
        {
            let ctrl = self.clone();
            let open_weak = Rc::downgrade(&open);
            followup_btn.connect_clicked(move |_| {
                let prior = ctrl.inner.last_exchange.borrow().as_ref().and_then(|ex| {
                    ex.error
                        .is_none()
                        .then(|| (ex.question.clone(), ex.answer.clone()))
                });
                if let Some(op) = open_weak.upgrade() {
                    ctrl.configure_ask_page(&op, prior);
                }
            });
        }

        // New Question button
        {
            let ctrl = self.clone();
            let open_weak = Rc::downgrade(&open);
            new_question_btn.connect_clicked(move |_| {
                if let Some(op) = open_weak.upgrade() {
                    ctrl.configure_ask_page(&op, None);
                }
            });
        }

        // Retry button (when provider failed or returned empty)
        {
            let ctrl = self.clone();
            let dlg = dialog.clone();
            retry_btn.connect_clicked(move |_| {
                if let Some(ex) = ctrl.inner.last_exchange.borrow().clone() {
                    dlg.close();
                    ctrl.submit_question(&ex.question, ex.prior_exchange);
                }
            });
        }

        // Copy Answer button + `c` shortcut on the Answer view (matching OpenCode)
        let copy_action: Rc<dyn Fn()> = {
            let ctrl = self.clone();
            let open_weak = Rc::downgrade(&open);
            Rc::new(move || {
                let Some(answer_text) = ctrl
                    .inner
                    .last_exchange
                    .borrow()
                    .as_ref()
                    .filter(|ex| ex.error.is_none() && !ex.answer.is_empty())
                    .map(|ex| ex.answer.clone())
                else {
                    return;
                };
                if let Some(display) = gtk::gdk::Display::default() {
                    display.clipboard().set_text(&answer_text);
                }
                let Some(op) = open_weak.upgrade() else {
                    return;
                };
                op.copy_btn_icon
                    .set_icon_name(Some("boxxy-object-select-symbolic"));
                op.copy_btn_label.set_label("Copied");
                op.copy_answer_btn.add_css_class("success");
                if let Some(old_tid) = op.copy_reset_timer.borrow_mut().take() {
                    old_tid.remove();
                }
                let op_weak_timer = Rc::downgrade(&op);
                let tid = gtk::glib::timeout_add_local_once(
                    std::time::Duration::from_secs(2),
                    move || {
                        if let Some(op_alive) = op_weak_timer.upgrade() {
                            let _ = op_alive.copy_reset_timer.borrow_mut().take();
                            op_alive
                                .copy_btn_icon
                                .set_icon_name(Some("boxxy-edit-copy-symbolic"));
                            op_alive.copy_btn_label.set_label("Copy");
                            op_alive.copy_answer_btn.remove_css_class("success");
                        }
                    },
                );
                *op.copy_reset_timer.borrow_mut() = Some(tid);
            })
        };

        {
            let copy_click = copy_action.clone();
            copy_answer_btn.connect_clicked(move |_| {
                copy_click();
            });
        }

        {
            let dialog_keys = gtk::EventControllerKey::new();
            let vs = view_stack;
            let copy_key = copy_action;
            let copy_btn_check = copy_answer_btn;
            dialog_keys.connect_key_pressed(move |_, key, _, mods| {
                let has_mods = mods.intersects(
                    gtk::gdk::ModifierType::CONTROL_MASK
                        | gtk::gdk::ModifierType::ALT_MASK
                        | gtk::gdk::ModifierType::SUPER_MASK,
                );
                if !has_mods
                    && (key == gtk::gdk::Key::c || key == gtk::gdk::Key::C)
                    && vs.visible_child_name().as_deref() == Some("answer")
                    && copy_btn_check.is_visible()
                {
                    copy_key();
                    return gtk::glib::Propagation::Stop;
                }
                gtk::glib::Propagation::Proceed
            });
            dialog.add_controller(dialog_keys);
        }

        *self.inner.open_dialog.borrow_mut() = Some(open.clone());
        dialog.present(Some(&root_widget));
        Some(open)
    }

    fn configure_ask_page(&self, open: &OpenBtwDialog, prior: Option<(String, String)>) {
        let (agent_name, character_id) = self.resolve_current_speaker();
        Self::apply_header_identity(open, &agent_name, &character_id, "Side question");

        let has_last = self.inner.last_exchange.borrow().is_some();
        open.last_answer_btn.set_visible(has_last);

        if let Some((ref prev_q, _)) = prior {
            open.followup_label
                .set_label(&format!("Following up on: {}", single_line_preview(prev_q, 64)));
            open.followup_box.set_visible(true);
        } else {
            open.followup_box.set_visible(false);
        }
        *open.followup_context.borrow_mut() = prior;

        open.ask_entry.buffer().set_text("");
        open.view_stack.set_visible_child_name("ask");

        let entry = open.ask_entry.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(30), move || {
            entry.grab_focus();
        });
    }

    fn present_ask_view(&self, prior: Option<(String, String)>) {
        if let Some(open) = self.ensure_dialog() {
            self.configure_ask_page(&open, prior);
        }
    }

    fn present_loading_peek(&self, question: &str, active_status: &str, streamed_text: &str) {
        let Some(open) = self.ensure_dialog() else {
            return;
        };
        let (agent_name, character_id) = self.resolve_current_speaker();
        Self::apply_header_identity(&open, &agent_name, &character_id, "Working…");
        open.last_answer_btn.set_visible(false);

        open.question_display_label.set_label(question);
        open.loading_status_label.set_label(if active_status.is_empty() {
            "Thinking…"
        } else {
            active_status
        });
        open.loading_status_box.set_visible(true);
        open.loading_spinner_playing.set(true);
        open.loading_spinner_svg.play();

        if streamed_text.is_empty() {
            open.viewer.park_for_recycle();
        } else {
            open.viewer.set_streaming_content(streamed_text, false);
        }

        open.abort_btw_btn.set_visible(true);
        open.followup_btn.set_visible(false);
        open.new_question_btn.set_visible(false);
        open.retry_btn.set_visible(false);
        open.copy_answer_btn.set_visible(false);
        open.done_btn.set_label("Hide");

        open.view_stack.set_visible_child_name("answer");
        let done_focus = open.done_btn.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(30), move || {
            done_focus.grab_focus();
        });
    }

    fn populate_answer_dialog(open: &OpenBtwDialog, ex: &BtwExchange) {
        let meta = format_duration_meta(&ex.model_label, ex.duration_ms);
        Self::apply_header_identity(open, &ex.agent_name, &ex.character_id, &meta);
        open.last_answer_btn.set_visible(false);

        open.loading_spinner_playing.set(false);
        open.loading_spinner_svg.pause();
        open.loading_status_box.set_visible(false);

        open.question_display_label.set_label(&ex.question);
        open.abort_btw_btn.set_visible(false);
        open.done_btn.set_label("Close");

        open.copy_btn_icon
            .set_icon_name(Some("boxxy-edit-copy-symbolic"));
        open.copy_btn_label.set_label("Copy");
        open.copy_answer_btn.remove_css_class("success");

        if let Some(ref err_msg) = ex.error {
            open.viewer.set_content(err_msg);
            open.retry_btn.set_visible(true);
            open.copy_answer_btn.set_visible(false);
            open.followup_btn.set_visible(false);
        } else {
            open.viewer.set_content(&ex.answer);
            open.retry_btn.set_visible(false);
            open.copy_answer_btn.set_visible(true);
            open.followup_btn.set_visible(true);
        }
        open.new_question_btn.set_visible(true);

        open.view_stack.set_visible_child_name("answer");
        let focus_btn = if open.retry_btn.is_visible() {
            open.retry_btn.clone()
        } else {
            open.done_btn.clone()
        };
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(30), move || {
            focus_btn.grab_focus();
        });
    }

    fn present_answer_view(&self, ex: &BtwExchange) {
        if let Some(open) = self.ensure_dialog() {
            Self::populate_answer_dialog(&open, ex);
        }
    }
}

fn single_line_preview(s: &str, max_chars: usize) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = joined.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::{format_duration_meta, single_line_preview};

    #[test]
    fn formats_duration_and_single_line_previews_cleanly() {
        assert_eq!(format_duration_meta("", 0), "Side question");
        assert_eq!(
            format_duration_meta("GPT-6.1 Terra", 1_850),
            "GPT-6.1 Terra · 1.8s"
        );
        assert_eq!(
            format_duration_meta("Claude Sonnet", 14_600),
            "Claude Sonnet · 15s"
        );
        assert_eq!(format_duration_meta("Model X", 75_000), "Model X · 1m 15s");

        assert_eq!(
            single_line_preview("  why   did\n  src/parser.rs  fail? ", 40),
            "why did src/parser.rs fail?"
        );
        assert_eq!(single_line_preview("abcdefghij", 5), "abcde…");
    }
}
