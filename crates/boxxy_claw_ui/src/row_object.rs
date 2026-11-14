use boxxy_claw_protocol::{FileDiff, PersistentClawRow, TurnTraceItem, UsageWrapper};
use gtk4::glib;
use gtk4::subclass::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

/// Granularity of an in-place `ClawRowObject` mutation so bound `GtkListView` slots
/// can fast-path high-frequency prose streaming without rebinding avatars or trace headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowUpdateKind {
    Full,
    ProseOnly,
}

pub type RowUpdateCallback = Rc<dyn Fn(RowUpdateKind)>;

mod imp {
    use super::{
        Arc, Cell, ObjectImpl, ObjectSubclass, PersistentClawRow, RefCell, RowUpdateCallback, glib,
    };

    #[derive(Default)]
    pub struct ClawRowObject {
        pub row: RefCell<Option<PersistentClawRow>>,
        pub typewriter: Cell<bool>,
        pub is_working: Cell<bool>,
        pub active_status: RefCell<Option<String>>,
        pub pending_approval_diff: RefCell<Option<Arc<str>>>,
        pub trace_expanded: Cell<bool>,
        pub revision: Cell<u64>,
        pub cached_diff_json: RefCell<Option<Arc<str>>>,
        pub cached_edit_jsons: RefCell<Option<Vec<Arc<str>>>>,
        pub on_updated: RefCell<Option<RowUpdateCallback>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ClawRowObject {
        const NAME: &'static str = "ClawRowObject";
        type Type = super::ClawRowObject;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for ClawRowObject {}
}

glib::wrapper! {
    pub struct ClawRowObject(ObjectSubclass<imp::ClawRowObject>);
}

impl ClawRowObject {
    #[must_use]
    pub fn new(row: PersistentClawRow) -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.imp().row.replace(Some(row));
        obj.imp().revision.set(1);
        obj
    }

    #[must_use]
    pub fn new_with_typewriter(row: PersistentClawRow, typewriter: bool) -> Self {
        let obj = Self::new(row);
        obj.imp().typewriter.set(typewriter);
        obj
    }

    /// Creates an active in-progress turn row for `agent_name` / `character_id` immediately when
    /// thinking begins, displaying the pulsing character avatar, animated dots, and live trace.
    #[must_use]
    pub fn new_working(
        pane_id: String,
        agent_name: Option<String>,
        character_id: Option<String>,
    ) -> Self {
        let obj = Self::new(PersistentClawRow::Diagnosis {
            pane_id,
            agent_name,
            character_id,
            content: String::new(),
            trace: Vec::new(),
            usage: None,
        });
        let imp = obj.imp();
        imp.is_working.set(true);
        *imp.active_status.borrow_mut() = Some("Thinking…".to_string());
        obj
    }

    #[must_use]
    pub fn take_typewriter(&self) -> bool {
        self.imp().typewriter.replace(false)
    }

    #[must_use]
    pub fn is_working(&self) -> bool {
        self.imp().is_working.get()
    }

    pub fn set_working(&self, working: bool) {
        let imp = self.imp();
        let changed = imp.is_working.replace(working) != working;
        let status_cleared = if !working && imp.active_status.borrow().is_some() {
            *imp.active_status.borrow_mut() = None;
            true
        } else {
            false
        };
        if changed || status_cleared {
            self.bump_and_notify();
        }
    }

    #[must_use]
    pub fn active_status(&self) -> Option<String> {
        self.imp().active_status.borrow().clone()
    }

    pub fn set_active_status(&self, status: Option<String>) {
        let imp = self.imp();
        if *imp.active_status.borrow() != status {
            *imp.active_status.borrow_mut() = status;
            self.bump_and_notify();
        }
    }

    #[must_use]
    pub fn trace_expanded(&self) -> bool {
        self.imp().trace_expanded.get()
    }

    pub fn set_trace_expanded(&self, expanded: bool) {
        self.imp().trace_expanded.set(expanded);
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        self.imp().revision.get()
    }

    #[must_use]
    pub fn pending_approval_diff(&self) -> Option<Arc<str>> {
        self.imp().pending_approval_diff.borrow().clone()
    }

