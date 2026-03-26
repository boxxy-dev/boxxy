use crate::Bookmark;
use crate::manager::BookmarksManager;
use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use sourceview5::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

pub struct BookmarkEditor;

impl BookmarkEditor {
    pub fn show(
        parent: &impl IsA<gtk::Widget>,
        bookmark: Option<Bookmark>,
        _on_run: Option<Box<dyn Fn(String)>>,
    ) {
        let dialog = adw::Dialog::builder()
            .content_width(800)
            .content_height(600)
            .build();

        let main_vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);

        // ─── Custom Top Bar ──────────────────────────────────────────────────
        let top_bar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        top_bar.add_css_class("headerbar");
        top_bar.set_margin_start(12);
        top_bar.set_margin_end(12);
        top_bar.set_margin_top(6);
        top_bar.set_margin_bottom(6);

        let name_entry = gtk::Entry::builder()
            .placeholder_text("Script Name...")
            .text(bookmark.as_ref().map(|b| b.name.as_str()).unwrap_or(""))
            .valign(gtk::Align::Center)
            .width_request(400)
            .build();
        top_bar.append(&name_entry);

        let ai_btn = gtk::Button::builder()
            .label("Create with AI")
            .valign(gtk::Align::Center)
            .build();
        top_bar.append(&ai_btn);

        // Spacer to push close button to the end
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        top_bar.append(&spacer);

        let close_btn = gtk::Button::builder()
            .icon_name("boxxy-window-close-symbolic")
            .valign(gtk::Align::Center)
            .build();
        close_btn.add_css_class("flat");
        close_btn.add_css_class("circular");
        let dialog_close = dialog.clone();
        close_btn.connect_clicked(move |_| {
            dialog_close.close();
        });
        top_bar.append(&close_btn);

        main_vbox.append(&top_bar);

        // ─── Editor Content ──────────────────────────────────────────────────
        let buffer = sourceview5::Buffer::new(None);
        let lang_manager = sourceview5::LanguageManager::default();
        buffer.set_language(lang_manager.language("sh").as_ref());

        let settings = boxxy_preferences::Settings::load();
        let palette = boxxy_themes::load_palette(&settings.theme);
        let is_dark = adw::StyleManager::default().is_dark();
        boxxy_themes::apply_sourceview_palette(&buffer, palette.as_ref(), is_dark);

        if let Some(bm) = &bookmark {
            let script = if bm.script.is_empty() {
                BookmarksManager::get_script(&bm.filename).unwrap_or_default()
            } else {
                bm.script.clone()
            };
            buffer.set_text(&script);
        }

        let source_view = sourceview5::View::with_buffer(&buffer);
        source_view.set_show_line_numbers(true);
        source_view.set_highlight_current_line(true);
        source_view.set_vexpand(true);
        source_view.set_hexpand(true);

        // Set the same font as the terminal via CSS
        let current_font_size = Rc::new(RefCell::new(settings.font_size as f64));
        let provider = gtk::CssProvider::new();
        let font_name = settings.font_name.clone();

        let update_font = {
            let provider = provider.clone();
            let font_name = font_name.clone();
            let current_font_size = current_font_size.clone();
            move || {
                let size = *current_font_size.borrow();
                let css = format!(
                    "textview {{ font-family: \"{}\"; font-size: {}pt; }}",
                    font_name, size
                );
                provider.load_from_string(&css);
            }
        };
        update_font();

        #[allow(deprecated)]
        source_view
            .style_context()
            .add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

        // Add Zoom Shortcuts
        let zoom_controller = gtk::ShortcutController::new();

        // Zoom In
        {
            let action = {
                let update_font = update_font.clone();
                let current_font_size = current_font_size.clone();
                gtk::CallbackAction::new(move |_, _| {
                    *current_font_size.borrow_mut() += 1.0;
                    update_font();
                    gtk::glib::Propagation::Stop
                })
            };
            let shortcut = gtk::Shortcut::builder()
                .trigger(&gtk::ShortcutTrigger::parse_string("<Control>plus").unwrap())
                .action(&action)
                .build();
            zoom_controller.add_shortcut(shortcut);

            let action = {
                let update_font = update_font.clone();
                let current_font_size = current_font_size.clone();
                gtk::CallbackAction::new(move |_, _| {
                    *current_font_size.borrow_mut() += 1.0;
                    update_font();
                    gtk::glib::Propagation::Stop
                })
            };
            let shortcut = gtk::Shortcut::builder()
                .trigger(&gtk::ShortcutTrigger::parse_string("<Control>equal").unwrap())
                .action(&action)
                .build();
            zoom_controller.add_shortcut(shortcut);
        }

