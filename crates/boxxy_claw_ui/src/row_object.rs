use boxxy_claw_protocol::PersistentClawRow;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use std::cell::RefCell;

mod imp {
    use super::*;
    use gtk4::glib;

    #[derive(Default)]
    pub struct ClawRowObject {
        pub row: RefCell<Option<PersistentClawRow>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ClawRowObject {
        const NAME: &'static str = "ClawRowObject";
        type Type = super::ClawRowObject;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for ClawRowObject {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: std::sync::LazyLock<Vec<glib::ParamSpec>> =
                std::sync::LazyLock::new(|| {
                    vec![glib::ParamSpecString::builder("content").build()]
                });
            PROPERTIES.as_ref()
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "content" => {
                    let row = self.row.borrow();
                    match row.as_ref().expect("Row should be set") {
                        PersistentClawRow::Diagnosis { content, .. } => content.to_value(),
                        PersistentClawRow::User { content, .. } => content.to_value(),
                        PersistentClawRow::Suggested { diagnosis, .. } => diagnosis.to_value(),
                        PersistentClawRow::ProcessList { result_json, .. } => {
                            result_json.to_value()
                        }
                        PersistentClawRow::ToolCall { result, .. } => result.to_value(),
                        PersistentClawRow::Command { command, .. } => command.to_value(),
                        PersistentClawRow::SystemMessage { content, .. } => content.to_value(),
                        PersistentClawRow::FileDiff { diff, .. } => diff.path.to_value(),
                    }
                }
                _ => unimplemented!(),
            }
        }
    }
}

glib::wrapper! {
    pub struct ClawRowObject(ObjectSubclass<imp::ClawRowObject>);
}

impl ClawRowObject {
    pub fn new(row: PersistentClawRow) -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.imp().row.replace(Some(row));
        obj
    }

    pub fn get_row(&self) -> PersistentClawRow {
        self.imp()
            .row
            .borrow()
            .as_ref()
            .expect("Row should be set")
            .clone()
    }
}
