use crate::widgets::notification::Notification;
use gtk::CompositeTemplate;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk4 as gtk;

mod imp {
    use super::*;

    #[derive(Default, CompositeTemplate)]
    #[template(resource = "/dev/boxxy/BoxxyTerminal/ui/widgets/notification_details.ui")]
    pub struct BoxxyNotificationDetails {
        #[template_child]
        pub title: TemplateChild<gtk::Label>,
        #[template_child]
        pub details_grid: TemplateChild<gtk::Grid>,
        #[template_child]
        pub actions_box: TemplateChild<gtk::Box>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BoxxyNotificationDetails {
        const NAME: &'static str = "BoxxyNotificationDetails";
        type Type = super::BoxxyNotificationDetails;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for BoxxyNotificationDetails {}
    impl WidgetImpl for BoxxyNotificationDetails {}
    impl BoxImpl for BoxxyNotificationDetails {}
}

glib::wrapper! {
    pub struct BoxxyNotificationDetails(ObjectSubclass<imp::BoxxyNotificationDetails>)
        @extends gtk::Widget, gtk::Box,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl BoxxyNotificationDetails {
    pub fn new(
        notification: &Notification,
        tx: async_channel::Sender<crate::state::AppInput>,
    ) -> Self {
        let obj: Self = glib::Object::builder().build();
        let imp = obj.imp();

        imp.title
            .set_markup(&format!("<b>{}</b>", notification.title));

        // Dynamically add details
        for (i, (key, value)) in notification.details.iter().enumerate() {
            if key == "Url" || key == "ChecksumUrl" {
                continue;
            }

            let key_label = gtk::Label::builder()
                .label(key)
                .halign(gtk::Align::Start)
                .css_classes(["dim-label"])
                .build();
            let value_label = gtk::Label::builder()
                .label(value)
                .halign(gtk::Align::Start)
                .build();

            imp.details_grid.attach(&key_label, 0, i as i32, 1, 1);
            imp.details_grid.attach(&value_label, 1, i as i32, 1, 1);
        }

        // Dynamically add actions
        for action in &notification.actions {
            let btn = gtk::Button::builder().label(&action.label).build();

            if action.is_primary {
                btn.add_css_class("suggested-action");
            }

            let tx = tx.clone();
            let action_name = action.action_name.clone();
            let url = notification
                .details
                .iter()
                .find(|(k, _)| k == "Url")
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            let date = notification
                .details
                .iter()
                .find(|(k, _)| k == "Date")
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            let checksum_url = notification
                .details
                .iter()
                .find(|(k, _)| k == "ChecksumUrl")
                .map(|(_, v)| v.clone());

            let id = notification.id.clone();

            btn.connect_clicked(glib::clone!(
                #[weak]
                obj,
                move |_| {
                    if action_name == "win.start-download" {
                        let _ = tx.send_blocking(crate::state::AppInput::StartUpdateDownload(
                            url.clone(),
                            date.clone(),
                            checksum_url.clone(),
                        ));
                        let _ = tx
                            .send_blocking(crate::state::AppInput::DismissNotification(id.clone()));
                    } else if action_name == "win.apply-update" {
                        let _ = tx.send_blocking(crate::state::AppInput::ApplyUpdateAndRestart);
                    } else if action_name == "win.dismiss-notification" {
                        let _ = tx
                            .send_blocking(crate::state::AppInput::DismissNotification(id.clone()));
                    }

                    if let Some(popover) = obj.ancestor(gtk::Popover::static_type()) {
                        popover.downcast::<gtk::Popover>().unwrap().popdown();
                    }
                }
            ));

            imp.actions_box.append(&btn);
        }

        obj
    }
}
