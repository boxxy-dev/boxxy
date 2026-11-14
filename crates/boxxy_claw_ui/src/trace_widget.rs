//! Collapsible OpenCode-style per-turn activity trace widget (`+ 4 commands, 19 edits, 8 thoughts, 2 reads, 1 tool`),
//! lazy thought/exploration/command disclosures, inline X-ray diffs (`Ctrl+Shift+D`), and pending approval diff slot.

use crate::row_object::ClawRowObject;
use boxxy_claw_protocol::{ActivitySummary, PersistentClawRow, TraceCategory, TurnTraceItem};
use boxxy_viewer::{StructuredViewer, ViewerRegistry};
use gtk::prelude::*;
use gtk4 as gtk;
use gtk4::glib;
use gtk4::pango;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

enum GroupedStep<'a> {
    Thoughts(Vec<&'a TurnTraceItem>),
    Exploration(Vec<&'a TurnTraceItem>),
    SingleTool(&'a TurnTraceItem),
    Edit {
        diff_path: &'a str,
        added: usize,
        removed: usize,
        edit_idx: usize,
    },
}

fn group_trace_steps(items: &[TurnTraceItem]) -> Vec<GroupedStep<'_>> {
    let mut groups: Vec<GroupedStep<'_>> = Vec::new();
    let mut edit_counter = 0usize;
    let mut idx = 0usize;

    while idx < items.len() {
        match &items[idx] {
            TurnTraceItem::Skill { .. } => {
                idx += 1;
            }
            TurnTraceItem::Thought { content, .. } if !content.trim().is_empty() => {
                let mut batch = vec![&items[idx]];
                idx += 1;
                while idx < items.len() {
                    if let TurnTraceItem::Thought { content: c, .. } = &items[idx] {
                        if !c.trim().is_empty() {
                            batch.push(&items[idx]);
                        }
                        idx += 1;
                    } else {
                        break;
                    }
                }
                groups.push(GroupedStep::Thoughts(batch));
            }
            TurnTraceItem::Thought { .. } => {
                idx += 1;
            }
            TurnTraceItem::Tool {
                category: TraceCategory::Read | TraceCategory::Search,
                ..
            } => {
                let mut batch = vec![&items[idx]];
                idx += 1;
                while idx < items.len() {
                    if matches!(
                        &items[idx],
                        TurnTraceItem::Tool {
                            category: TraceCategory::Read | TraceCategory::Search,
                            ..
                        }
                    ) {
                        batch.push(&items[idx]);
                        idx += 1;
                    } else {
                        break;
                    }
                }
                if batch.len() >= 2 {
                    groups.push(GroupedStep::Exploration(batch));
                } else {
                    groups.push(GroupedStep::SingleTool(batch[0]));
                }
            }
            TurnTraceItem::Tool { .. } => {
                groups.push(GroupedStep::SingleTool(&items[idx]));
                idx += 1;
            }
            TurnTraceItem::Edit { diff, .. } => {
                let current_edit_idx = edit_counter;
                edit_counter += 1;
                groups.push(GroupedStep::Edit {
                    diff_path: &diff.path,
                    added: diff.stats.added_lines,
                    removed: diff.stats.removed_lines,
                    edit_idx: current_edit_idx,
                });
                idx += 1;
            }
        }
    }

    groups
}

fn format_duration_ms(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else {
        let secs = ms as f64 / 1_000.0;
        if secs < 60.0 {
            format!("{secs:.1}s")
        } else {
            let mins = (secs / 60.0).floor() as u64;
            let rem = (secs % 60.0).round() as u64;
            format!("{mins}m {rem}s")
        }
    }
}

/// Reusable per-slot trace container attached to each `GtkListItem` in `create_claw_message_list()`.
#[derive(Clone)]
pub struct TurnTraceSlot {
    root: gtk::Box,
    summary_btn: gtk::Button,
    toggle_glyph: gtk::Label,
    summary_lbl: gtk::Label,
    skills_lbl: gtk::Label,
    trace_revealer: gtk::Revealer,
    trace_rail_box: gtk::Box,
    inline_diffs_box: gtk::Box,
    pending_diff_viewer: StructuredViewer,
    registry: Rc<ViewerRegistry>,
    bound_row: Rc<RefCell<Option<ClawRowObject>>>,
    mounted_rail_stamp: Rc<Cell<(usize, usize, bool)>>,
    mounted_inline_stamp: Rc<Cell<(usize, usize, bool)>>,
    mounted_pending_ptr: Rc<Cell<usize>>,
}

impl TurnTraceSlot {
    #[must_use]
    pub fn new(registry: Rc<ViewerRegistry>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.add_css_class("claw-trace-container");
        root.set_hexpand(true);
        root.set_visible(false);

        let summary_btn = gtk::Button::builder()
            .css_classes(["flat", "claw-trace-summary-btn"])
            .halign(gtk::Align::Start)
            .focus_on_click(false)
            .build();

        let summary_hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let toggle_glyph = gtk::Label::new(Some("+"));
        toggle_glyph.add_css_class("monospace");
        toggle_glyph.add_css_class("claw-trace-glyph");
        toggle_glyph.set_valign(gtk::Align::Center);
        summary_hbox.append(&toggle_glyph);

        let summary_lbl = gtk::Label::new(None);
        summary_lbl.set_use_markup(true);
        summary_lbl.set_halign(gtk::Align::Start);
        summary_lbl.set_valign(gtk::Align::Center);
        summary_lbl.set_ellipsize(pango::EllipsizeMode::End);
        summary_lbl.add_css_class("claw-trace-summary-label");
        summary_hbox.append(&summary_lbl);
        summary_btn.set_child(Some(&summary_hbox));
        root.append(&summary_btn);

        let trace_revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .transition_duration(160)
            .reveal_child(false)
            .hexpand(true)
            .build();

        let trace_rail_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        trace_rail_box.add_css_class("claw-trace-rail");
        trace_rail_box.set_hexpand(true);
        trace_revealer.set_child(Some(&trace_rail_box));
        root.append(&trace_revealer);

        let skills_lbl = gtk::Label::new(None);
        skills_lbl.add_css_class("caption");
        skills_lbl.add_css_class("dim-label");
        skills_lbl.add_css_class("monospace");
        skills_lbl.add_css_class("claw-trace-skill-line");
        skills_lbl.set_halign(gtk::Align::Start);
        skills_lbl.set_ellipsize(pango::EllipsizeMode::End);
        skills_lbl.set_visible(false);
        root.append(&skills_lbl);

        let inline_diffs_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        inline_diffs_box.set_hexpand(true);
        inline_diffs_box.set_visible(false);
        root.append(&inline_diffs_box);

        let pending_diff_viewer = StructuredViewer::new(registry.clone());
        pending_diff_viewer.widget().set_visible(false);
        root.append(pending_diff_viewer.widget());

        let slot = Self {
            root,
            summary_btn: summary_btn.clone(),
            toggle_glyph,
            summary_lbl,
            skills_lbl,
            trace_revealer,
            trace_rail_box,
            inline_diffs_box,
            pending_diff_viewer,
            registry,
            bound_row: Rc::new(RefCell::new(None)),
            mounted_rail_stamp: Rc::new(Cell::new((0, 0, false))),
            mounted_inline_stamp: Rc::new(Cell::new((0, 0, false))),
            mounted_pending_ptr: Rc::new(Cell::new(0)),
        };

        let slot_for_click = slot.clone();
        summary_btn.connect_clicked(move |_| {
            slot_for_click.toggle_expanded();
        });

        slot
    }

    #[must_use]
    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    pub fn reset_for_unbind(&self) {
        *self.bound_row.borrow_mut() = None;
        self.mounted_rail_stamp.set((0, 0, false));
        self.mounted_inline_stamp.set((0, 0, false));
        self.mounted_pending_ptr.set(0);
        self.trace_revealer.set_reveal_child(false);
        while let Some(child) = self.trace_rail_box.first_child() {
            self.trace_rail_box.remove(&child);
        }
        while let Some(child) = self.inline_diffs_box.first_child() {
            self.inline_diffs_box.remove(&child);
        }
        self.inline_diffs_box.set_visible(false);
        self.pending_diff_viewer.park_for_recycle();
        self.pending_diff_viewer.widget().set_visible(false);
        self.skills_lbl.set_visible(false);
        self.root.set_visible(false);
    }

    fn toggle_expanded(&self) {
        let row_opt = self.bound_row.borrow().clone();
        let Some(obj) = row_opt else {
            return;
        };
        let has_steps = obj.with_row(|r| match r {
            PersistentClawRow::Diagnosis { trace, .. }
            | PersistentClawRow::Suggested { trace, .. } => {
                ActivitySummary::summarize(trace).has_steps()
            }
            _ => false,
        });
        if !has_steps {
            return;
        }
        let next = !obj.trace_expanded();
        obj.set_trace_expanded(next);
        let always_code = boxxy_viewer::always_expand_code_blocks();
        self.sync(&obj, always_code);
    }

    pub fn sync(&self, obj: &ClawRowObject, always_show_code_blocks: bool) {
        let obj_ptr = obj.as_ptr() as usize;
        *self.bound_row.borrow_mut() = Some(obj.clone());

        let is_working = obj.is_working();
        let active_status = obj.active_status();
        let pending_diff = obj.pending_approval_diff();
        let expanded = obj.trace_expanded();

        let (summary, trace_len) = obj.with_row(|r| match r {
            PersistentClawRow::Diagnosis { trace, .. }
            | PersistentClawRow::Suggested { trace, .. } => {
                (ActivitySummary::summarize(trace), trace.len())
            }
            _ => (ActivitySummary::default(), 0),
        });

        let has_steps = summary.has_steps();
        let show_summary_bar = has_steps || is_working;
        let show_skills = !summary.skills.is_empty();
        let edit_jsons = if always_show_code_blocks || expanded {
            obj.cached_trace_edit_jsons()
        } else {
            Vec::new()
        };
        let show_inline_diffs = always_show_code_blocks && !edit_jsons.is_empty();
        let show_pending_diff = pending_diff.is_some();

        if !show_summary_bar && !show_skills && !show_inline_diffs && !show_pending_diff {
            self.root.set_visible(false);
            return;
        }
        self.root.set_visible(true);

        // 1. Summary Bar (`+ 4 commands, 19 edits, 8 thoughts, 2 reads, 1 tool`)
        self.summary_btn.set_visible(show_summary_bar);
        if show_summary_bar {
            self.summary_btn.set_sensitive(has_steps);
            self.toggle_glyph.set_visible(has_steps);
            self.toggle_glyph.set_text(if expanded { "−" } else { "+" });

            let markup = if has_steps {
                let counts_escaped = glib::markup_escape_text(&summary.format_label());
                if is_working
                    && let Some(ref st) = active_status
                    && !st.is_empty()
                {
                    format!(
                        "{counts_escaped}  <span alpha=\"65%\">· {}</span>",
                        glib::markup_escape_text(st)
                    )
                } else {
                    counts_escaped.to_string()
                }
            } else {
                let st = active_status.as_deref().unwrap_or("Thinking…");
                format!("<span alpha=\"72%\">{}</span>", glib::markup_escape_text(st))
            };
            self.summary_lbl.set_markup(&markup);
        }

        // 2. Skills sub-row (`◇ Skill: ...`, matching OpenCode's `◇ Instructions: ...`)
        if show_skills {
            self.skills_lbl
                .set_text(&format!("◇ Skill: {}", summary.skills.join(", ")));
            self.skills_lbl.set_visible(true);
        } else {
            self.skills_lbl.set_visible(false);
        }

        // 3. Expanded Trace Rail (lazily mounted ONLY when `expanded && has_steps`)
        if expanded && has_steps {
            let target_stamp = (obj_ptr, trace_len, always_show_code_blocks);
            if self.mounted_rail_stamp.get() != target_stamp {
                self.mounted_rail_stamp.set(target_stamp);
                self.rebuild_trace_rail(obj, &edit_jsons, always_show_code_blocks);
            }
            self.trace_revealer.set_reveal_child(true);
        } else {
            self.trace_revealer.set_reveal_child(false);
        }

        // 4. Inline X-Ray Diffs (`Ctrl+Shift+D` / `always_show_code_blocks == true`)
        if show_inline_diffs {
            let target_inline = (obj_ptr, edit_jsons.len(), true);
            if self.mounted_inline_stamp.get() != target_inline {
                self.mounted_inline_stamp.set(target_inline);
                while let Some(child) = self.inline_diffs_box.first_child() {
                    self.inline_diffs_box.remove(&child);
                }
                for edit_json in &edit_jsons {
                    let v = StructuredViewer::new(self.registry.clone());
                    v.append_custom_block("file_diff", edit_json);
                    self.inline_diffs_box.append(v.widget());
                }
            }
            self.inline_diffs_box.set_visible(true);
        } else {
            if self.mounted_inline_stamp.get().2 {
                self.mounted_inline_stamp.set((0, 0, false));
                while let Some(child) = self.inline_diffs_box.first_child() {
                    self.inline_diffs_box.remove(&child);
                }
            }
            self.inline_diffs_box.set_visible(false);
        }

        // 5. Active Pending Approval Diff (`!auto_approve` while deciding)
        if let Some(ref diff_json) = pending_diff {
            let ptr = diff_json.as_ptr() as usize;
            if self.mounted_pending_ptr.replace(ptr) != ptr {
                self.pending_diff_viewer.park_for_recycle();
                self.pending_diff_viewer
                    .append_custom_block("file_diff", diff_json);
            }
            self.pending_diff_viewer.widget().set_visible(true);
        } else if self.mounted_pending_ptr.replace(0) != 0 {
            self.pending_diff_viewer.park_for_recycle();
            self.pending_diff_viewer.widget().set_visible(false);
        }
    }

    fn rebuild_trace_rail(
        &self,
        obj: &ClawRowObject,
        edit_jsons: &[Arc<str>],
        always_show_code_blocks: bool,
    ) {
        while let Some(child) = self.trace_rail_box.first_child() {
            self.trace_rail_box.remove(&child);
        }

        obj.with_row(|row| {
            let trace: &[TurnTraceItem] = match row {
                PersistentClawRow::Diagnosis { trace, .. }
                | PersistentClawRow::Suggested { trace, .. } => trace,
                _ => return,
            };

            let groups = group_trace_steps(trace);
            for grp in groups {
                match grp {
                    GroupedStep::Thoughts(thoughts) => {
                        self.trace_rail_box
                            .append(&self.build_thought_group_widget(&thoughts));
                    }
                    GroupedStep::Exploration(items) => {
                        self.trace_rail_box
                            .append(&self.build_exploration_group_widget(&items));
                    }
                    GroupedStep::SingleTool(item) => {
                        self.trace_rail_box
                            .append(&Self::build_single_tool_widget(item));
                    }
                    GroupedStep::Edit {
                        diff_path,
                        added,
                        removed,
                        edit_idx,
                    } => {
                        if always_show_code_blocks {
                            // Full diff is already expanded right below in `inline_diffs_box`;
                            // show a clean 1-line edit marker in the rail to avoid duplicate diff cards.
                            let lbl = gtk::Label::new(None);
                            lbl.set_use_markup(true);
                            lbl.set_halign(gtk::Align::Start);
                            lbl.set_ellipsize(pango::EllipsizeMode::Middle);
                            lbl.add_css_class("monospace");
                            lbl.add_css_class("caption");
                            lbl.add_css_class("dim-label");
                            lbl.set_markup(&format!(
                                "← Edited <b>{}</b>  <span foreground=\"#2ec27e\">+{added}</span> <span foreground=\"#e01b24\">−{removed}</span>",
                                glib::markup_escape_text(diff_path)
                            ));
                            self.trace_rail_box.append(&lbl);
                        } else if let Some(edit_json) = edit_jsons.get(edit_idx) {
                            let v = StructuredViewer::new(self.registry.clone());
                            v.append_custom_block("file_diff_collapsed", edit_json);
                            self.trace_rail_box.append(v.widget());
                        }
                    }
                }
            }
        });
    }

    fn build_thought_group_widget(&self, thoughts: &[&TurnTraceItem]) -> gtk::Box {
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
        vbox.set_hexpand(true);

        let mut latest_title: Option<&str> = None;
        let mut total_ms: u64 = 0;
        let mut combined_md = String::new();

        for (i, t) in thoughts.iter().enumerate() {
            if let TurnTraceItem::Thought {
                summary,
                content,
                duration_ms,
            } = t
            {
                if let Some(s) = summary.as_deref().filter(|s| !s.is_empty()) {
                    latest_title = Some(s);
                }
                if let Some(ms) = duration_ms {
                    total_ms = total_ms.saturating_add(*ms);
                }
                if i > 0 {
                    combined_md.push_str("\n\n---\n\n");
                }
                if let Some(s) = summary.as_deref().filter(|s| !s.is_empty())
                    && s != content.trim()
                {
                    combined_md.push_str("**");
                    combined_md.push_str(s);
                    combined_md.push_str("**\n\n");
                }
                combined_md.push_str(content.trim());
            }
        }

        let mut header_parts = String::from("Thought");
        if let Some(title) = latest_title {
            header_parts.push_str(": ");
            header_parts.push_str(title);
        }
        if thoughts.len() > 1 {
            header_parts.push_str(&format!(" · {} steps", thoughts.len()));
        }
        if total_ms > 0 {
            header_parts.push_str(" · ");
            header_parts.push_str(&format_duration_ms(total_ms));
        }

        let btn = gtk::Button::builder()
            .css_classes(["flat", "claw-trace-item-btn", "claw-trace-thought-btn"])
            .halign(gtk::Align::Start)
            .focus_on_click(false)
            .build();
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let glyph = gtk::Label::new(Some("+"));
        glyph.add_css_class("monospace");
        hbox.append(&glyph);

        let lbl = gtk::Label::new(Some(&header_parts));
        lbl.set_halign(gtk::Align::Start);
        lbl.set_ellipsize(pango::EllipsizeMode::End);
        hbox.append(&lbl);
        btn.set_child(Some(&hbox));
        vbox.append(&btn);

        let rev = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .transition_duration(140)
            .reveal_child(false)
            .hexpand(true)
            .build();

        let mounted = Rc::new(Cell::new(false));
        let rev_for_click = rev.clone();
        let glyph_for_click = glyph;
        let registry = self.registry.clone();
        btn.connect_clicked(move |_| {
            let next = !rev_for_click.reveals_child();
            if next && !mounted.replace(true) {
                let inner_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
                inner_box.add_css_class("claw-thought-body");
                let viewer = StructuredViewer::new(registry.clone());
                viewer.set_content(&combined_md);
                inner_box.append(viewer.widget());
                rev_for_click.set_child(Some(&inner_box));
            }
            glyph_for_click.set_text(if next { "−" } else { "+" });
            rev_for_click.set_reveal_child(next);
        });

        vbox.append(&rev);
        vbox
    }

    fn build_exploration_group_widget(&self, items: &[&TurnTraceItem]) -> gtk::Box {
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
        vbox.set_hexpand(true);

        let mut reads = 0usize;
        let mut searches = 0usize;
        for item in items {
            if let TurnTraceItem::Tool { category, .. } = item {
                match category {
                    TraceCategory::Read => reads += 1,
                    _ => searches += 1,
                }
            }
        }
        let mut parts = Vec::new();
        if reads > 0 {
            parts.push(if reads == 1 {
                "1 read".to_string()
            } else {
                format!("{reads} reads")
            });
        }
        if searches > 0 {
            parts.push(if searches == 1 {
                "1 search".to_string()
            } else {
                format!("{searches} searches")
            });
        }
        let summary_text = format!("Explored: {}", parts.join(", "));

        let btn = gtk::Button::builder()
            .css_classes(["flat", "claw-trace-item-btn"])
            .halign(gtk::Align::Start)
            .focus_on_click(false)
            .build();
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let glyph = gtk::Label::new(Some("→"));
        glyph.add_css_class("monospace");
        glyph.add_css_class("dim-label");
        hbox.append(&glyph);

        let lbl = gtk::Label::new(Some(&summary_text));
        lbl.add_css_class("dim-label");
        lbl.set_halign(gtk::Align::Start);
        hbox.append(&lbl);
        btn.set_child(Some(&hbox));
        vbox.append(&btn);

        let rev = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .transition_duration(140)
            .reveal_child(false)
            .hexpand(true)
            .build();

        let owned_children: Vec<TurnTraceItem> = items.iter().map(|i| (*i).clone()).collect();
        let mounted = Rc::new(Cell::new(false));
        let rev_for_click = rev.clone();
        btn.connect_clicked(move |_| {
            let next = !rev_for_click.reveals_child();
            if next && !mounted.replace(true) {
                let sub_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
                sub_box.set_margin_start(14);
                for child in &owned_children {
                    sub_box.append(&Self::build_single_tool_widget(child));
                }
                rev_for_click.set_child(Some(&sub_box));
            }
            rev_for_click.set_reveal_child(next);
        });

        vbox.append(&rev);
        vbox
    }

    fn build_single_tool_widget(item: &TurnTraceItem) -> gtk::Box {
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
        vbox.set_hexpand(true);

        let TurnTraceItem::Tool {
            label,
            preview,
            failed,
            ..
        } = item
        else {
            return vbox;
        };

        let has_preview = preview.as_ref().is_some_and(|p| !p.trim().is_empty());

        let btn = gtk::Button::builder()
            .css_classes(["flat", "claw-trace-item-btn"])
            .halign(gtk::Align::Start)
            .sensitive(has_preview)
            .focus_on_click(false)
            .build();
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);

        let icon_text = if *failed {
            "✗"
        } else if has_preview {
            "▸"
        } else {
            "•"
        };
        let glyph = gtk::Label::new(Some(icon_text));
        glyph.add_css_class("monospace");
        if *failed {
            glyph.add_css_class("error");
        } else {
            glyph.add_css_class("dim-label");
        }
        hbox.append(&glyph);

        let lbl = gtk::Label::new(Some(label));
        lbl.add_css_class("monospace");
        lbl.add_css_class("caption");
        if *failed {
            lbl.add_css_class("error");
        } else {
            lbl.add_css_class("dim-label");
        }
        lbl.set_halign(gtk::Align::Start);
        lbl.set_ellipsize(pango::EllipsizeMode::End);
        hbox.append(&lbl);
        btn.set_child(Some(&hbox));
        vbox.append(&btn);

        if let Some(preview_str) = preview.clone().filter(|s| !s.trim().is_empty()) {
            let rev = gtk::Revealer::builder()
                .transition_type(gtk::RevealerTransitionType::SlideDown)
                .transition_duration(140)
                .reveal_child(false)
                .hexpand(true)
                .build();
            let mounted = Rc::new(Cell::new(false));
            let rev_for_click = rev.clone();
            let glyph_for_click = glyph;
            let is_failed = *failed;
            btn.connect_clicked(move |_| {
                let next = !rev_for_click.reveals_child();
                if next && !mounted.replace(true) {
                    let pre_lbl = gtk::Label::new(Some(&preview_str));
                    pre_lbl.add_css_class("monospace");
                    pre_lbl.add_css_class("caption");
                    pre_lbl.add_css_class("claw-trace-preview");
                    pre_lbl.set_halign(gtk::Align::Start);
                    pre_lbl.set_xalign(0.0);
                    pre_lbl.set_wrap(true);
                    pre_lbl.set_wrap_mode(pango::WrapMode::WordChar);
                    pre_lbl.set_selectable(true);
                    rev_for_click.set_child(Some(&pre_lbl));
                }
                if !is_failed {
                    glyph_for_click.set_text(if next { "▾" } else { "▸" });
                }
                rev_for_click.set_reveal_child(next);
            });
            vbox.append(&rev);
        }

        vbox
    }
}
