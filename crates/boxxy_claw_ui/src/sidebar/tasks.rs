//! Middle-row Pending Tasks & Multi-Round Interval Session UI for the Claw sidebar.
//!
//! Occupies the flexible middle zone (`vexpand = true`) between the top Character Identity
//! header and the bottom Models / Context / MCP footer during active sessions. Supports
//! both compact single-shot reminder/task cards and dedicated multi-round interval cards
//! (boxing laps, Pomodoro, HIIT, study blocks) with Pause/Resume, round pips, and a live
//! progress bar.

use boxxy_claw_protocol::{IntervalPhase, IntervalSchedule, ScheduledTask, TaskStatus, TaskType};
use chrono::{DateTime, Datelike, Duration, Local, TimeZone, Utc};
use gtk4 as gtk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use uuid::Uuid;

const CLOCK_GPA_RESOURCE: &str = "/dev/boxxy/BoxxyTerminal/icons/boxxy-clock.gpa";
const BELL_GPA_RESOURCE: &str = "/dev/boxxy/BoxxyTerminal/icons/boxxy-bell.gpa";

/// Live references for each rendered task or round card so the 1-second ticker updates
/// countdown text, stopwatch readout, and progress bar fraction without rebuilding widgets.
enum LiveTaskEntry {
    Single {
        due_at: DateTime<Utc>,
        paused_remaining_ms: Option<i64>,
        countdown_lbl: gtk::Label,
        sep_lbl: gtk::Label,
        clock_lbl: gtk::Label,
    },
    Interval {
        due_at: DateTime<Utc>,
        paused_remaining_ms: Option<i64>,
        phase_duration_secs: u32,
        timer_lbl: gtk::Label,
        sep_lbl: gtk::Label,
        clock_lbl: gtk::Label,
        progress_bar: gtk::ProgressBar,
    },
}

impl LiveTaskEntry {
    const fn is_running(&self) -> bool {
        match self {
            Self::Single {
                paused_remaining_ms,
                ..
            }
            | Self::Interval {
                paused_remaining_ms,
                ..
            } => paused_remaining_ms.is_none(),
        }
    }
}

type IntervalSig = (u32, u32, u8, u32, u32, String);
type TaskSignature = Vec<(Uuid, i64, Option<i64>, String, u8, Option<IntervalSig>)>;
type CancelAllCallback = Rc<RefCell<Option<Rc<dyn Fn() + 'static>>>>;
type TogglePauseCallback = Rc<RefCell<Option<Rc<dyn Fn(Uuid) + 'static>>>>;

pub(super) struct TasksSection {
    root: gtk::Box,
    header_row: gtk::Box,
    count_pill: gtk::Label,
    dismiss_all_lbl: gtk::Label,
    stack: gtk::Stack,
    empty_hint_lbl: gtk::Label,
    cards_box: gtk::Box,
    show_when_empty: Cell<bool>,
    pending_count: Cell<usize>,
    rendered_sig: RefCell<TaskSignature>,
    live_cards: Rc<RefCell<Vec<LiveTaskEntry>>>,
    ticker_source: Rc<RefCell<Option<glib::SourceId>>>,
    on_cancel_task: Rc<dyn Fn(Uuid) + 'static>,
    on_toggle_pause: TogglePauseCallback,
    on_cancel_all: CancelAllCallback,
}

impl TasksSection {
    #[allow(clippy::too_many_lines)]
    pub(super) fn new(on_cancel_task: Rc<dyn Fn(Uuid) + 'static>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.add_css_class("claw-tasks-section");
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.set_halign(gtk::Align::Fill);
        root.set_valign(gtk::Align::Fill);
        root.set_margin_start(14);
        root.set_margin_end(14);
        root.set_visible(false);

        // --- Section Header (shown only when >= 1 pending task exists) ---
        let header_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header_row.set_hexpand(true);
        header_row.set_vexpand(false);
        header_row.set_valign(gtk::Align::Start);
        header_row.set_margin_top(6);
        header_row.set_visible(false);

        let header_title = gtk::Label::builder()
            .label("Pending Tasks")
            .css_classes(["caption", "heading"])
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Center)
            .xalign(0.0)
            .build();
        header_row.append(&header_title);

        let count_pill = gtk::Label::builder()
            .label("0")
            .css_classes(["claw-tasks-count-pill"])
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        header_row.append(&count_pill);

        let header_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header_spacer.set_hexpand(true);
        header_row.append(&header_spacer);

        let on_cancel_all: CancelAllCallback = Rc::new(RefCell::new(None));
        let dismiss_all_lbl = gtk::Label::builder()
            .label("Dismiss all")
            .css_classes(["caption", "accent", "boxxy-link"])
            .halign(gtk::Align::End)
            .valign(gtk::Align::Center)
            .tooltip_text("Dismiss all pending tasks")
            .visible(false)
            .build();
        dismiss_all_lbl.set_cursor_from_name(Some("pointer"));
        {
            let gesture = gtk::GestureClick::new();
            let cb_slot = on_cancel_all.clone();
            let lbl_weak = dismiss_all_lbl.downgrade();
            gesture.connect_released(move |_, _, x, y| {
                if let Some(lbl) = lbl_weak.upgrade()
                    && lbl.contains(x, y)
                    && let Some(cb) = cb_slot.borrow().as_ref().cloned()
                {
                    cb();
                }
            });
            dismiss_all_lbl.add_controller(gesture);
        }
        header_row.append(&dismiss_all_lbl);
        root.append(&header_row);