    pub fn set_pending_approval_diff(&self, diff: Option<&FileDiff>) {
        let imp = self.imp();
        let next: Option<Arc<str>> =
            diff.and_then(|d| serde_json::to_string(d).ok().map(Arc::from));
        if *imp.pending_approval_diff.borrow() != next {
            *imp.pending_approval_diff.borrow_mut() = next;
            self.bump_and_notify();
        }
    }

    /// Updates character attribution on an active working row if the speaker changed (e.g. Roundtable).
    pub fn ensure_speaker(&self, agent_name: &str, character_id: &str) {
        let mut changed = false;
        if let Some(
            PersistentClawRow::Diagnosis {
                agent_name: ref mut r_name,
                character_id: ref mut r_cid,
                ..
            }
            | PersistentClawRow::Suggested {
                agent_name: ref mut r_name,
                character_id: ref mut r_cid,
                ..
            },
        ) = *self.imp().row.borrow_mut()
        {
            if !agent_name.is_empty() && r_name.as_deref() != Some(agent_name) {
                *r_name = Some(agent_name.to_string());
                changed = true;
            }
            if !character_id.is_empty() && r_cid.as_deref() != Some(character_id) {
                *r_cid = Some(character_id.to_string());
                changed = true;
            }
        }
        if changed {
            self.bump_and_notify();
        }
    }

    /// Appends a completed reasoning/tool/edit/skill item to this row's trace in-place.
    pub fn push_trace_item(&self, item: TurnTraceItem) {
        let imp = self.imp();
        if matches!(item, TurnTraceItem::Edit { .. }) {
            *imp.cached_edit_jsons.borrow_mut() = None;
            *imp.pending_approval_diff.borrow_mut() = None;
        }
        if let Some(
            PersistentClawRow::Diagnosis { ref mut trace, .. }
            | PersistentClawRow::Suggested { ref mut trace, .. },
        ) = *imp.row.borrow_mut()
        {
            trace.push(item);
        }
        self.bump_and_notify();
    }

    /// Updates the live visible prose snapshot while the model streams its final answer.
    pub fn set_streaming_prose(&self, snapshot_text: &str) {
        let changed = if let Some(PersistentClawRow::Diagnosis { content, .. }) =
            &mut *self.imp().row.borrow_mut()
            && content != snapshot_text
        {
            content.clear();
            content.push_str(snapshot_text);
            true
        } else {
            false
        };
        if changed && let Some(cb) = self.imp().on_updated.borrow().clone() {
            cb(RowUpdateKind::ProseOnly);
        }
    }

    /// Settles an in-progress working row into a completed `Diagnosis` row.
    pub fn finalize_diagnosis(
        &self,
        agent_name: Option<String>,
        character_id: Option<String>,
        diagnosis: String,
        final_trace: Vec<TurnTraceItem>,
        usage: Option<UsageWrapper>,
        typewriter: bool,
    ) {
        let imp = self.imp();
        imp.is_working.set(false);
        *imp.active_status.borrow_mut() = None;
        *imp.pending_approval_diff.borrow_mut() = None;
        *imp.cached_edit_jsons.borrow_mut() = None;
        if typewriter {
            imp.typewriter.set(true);
        }

        let pane_id = self.with_row(|r| match r {
            PersistentClawRow::Diagnosis { pane_id, .. }
            | PersistentClawRow::Suggested { pane_id, .. } => pane_id.clone(),
            _ => String::new(),
        });

        // If `final_trace` is empty (e.g. synthetic status card), retain any trace items already gathered live.
        let resolved_trace = if final_trace.is_empty() {
            self.with_row(|r| match r {
                PersistentClawRow::Diagnosis { trace, .. }
                | PersistentClawRow::Suggested { trace, .. } => trace.clone(),
                _ => Vec::new(),
            })
        } else {
            final_trace
        };

        imp.row.replace(Some(PersistentClawRow::Diagnosis {
            pane_id,
            agent_name,
            character_id,
            content: diagnosis,
            trace: resolved_trace,
            usage,
        }));
        self.bump_and_notify();
    }

