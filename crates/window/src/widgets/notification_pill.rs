use gtk4 as gtk;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::glib;
use gtk::CompositeTemplate;
use crate::widgets::notification::Notification;

mod imp {
    use super::*;

    #[derive(Default, CompositeTemplate)]
    #[template(resource = "/play/mii/Boxxy/ui/widgets/notification_pill.ui")]
    pub struct BoxxyNotificationPill {
        #[template_child]
        pub icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub label: TemplateChild<gtk::Label>,
        
        pub notification: std::cell::RefCell<Option<Notification>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BoxxyNotificationPill {
        const NAME: &'static str = "BoxxyNotificationPill";
        type Type = super::BoxxyNotificationPill;
        type ParentType = gtk::Button;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for BoxxyNotificationPill {}
    impl WidgetImpl for BoxxyNotificationPill {}
    impl ButtonImpl for BoxxyNotificationPill {}
}

glib::wrapper! {
    pub struct BoxxyNotificationPill(ObjectSubclass<imp::BoxxyNotificationPill>)
        @extends gtk::Widget, gtk::Button,
        @implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}

impl BoxxyNotificationPill {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    pub fn set_notification(&self, notification: Notification) {
        let imp = self.imp();
        imp.icon.set_icon_name(Some(&notification.icon_name));
        imp.label.set_label(&notification.title);
        imp.notification.replace(Some(notification));
        self.set_visible(true);
    }

    pub fn clear(&self) {
        self.imp().notification.replace(None);
        self.set_visible(false);
    }

    pub fn get_notification(&self) -> Option<Notification> {
        self.imp().notification.borrow().clone()
    }
}