        // --- Middle-Zone Stack (`vexpand = true` to utilize all middle sidebar space) ---
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(140)
            .vhomogeneous(false)
            .hhomogeneous(true)
            .hexpand(true)
            .vexpand(true)
            .valign(gtk::Align::Fill)
            .build();

        // 1. Empty State Card — top-aligned right below Character Info with crisp margin
        let empty_wrap = gtk::Box::new(gtk::Orientation::Vertical, 0);
        empty_wrap.set_hexpand(true);
        empty_wrap.set_vexpand(false);
        empty_wrap.set_valign(gtk::Align::Start);
        empty_wrap.set_margin_top(10);
        empty_wrap.set_margin_bottom(4);

        let empty_card = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        empty_card.add_css_class("claw-tasks-empty-card");
        empty_card.set_hexpand(true);
        empty_card.set_vexpand(false);
        empty_card.set_valign(gtk::Align::Start);

        let empty_icon = build_task_icon(
            Some(CLOCK_GPA_RESOURCE),
            "boxxy-timer-symbolic",
            Some("dim-label"),
        );
        empty_card.append(&empty_icon);

        let empty_text_vbox = gtk::Box::new(gtk::Orientation::Vertical, 1);
        empty_text_vbox.set_hexpand(true);
        empty_text_vbox.set_vexpand(false);
        empty_text_vbox.set_valign(gtk::Align::Center);

        let empty_title_lbl = gtk::Label::builder()
            .label("No pending tasks")
            .css_classes(["caption", "heading"])
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .build();
        empty_text_vbox.append(&empty_title_lbl);

        let empty_hint_lbl = gtk::Label::builder()
            .label("Ask Claw to set a reminder")
            .css_classes(["caption", "dim-label"])
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(2)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        empty_text_vbox.append(&empty_hint_lbl);
        empty_card.append(&empty_text_vbox);
        empty_wrap.append(&empty_card);

        stack.add_named(&empty_wrap, Some("empty"));

        // 2. Populated List (fills the entire middle zone; cards stack compactly from top)
        let cards_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        cards_box.set_hexpand(true);
        cards_box.set_vexpand(false);
        cards_box.set_valign(gtk::Align::Start);
        cards_box.set_margin_top(1);
        cards_box.set_margin_bottom(4);

        let tasks_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .valign(gtk::Align::Fill)
            .css_classes(["claw-tasks-scroll"])
            .child(&cards_box)
            .build();
        stack.add_named(&tasks_scroll, Some("list"));

        root.append(&stack);

