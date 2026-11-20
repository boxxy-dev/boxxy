use crate::catalog::{CommandItem, builtin_commands};
use crate::search::{IndexedEntry, score_entries};
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

struct PaletteState {
    static_count: usize,
    entries: Vec<IndexedEntry>,
    scores: Vec<Option<u32>>,
    row_to_entry: HashMap<usize, usize>,
}

impl PaletteState {
    fn new(static_items: Vec<CommandItem>) -> Self {
        let static_count = static_items.len();
        let entries: Vec<IndexedEntry> = static_items.into_iter().map(IndexedEntry::new).collect();
        let scores = vec![Some(0); entries.len()];
        Self {
            static_count,
            entries,
            scores,
            row_to_entry: HashMap::with_capacity(static_count + 16),
        }
    }

    fn row_key(row: &gtk4::ListBoxRow) -> usize {
        row.as_ptr() as usize
    }

    fn entry_index_for_row(&self, row: &gtk4::ListBoxRow) -> Option<usize> {
        self.row_to_entry.get(&Self::row_key(row)).copied()
    }

    fn entry_for_row(&self, row: &gtk4::ListBoxRow) -> Option<&IndexedEntry> {
        let idx = self.entry_index_for_row(row)?;
        self.entries.get(idx)
    }

    fn score_for_row(&self, row: &gtk4::ListBoxRow) -> Option<u32> {
        let idx = self.entry_index_for_row(row)?;
        self.scores.get(idx).copied().flatten()
    }

    fn is_row_visible(&self, row: &gtk4::ListBoxRow) -> bool {
        self.score_for_row(row).is_some()
    }

    fn update_query(&mut self, query: &str) {
        score_entries(&self.entries, query, &mut self.scores);
    }

    fn dynamic_items_equal(&self, incoming: &[CommandItem]) -> bool {
        let current_dynamic = &self.entries[self.static_count..];
        if current_dynamic.len() != incoming.len() {
            return false;
        }
        current_dynamic
            .iter()
            .zip(incoming.iter())
            .all(|(indexed, item)| &indexed.item == item)
    }
}

/// Searchable `Ctrl+Shift+P` Command Palette popover component.
#[derive(Clone)]
pub struct CommandPaletteComponent {
    popover: gtk4::Popover,
    search_entry: gtk4::SearchEntry,
    listbox: gtk4::ListBox,
    scrolled_window: gtk4::ScrolledWindow,
    state: Rc<RefCell<PaletteState>>,
    focus_source: Rc<Cell<Option<glib::SourceId>>>,
}

impl std::fmt::Debug for CommandPaletteComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandPaletteComponent")
            .finish_non_exhaustive()
    }
}

impl CommandPaletteComponent {
    /// Constructs a new Command Palette loaded from `/dev/boxxy/BoxxyTerminal/ui/command_palette.ui`.
    ///
    /// # Panics
    /// Panics if the compiled `GResource` `ui/command_palette.ui` is missing expected widget IDs.
    #[must_use]
    pub fn new() -> Self {
        let builder =
            gtk4::Builder::from_resource("/dev/boxxy/BoxxyTerminal/ui/command_palette.ui");

        let popover: gtk4::Popover = builder
            .object("popover")
            .expect("popover in command_palette.ui");
        let search_entry: gtk4::SearchEntry = builder
            .object("search_entry")
            .expect("search_entry in command_palette.ui");
        let listbox: gtk4::ListBox = builder
            .object("listbox")
            .expect("listbox in command_palette.ui");
        let scrolled_window: gtk4::ScrolledWindow = builder
            .object("scrolled_window")
            .expect("scrolled_window in command_palette.ui");
        let empty_placeholder: gtk4::Box = builder
            .object("empty_placeholder")
            .expect("empty_placeholder in command_palette.ui");

        listbox.set_placeholder(Some(&empty_placeholder));
        search_entry.set_key_capture_widget(Some(&listbox));

        let state = Rc::new(RefCell::new(PaletteState::new(builtin_commands())));
        let focus_source: Rc<Cell<Option<glib::SourceId>>> = Rc::new(Cell::new(None));

        let comp = Self {
            popover,
            search_entry,
            listbox,
            scrolled_window,
            state,
            focus_source,
        };

        comp.populate_initial_rows();
        comp.setup_filtering_and_sorting();
        comp.setup_signals();

        comp
    }

    fn populate_initial_rows(&self) {
        let rows = {
            let mut state = self.state.borrow_mut();
            let mut built = Vec::with_capacity(state.static_count);
            for idx in 0..state.static_count {
                let row = build_row(&state.entries[idx].item);
                let key = PaletteState::row_key(&row);
                state.row_to_entry.insert(key, idx);
                built.push(row);
            }
            built
        };

        for row in rows {
            self.listbox.append(&row);
        }
    }

