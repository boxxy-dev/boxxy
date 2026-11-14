use boxxy_claw_protocol::PersistentClawRow;
use gtk4::glib;
use gtk4::subclass::prelude::*;
use std::cell::{Cell, RefCell};
use std::sync::Arc;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct ClawRowObject {
        pub row: RefCell<Option<PersistentClawRow>>,
        pub typewriter: Cell<bool>,
        pub cached_diff_json: RefCell<Option<Arc<str>>>,
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
        obj
    }

    #[must_use]
    pub fn new_with_typewriter(row: PersistentClawRow, typewriter: bool) -> Self {
        let obj = Self::new(row);
        obj.imp().typewriter.set(typewriter);
        obj
    }

    pub fn take_typewriter(&self) -> bool {
        self.imp().typewriter.replace(false)
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

    /// Lazily serializes and caches a `FileDiff` payload once as an `Arc<str>` so `GtkListView`
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use boxxy_claw_protocol::FileDiff;

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
}