        let live_cards: Rc<RefCell<Vec<LiveTaskEntry>>> = Rc::new(RefCell::new(Vec::new()));
        let ticker_source: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));

        {
            let cards_for_map = live_cards.clone();
            let source_for_map = ticker_source.clone();
            root.connect_map(move |w| {
                refresh_live_cards(&cards_for_map.borrow(), Utc::now());
                sync_ticker(w, &cards_for_map, &source_for_map);
            });
        }
        {
            let source_for_unmap = ticker_source.clone();
            root.connect_unmap(move |_| {
                if let Some(src) = source_for_unmap.borrow_mut().take() {
                    src.remove();
                }
            });
        }

        Self {
            root,
            header_row,
            count_pill,
            dismiss_all_lbl,
            stack,
            empty_hint_lbl,
            cards_box,
            show_when_empty: Cell::new(false),
            pending_count: Cell::new(0),
            rendered_sig: RefCell::new(Vec::new()),
            live_cards,
            ticker_source,
            on_cancel_task,
            on_toggle_pause: Rc::new(RefCell::new(None)),
            on_cancel_all,
        }
    }

    pub(super) const fn widget(&self) -> &gtk::Box {
        &self.root
    }

    pub(super) fn set_on_cancel_all<FA: Fn() + 'static>(&self, cb: FA) {
        *self.on_cancel_all.borrow_mut() = Some(Rc::new(cb));
    }

    pub(super) fn set_on_toggle_pause<FP: Fn(Uuid) + 'static>(&self, cb: FP) {
        *self.on_toggle_pause.borrow_mut() = Some(Rc::new(cb));
    }

    pub(super) fn set_character_context(
        &self,
        character_name: Option<&str>,
        show_when_empty: bool,
    ) {
        self.show_when_empty.set(show_when_empty);
        let name = character_name
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("Claw");
        self.empty_hint_lbl
            .set_label(&format!("Ask {name} to set a reminder"));
        self.sync_visibility();
    }

    pub(super) fn update_tasks(&self, tasks: Vec<ScheduledTask>) {
        let now = Utc::now();
        let sorted = sort_and_filter_pending_tasks(tasks, now);
        let pending_count = sorted.len();
        self.pending_count.set(pending_count);

        let new_sig: TaskSignature = sorted
            .iter()
            .map(|t| {
                let kind_tag = match t.task_type {
                    TaskType::Notification => 0,
                    TaskType::Command => 1,
                    TaskType::Query => 2,
                };
                let iv_sig = t.interval.as_ref().map(|iv| {
                    let phase_tag = match iv.phase {
                        IntervalPhase::Work => 0,
                        IntervalPhase::Break => 1,
                    };
                    (
                        iv.total_rounds,
                        iv.current_round,
                        phase_tag,
                        iv.work_secs,
                        iv.break_secs,
                        iv.round_label.clone(),
                    )
                });
                let due_sig = if t.is_paused() {
                    0
                } else {
                    t.due_at.timestamp()
                };
                (
                    t.id,
                    due_sig,
                    t.paused_remaining_ms,
                    t.payload.clone(),
                    kind_tag,
                    iv_sig,
                )
            })
            .collect();

        if *self.rendered_sig.borrow() == new_sig {
            refresh_live_cards(&self.live_cards.borrow(), now);
            self.sync_visibility();
            sync_ticker(&self.root, &self.live_cards, &self.ticker_source);
            return;
        }
        *self.rendered_sig.borrow_mut() = new_sig;

        while let Some(child) = self.cards_box.first_child() {
            self.cards_box.remove(&child);
        }
        self.live_cards.borrow_mut().clear();

        let mut new_live = Vec::with_capacity(pending_count);
        for task in sorted {
            let (card_widget, live_entry) = task.interval.as_ref().map_or_else(
                || build_single_task_card(&task, now, &self.on_toggle_pause, &self.on_cancel_task),
                |iv| {
                    build_interval_card(&task, iv, now, &self.on_toggle_pause, &self.on_cancel_task)
                },
            );
            self.cards_box.append(&card_widget);
            new_live.push(live_entry);
        }
        *self.live_cards.borrow_mut() = new_live;

        self.sync_visibility();
        sync_ticker(&self.root, &self.live_cards, &self.ticker_source);
    }

    fn sync_visibility(&self) {
        let count = self.pending_count.get();
        if count > 0 {
            self.header_row.set_visible(true);
            self.count_pill.set_label(&count.to_string());
            self.count_pill.set_visible(true);
            self.dismiss_all_lbl.set_visible(count >= 2);
            self.stack.set_visible_child_name("list");
            self.root.set_visible(true);
        } else if self.show_when_empty.get() {
            self.header_row.set_visible(false);
            self.count_pill.set_visible(false);
            self.dismiss_all_lbl.set_visible(false);
            self.stack.set_visible_child_name("empty");
            self.root.set_visible(true);
        } else {
            self.header_row.set_visible(false);
            self.count_pill.set_visible(false);
            self.dismiss_all_lbl.set_visible(false);
            self.root.set_visible(false);
        }
    }
}

/// Builds a clean `20×20` vector icon (`GtkImage` with `hexpand = false`, `vexpand = false`,
/// zero background/border frame) so only the icon itself renders cleanly on the left.
fn build_task_icon(
    gpa_resource_path: Option<&str>,
    fallback_icon: &str,
    tint_class: Option<&str>,
) -> gtk::Image {
    let image = if let Some(gpa_path) = gpa_resource_path
        && gio::resources_get_info(gpa_path, gio::ResourceLookupFlags::NONE).is_ok()
    {
        let svg = gtk::Svg::from_resource(gpa_path);
        svg.set_state(0);
        gtk::Image::builder().paintable(&svg).pixel_size(17).build()
    } else {
        gtk::Image::builder()
            .icon_name(fallback_icon)
            .pixel_size(17)
            .build()
    };
    image.set_size_request(20, 20);
    image.set_hexpand(false);
    image.set_vexpand(false);
    image.set_halign(gtk::Align::Start);
    image.set_valign(gtk::Align::Center);
    image.add_css_class("claw-task-icon");
    if let Some(cls) = tint_class {
        image.add_css_class(cls);
    }
    image
}