    fn setup_filtering_and_sorting(&self) {
        let state_filter = Rc::downgrade(&self.state);
        self.listbox.set_filter_func(move |row| {
            state_filter
                .upgrade()
                .and_then(|st| st.try_borrow().ok().map(|s| s.is_row_visible(row)))
                .unwrap_or(true)
        });

        let state_sort = Rc::downgrade(&self.state);
        self.listbox.set_sort_func(move |row_a, row_b| {
            let Some(st) = state_sort.upgrade() else {
                return gtk4::Ordering::Equal;
            };
            let Ok(state) = st.try_borrow() else {
                return gtk4::Ordering::Equal;
            };
            let idx_a = state.entry_index_for_row(row_a).unwrap_or(usize::MAX);
            let idx_b = state.entry_index_for_row(row_b).unwrap_or(usize::MAX);
            let score_a = state.scores.get(idx_a).copied().flatten().unwrap_or(0);
            let score_b = state.scores.get(idx_b).copied().flatten().unwrap_or(0);

            // Higher scores rank first; ties preserve registration order (`idx_a < idx_b`).
            let ord = score_b.cmp(&score_a).then_with(|| idx_a.cmp(&idx_b));
            match ord {
                Ordering::Less => gtk4::Ordering::Smaller,
                Ordering::Equal => gtk4::Ordering::Equal,
                Ordering::Greater => gtk4::Ordering::Larger,
            }
        });
    }

    #[allow(clippy::too_many_lines)]
    fn setup_signals(&self) {
        // Capture-phase dismissal on Escape or the active Command Palette shortcut
        // (GtkPopover is its own GtkNative surface, so window-level controllers don't see keys while open).
        let esc_ctrl = gtk4::EventControllerKey::new();
        esc_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
        esc_ctrl.connect_key_pressed(glib::clone!(
            #[weak(rename_to = popover)]
            self.popover,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |ctrl, keyval, _, _| {
                if keyval == gtk4::gdk::Key::Escape {
                    popover.popdown();
                    return glib::Propagation::Stop;
                }
                if let Some(ev) = ctrl.current_event()
                    && let Ok(key_ev) = ev.downcast::<gtk4::gdk::KeyEvent>()
                {
                    let km = boxxy_keybindings::keymap();
                    for chord in km.chords_for(boxxy_keybindings::ShortcutId::CommandPalette) {
                        if !matches!(
                            key_ev.matches(chord.keyval, chord.modifiers),
                            gtk4::gdk::KeyMatch::None
                        ) {
                            popover.popdown();
                            return glib::Propagation::Stop;
                        }
                    }
                }
                glib::Propagation::Proceed
            }
        ));
        self.popover.add_controller(esc_ctrl);

        // Single batch scoring + filter/sort refresh per query edit.
        let state_search = Rc::downgrade(&self.state);
        self.search_entry.connect_search_changed(glib::clone!(
            #[weak(rename_to = listbox)]
            self.listbox,
            #[weak(rename_to = scrolled)]
            self.scrolled_window,
            move |entry| {
                let Some(state_rc) = state_search.upgrade() else {
                    return;
                };
                let query = entry.text();
                state_rc.borrow_mut().update_query(query.as_str());

                listbox.invalidate_filter();
                listbox.invalidate_sort();
                scrolled.vadjustment().set_value(0.0);

                let first = find_first_visible_row(&listbox, &state_rc.borrow());
                select_or_clear(&listbox, first.as_ref());
            }
        ));

        // Enter in SearchEntry activates the currently selected visible row synchronously.
        let state_activate = Rc::downgrade(&self.state);
        self.search_entry.connect_activate(glib::clone!(
            #[weak(rename_to = popover)]
            self.popover,
            #[weak(rename_to = listbox)]
            self.listbox,
            move |_| {
                if let Some(row) = listbox.selected_row() {
                    activate_row(&popover, &state_activate, &row);
                }
            }
        ));

        // Clicking / activating a ListBoxRow dispatches through the same synchronous path.
        let state_row_act = Rc::downgrade(&self.state);
        self.listbox.connect_row_activated(glib::clone!(
            #[weak(rename_to = popover)]
            self.popover,
            move |_, row| {
                activate_row(&popover, &state_row_act, row);
            }
        ));

        // Capture-phase keyboard navigation (`Up`, `Down`, `Page_Up`, `Page_Down`) that scrolls
        // rows smoothly without stealing focus from `SearchEntry` or selecting its text.
        let nav_ctrl = gtk4::EventControllerKey::new();
        nav_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let state_nav = Rc::downgrade(&self.state);
        nav_ctrl.connect_key_pressed(glib::clone!(
            #[weak(rename_to = listbox)]
            self.listbox,
            #[weak(rename_to = scrolled)]
            self.scrolled_window,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, keyval, _, _| {
                let step = match keyval {
                    gtk4::gdk::Key::Down => NavStep::Next,
                    gtk4::gdk::Key::Up => NavStep::Prev,
                    gtk4::gdk::Key::Page_Down => NavStep::PageDown,
                    gtk4::gdk::Key::Page_Up => NavStep::PageUp,
                    _ => return glib::Propagation::Proceed,
                };

                let Some(state_rc) = state_nav.upgrade() else {
                    return glib::Propagation::Stop;
                };
                let visible = collect_visible_rows(&listbox, &state_rc.borrow());
                if visible.is_empty() {
                    listbox.unselect_all();
                    return glib::Propagation::Stop;
                }

                let cur_pos = listbox
                    .selected_row()
                    .and_then(|sel| visible.iter().position(|r| r == &sel));

                let len = visible.len();
                let target_pos = match step {
                    NavStep::Next => cur_pos.map_or(0, |p| (p + 1) % len),
                    NavStep::Prev => {
                        cur_pos.map_or(len - 1, |p| if p == 0 { len - 1 } else { p - 1 })
                    }
                    NavStep::PageDown => cur_pos.map_or(0, |p| (p + 6).min(len - 1)),
                    NavStep::PageUp => cur_pos.map_or(0, |p| p.saturating_sub(6)),
                };

                let target_row = &visible[target_pos];
                listbox.select_row(Some(target_row));
                scroll_row_into_view(&scrolled, &listbox, target_row);
                glib::Propagation::Stop
            }
        ));
        self.search_entry.add_controller(nav_ctrl);

        // Tracked idle focus backup on map, cancelled immediately on unmap to avoid timers outliving popdown.
        let focus_on_map = self.focus_source.clone();
        self.popover.connect_map(glib::clone!(
            #[weak(rename_to = popover)]
            self.popover,
            #[weak(rename_to = entry)]
            self.search_entry,
            move |_| {
                if let Some(prev) = focus_on_map.take() {
                    prev.remove();
                }
                let focus_slot = focus_on_map.clone();
                let source_id = glib::idle_add_local_once(move || {
                    focus_slot.set(None);
                    if popover.is_visible() && !entry.has_focus() {
                        entry.grab_focus();
                    }
                });
                focus_on_map.set(Some(source_id));
            }
        ));

        let focus_on_unmap = self.focus_source.clone();
        self.popover.connect_unmap(move |_| {
            if let Some(pending) = focus_on_unmap.take() {
                pending.remove();
            }
        });
    }

