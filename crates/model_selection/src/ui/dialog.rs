use crate::models::{ModelProvider, OpenAiCompatibleEndpoint};
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
    pub fn new<F>(
        init_ai: Option<ModelProvider>,
        init_claw: Option<ModelProvider>,
        init_memory: Option<ModelProvider>,
        ollama_url: String,
        api_keys: std::collections::HashMap<String, String>,
        on_done: F,
    ) -> Self
    where
        F: Fn(Option<ModelProvider>, Option<ModelProvider>, Option<ModelProvider>) + 'static,
    {
        Self::new_with_custom(
            init_ai,
            init_claw,
            init_memory,
            ollama_url,
            api_keys,
            Vec::new(),
            on_done,
        )
    }

    pub fn new_with_custom<F>(
        init_ai: Option<ModelProvider>,
        init_claw: Option<ModelProvider>,
        init_memory: Option<ModelProvider>,
        ollama_url: String,
        api_keys: std::collections::HashMap<String, String>,
        custom_endpoints: Vec<OpenAiCompatibleEndpoint>,
        on_done: F,
    ) -> Self
    where
        F: Fn(Option<ModelProvider>, Option<ModelProvider>, Option<ModelProvider>) + 'static,
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

        let ai_chat_selector = SingleModelSelector::new_with_custom(
            init_ai,
            ollama_url.clone(),
            api_keys.clone(),
            custom_endpoints.clone(),
            |_| {},
        );
        let claw_selector = SingleModelSelector::new_with_custom(
            init_claw.clone(),
            ollama_url.clone(),
            api_keys.clone(),
            custom_endpoints.clone(),
            |_| {},
        );

        let mem_initial = init_memory.or(init_claw);
        let memory_selector = SingleModelSelector::new_with_custom(
            mem_initial,
            ollama_url,
            api_keys,
            custom_endpoints,
            |_| {},
        );

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
            "This model is used to extract background facts and run the background Dream Cycle. Use a fast, lightweight model here.",
        );

        stack.add_titled(&ai_tab, Some("ai"), "AI Chat");
        stack.add_titled(&claw_tab, Some("claw"), "Boxxy Claw");
        stack.add_titled(&mem_tab, Some("memory"), "Memories and Dreams");

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
        let ai_sel = ai_chat_selector.clone();
        let claw_sel = claw_selector.clone();
        let mem_sel = memory_selector.clone();
        close_btn.connect_clicked(move |_| {
            on_done(
                ai_sel.get_current_provider(),
                claw_sel.get_current_provider(),
                mem_sel.get_current_provider(),
            );
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