/// Builds the dedicated Multi-Round Interval Session card (e.g. Boxing 6×1m rounds + 20s breaks).
#[allow(clippy::too_many_lines)]
fn build_interval_card(
    task: &ScheduledTask,
    iv: &IntervalSchedule,
    now: DateTime<Utc>,
    on_toggle_pause: &TogglePauseCallback,
    on_cancel: &Rc<dyn Fn(Uuid) + 'static>,
) -> (gtk::Box, LiveTaskEntry) {
    let is_paused = task.is_paused();
    let card = gtk::Box::new(gtk::Orientation::Vertical, 7);
    card.add_css_class("claw-task-card");
    card.add_css_class("claw-round-card");
    if is_paused {
        card.add_css_class("claw-round-card-paused");
    } else if iv.phase == IntervalPhase::Break {
        card.add_css_class("claw-round-card-break");
    }
    card.set_hexpand(true);
    card.set_vexpand(false);
    card.set_valign(gtk::Align::Start);

    // 1. Top Header Row: Icon Badge + (Line 1: Title + [Phase Pill] [Pause/Play] [×]; Line 2: Cadence)
    let top_hbox = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    top_hbox.set_hexpand(true);
    top_hbox.set_vexpand(false);
    top_hbox.set_valign(gtk::Align::Start);

    let icon_badge = if is_paused {
        build_task_icon(
            Some(CLOCK_GPA_RESOURCE),
            "boxxy-timer-symbolic",
            Some("dim-label"),
        )
    } else {
        match iv.phase {
            IntervalPhase::Work => {
                build_task_icon(Some(BELL_GPA_RESOURCE), "boxxy-timer-symbolic", None)
            }
            IntervalPhase::Break => {
                build_task_icon(Some(CLOCK_GPA_RESOURCE), "boxxy-timer-symbolic", None)
            }
        }
    };
    icon_badge.set_valign(gtk::Align::Start);
    icon_badge.set_margin_top(4);
    top_hbox.append(&icon_badge);

    let header_vbox = gtk::Box::new(gtk::Orientation::Vertical, 1);
    header_vbox.set_hexpand(true);
    header_vbox.set_vexpand(false);
    header_vbox.set_valign(gtk::Align::Start);

    let title_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    title_row.set_hexpand(true);
    title_row.set_valign(gtk::Align::Center);

    let title_lbl = gtk::Label::builder()
        .label(&task.payload)
        .css_classes(["claw-task-payload"])
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .single_line_mode(true)
        .xalign(0.0)
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .build();
    if task.payload.chars().count() > 24 {
        title_lbl.set_tooltip_text(Some(&task.payload));
    }
    title_row.append(&title_lbl);

    let unit = if iv.round_label.trim().is_empty() {
        "Round"
    } else {
        iv.round_label.trim()
    };
    let (pill_text, pill_mod_class) = if is_paused {
        ("❚❚ PAUSED".to_string(), "claw-round-phase-paused")
    } else {
        match iv.phase {
            IntervalPhase::Work => (
                format!(
                    "● {} {}/{}",
                    unit.to_uppercase(),
                    iv.current_round,
                    iv.total_rounds
                ),
                "claw-round-phase-work",
            ),
            IntervalPhase::Break => ("● BREAK".to_string(), "claw-round-phase-break"),
        }
    };
    let phase_pill = gtk::Label::builder()
        .label(&pill_text)
        .css_classes(["claw-round-phase-pill", pill_mod_class])
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .margin_end(4)
        .build();
    title_row.append(&phase_pill);

    // Top-Right Action Controls (`[⏸/▶]` + `[×]`) aligned cleanly on Line 1
    let controls_hbox = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    controls_hbox.set_halign(gtk::Align::End);
    controls_hbox.set_valign(gtk::Align::Center);

    let pause_icon_name = if is_paused {
        "boxxy-media-playback-start-symbolic"
    } else {
        "boxxy-media-playback-pause-symbolic"
    };
    let pause_tooltip = if is_paused {
        "Resume session"
    } else {
        "Pause session"
    };
    let pause_btn = gtk::Button::builder()
        .icon_name(pause_icon_name)
        .css_classes(["flat", "circular", "claw-task-dismiss-btn"])
        .valign(gtk::Align::Center)
        .hexpand(false)
        .vexpand(false)
        .can_focus(false)
        .focus_on_click(false)
        .tooltip_text(pause_tooltip)
        .build();
    {
        let task_id = task.id;
        let pause_cb_slot = on_toggle_pause.clone();
        pause_btn.connect_clicked(move |_| {
            if let Some(cb) = pause_cb_slot.borrow().as_ref().cloned() {
                cb(task_id);
            }
        });
    }
    controls_hbox.append(&pause_btn);

    let dismiss_btn = gtk::Button::builder()
        .icon_name("boxxy-cross-small-symbolic")
        .css_classes(["flat", "circular", "claw-task-dismiss-btn"])
        .valign(gtk::Align::Center)
        .hexpand(false)
        .vexpand(false)
        .can_focus(false)
        .focus_on_click(false)
        .tooltip_text("Dismiss session")
        .build();
    {
        let task_id = task.id;
        let cancel_cb = on_cancel.clone();
        dismiss_btn.connect_clicked(move |_| cancel_cb(task_id));
    }
    controls_hbox.append(&dismiss_btn);
    title_row.append(&controls_hbox);

    header_vbox.append(&title_row);

    let cadence_text = if is_paused {
        match iv.phase {
            IntervalPhase::Work => {
                format!(
                    "Paused on {unit} {} of {}",
                    iv.current_round, iv.total_rounds
                )
            }
            IntervalPhase::Break => {
                let next_round = (iv.current_round + 1).min(iv.total_rounds);
                format!(
                    "Paused on break  •  Next: {unit} {next_round} of {}",
                    iv.total_rounds
                )
            }
        }
    } else {
        match iv.phase {
            IntervalPhase::Work => {
                let work_str = format_short_span(iv.work_secs);
                if iv.break_secs > 0 {
                    format!(
                        "{work_str} {}  •  {} break",
                        unit.to_lowercase(),
                        format_short_span(iv.break_secs)
                    )
                } else {
                    format!("{work_str} per {}", unit.to_lowercase())
                }
            }
            IntervalPhase::Break => {
                let next_round = (iv.current_round + 1).min(iv.total_rounds);
                format!(
                    "Rest  •  Up next: {unit} {next_round} of {}",
                    iv.total_rounds
                )
            }
        }
    };
    let cadence_lbl = gtk::Label::builder()
        .label(&cadence_text)
        .css_classes(["caption", "dim-label"])
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    header_vbox.append(&cadence_lbl);
    top_hbox.append(&header_vbox);

    card.append(&top_hbox);

    // 2. Round Pip Strip + Live Stopwatch Countdown Row
    let pips_and_timer_hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    pips_and_timer_hbox.set_hexpand(true);
    pips_and_timer_hbox.set_vexpand(false);
    pips_and_timer_hbox.set_valign(gtk::Align::Center);

    if iv.total_rounds <= 12 {
        let pips_hbox = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        pips_hbox.set_halign(gtk::Align::Start);
        pips_hbox.set_valign(gtk::Align::Center);

        for r in 1..=iv.total_rounds {
            let pip = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            pip.add_css_class("claw-round-pip");
            let state_cls = match iv.phase {
                IntervalPhase::Work => match r.cmp(&iv.current_round) {
                    std::cmp::Ordering::Less => "claw-round-pip-done",
                    std::cmp::Ordering::Equal => {
                        if is_paused {
                            "claw-round-pip-paused"
                        } else {
                            "claw-round-pip-active"
                        }
                    }
                    std::cmp::Ordering::Greater => "claw-round-pip-upcoming",
                },
                IntervalPhase::Break => {
                    if r <= iv.current_round {
                        "claw-round-pip-done"
                    } else if r == iv.current_round + 1 {
                        if is_paused {
                            "claw-round-pip-paused"
                        } else {
                            "claw-round-pip-resting"
                        }
                    } else {
                        "claw-round-pip-upcoming"
                    }
                }
            };
            pip.add_css_class(state_cls);
            pip.set_tooltip_text(Some(&format!("{unit} {r} of {}", iv.total_rounds)));
            pips_hbox.append(&pip);
        }
        pips_and_timer_hbox.append(&pips_hbox);
    } else {
        let done_count = match iv.phase {
            IntervalPhase::Work => iv.current_round.saturating_sub(1),
            IntervalPhase::Break => iv.current_round,
        };
        let summary_lbl = gtk::Label::builder()
            .label(format!("{done_count} / {} done", iv.total_rounds))
            .css_classes(["caption", "dim-label"])
            .halign(gtk::Align::Start)
            .build();
        pips_and_timer_hbox.append(&summary_lbl);
    }

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    pips_and_timer_hbox.append(&spacer);

    let timer_lbl = gtk::Label::builder()
        .css_classes(["caption", "claw-task-countdown", "claw-round-timer"])
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .build();
    if is_paused {
        timer_lbl.add_css_class("dim-label");
    }
    pips_and_timer_hbox.append(&timer_lbl);

    let sep_lbl = gtk::Label::builder()
        .label("•")
        .css_classes(["caption", "dim-label"])
        .valign(gtk::Align::Center)
        .visible(!is_paused)
        .build();
    pips_and_timer_hbox.append(&sep_lbl);

    let clock_lbl = gtk::Label::builder()
        .css_classes(["caption", "dim-label"])
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .visible(!is_paused)
        .build();
    pips_and_timer_hbox.append(&clock_lbl);

    card.append(&pips_and_timer_hbox);

    // 3. Live Phase Progress Bar
    let progress_bar = gtk::ProgressBar::builder()
        .hexpand(true)
        .vexpand(false)
        .valign(gtk::Align::Center)
        .css_classes(["claw-round-progress"])
        .build();
    if is_paused {
        progress_bar.add_css_class("claw-round-progress-paused");
    } else if iv.phase == IntervalPhase::Break {
        progress_bar.add_css_class("claw-round-progress-break");
    }
    card.append(&progress_bar);

    let phase_duration_secs = match iv.phase {
        IntervalPhase::Work => iv.work_secs.max(1),
        IntervalPhase::Break => iv.break_secs.max(1),
    };

    let entry = LiveTaskEntry::Interval {
        due_at: task.due_at,
        paused_remaining_ms: task.paused_remaining_ms,
        phase_duration_secs,
        timer_lbl,
        sep_lbl,
        clock_lbl,
        progress_bar,
    };
    apply_live_timing(&entry, now, Local::now().offset());
    (card, entry)
}