        // Zoom Out
        {
            let action = {
                let update_font = update_font.clone();
                let current_font_size = current_font_size.clone();
                gtk::CallbackAction::new(move |_, _| {
                    let mut size = current_font_size.borrow_mut();
                    if *size > 4.0 {
                        *size -= 1.0;
                    }
                    drop(size);
                    update_font();
                    gtk::glib::Propagation::Stop
                })
            };
            let shortcut = gtk::Shortcut::builder()
                .trigger(&gtk::ShortcutTrigger::parse_string("<Control>minus").unwrap())
                .action(&action)
                .build();
            zoom_controller.add_shortcut(shortcut);
        }

        // Reset Zoom
        {
            let action = {
                let update_font = update_font.clone();
                let current_font_size = current_font_size.clone();
                let original_size = settings.font_size as f64;
                gtk::CallbackAction::new(move |_, _| {
                    *current_font_size.borrow_mut() = original_size;
                    update_font();
                    gtk::glib::Propagation::Stop
                })
            };
            let shortcut = gtk::Shortcut::builder()
                .trigger(&gtk::ShortcutTrigger::parse_string("<Control>0").unwrap())
                .action(&action)
                .build();
            zoom_controller.add_shortcut(shortcut);
        }

        source_view.add_controller(zoom_controller);

        let scroll = gtk::ScrolledWindow::builder()
            .child(&source_view)
            .vexpand(true)
            .build();
        main_vbox.append(&scroll);

        // ─── Custom Bottom Bar ───────────────────────────────────────────────
        let bottom_bar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        bottom_bar.add_css_class("headerbar");
        bottom_bar.set_margin_top(6);
        bottom_bar.set_margin_bottom(6);

        let actions_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        actions_box.set_halign(gtk::Align::Center);
        actions_box.set_hexpand(true);

        let cancel_btn = gtk::Button::with_label("Cancel");
        let dialog_cancel = dialog.clone();
        cancel_btn.connect_clicked(move |_| {
            dialog_cancel.close();
        });
        actions_box.append(&cancel_btn);

        let save_btn = gtk::Button::with_label("Save");
        save_btn.add_css_class("suggested-action");
        actions_box.append(&save_btn);

        bottom_bar.append(&actions_box);
        main_vbox.append(&bottom_bar);

        dialog.set_child(Some(&main_vbox));

        // ─── AI Generation Popover ───────────────────────────────────────────
        let ai_popover = gtk::Popover::builder()
            .autohide(true)
            .position(gtk::PositionType::Bottom)
            .build();
        ai_popover.set_parent(&ai_btn);

        let ai_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        ai_box.set_margin_top(12);
        ai_box.set_margin_bottom(12);
        ai_box.set_margin_start(12);
        ai_box.set_margin_end(12);
        ai_box.set_width_request(300);

        let ai_label = gtk::Label::new(Some("What should this script do?"));
        ai_label.set_halign(gtk::Align::Start);
        ai_label.add_css_class("caption");
        ai_box.append(&ai_label);

        let ai_text_view = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .accepts_tab(false)
            .build();
        ai_text_view.set_top_margin(8);
        ai_text_view.set_bottom_margin(8);
        ai_text_view.set_left_margin(8);
        ai_text_view.set_right_margin(8);
        ai_text_view.add_css_class("view");
        ai_text_view.add_css_class("bookmarks-ai-input");