    /// Replaces dynamic commands (e.g. user script bookmarks) incrementally without recreating
    /// the static command and Preferences category rows.
    pub fn set_dynamic_commands(&self, commands: Vec<CommandItem>) {
        {
            let state = self.state.borrow();
            if state.dynamic_items_equal(&commands) {
                return;
            }
        }

        let static_count = self.state.borrow().static_count;

        // Remove only trailing dynamic rows from `listbox` and `row_to_entry`.
        let mut to_remove = Vec::new();
        let mut child_opt = self.listbox.first_child();
        while let Some(child) = child_opt {
            let next = child.next_sibling();
            if let Ok(row) = child.downcast::<gtk4::ListBoxRow>() {
                let is_dynamic = self
                    .state
                    .borrow()
                    .entry_index_for_row(&row)
                    .is_some_and(|idx| idx >= static_count);
                if is_dynamic {
                    to_remove.push(row);
                }
            }
            child_opt = next;
        }

        for row in &to_remove {
            self.listbox.remove(row);
        }

        let new_rows: Vec<gtk4::ListBoxRow> = commands.iter().map(build_row).collect();
        let query = self.search_entry.text();

        // Update pure Rust state completely before calling `listbox.append`, because GTK's
        // `gtk_list_box_insert` synchronously invokes `sort_func` and `filter_func`.
        {
            let mut state = self.state.borrow_mut();
            for row in &to_remove {
                state.row_to_entry.remove(&PaletteState::row_key(row));
            }

            state.entries.truncate(static_count);
            state.entries.reserve(commands.len());

            for (item, row) in commands.into_iter().zip(&new_rows) {
                let idx = state.entries.len();
                let key = PaletteState::row_key(row);
                state.entries.push(IndexedEntry::new(item));
                state.row_to_entry.insert(key, idx);
            }

            state.update_query(query.as_str());
        }

        for row in new_rows {
            self.listbox.append(&row);
        }

        self.listbox.invalidate_filter();
        self.listbox.invalidate_sort();
        if self.popover.is_visible() {
            let first = find_first_visible_row(&self.listbox, &self.state.borrow());
            select_or_clear(&self.listbox, first.as_ref());
        }
    }