#[allow(clippy::too_many_lines)]
fn build_single_task_card(
    task: &ScheduledTask,
    now: DateTime<Utc>,
    on_toggle_pause: &TogglePauseCallback,
    on_cancel: &Rc<dyn Fn(Uuid) + 'static>,
) -> (gtk::Box, LiveTaskEntry) {
    let is_paused = task.is_paused();
    let card = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    card.add_css_class("claw-task-card");
    card.set_hexpand(true);
    card.set_vexpand(false);
    card.set_valign(gtk::Align::Start);

    let (icon_badge, kind_badge) = match task.task_type {
        TaskType::Notification => (
            build_task_icon(Some(BELL_GPA_RESOURCE), "boxxy-timer-symbolic", None),
            None,
        ),
        TaskType::Command => (
            build_task_icon(None, "boxxy-tui-symbolic", None),
            Some("Command"),
        ),
        TaskType::Query => (
            build_task_icon(None, "boxxy-chat-symbolic", None),
            Some("Query"),
        ),
    };
    card.append(&icon_badge);

    let body_vbox = gtk::Box::new(gtk::Orientation::Vertical, 1);
    body_vbox.set_hexpand(true);
    body_vbox.set_vexpand(false);
    body_vbox.set_valign(gtk::Align::Center);

    let payload_lbl = gtk::Label::builder()
        .label(&task.payload)
        .css_classes(["claw-task-payload"])
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .lines(2)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .xalign(0.0)
        .halign(gtk::Align::Start)
        .hexpand(true)
        .vexpand(false)
        .build();
    if task.task_type == TaskType::Command {
        payload_lbl.add_css_class("monospace");
    }
    if task.payload.chars().count() > 44 {
        payload_lbl.set_tooltip_text(Some(&task.payload));
    }
    body_vbox.append(&payload_lbl);

    let meta_hbox = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    meta_hbox.set_halign(gtk::Align::Start);
    meta_hbox.set_valign(gtk::Align::Center);
    meta_hbox.set_vexpand(false);

    if let Some(kind_text) = kind_badge {
        let kind_lbl = gtk::Label::builder()
            .label(kind_text)
            .css_classes(["claw-task-kind-badge"])
            .valign(gtk::Align::Center)
            .build();
        meta_hbox.append(&kind_lbl);
    }

    let countdown_lbl = gtk::Label::builder()
        .css_classes(["caption", "claw-task-countdown"])
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Center)
        .build();
    meta_hbox.append(&countdown_lbl);

    let sep_lbl = gtk::Label::builder()
        .label("•")
        .css_classes(["caption", "dim-label"])
        .valign(gtk::Align::Center)
        .visible(!is_paused)
        .build();
    meta_hbox.append(&sep_lbl);

    let clock_lbl = gtk::Label::builder()
        .css_classes(["caption", "dim-label"])
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Center)
        .visible(!is_paused)
        .build();
    meta_hbox.append(&clock_lbl);

    body_vbox.append(&meta_hbox);
    card.append(&body_vbox);

    if is_paused {
        let resume_btn = gtk::Button::builder()
            .icon_name("boxxy-media-playback-start-symbolic")
            .css_classes(["flat", "circular", "claw-task-dismiss-btn"])
            .halign(gtk::Align::End)
            .valign(gtk::Align::Center)
            .hexpand(false)
            .vexpand(false)
            .can_focus(false)
            .focus_on_click(false)
            .tooltip_text("Resume task")
            .build();
        let task_id = task.id;
        let pause_cb_slot = on_toggle_pause.clone();
        resume_btn.connect_clicked(move |_| {
            if let Some(cb) = pause_cb_slot.borrow().as_ref().cloned() {
                cb(task_id);
            }
        });
        card.append(&resume_btn);
    }

    let dismiss_btn = gtk::Button::builder()
        .icon_name("boxxy-cross-small-symbolic")
        .css_classes(["flat", "circular", "claw-task-dismiss-btn"])
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .hexpand(false)
        .vexpand(false)
        .can_focus(false)
        .focus_on_click(false)
        .tooltip_text("Dismiss task")
        .build();
    let task_id = task.id;
    let cancel_cb = on_cancel.clone();
    dismiss_btn.connect_clicked(move |_| cancel_cb(task_id));
    card.append(&dismiss_btn);

    let entry = LiveTaskEntry::Single {
        due_at: task.due_at,
        paused_remaining_ms: task.paused_remaining_ms,
        countdown_lbl,
        sep_lbl,
        clock_lbl,
    };
    apply_live_timing(&entry, now, Local::now().offset());
    (card, entry)
}