        // Apply rounded corners and background to the AI input
        let ai_css_provider = gtk::CssProvider::new();
        ai_css_provider.load_from_string(
            "
            .bookmarks-ai-input {
                border-radius: 8px;
                border: 1px solid alpha(currentColor, 0.1);
                background-color: alpha(@window_bg_color, 0.5);
            }
        ",
        );
        #[allow(deprecated)]
        ai_text_view
            .style_context()
            .add_provider(&ai_css_provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

        let ai_text_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .min_content_height(100)
            .child(&ai_text_view)
            .build();
        ai_box.append(&ai_text_scroll);

        let ai_generate_btn = gtk::Button::builder()
            .css_classes(["suggested-action"])
            .build();

        let btn_content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        btn_content.set_halign(gtk::Align::Center);

        let btn_label = gtk::Label::new(Some("Generate"));
        btn_content.append(&btn_label);

        let ai_spinner = gtk::Spinner::new();
        ai_spinner.set_visible(false);
        btn_content.append(&ai_spinner);

        ai_generate_btn.set_child(Some(&btn_content));
        ai_box.append(&ai_generate_btn);

        ai_popover.set_child(Some(&ai_box));

        let ai_pop_clone = ai_popover.clone();
        ai_btn.connect_clicked(move |_| {
            ai_pop_clone.popup();
        });

        // AI Generation Logic
        let buffer_ai = buffer.clone();
        let ai_pop_gen = ai_popover.clone();
        let ai_text_view_gen = ai_text_view.clone();
        let ai_spinner_gen = ai_spinner.clone();
        let ai_btn_gen = ai_generate_btn.clone();

        ai_generate_btn.connect_clicked(move |_| {
            let buffer = ai_text_view_gen.buffer();
            let prompt = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .to_string();
            if prompt.is_empty() {
                return;
            }

            ai_spinner_gen.set_visible(true);
            ai_spinner_gen.start();
            ai_btn_gen.set_sensitive(false);
            ai_text_view_gen.set_sensitive(false);

            let settings = boxxy_preferences::Settings::load();
            let model = settings.claw_model.clone();
            let creds = boxxy_ai_core::AiCredentials::new(
                settings.api_keys.clone(),
                settings.ollama_base_url.clone(),
            );

            let data = gtk::gio::resources_lookup_data(
                "/dev/boxxy/BoxxyTerminal/prompts/bookmark_generator.md",
                gtk::gio::ResourceLookupFlags::NONE,
            )
            .expect("Failed to load bookmark generator prompt resource");
            let system_prompt =
                String::from_utf8(data.to_vec()).expect("Prompt resource is not valid UTF-8");

            let buffer_inner = buffer_ai.clone();
            let ai_pop_inner = ai_pop_gen.clone();
            let ai_spinner_inner = ai_spinner_gen.clone();
            let ai_btn_inner = ai_btn_gen.clone();
            let ai_text_view_inner = ai_text_view_gen.clone();

            gtk::glib::spawn_future_local(async move {
                let (tx, rx) = tokio::sync::oneshot::channel();
                tokio::spawn(async move {
                    let agent = boxxy_ai_core::create_agent(&model, &creds, &system_prompt);
                    let res = agent.prompt(&prompt).await;
                    let _ = tx.send(res);
                });

                if let Ok(res) = rx.await {
                    match res {
                        Ok(code) => {
                            let clean_code = code
                                .trim_start_matches("```bash")
                                .trim_start_matches("```sh")
                                .trim_start_matches("```")
                                .trim_end_matches("```")
                                .trim()
                                .to_string();
                            buffer_inner.set_text(&clean_code);
                            ai_pop_inner.popdown();
                        }
                        Err(e) => {
                            log::error!("AI Generation failed: {}", e);
                        }
                    }
                }
                ai_spinner_inner.stop();
                ai_spinner_inner.set_visible(false);
                ai_btn_inner.set_sensitive(true);
                ai_text_view_inner.set_sensitive(true);
            });
        });

        // ─── Save Logic ──────────────────────────────────────────────────────
        let id = bookmark.map(|b| b.id);
        let dialog_save = dialog.clone();
        let name_entry_save = name_entry.clone();
        let buffer_save = buffer.clone();

        save_btn.connect_clicked(move |_| {
            let name = name_entry_save.text().to_string();
            let script = buffer_save
                .text(&buffer_save.start_iter(), &buffer_save.end_iter(), false)
                .to_string();

            if !name.is_empty() {
                if let Some(existing_id) = id {
                    BookmarksManager::update(existing_id, name, script);
                } else {
                    BookmarksManager::add(name, script);
                }
                dialog_save.close();
            }
        });

        dialog.present(Some(parent));
    }
}
