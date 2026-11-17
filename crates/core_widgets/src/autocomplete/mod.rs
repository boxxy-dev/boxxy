//! Popover-based autocomplete controller for [`gtk::Entry`] and [`gtk::TextView`] inputs.

mod engine;
mod style;

use self::engine::{compute_completion_edit, find_trigger_candidates};
use self::style::{badge_color_css_class, load_cached_avatar_texture};
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub display_name: String,
    pub replacement_text: String,
    pub icon_name: Option<String>,
    /// File-system path to an image (PNG/JPEG/WebP). Takes priority over `icon_name`.
    pub icon_path: Option<String>,
    pub secondary_text: Option<String>,
    pub badge_text: Option<String>,
    pub badge_color: Option<String>,
    pub disabled: bool,
}

pub trait CompletionProvider {
    fn trigger(&self) -> String;
    fn get_completions(&self, query: &str) -> Vec<CompletionItem>;
}

pub trait AutocompleteInput: Clone + 'static {
    fn widget(&self) -> &gtk::Widget;
    fn get_text(&self) -> String;
    fn cursor_char_pos(&self) -> usize;
    fn set_text_and_cursor(&self, text: &str, char_pos: usize);
    fn connect_text_changed(&self, f: Box<dyn Fn() + 'static>);
    fn connect_cursor_changed(&self, _f: Box<dyn Fn() + 'static>) {}
}

impl AutocompleteInput for gtk::Entry {
    fn widget(&self) -> &gtk::Widget {
        self.upcast_ref()
    }

    fn get_text(&self) -> String {
        self.text().to_string()
    }

    fn cursor_char_pos(&self) -> usize {
        usize::try_from(self.position().max(0)).unwrap_or(0)
    }

    fn set_text_and_cursor(&self, text: &str, char_pos: usize) {
        self.set_text(text);
        self.set_position(i32::try_from(char_pos).unwrap_or(i32::MAX));
    }

    fn connect_text_changed(&self, f: Box<dyn Fn() + 'static>) {
        self.connect_changed(move |_| f());
    }

    fn connect_cursor_changed(&self, f: Box<dyn Fn() + 'static>) {
        self.connect_cursor_position_notify(move |_| f());
    }
}

impl AutocompleteInput for gtk::TextView {
    fn widget(&self) -> &gtk::Widget {
        self.upcast_ref()
    }

    fn get_text(&self) -> String {
        let buf = self.buffer();
        buf.text(&buf.start_iter(), &buf.end_iter(), false)
            .to_string()
    }

    fn cursor_char_pos(&self) -> usize {
        usize::try_from(self.buffer().cursor_position().max(0)).unwrap_or(0)
    }

    fn set_text_and_cursor(&self, text: &str, char_pos: usize) {
        let buf = self.buffer();
        buf.set_text(text);
        let iter = buf.iter_at_offset(i32::try_from(char_pos).unwrap_or(i32::MAX));
        buf.place_cursor(&iter);
    }

    fn connect_text_changed(&self, f: Box<dyn Fn() + 'static>) {
        self.buffer().connect_changed(move |_| f());
    }

    fn connect_cursor_changed(&self, f: Box<dyn Fn() + 'static>) {
        self.buffer().connect_cursor_position_notify(move |_| f());
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AutocompleteState {
    Hidden,
    Active {
        trigger: String,
        start_idx: usize,
    },
    Dismissed {
        trigger: String,
        start_idx: usize,
        text_snapshot: String,
    },
}

trait AutocompleteInputObj {
    fn widget(&self) -> &gtk::Widget;
    fn get_text(&self) -> String;
    fn cursor_char_pos(&self) -> usize;
    fn set_text_and_cursor(&self, text: &str, char_pos: usize);
    fn connect_text_changed(&self, f: Box<dyn Fn() + 'static>);
    fn connect_cursor_changed(&self, f: Box<dyn Fn() + 'static>);
}

impl<T: AutocompleteInput> AutocompleteInputObj for T {
    fn widget(&self) -> &gtk::Widget {
        AutocompleteInput::widget(self)
    }

    fn get_text(&self) -> String {
        AutocompleteInput::get_text(self)
    }

    fn cursor_char_pos(&self) -> usize {
        AutocompleteInput::cursor_char_pos(self)
    }

    fn set_text_and_cursor(&self, text: &str, char_pos: usize) {
        AutocompleteInput::set_text_and_cursor(self, text, char_pos);
    }

    fn connect_text_changed(&self, f: Box<dyn Fn() + 'static>) {
        AutocompleteInput::connect_text_changed(self, f);
    }

    fn connect_cursor_changed(&self, f: Box<dyn Fn() + 'static>) {
        AutocompleteInput::connect_cursor_changed(self, f);
    }
}

pub struct AutocompleteController {
    input: Box<dyn AutocompleteInputObj>,
    popover: gtk::Popover,
    list: gtk::ListBox,
    plain_cmd_size_group: gtk::SizeGroup,
    providers: Vec<Box<dyn CompletionProvider>>,
    provider_triggers: Vec<String>,
    state: RefCell<AutocompleteState>,
    suppress_depth: Cell<u32>,
    last_items: RefCell<Vec<CompletionItem>>,
    last_query_key: RefCell<Option<(String, usize, String)>>,
    on_activated: Option<Box<dyn Fn(String) + 'static>>,
}

impl Drop for AutocompleteController {
    fn drop(&mut self) {
        if self.popover.parent().is_some() {
            self.popover.unparent();
        }
    }
}

impl AutocompleteController {
    #[must_use]
    pub fn new<I: AutocompleteInput>(
        entry: &I,
        providers: Vec<Box<dyn CompletionProvider>>,
        on_activated: Option<Box<dyn Fn(String) + 'static>>,
    ) -> Rc<Self> {
        let popover = gtk::Popover::new();
        popover.set_parent(entry.widget());
        popover.set_position(gtk::PositionType::Top);
        popover.set_autohide(false);
        popover.set_has_arrow(false);
        popover.add_css_class("autocomplete-popover");

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.add_css_class("completion-list");
        list.set_focusable(false);
        list.set_valign(gtk::Align::Start);

        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_propagate_natural_height(true);
        scroll.set_max_content_height(300);
        scroll.set_child(Some(&list));

        popover.set_child(Some(&scroll));
        popover.set_halign(gtk::Align::Start);

        let provider_triggers = providers.iter().map(|p| p.trigger()).collect();

        let controller = Rc::new(Self {
            input: Box::new(entry.clone()),
            popover,
            list,
            plain_cmd_size_group: gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal),
            providers,
            provider_triggers,
            state: RefCell::new(AutocompleteState::Hidden),
            suppress_depth: Cell::new(0),
            last_items: RefCell::new(Vec::new()),
            last_query_key: RefCell::new(None),
            on_activated,
        });

        controller.setup_signals();
        controller
    }

    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.popover.is_visible()
    }

    #[must_use]
    pub fn state(&self) -> AutocompleteState {
        self.state.borrow().clone()
    }

    /// Dismisses the autocomplete popup if it is currently visible.
    /// Transitions state to `Dismissed` for the current token/text snapshot
    /// so it stays closed until the user modifies the text.
    /// Returns `true` if the popup was visible and was dismissed.
    pub fn dismiss(&self) -> bool {
        if self.popover.is_visible() {
            let current = self.state.borrow().clone();
            let next_state = if let AutocompleteState::Active { trigger, start_idx } = current {
                AutocompleteState::Dismissed {
                    trigger,
                    start_idx,
                    text_snapshot: self.input.get_text(),
                }
            } else {
                AutocompleteState::Hidden
            };
            *self.state.borrow_mut() = next_state;
            self.popover.popdown();
            true
        } else {
            false
        }
    }

    pub fn hide(&self) {
        *self.state.borrow_mut() = AutocompleteState::Hidden;
        *self.last_query_key.borrow_mut() = None;
        if self.popover.is_visible() {
            self.popover.popdown();
        }
    }

    /// Runs a closure (such as programmatic buffer changes from history navigation)
    /// while suppressing autocomplete popup evaluation and hiding any open popup.
    pub fn with_suppressed<F: FnOnce()>(&self, f: F) {
        self.hide();
        self.suppress_depth
            .set(self.suppress_depth.get().saturating_add(1));
        f();
        self.suppress_depth
            .set(self.suppress_depth.get().saturating_sub(1));
    }

    /// Handles navigation, completion activation, and Escape dismissal without modifier state.
    pub fn handle_key_pressed(&self, key: gdk::Key) -> glib::Propagation {
        self.handle_key_pressed_with_state(key, gdk::ModifierType::empty())
    }

    /// Handles navigation, completion activation, and Escape dismissal when the
    /// autocomplete popup is visible, respecting modifier keys (`Shift+Enter`, `Shift+Tab`, `Ctrl`).
    pub fn handle_key_pressed_with_state(
        &self,
        key: gdk::Key,
        state: gdk::ModifierType,
    ) -> glib::Propagation {
        if !self.popover.is_visible() {
            return glib::Propagation::Proceed;
        }

        // Never intercept Ctrl/Alt/Super/Meta shortcuts while the popup is open.
        let chord_mods = state.intersection(
            gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::ALT_MASK
                | gdk::ModifierType::SUPER_MASK
                | gdk::ModifierType::META_MASK,
        );
        if !chord_mods.is_empty() {
            return glib::Propagation::Proceed;
        }

        let is_shift = state.contains(gdk::ModifierType::SHIFT_MASK);

        match key {
            gdk::Key::Up if !is_shift => {
                self.move_selection(-1);
                glib::Propagation::Stop
            }
            gdk::Key::ISO_Left_Tab => {
                self.move_selection(-1);
                glib::Propagation::Stop
            }
            gdk::Key::Tab if is_shift => {
                self.move_selection(-1);
                glib::Propagation::Stop
            }
            gdk::Key::Down if !is_shift => {
                self.move_selection(1);
                glib::Propagation::Stop
            }
            gdk::Key::Page_Up if !is_shift => {
                self.move_selection(-8);
                glib::Propagation::Stop
            }
            gdk::Key::Page_Down if !is_shift => {
                self.move_selection(8);
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::Tab if !is_shift => {
                if self.activate_selected_row() {
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
            gdk::Key::Escape => {
                self.dismiss();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    fn setup_signals(self: &Rc<Self>) {
        // Ensure the custom popover child is cleanly unparented if the host widget disposes first.
        let popover_weak = self.popover.downgrade();
        self.input.widget().connect_destroy(move |_| {
            if let Some(p) = popover_weak.upgrade()
                && p.parent().is_some()
            {
                p.unparent();
            }
        });

        // Capture `Weak<Self>` across all widget signal closures so closing a pane drops
        // `AutocompleteController`, `GtkPopover`, and `CompletionProvider`s without reference cycles.
        let weak_text = Rc::downgrade(self);
        self.input.connect_text_changed(Box::new(move || {
            if let Some(ctrl) = weak_text.upgrade() {
                ctrl.evaluate_completions(true);
            }
        }));

        let weak_cursor = Rc::downgrade(self);
        self.input.connect_cursor_changed(Box::new(move || {
            if let Some(ctrl) = weak_cursor.upgrade()
                && ctrl.popover.is_visible()
            {
                // Re-validate active trigger bounds when the cursor moves while open so moving
                // outside the token closes the popup instead of retaining a stale `start_idx`.
                ctrl.evaluate_completions(false);
            }
        }));

        let weak_row = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            if let Some(ctrl) = weak_row.upgrade() {
                ctrl.activate_row_index(row.index());
            }
        });

        let key_ctrl = gtk::EventControllerKey::new();
        key_ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak_key = Rc::downgrade(self);
        key_ctrl.connect_key_pressed(move |_, key, _, state| {
            weak_key
                .upgrade()
                .map_or(glib::Propagation::Proceed, |ctrl| {
                    ctrl.handle_key_pressed_with_state(key, state)
                })
        });
        self.input.widget().add_controller(key_ctrl);
    }

    fn evaluate_completions(&self, allow_open: bool) {
        if self.suppress_depth.get() > 0 {
            self.hide();
            return;
        }

        let text = self.input.get_text();
        let cursor_pos = self.input.cursor_char_pos();

        let candidates = find_trigger_candidates(
            &text,
            cursor_pos,
            self.provider_triggers
                .iter()
                .enumerate()
                .map(|(i, t)| (i, t.as_str())),
        );

        let mut found_trigger = None;
        for cand in candidates {
            let completions = self.providers[cand.provider_idx].get_completions(cand.query);
            if !completions.is_empty() {
                let trigger = self.provider_triggers[cand.provider_idx].clone();
                let query_owned = cand.query.to_string();
                found_trigger = Some((completions, cand.start_byte_idx, trigger, query_owned));
                break;
            }
        }

        let Some((completions, idx, trigger, query_owned)) = found_trigger else {
            self.hide();
            return;
        };

        // Keep dismissed if the user explicitly pressed Escape on this exact token + text snapshot.
        let is_dismissed_same = matches!(
            &*self.state.borrow(),
            AutocompleteState::Dismissed {
                trigger: d_trig,
                start_idx: d_idx,
                text_snapshot: d_text,
            } if d_trig == &trigger && *d_idx == idx && d_text == &text
        );
        if is_dismissed_same {
            return;
        }

        let was_visible = self.popover.is_visible();
        if !was_visible && !allow_open {
            return;
        }

        let query_key = (trigger.clone(), idx, query_owned);
        let same_query = self.last_query_key.borrow().as_ref() == Some(&query_key);
        let items_changed = *self.last_items.borrow() != completions;

        if items_changed || !was_visible {
            self.update_list(&completions, was_visible, same_query);
            *self.last_items.borrow_mut() = completions;
        }
        *self.last_query_key.borrow_mut() = Some(query_key);
        *self.state.borrow_mut() = AutocompleteState::Active {
            trigger,
            start_idx: idx,
        };

        if was_visible {
            if items_changed {
                // Resize and present in place so the popup surface does not unmap/remap per keystroke.
                self.popover.queue_resize();
                self.popover.present();
            }
        } else {
            self.popover.popup();
        }
    }

    #[allow(clippy::too_many_lines)]
    fn update_list(&self, items: &[CompletionItem], was_visible: bool, preserve_selection: bool) {
        let prev_selected_replacement = if preserve_selection {
            self.list
                .selected_row()
                .and_then(|r| usize::try_from(r.index()).ok())
                .and_then(|i| {
                    self.last_items
                        .borrow()
                        .get(i)
                        .map(|it| it.replacement_text.clone())
                })
        } else {
            None
        };

        let prev_items = self.last_items.borrow();
        let mut first_selectable: Option<gtk::ListBoxRow> = None;
        let mut preserved_row: Option<gtk::ListBoxRow> = None;

        for (idx, item) in items.iter().enumerate() {
            let row_idx = i32::try_from(idx).unwrap_or(i32::MAX);

            if was_visible
                && prev_items.get(idx) == Some(item)
                && let Some(existing) = self.list.row_at_index(row_idx)
            {
                if !item.disabled {
                    if first_selectable.is_none() {
                        first_selectable = Some(existing.clone());
                    }
                    if preserved_row.is_none()
                        && prev_selected_replacement.as_deref()
                            == Some(item.replacement_text.as_str())
                    {
                        preserved_row = Some(existing);
                    }
                }
                continue;
            }

            let row = self.list.row_at_index(row_idx).unwrap_or_else(|| {
                let new_row = gtk::ListBoxRow::new();
                self.list.append(&new_row);
                new_row
            });

            row.set_sensitive(!item.disabled);
            row.set_activatable(!item.disabled);
            row.set_selectable(!item.disabled);

            let is_plain_command =
                item.badge_text.is_none() && item.icon_name.is_none() && item.icon_path.is_none();

            let hbox = gtk::Box::new(
                gtk::Orientation::Horizontal,
                if is_plain_command { 12 } else { 8 },
            );
            hbox.set_margin_top(4);
            hbox.set_margin_bottom(4);
            hbox.set_margin_start(6);
            hbox.set_margin_end(6);

            if let Some(path) = item.icon_path.as_deref().filter(|p| !p.is_empty()) {
                if let Some(texture) = load_cached_avatar_texture(path) {
                    let avatar = adw::Avatar::new(32, None, false);
                    avatar.set_custom_image(Some(&texture));
                    hbox.append(&avatar);
                } else if let Some(icon) = item.icon_name.as_deref() {
                    let img = gtk::Image::from_icon_name(icon);
                    img.add_css_class("dim-label");
                    hbox.append(&img);
                }
            } else if let Some(icon) = item.icon_name.as_deref() {
                let img = gtk::Image::from_icon_name(icon);
                img.add_css_class("dim-label");
                hbox.append(&img);
            }

            let label = gtk::Label::new(Some(&item.display_name));
            label.set_xalign(0.0);

            let is_file_item = item.replacement_text.starts_with('@');

            // Only force a wide width for rich items (session resume or @file paths)
            // so simple slash command suggestions stay compact.
            if item.badge_text.is_some() {
                label.set_hexpand(true);
                label.set_halign(gtk::Align::Fill);
                label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                label.set_width_request(400);
            } else if is_file_item {
                label.set_hexpand(true);
                label.set_halign(gtk::Align::Fill);
                label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                label.set_width_request(380);
            } else if is_plain_command {
                label.set_hexpand(false);
                label.set_halign(gtk::Align::Start);
                label.set_ellipsize(gtk::pango::EllipsizeMode::None);
                label.set_width_request(-1);
                self.plain_cmd_size_group.add_widget(&label);
            } else {
                label.set_hexpand(true);
                label.set_halign(gtk::Align::Fill);
                label.set_ellipsize(gtk::pango::EllipsizeMode::None);
                label.set_width_request(-1);
            }

            let has_subtitle = item.display_name.contains('\n');
            if let Some((title_line, sub_line)) = item.display_name.split_once('\n') {
                label.set_text(title_line);
                let sub_label = gtk::Label::new(Some(sub_line));
                sub_label.set_xalign(0.0);
                sub_label.set_halign(gtk::Align::Start);
                sub_label.add_css_class("caption");
                sub_label.add_css_class("dim-label");

                let text_vbox = gtk::Box::new(gtk::Orientation::Vertical, 1);
                text_vbox.set_valign(gtk::Align::Center);
                text_vbox.set_hexpand(true);
                text_vbox.set_margin_end(8);
                text_vbox.append(&label);
                text_vbox.append(&sub_label);
                hbox.append(&text_vbox);
            } else {
                hbox.append(&label);
            }

            let right_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            right_box.set_valign(if has_subtitle {
                gtk::Align::Start
            } else {
                gtk::Align::Center
            });
            if has_subtitle {
                right_box.set_margin_top(2);
            }
            if is_plain_command {
                right_box.set_hexpand(true);
                right_box.set_halign(gtk::Align::Start);
            } else {
                right_box.set_halign(gtk::Align::End);
            }

            if let Some(ref secondary) = item.secondary_text {
                let sec_label = gtk::Label::new(Some(secondary));
                sec_label.add_css_class("caption");
                sec_label.add_css_class("dim-label");
                if is_plain_command {
                    sec_label.set_xalign(0.0);
                    sec_label.set_halign(gtk::Align::Start);
                } else {
                    sec_label.set_width_request(if is_file_item { 56 } else { 40 });
                    sec_label.set_xalign(1.0);
                    sec_label.set_halign(gtk::Align::End);
                }
                right_box.append(&sec_label);
            }

            if let Some(ref badge) = item.badge_text {
                let badge_label = gtk::Label::new(Some(badge));
                badge_label.add_css_class("caption");

                let badge_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                badge_box.set_width_request(140);
                badge_box.set_halign(gtk::Align::End);

                let inner_badge_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                inner_badge_box.set_halign(gtk::Align::Center);

                if let Some(ref color) = item.badge_color {
                    let css_class = badge_color_css_class(color);
                    inner_badge_box.add_css_class(&css_class);
                }

                inner_badge_box.append(&badge_label);
                badge_box.append(&inner_badge_box);
                right_box.append(&badge_box);
            }

            hbox.append(&right_box);
            row.set_child(Some(&hbox));

            if !item.disabled {
                if first_selectable.is_none() {
                    first_selectable = Some(row.clone());
                }
                if preserved_row.is_none()
                    && prev_selected_replacement.as_deref() == Some(item.replacement_text.as_str())
                {
                    preserved_row = Some(row);
                }
            }
        }

        drop(prev_items);

        // Trim leftover rows from a previously larger completion list
        let tail_start = i32::try_from(items.len()).unwrap_or(i32::MAX);
        while let Some(extra) = self.list.row_at_index(tail_start) {
            self.list.remove(&extra);
        }

        let target_selection = preserved_row.or(first_selectable);
        if self.list.selected_row() != target_selection {
            self.list.select_row(target_selection.as_ref());
        }
    }

    fn move_selection(&self, delta: i32) {
        if delta == 0 {
            return;
        }

        let step = delta.signum();
        let steps_desired = delta.unsigned_abs();
        let total_rows = i32::try_from(self.last_items.borrow().len()).unwrap_or(0);
        if total_rows <= 0 {
            return;
        }

        let mut cursor = self
            .list
            .selected_row()
            .map_or_else(|| if step > 0 { -1 } else { total_rows }, |r| r.index());

        let mut chosen: Option<gtk::ListBoxRow> = None;
        let mut steps_taken = 0_u32;

        while steps_taken < steps_desired {
            cursor += step;
            if cursor < 0 || cursor >= total_rows {
                break;
            }
            if let Some(row) = self.list.row_at_index(cursor) {
                if row.is_selectable() {
                    chosen = Some(row);
                    steps_taken += 1;
                }
            } else {
                break;
            }
        }

        if let Some(row) = chosen {
            self.list.select_row(Some(&row));
            self.scroll_row_into_view(&row);
        }
    }

    fn scroll_row_into_view(&self, row: &gtk::ListBoxRow) {
        // In GTK4, ListBox does not auto-scroll when selection changes without focus transfer.
        let mut current_parent = self.list.parent();
        while let Some(p) = current_parent {
            if let Some(scroll) = p.downcast_ref::<gtk::ScrolledWindow>() {
                let vadj = scroll.vadjustment();
                let row_h = f64::from(row.height());
                let row_y = row
                    .compute_point(&self.list, &gtk::graphene::Point::new(0.0, 0.0))
                    .map_or(0.0, |pt| f64::from(pt.y()));

                let page_size = vadj.page_size();
                let value = vadj.value();

                if row_y < value {
                    vadj.set_value(row_y);
                } else if row_y + row_h > value + page_size {
                    vadj.set_value(row_y + row_h - page_size);
                }
                break;
            }
            current_parent = p.parent();
        }
    }

    fn activate_selected_row(&self) -> bool {
        let Some(row) = self.list.selected_row() else {
            return false;
        };
        self.activate_row_index(row.index())
    }

    fn activate_row_index(&self, row_index: i32) -> bool {
        let Ok(idx) = usize::try_from(row_index) else {
            return false;
        };
        let replacement = {
            let items = self.last_items.borrow();
            let Some(item) = items.get(idx) else {
                return false;
            };
            if item.disabled {
                return false;
            }
            item.replacement_text.clone()
        };
        self.apply_completion(&replacement);
        true
    }

    fn apply_completion(&self, replacement: &str) {
        self.popover.popdown();
        *self.last_query_key.borrow_mut() = None;
        let current = std::mem::replace(&mut *self.state.borrow_mut(), AutocompleteState::Hidden);

        let AutocompleteState::Active { start_idx, .. } = current else {
            return;
        };

        let text = self.input.get_text();
        let cursor_pos = self.input.cursor_char_pos();
        let Some(edit) = compute_completion_edit(&text, cursor_pos, start_idx, replacement) else {
            return;
        };

        if edit.should_retrigger {
            self.input
                .set_text_and_cursor(&edit.new_text, edit.new_cursor_char_pos);
        } else {
            self.with_suppressed(|| {
                self.input
                    .set_text_and_cursor(&edit.new_text, edit.new_cursor_char_pos);
            });
        }

        if let Some(on_activated) = &self.on_activated {
            on_activated(replacement.to_string());
        }
    }
}