fn sync_ticker(
    root: &gtk::Box,
    live_cards: &Rc<RefCell<Vec<LiveTaskEntry>>>,
    ticker_source: &Rc<RefCell<Option<glib::SourceId>>>,
) {
    let any_running = live_cards.borrow().iter().any(LiveTaskEntry::is_running);
    let should_run = root.is_mapped() && any_running;
    if !should_run {
        if let Some(src) = ticker_source.borrow_mut().take() {
            src.remove();
        }
        return;
    }

    if ticker_source.borrow().is_some() {
        return;
    }

    let cards_weak = Rc::downgrade(live_cards);
    let source_slot = ticker_source.clone();
    let root_weak = root.downgrade();
    let source_id = glib::timeout_add_seconds_local(1, move || {
        let Some(root_box) = root_weak.upgrade() else {
            let _ = source_slot.borrow_mut().take();
            return glib::ControlFlow::Break;
        };
        let Some(cards_rc) = cards_weak.upgrade() else {
            let _ = source_slot.borrow_mut().take();
            return glib::ControlFlow::Break;
        };
        let cards_borrow = cards_rc.borrow();
        if !root_box.is_mapped() || !cards_borrow.iter().any(LiveTaskEntry::is_running) {
            drop(cards_borrow);
            let _ = source_slot.borrow_mut().take();
            return glib::ControlFlow::Break;
        }
        refresh_live_cards(&cards_borrow, Utc::now());
        glib::ControlFlow::Continue
    });
    *ticker_source.borrow_mut() = Some(source_id);
}