    #[must_use]
    pub const fn widget(&self) -> &gtk4::Popover {
        &self.popover
    }

    /// Detaches the popover from its parent window during teardown so GTK does not emit
    /// leftover-child warnings when destroying a window.
    pub fn unparent(&self) {
        if let Some(pending) = self.focus_source.take() {
            pending.remove();
        }
        if self.popover.parent().is_some() {
            self.popover.unparent();
        }
    }

    /// Synchronizes static command shortcut badges with the active `boxxy_keybindings::keymap()`.
    pub fn sync_shortcuts(&self) {
        let fresh = builtin_commands();
        let mut changed_indices = Vec::new();
        {
            let mut state = self.state.borrow_mut();
            if fresh.len() != state.static_count {
                return;
            }
            for (idx, item) in fresh.into_iter().enumerate() {
                if state.entries[idx].item.shortcut != item.shortcut {
                    state.entries[idx] = IndexedEntry::new(item);
                    changed_indices.push(idx);
                }
            }
        }

        if changed_indices.is_empty() {
            return;
        }

        let state = self.state.borrow();
        let mut child_opt = self.listbox.first_child();
        while let Some(child) = child_opt {
            if let Ok(row) = child.clone().downcast::<gtk4::ListBoxRow>()
                && let Some(idx) = state.entry_index_for_row(&row)
                && changed_indices.contains(&idx)
            {
                update_row_shortcut(&row, state.entries[idx].item.shortcut.as_deref());
            }
            child_opt = child.next_sibling();
        }
    }

    /// Opens (or toggles closed if already open on `parent`) the Command Palette centered near
    /// the top of `parent`.
    pub fn show(&self, parent: &impl IsA<gtk4::Widget>) {
        self.sync_shortcuts();
        let parent_widget = parent.upcast_ref();
        if self.popover.is_visible() && self.popover.parent().as_ref() == Some(parent_widget) {
            self.popover.popdown();
            return;
        }

        if self.popover.parent().as_ref() != Some(parent_widget) {
            if self.popover.parent().is_some() {
                self.popover.unparent();
            }
            self.popover.set_parent(parent_widget);
        }

        let target_width = (parent_widget.width() - 56).clamp(260, 450);
        self.search_entry.set_width_request(target_width);

        self.popover.set_halign(gtk4::Align::Center);
        self.popover.set_valign(gtk4::Align::Start);
        self.popover.set_position(gtk4::PositionType::Bottom);

        let rect = gtk4::gdk::Rectangle::new(parent_widget.width() / 2, 60, 0, 0);
        self.popover.set_pointing_to(Some(&rect));

        // Reset query and selection synchronously prior to popup so row 0 is ready with 0ms delay.
        if self.search_entry.text().is_empty() {
            self.state.borrow_mut().update_query("");
            self.listbox.invalidate_filter();
            self.listbox.invalidate_sort();
            let first = find_first_visible_row(&self.listbox, &self.state.borrow());
            select_or_clear(&self.listbox, first.as_ref());
        } else {
            self.search_entry.set_text("");
        }
        self.scrolled_window.vadjustment().set_value(0.0);

        self.popover.popup();
        self.search_entry.grab_focus();
    }
}