    /// Settles an in-progress working row into a `Suggested` command-proposal row.
    pub fn finalize_suggested(
        &self,
        agent_name: Option<String>,
        character_id: Option<String>,
        diagnosis: String,
        command: String,
        final_trace: Vec<TurnTraceItem>,
        usage: Option<UsageWrapper>,
    ) {
        let imp = self.imp();
        imp.is_working.set(false);
        *imp.active_status.borrow_mut() = None;
        *imp.pending_approval_diff.borrow_mut() = None;
        *imp.cached_edit_jsons.borrow_mut() = None;

        let pane_id = self.with_row(|r| match r {
            PersistentClawRow::Diagnosis { pane_id, .. }
            | PersistentClawRow::Suggested { pane_id, .. } => pane_id.clone(),
            _ => String::new(),
        });

        let resolved_trace = if final_trace.is_empty() {
            self.with_row(|r| match r {
                PersistentClawRow::Diagnosis { trace, .. }
                | PersistentClawRow::Suggested { trace, .. } => trace.clone(),
                _ => Vec::new(),
            })
        } else {
            final_trace
        };

        imp.row.replace(Some(PersistentClawRow::Suggested {
            pane_id,
            agent_name,
            character_id,
            diagnosis,
            command,
            trace: resolved_trace,
            usage,
        }));
        self.bump_and_notify();
    }

    /// Returns `true` if this row has zero visible prose, zero visible trace steps/skills,
    /// and zero pending diffs (used to prune an empty working placeholder if cancelled,
    /// including when a model emitted only empty/redacted `Thought` items).
    #[must_use]
    pub fn is_empty_working_placeholder(&self) -> bool {
        if self.imp().pending_approval_diff.borrow().is_some() {
            return false;
        }
        self.with_row(|r| match r {
            PersistentClawRow::Diagnosis { content, trace, .. } => {
                if !content.trim().is_empty() {
                    return false;
                }
                let summary = boxxy_claw_protocol::ActivitySummary::summarize(trace);
                !summary.has_steps() && summary.skills.is_empty()
            }
            _ => false,
        })
    }

    pub fn bind_update_listener(&self, listener: Rc<dyn Fn(RowUpdateKind)>) {
        *self.imp().on_updated.borrow_mut() = Some(listener);
    }

    pub fn clear_update_listener(&self) {
        *self.imp().on_updated.borrow_mut() = None;
    }

    pub fn notify_external_refresh(&self) {
        self.bump_and_notify();
    }

    fn bump_and_notify(&self) {
        let imp = self.imp();
        imp.revision.set(imp.revision.get().wrapping_add(1));
        let cb_opt = imp.on_updated.borrow().clone();
        if let Some(cb) = cb_opt {
            cb(RowUpdateKind::Full);
        }
    }

    /// Borrows the underlying `PersistentClawRow` without cloning strings or diff payloads.
    pub fn with_row<R>(&self, f: impl FnOnce(&PersistentClawRow) -> R) -> R {
        let borrow = self.imp().row.borrow();
        f(borrow.as_ref().expect("Row should be set"))
    }

    /// Returns a cloned `PersistentClawRow` for callers that require ownership.
    #[must_use]
    pub fn get_row(&self) -> PersistentClawRow {
        self.with_row(Clone::clone)
    }

    /// Lazily serializes and caches a standalone `FileDiff` payload once as an `Arc<str>` so `GtkListView`
    /// scrolling never re-runs `serde_json::to_string` on the GTK main thread.
    #[must_use]
    pub fn cached_file_diff_json(&self) -> Option<Arc<str>> {
        let imp = self.imp();
        if let Some(ref cached) = *imp.cached_diff_json.borrow() {
            return Some(Arc::clone(cached));
        }

        let serialized: Arc<str> = self.with_row(|row| match row {
            PersistentClawRow::FileDiff { diff, .. } => {
                serde_json::to_string(diff).ok().map(Arc::from)
            }
            _ => None,
        })?;

        *imp.cached_diff_json.borrow_mut() = Some(Arc::clone(&serialized));
        Some(serialized)
    }