fn set_label_if_changed(lbl: &gtk::Label, next: &str) {
    if lbl.label() != next {
        lbl.set_label(next);
    }
}

fn refresh_live_cards(cards: &[LiveTaskEntry], now: DateTime<Utc>) {
    let local_offset = *Local::now().offset();
    for entry in cards {
        apply_live_timing(entry, now, &local_offset);
    }
}

fn apply_live_timing<Tz: TimeZone>(entry: &LiveTaskEntry, now: DateTime<Utc>, tz: &Tz)
where
    Tz::Offset: std::fmt::Display,
{
    match entry {
        LiveTaskEntry::Single {
            due_at,
            paused_remaining_ms,
            countdown_lbl,
            sep_lbl,
            clock_lbl,
        } => {
            if let Some(rem_ms) = paused_remaining_ms {
                let rem_secs = (*rem_ms + 999) / 1000;
                set_label_if_changed(
                    countdown_lbl,
                    &format!("Paused ({})", format_stopwatch_clock(rem_secs)),
                );
                countdown_lbl.remove_css_class("claw-task-countdown-urgent");
                sep_lbl.set_visible(false);
                clock_lbl.set_visible(false);
                return;
            }
            let remaining_secs = due_at.signed_duration_since(now).num_seconds();
            set_label_if_changed(countdown_lbl, &format_task_countdown(remaining_secs));
            set_label_if_changed(clock_lbl, &format_due_clock(due_at, &now, tz));
            sep_lbl.set_visible(true);
            clock_lbl.set_visible(true);

            if remaining_secs <= 15 {
                countdown_lbl.add_css_class("claw-task-countdown-urgent");
            } else {
                countdown_lbl.remove_css_class("claw-task-countdown-urgent");
            }
        }
        LiveTaskEntry::Interval {
            due_at,
            paused_remaining_ms,
            phase_duration_secs,
            timer_lbl,
            sep_lbl,
            clock_lbl,
            progress_bar,
        } => {
            let remaining_ms = paused_remaining_ms
                .unwrap_or_else(|| due_at.signed_duration_since(now).num_milliseconds().max(0));
            let remaining_secs = (remaining_ms + 999) / 1000;
            set_label_if_changed(timer_lbl, &format_stopwatch_remaining(remaining_secs));

            if paused_remaining_ms.is_some() {
                sep_lbl.set_visible(false);
                clock_lbl.set_visible(false);
            } else {
                set_label_if_changed(clock_lbl, &format_due_clock(due_at, &now, tz));
                sep_lbl.set_visible(true);
                clock_lbl.set_visible(true);
            }

            let total_ms = (i64::from(*phase_duration_secs) * 1000).max(1000);
            let elapsed_ms = (total_ms - remaining_ms).clamp(0, total_ms);
            #[allow(clippy::cast_precision_loss)]
            progress_bar.set_fraction(elapsed_ms as f64 / total_ms as f64);
        }
    }
}

fn format_short_span(secs: u32) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}

#[must_use]
fn format_stopwatch_clock(remaining_secs: i64) -> String {
    let clamped = remaining_secs.max(0);
    if clamped >= 3600 {
        let h = clamped / 3600;
        let m = (clamped % 3600) / 60;
        let s = clamped % 60;
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        let m = clamped / 60;
        let s = clamped % 60;
        format!("{m:02}:{s:02}")
    }
}

#[must_use]
fn format_stopwatch_remaining(remaining_secs: i64) -> String {
    format!("{} left", format_stopwatch_clock(remaining_secs))
}

/// Filters to `TaskStatus::Pending` and sorts soonest effective due time first
/// (so pausing/resuming a task keeps its relative position stable without jumping).
#[must_use]
pub(super) fn sort_and_filter_pending_tasks(
    tasks: Vec<ScheduledTask>,
    now: DateTime<Utc>,
) -> Vec<ScheduledTask> {
    let effective_due = |t: &ScheduledTask| -> DateTime<Utc> {
        t.paused_remaining_ms
            .map_or(t.due_at, |ms| now + Duration::milliseconds(ms.max(0)))
    };

    let mut pending: Vec<ScheduledTask> = tasks
        .into_iter()
        .filter(|t| t.status == TaskStatus::Pending)
        .collect();
    pending.sort_by(|a, b| {
        effective_due(a)
            .cmp(&effective_due(b))
            .then_with(|| a.id.cmp(&b.id))
    });
    pending
}

