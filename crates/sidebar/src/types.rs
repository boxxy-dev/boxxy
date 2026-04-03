use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk4 as gtk;
use rig::message::Message;
use std::cell::Cell;
use std::cell::RefCell;

#[derive(Debug, Clone, Copy, PartialEq, Eq, glib::Enum, Default)]
#[enum_type(name = "ChatRole")]
pub enum Role {
    #[default]
    User,
    Assistant,
    System,
}

mod imp {
    use super::*;
    use gtk::glib;

    #[derive(Default)]
    pub struct ChatMessageObject {
        pub role: Cell<Role>,
        pub content: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChatMessageObject {
        const NAME: &'static str = "ChatMessageObject";
        type Type = super::ChatMessageObject;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for ChatMessageObject {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: std::sync::LazyLock<Vec<glib::ParamSpec>> =
                std::sync::LazyLock::new(|| {
                    vec![
                        glib::ParamSpecEnum::builder::<Role>("role").build(),
                        glib::ParamSpecString::builder("content").build(),
                    ]
                });
            PROPERTIES.as_ref()
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "role" => {
                    self.role.set(value.get().unwrap());
                }
                "content" => {
                    self.content.replace(value.get().unwrap());
                }
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "role" => self.role.get().to_value(),
                "content" => self.content.borrow().to_value(),
                _ => unimplemented!(),
            }
        }
    }
}

glib::wrapper! {
    pub struct ChatMessageObject(ObjectSubclass<imp::ChatMessageObject>);
}

impl ChatMessageObject {
    pub fn new(role: Role, content: String) -> Self {
        glib::Object::builder()
            .property("role", role)
            .property("content", content)
            .build()
    }

    pub fn role(&self) -> Role {
        self.property("role")
    }

    pub fn content(&self) -> String {
        self.property("content")
    }

    pub fn set_content(&self, content: String) {
        self.set_property("content", content);
    }

    pub fn to_rig_message(&self) -> Message {
        match self.role() {
            Role::User | Role::System => Message::user(&self.content()),
            Role::Assistant => Message::assistant(&self.content()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    pub fn to_rig_message(&self) -> Message {
        match self.role {
            Role::User | Role::System => Message::user(&self.content),
            Role::Assistant => Message::assistant(&self.content),
        }
    }
}