impl Default for CommandPaletteComponent {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
enum NavStep {
    Next,
    Prev,
    PageDown,
    PageUp,
}

fn build_row(cmd: &CommandItem) -> gtk4::ListBoxRow {
    let row = gtk4::ListBoxRow::new();

    let hbox = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    hbox.set_margin_start(12);
    hbox.set_margin_end(12);
    hbox.set_margin_top(8);
    hbox.set_margin_bottom(8);

    let left_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    left_vbox.set_hexpand(true);
    left_vbox.set_valign(gtk4::Align::Center);

    let title_label = gtk4::Label::builder()
        .label(&cmd.title)
        .halign(gtk4::Align::Start)
        .xalign(0.0)
        .ellipsize(gtk4::pango::EllipsizeMode::End)
        .css_classes(["command-palette-title"])
        .build();
    left_vbox.append(&title_label);

    let sub_label = gtk4::Label::builder()
        .label(cmd.display_subtitle())
        .halign(gtk4::Align::Start)
        .xalign(0.0)
        .ellipsize(gtk4::pango::EllipsizeMode::End)
        .css_classes([
            "dim-label",
            "caption-heading",
            "monospace",
            "command-palette-subtitle",
        ])
        .build();
    left_vbox.append(&sub_label);

    hbox.append(&left_vbox);

    if let Some(ref sc) = cmd.shortcut {
        let shortcut_label = libadwaita::ShortcutLabel::new(sc);
        shortcut_label.set_valign(gtk4::Align::Center);
        shortcut_label.add_css_class("palette-shortcut");
        hbox.append(&shortcut_label);
    }

    row.set_child(Some(&hbox));
    row
}

fn update_row_shortcut(row: &gtk4::ListBoxRow, shortcut: Option<&str>) {
    let Some(hbox) = row.child().and_then(|c| c.downcast::<gtk4::Box>().ok()) else {
        return;
    };
    if let Some(last) = hbox.last_child()
        && last.has_css_class("palette-shortcut")
    {
        hbox.remove(&last);
    }
    if let Some(sc) = shortcut {
        let shortcut_label = libadwaita::ShortcutLabel::new(sc);
        shortcut_label.set_valign(gtk4::Align::Center);
        shortcut_label.add_css_class("palette-shortcut");
        hbox.append(&shortcut_label);
    }
}

fn collect_visible_rows(listbox: &gtk4::ListBox, state: &PaletteState) -> Vec<gtk4::ListBoxRow> {
    let mut visible = Vec::with_capacity(state.entries.len());
    let mut idx = 0i32;
    while let Some(row) = listbox.row_at_index(idx) {
        if state.is_row_visible(&row) {
            visible.push(row);
        }
        idx += 1;
    }
    visible
}

fn find_first_visible_row(
    listbox: &gtk4::ListBox,
    state: &PaletteState,
) -> Option<gtk4::ListBoxRow> {
    let mut idx = 0i32;
    while let Some(row) = listbox.row_at_index(idx) {
        if state.is_row_visible(&row) {
            return Some(row);
        }
        idx += 1;
    }
    None
}

fn select_or_clear(listbox: &gtk4::ListBox, row: Option<&gtk4::ListBoxRow>) {
    if row.is_some() {
        listbox.select_row(row);
    } else {
        listbox.unselect_all();
    }
}

/// Scrolls `row` into view within `scrolled_window` using `row`'s static document coordinates
/// inside `listbox` (`0..upper`), avoiding both `GtkViewport`'s frame-delayed scroll transform
/// and `grab_focus()` calls that would select typed text in `GtkSearchEntry`.
fn scroll_row_into_view(
    scrolled_window: &gtk4::ScrolledWindow,
    listbox: &gtk4::ListBox,
    row: &gtk4::ListBoxRow,
) {
    let Some(bounds) = row.compute_bounds(listbox) else {
        return;
    };

    let adj = scrolled_window.vadjustment();
    let page_size = adj.page_size();
    if page_size <= 0.0 {
        return;
    }

    let pad = 6.0;
    let list_offset = f64::from(listbox.margin_top());
    let doc_top = (list_offset + f64::from(bounds.y()) - pad).max(adj.lower());
    let doc_bottom = list_offset + f64::from(bounds.y() + bounds.height()) + pad;
    let page_top = adj.value();
    let page_bottom = page_top + page_size;
    let max_val = (adj.upper() - page_size).max(adj.lower());

    if doc_top < page_top {
        adj.set_value(doc_top.min(max_val));
    } else if doc_bottom > page_bottom {
        adj.set_value((doc_bottom - page_size).clamp(adj.lower(), max_val));
    }
}

/// Resolves the host `GtkWindow` before popping down and dispatches the `GAction` synchronously
/// so Wayland clipboard actions (`win.copy`, `win.copy-last-output`) retain the active input serial.
fn activate_row(
    popover: &gtk4::Popover,
    state_weak: &Weak<RefCell<PaletteState>>,
    row: &gtk4::ListBoxRow,
) {
    let action_and_param = state_weak.upgrade().and_then(|st| {
        let state = st.borrow();
        if !state.is_row_visible(row) {
            return None;
        }
        state
            .entry_for_row(row)
            .map(|e| (e.item.action.clone(), e.item.parameter.clone()))
    });

    let window = popover.root().and_downcast::<gtk4::Window>();
    popover.popdown();

    if let (Some(win), Some((action_name, param))) = (window, action_and_param)
        && let Err(err) = WidgetExt::activate_action(&win, &action_name, param.as_ref())
    {
        log::warn!("Command palette failed to activate '{action_name}': {err}");
    }
}
