use crate::models::ModelProvider;
use crate::ui::selector::SingleModelSelector;
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita::prelude::*;

#[derive(Clone)]
pub struct GlobalModelSelectorDialog {
    dialog: libadwaita::Dialog,
    pub ai_chat_selector: SingleModelSelector,
    pub claw_selector: SingleModelSelector,
    pub memory_selector: SingleModelSelector,
}

impl GlobalModelSelectorDialog {
    pub fn new<F1, F2, F3>(
        init_ai: Option<ModelProvider>,
        init_claw: Option<ModelProvider>,
        init_memory: Option<ModelProvider>,
        ollama_url: String,
        api_keys: std::collections::HashMap<String, String>,
        on_ai_change: F1,
        on_claw_change: F2,
        on_memory_change: F3,
    ) -> Self
    where
        F1: Fn(Option<ModelProvider>) + 'static,
        F2: Fn(Option<ModelProvider>) + 'static,
        F3: Fn(Option<ModelProvider>) + 'static,
    {
        let dialog = libadwaita::Dialog::builder()
            .title("Models Selection")
            .content_width(450)
            .content_height(400)
            .build();

        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
        stack.set_hhomogeneous(true);
        stack.set_vhomogeneous(true);

        let ai_chat_selector =
            SingleModelSelector::new(init_ai, ollama_url.clone(), api_keys.clone(), on_ai_change);
        let claw_selector = SingleModelSelector::new(
            init_claw.clone(),
            ollama_url.clone(),
            api_keys.clone(),
            on_claw_change,
        );

        let mem_initial = init_memory.or(init_claw);
        let memory_selector =
            SingleModelSelector::new(mem_initial, ollama_url, api_keys, move |new_prov| {
                on_memory_change(new_prov);
            });

        let build_tab = |selector: &SingleModelSelector, help_text: &str| -> gtk::Box {
            let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);

            let help_label = gtk::Label::new(Some(help_text));
            help_label.set_halign(gtk::Align::Start);
            help_label.set_wrap(true);
            help_label.add_css_class("caption");
            help_label.add_css_class("dim-label");
            help_label.set_margin_start(10);
            help_label.set_margin_end(10);
            help_label.set_margin_top(10);
            help_label.set_margin_bottom(0);

            vbox.append(&help_label);
            vbox.append(selector.widget());
            vbox
        };

        let ai_tab = build_tab(
            &ai_chat_selector,
            "This model is only used in AI Chat in the Sidebar.",
        );
        let claw_tab = build_tab(
            &claw_selector,
            "This model is used to run Boxxy Agents. A highly capable reasoning model is recommended.",
        );
        let mem_tab = build_tab(
            &memory_selector,
            "This model is used to extract background facts and match database memories. Use a fast, lightweight model here.",
        );

        stack.add_titled(&ai_tab, Some("ai"), "AI Chat");
        stack.add_titled(&claw_tab, Some("claw"), "Boxxy Claw");
        stack.add_titled(&mem_tab, Some("memory"), "Memories");

        // Make Boxxy Claw the default view
        stack.set_visible_child_name("claw");

        let switcher = gtk::StackSwitcher::new();
        switcher.set_stack(Some(&stack));
        switcher.set_margin_top(6);
        switcher.set_margin_start(10);
        switcher.set_margin_end(10);
        switcher.set_margin_bottom(6);
        switcher.set_halign(gtk::Align::Center);

        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.append(&switcher);
        container.append(&stack);

        let close_btn = gtk::Button::builder()
            .label("Done")
            .margin_start(10)
            .margin_end(10)
            .margin_bottom(10)
            .halign(gtk::Align::Center)
            .css_classes(["suggested-action", "pill"])
            .build();
        container.append(&close_btn);

        let d_clone = dialog.clone();
        close_btn.connect_clicked(move |_| {
            d_clone.close();
        });

        dialog.set_child(Some(&container));

        Self {
            dialog,
            ai_chat_selector,
            claw_selector,
            memory_selector,
        }
    }

    pub fn dialog(&self) -> &libadwaita::Dialog {
        &self.dialog
    }

    pub fn present(&self, parent: Option<&impl IsA<gtk::Widget>>) {
        self.dialog.present(parent);
    }
}