    /// Lazily serializes and caches all `TurnTraceItem::Edit` diffs inside this row's `trace`.
    #[must_use]
    pub fn cached_trace_edit_jsons(&self) -> Vec<Arc<str>> {
        let imp = self.imp();
        if let Some(ref cached) = *imp.cached_edit_jsons.borrow() {
            return cached.clone();
        }

        let built: Vec<Arc<str>> = self.with_row(|row| {
            let trace_slice: &[TurnTraceItem] = match row {
                PersistentClawRow::Diagnosis { trace, .. }
                | PersistentClawRow::Suggested { trace, .. } => trace,
                _ => &[],
            };
            trace_slice
                .iter()
                .filter_map(|item| match item {
                    TurnTraceItem::Edit { diff, .. } => {
                        serde_json::to_string(diff).ok().map(Arc::from)
                    }
                    _ => None,
                })
                .collect()
        });

        *imp.cached_edit_jsons.borrow_mut() = Some(built.clone());
        built
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_file_diff_json_serializes_once_and_shares_arc() {
        let diff = FileDiff::compute("src/lib.rs", Some("let a = 1;\n"), "let a = 2;\n");
        let obj = ClawRowObject::new(PersistentClawRow::FileDiff {
            pane_id: "pane-1".to_string(),
            agent_name: Some("Niko".to_string()),
            character_id: None,
            diff,
            auto_approved: true,
            usage: None,
        });

        let j1 = obj.cached_file_diff_json().expect("json");
        let j2 = obj.cached_file_diff_json().expect("cached json");
        assert!(Arc::ptr_eq(&j1, &j2));
        assert!(j1.contains("src/lib.rs"));

        let user_obj = ClawRowObject::new(PersistentClawRow::User {
            pane_id: "pane-1".to_string(),
            content: "hello".to_string(),
            images: Vec::new(),
        });
        assert!(user_obj.cached_file_diff_json().is_none());
    }

    #[test]
    fn working_row_mutates_in_place_and_caches_trace_edit_jsons() {
        let hits = Rc::new(Cell::new(0usize));
        let prose_hits = Rc::new(Cell::new(0usize));
        let hits_cb = hits.clone();
        let prose_cb = prose_hits.clone();

        let obj = ClawRowObject::new_working(
            "pane-1".to_string(),
            Some("Niko".to_string()),
            Some("char-1".to_string()),
        );
        assert!(obj.is_working());
        assert!(obj.is_empty_working_placeholder());

        obj.bind_update_listener(Rc::new(move |kind| match kind {
            RowUpdateKind::Full => hits_cb.set(hits_cb.get() + 1),
            RowUpdateKind::ProseOnly => prose_cb.set(prose_cb.get() + 1),
        }));

        // Empty/redacted reasoning items (e.g. encrypted reasoning blocks with empty text)
        // produce no visible trace UI and must still be treated as an empty placeholder on cancel.
        obj.push_trace_item(TurnTraceItem::Thought {
            summary: None,
            content: "   ".to_string(),
            duration_ms: Some(120),
        });
        assert!(obj.is_empty_working_placeholder());

        let rev_before_prose = obj.revision();
        obj.set_streaming_prose("Streaming...");
        assert_eq!(prose_hits.get(), 1);
        assert_eq!(obj.revision(), rev_before_prose);
        assert!(!obj.is_empty_working_placeholder());

        let diff = FileDiff::compute("src/module.rs", Some("fn old() {}\n"), "fn next() {}\n");
        obj.set_pending_approval_diff(Some(&diff));
        assert!(obj.pending_approval_diff().is_some());
        assert!(!obj.is_empty_working_placeholder());

        obj.push_trace_item(TurnTraceItem::Edit {
            diff,
            auto_approved: false,
        });
        assert!(obj.pending_approval_diff().is_none());
        let edits1 = obj.cached_trace_edit_jsons();
        let edits2 = obj.cached_trace_edit_jsons();
        assert_eq!(edits1.len(), 1);
        assert!(Arc::ptr_eq(&edits1[0], &edits2[0]));

        obj.finalize_diagnosis(
            Some("Niko".to_string()),
            Some("char-1".to_string()),
            "Done.".to_string(),
            Vec::new(),
            None,
            false,
        );
        assert!(!obj.is_working());
        assert!(hits.get() >= 3);
        obj.with_row(|r| match r {
            PersistentClawRow::Diagnosis { content, trace, .. } => {
                assert_eq!(content, "Done.");
                assert_eq!(trace.len(), 2);
            }
            other => panic!("unexpected row {other:?}"),
        });
    }
}