/// Formats remaining seconds into a concise relative countdown (`"in 4m 12s"`, `"in 2h 05m"`, `"Due now"`).
#[must_use]
pub(super) fn format_task_countdown(remaining_secs: i64) -> String {
    if remaining_secs <= 0 {
        return "Due now".to_string();
    }
    if remaining_secs < 60 {
        return format!("in {remaining_secs}s");
    }
    if remaining_secs < 3600 {
        let mins = remaining_secs / 60;
        let secs = remaining_secs % 60;
        return if secs == 0 {
            format!("in {mins}m")
        } else {
            format!("in {mins}m {secs:02}s")
        };
    }
    if remaining_secs < 86_400 {
        let hours = remaining_secs / 3600;
        let mins = (remaining_secs % 3600) / 60;
        return if mins == 0 {
            format!("in {hours}h")
        } else {
            format!("in {hours}h {mins:02}m")
        };
    }
    let days = remaining_secs / 86_400;
    let hours = (remaining_secs % 86_400) / 3600;
    if hours == 0 {
        format!("in {days}d")
    } else {
        format!("in {days}d {hours}h")
    }
}

fn format_due_clock<Tz: TimeZone>(
    due_utc: &DateTime<Utc>,
    now_utc: &DateTime<Utc>,
    tz: &Tz,
) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let due_local = due_utc.with_timezone(tz);
    let now_local = now_utc.with_timezone(tz);
    let remaining_secs = due_utc.signed_duration_since(*now_utc).num_seconds();

    let due_date = due_local.date_naive();
    let now_date = now_local.date_naive();

    if due_date == now_date {
        if remaining_secs < 120 {
            due_local.format("%H:%M:%S").to_string()
        } else {
            due_local.format("%H:%M").to_string()
        }
    } else if Some(due_date) == now_date.succ_opt() {
        format!("Tomorrow {}", due_local.format("%H:%M"))
    } else if due_local.year() == now_local.year() {
        due_local.format("%b %-d, %H:%M").to_string()
    } else {
        due_local.format("%Y-%m-%d %H:%M").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_task_countdown_and_stopwatch_handle_all_ranges() {
        assert_eq!(format_task_countdown(-5), "Due now");
        assert_eq!(format_task_countdown(0), "Due now");
        assert_eq!(format_task_countdown(42), "in 42s");
        assert_eq!(format_task_countdown(180), "in 3m");
        assert_eq!(format_task_countdown(252), "in 4m 12s");
        assert_eq!(format_task_countdown(7500), "in 2h 05m");
        assert_eq!(format_task_countdown(93_600), "in 1d 2h");

        assert_eq!(format_stopwatch_remaining(0), "00:00 left");
        assert_eq!(format_stopwatch_remaining(52), "00:52 left");
        assert_eq!(format_stopwatch_remaining(125), "02:05 left");
        assert_eq!(format_stopwatch_remaining(3661), "01:01:01 left");
    }

    #[test]
    fn sort_and_filter_pending_tasks_orders_soonest_first_and_preserves_paused_rank() {
        let base = Utc::now();
        let later_id = Uuid::new_v4();
        let sooner_id = Uuid::new_v4();
        let paused_id = Uuid::new_v4();
        let done_id = Uuid::new_v4();

        let input = vec![
            ScheduledTask {
                id: later_id,
                task_type: TaskType::Notification,
                payload: "Second reminder".to_string(),
                due_at: base + Duration::minutes(15),
                status: TaskStatus::Pending,
                interval: None,
                paused_remaining_ms: None,
            },
            ScheduledTask {
                id: paused_id,
                task_type: TaskType::Notification,
                payload: "Paused session".to_string(),
                // Stale past `due_at`, but 5 minutes frozen remaining -> sorts between 3m and 15m!
                due_at: base - Duration::minutes(10),
                status: TaskStatus::Pending,
                interval: None,
                paused_remaining_ms: Some(300_000),
            },
            ScheduledTask {
                id: done_id,
                task_type: TaskType::Notification,
                payload: "Already done".to_string(),
                due_at: base + Duration::minutes(1),
                status: TaskStatus::Completed,
                interval: None,
                paused_remaining_ms: None,
            },
            ScheduledTask {
                id: sooner_id,
                task_type: TaskType::Command,
                payload: "cargo check".to_string(),
                due_at: base + Duration::minutes(3),
                status: TaskStatus::Pending,
                interval: None,
                paused_remaining_ms: None,
            },
        ];

        let sorted = sort_and_filter_pending_tasks(input, base);
        assert_eq!(sorted.len(), 3);
        assert_eq!(sorted[0].id, sooner_id);
        assert_eq!(sorted[1].id, paused_id);
        assert_eq!(sorted[2].id, later_id);
    }
}
