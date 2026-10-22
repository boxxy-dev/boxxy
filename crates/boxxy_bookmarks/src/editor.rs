use crate::manager::BookmarksManager;
use crate::parser::BookmarkTemplate;
use crate::{Bookmark, ScriptLanguage};
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
        let builder =
            gtk::Builder::from_resource("/dev/boxxy/BoxxyTerminal/ui/bookmark_editor.ui");

        let dialog: adw::Dialog = builder
            .object("editor_dialog")
            .expect("editor_dialog in bookmark_editor.ui");
        let window_title: adw::WindowTitle = builder
            .object("window_title")
            .expect("window_title in bookmark_editor.ui");
        let cancel_btn: gtk::Button = builder
            .object("cancel_btn")
            .expect("cancel_btn in bookmark_editor.ui");
        let save_btn: gtk::Button = builder
            .object("save_btn")
            .expect("save_btn in bookmark_editor.ui");
        let lang_icon: gtk::Image = builder
            .object("lang_icon")
            .expect("lang_icon in bookmark_editor.ui");
        let name_entry: gtk::Entry = builder
            .object("name_entry")
            .expect("name_entry in bookmark_editor.ui");
        let lang_badge: gtk::Label = builder
            .object("lang_badge")
            .expect("lang_badge in bookmark_editor.ui");
        let ai_btn: gtk::Button = builder
            .object("ai_btn")
            .expect("ai_btn in bookmark_editor.ui");
        let editor_scroll: gtk::ScrolledWindow = builder
            .object("editor_scroll")
            .expect("editor_scroll in bookmark_editor.ui");
        let detected_vars_row: gtk::Box = builder
            .object("detected_vars_row")
            .expect("detected_vars_row in bookmark_editor.ui");
        let detected_vars_chips: gtk::Box = builder
            .object("detected_vars_chips")
            .expect("detected_vars_chips in bookmark_editor.ui");
        let insert_var_btn: gtk::Button = builder
            .object("insert_var_btn")
            .expect("insert_var_btn in bookmark_editor.ui");

        if let Some(bm) = &bookmark {
            window_title.set_title("Edit Bookmark");
            window_title.set_subtitle(&bm.filename);
            name_entry.set_text(&bm.name);
            save_btn.set_sensitive(!bm.name.trim().is_empty());
        } else {
            window_title.set_title("New Bookmark");
            window_title.set_subtitle("Bash or Python script with optional {{{variable}}} inputs");
            save_btn.set_sensitive(false);
        }

        let dialog_cancel = dialog.clone();
        cancel_btn.connect_clicked(move |_| {
            dialog_cancel.close();
        });

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
        source_view.set_top_margin(12);
        source_view.set_bottom_margin(12);
        source_view.set_left_margin(12);
        source_view.set_right_margin(12);
        source_view.set_vexpand(true);
        source_view.set_hexpand(true);

        static EDITOR_TV_COUNTER: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        let tv_id = EDITOR_TV_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tv_class = format!("bookmark-editor-tv-{tv_id}");
        source_view.add_css_class(&tv_class);

        let current_font_size = Rc::new(RefCell::new(f64::from(settings.font_size)));
        let provider = gtk::CssProvider::new();
        let font_name = settings.font_name.clone();

        let update_font: Rc<dyn Fn()> = Rc::new({
            let provider = provider.clone();
            let current_font_size = current_font_size.clone();
            move || {
                let size = *current_font_size.borrow();
                let css = format!(
                    "textview.{tv_class} {{ font-family: \"{font_name}\"; font-size: {size}pt; }}"
                );
                provider.load_from_string(&css);
            }
        });
        update_font();

        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
            let provider_cleanup = provider;
            source_view.connect_destroy(move |_| {
                if let Some(display) = gtk::gdk::Display::default() {
                    gtk::style_context_remove_provider_for_display(&display, &provider_cleanup);
                }
            });
        }

        // Zoom shortcuts (Ctrl++, Ctrl+-, Ctrl+0)
        Self::attach_zoom_shortcuts(
            &source_view,
            &current_font_size,
            f64::from(settings.font_size),
            &update_font,
        );

        editor_scroll.set_child(Some(&source_view));

        // ─── Live Language & Placeholder Detection ───────────────────────────
        let sync_metadata = {
            let name_entry = name_entry.clone();
            let buffer = buffer.clone();
            let lang_icon = lang_icon;
            let lang_badge = lang_badge;
            let save_btn = save_btn.clone();
            let detected_vars_row = detected_vars_row;
            let detected_vars_chips = detected_vars_chips;
            move || {
                let name = name_entry.text();
                save_btn.set_sensitive(!name.trim().is_empty());

                let script = buffer
                    .text(&buffer.start_iter(), &buffer.end_iter(), false)
                    .to_string();

                let lang = ScriptLanguage::detect(&name, &script);
                lang_icon.set_icon_name(Some(lang.icon_name()));
                lang_badge.set_label(lang.badge_label());

                let lang_mgr = sourceview5::LanguageManager::default();
                let target_lang = lang_mgr.language(lang.sourceview_lang_id());
                if buffer.language() != target_lang {
                    buffer.set_language(target_lang.as_ref());
                }

                // Update live detected {{{variable}}} chips
                while let Some(child) = detected_vars_chips.first_child() {
                    detected_vars_chips.remove(&child);
                }

                let tpl = BookmarkTemplate::parse(&script);
                if tpl.has_placeholders() {
                    for var in tpl.placeholders {
                        let chip = gtk::Label::new(Some(&format!("{{{{{{{var}}}}}}}")));
                        chip.add_css_class("bookmark-var-chip");
                        chip.add_css_class("monospace");
                        chip.add_css_class("caption");
                        detected_vars_chips.append(&chip);
                    }
                    detected_vars_row.set_visible(true);
                } else {
                    detected_vars_row.set_visible(false);
                }
            }
        };

        sync_metadata();

        let sync_on_name = sync_metadata.clone();
        name_entry.connect_changed(move |_| {
            sync_on_name();
        });

        let sync_on_buffer = sync_metadata;
        buffer.connect_changed(move |_| {
            sync_on_buffer();
        });

        // ─── "+ Insert {{{var}}}" Helper Button ──────────────────────────────
        let buffer_insert = buffer.clone();
        let sv_insert = source_view;
        insert_var_btn.connect_clicked(move |_| {
            let placeholder = "{{{variable}}}";
            buffer_insert.insert_at_cursor(placeholder);

            // Select "variable" inside {{{variable}}} so typing immediately replaces the name
            let cursor_mark = buffer_insert.get_insert();
            let mut end_iter = buffer_insert.iter_at_mark(&cursor_mark);
            let mut start_iter = end_iter;
            if end_iter.backward_chars(3) && start_iter.backward_chars(11) {
                buffer_insert.select_range(&start_iter, &end_iter);
            }
            sv_insert.grab_focus();
        });

        // ─── AI Generation Popover ───────────────────────────────────────────
        Self::setup_ai_popover(&ai_btn, &buffer, &name_entry);

        // ─── Save Logic ──────────────────────────────────────────────────────
        let id = bookmark.map(|b| b.id);
        let dialog_save = dialog.clone();
        let name_entry_save = name_entry.clone();
        let buffer_save = buffer;

        let do_save = Rc::new(move || {
            let name = name_entry_save.text().trim().to_string();
            if name.is_empty() {
                name_entry_save.grab_focus();
                return;
            }
            let script = buffer_save
                .text(&buffer_save.start_iter(), &buffer_save.end_iter(), false)
                .to_string();

            if let Some(existing_id) = id {
                BookmarksManager::update(existing_id, name, script);
            } else {
                BookmarksManager::add(name, script);
            }
            dialog_save.close();
        });

        let do_save_btn = do_save;
        save_btn.connect_clicked(move |_| {
            do_save_btn();
        });

        dialog.present(Some(parent));
        if name_entry.text().is_empty() {
            name_entry.grab_focus();
        }
    }

    fn setup_ai_popover(
        ai_btn: &gtk::Button,
        buffer: &sourceview5::Buffer,
        name_entry: &gtk::Entry,
    ) {
        let ai_popover = gtk::Popover::builder()
            .autohide(true)
            .position(gtk::PositionType::Bottom)
            .build();
        ai_popover.set_parent(ai_btn);

        let ai_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
        ai_box.set_margin_top(14);
        ai_box.set_margin_bottom(14);
        ai_box.set_margin_start(14);
        ai_box.set_margin_end(14);
        ai_box.set_width_request(340);

        let ai_title = gtk::Label::new(Some("Generate Script with AI"));
        ai_title.set_halign(gtk::Align::Start);
        ai_title.add_css_class("heading");
        ai_box.append(&ai_title);

        let ai_subtitle = gtk::Label::new(Some(
            "Describe what this script should do. Boxxy will automatically add {{{variable}}} placeholders for dynamic inputs.",
        ));
        ai_subtitle.set_halign(gtk::Align::Start);
        ai_subtitle.set_wrap(true);
        ai_subtitle.set_xalign(0.0);
        ai_subtitle.add_css_class("caption");
        ai_subtitle.add_css_class("dim-label");
        ai_box.append(&ai_subtitle);

        let ai_text_view = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .accepts_tab(false)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(8)
            .right_margin(8)
            .build();
        ai_text_view.add_css_class("view");
        ai_text_view.add_css_class("bookmarks-ai-input");

        let ai_text_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .min_content_height(100)
            .child(&ai_text_view)
            .build();
        ai_box.append(&ai_text_scroll);

        let ai_error_label = gtk::Label::new(None);
        ai_error_label.set_halign(gtk::Align::Start);
        ai_error_label.set_wrap(true);
        ai_error_label.set_xalign(0.0);
        ai_error_label.add_css_class("caption");
        ai_error_label.add_css_class("error");
        ai_error_label.set_visible(false);
        ai_box.append(&ai_error_label);

        let ai_generate_btn = gtk::Button::builder()
            .css_classes(["suggested-action"])
            .build();

        let btn_content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        btn_content.set_halign(gtk::Align::Center);

        let btn_label = gtk::Label::new(Some("Generate Script"));
        btn_content.append(&btn_label);

        let svg = gtk::Svg::from_resource("/dev/boxxy/BoxxyTerminal/icons/boxxy-spinner.gpa");
        svg.play();

        let ai_spinner = gtk::Image::builder().paintable(&svg).pixel_size(16).build();
        ai_spinner.set_visible(false);

        ai_spinner.connect_map({
            let svg = svg.clone();
            move |widget| {
                if let Some(native) = widget.native()
                    && let Some(surface) = native.surface()
                {
                    svg.set_frame_clock(&surface.frame_clock());
                }
            }
        });

        if let Some(clock) = ai_spinner.frame_clock() {
            svg.set_frame_clock(&clock);
        }
        btn_content.append(&ai_spinner);

        ai_generate_btn.set_child(Some(&btn_content));
        ai_box.append(&ai_generate_btn);

        ai_popover.set_child(Some(&ai_box));

        let ai_pop_clone = ai_popover.clone();
        let ai_tv_focus = ai_text_view.clone();
        ai_btn.connect_clicked(move |_| {
            ai_pop_clone.popup();
            ai_tv_focus.grab_focus();
        });

        let buffer_ai = buffer.clone();
        let name_entry_ai = name_entry.clone();
        let ai_pop_gen = ai_popover;
        let ai_text_view_gen = ai_text_view;
        let ai_spinner_gen = ai_spinner;
        let ai_btn_gen = ai_generate_btn.clone();
        let ai_err_gen = ai_error_label;

        ai_generate_btn.connect_clicked(move |_| {
            let prompt_buf = ai_text_view_gen.buffer();
            let prompt = prompt_buf
                .text(&prompt_buf.start_iter(), &prompt_buf.end_iter(), false)
                .trim()
                .to_string();
            if prompt.is_empty() {
                return;
            }

            ai_err_gen.set_visible(false);
            ai_spinner_gen.set_visible(true);
            ai_btn_gen.set_sensitive(false);
            ai_text_view_gen.set_sensitive(false);

            let settings = boxxy_preferences::Settings::load();
            let model = settings.claw_model.clone().or(settings.ai_chat_model.clone());
            let creds = boxxy_ai_core::AiCredentials::new(
                settings.get_effective_api_keys(),
                settings.ollama_base_url.clone(),
            )
            .with_custom_endpoints(settings.openai_compatible_endpoints.clone());

            let data = gtk::gio::resources_lookup_data(
                "/dev/boxxy/BoxxyTerminal/prompts/bookmark_generator.md",
                gtk::gio::ResourceLookupFlags::NONE,
            )
            .expect("Failed to load bookmark generator prompt resource");
            let system_prompt =
                String::from_utf8(data.to_vec()).expect("Prompt resource is not valid UTF-8");

            let buffer_inner = buffer_ai.clone();
            let name_entry_inner = name_entry_ai.clone();
            let ai_pop_inner = ai_pop_gen.clone();
            let ai_spinner_inner = ai_spinner_gen.clone();
            let ai_btn_inner = ai_btn_gen.clone();
            let ai_text_view_inner = ai_text_view_gen.clone();
            let ai_err_inner = ai_err_gen.clone();

            gtk::glib::spawn_future_local(async move {
                let (tx, rx) = tokio::sync::oneshot::channel();
                tokio::spawn(async move {
                    let agent = boxxy_ai_core::create_agent(&model, &creds, &system_prompt);
                    let res = agent.prompt(&prompt).await;
                    let _ = tx.send(res);
                });

                if let Ok(res) = rx.await {
                    match res {
                        Ok(res_val) => {
                            let code = res_val.0;
                            let clean_code = Self::strip_markdown_fences(&code);
                            buffer_inner.set_text(&clean_code);

                            // If the user hasn't named the bookmark yet, suggest a default name
                            if name_entry_inner.text().trim().is_empty() {
                                let ext = ScriptLanguage::detect("", &clean_code).default_extension();
                                name_entry_inner.set_text(&format!("custom_script{ext}"));
                            }

                            ai_pop_inner.popdown();
                        }
                        Err(e) => {
                            log::error!("AI Generation failed: {e}");
                            ai_err_inner.set_text(&format!("Generation failed: {e}"));
                            ai_err_inner.set_visible(true);
                        }
                    }
                }
                ai_spinner_inner.set_visible(false);
                ai_btn_inner.set_sensitive(true);
                ai_text_view_inner.set_sensitive(true);
            });
        });
    }

    fn strip_markdown_fences(code: &str) -> String {
        let trimmed = code.trim();
        if let Some(rest) = trimmed.strip_prefix("```") {
            let after_first_line = rest.find('\n').map_or(rest, |idx| &rest[idx + 1..]);
            after_first_line
                .trim_end()
                .strip_suffix("```")
                .unwrap_or(after_first_line)
                .trim()
                .to_string()
        } else {
            trimmed.to_string()
        }
    }

    fn attach_zoom_shortcuts(
        source_view: &sourceview5::View,
        current_font_size: &Rc<RefCell<f64>>,
        original_size: f64,
        update_font: &Rc<dyn Fn()>,
    ) {
        let zoom_controller = gtk::ShortcutController::new();

        // Zoom In
        {
            let update_font = update_font.clone();
            let current_font_size = current_font_size.clone();
            let action = gtk::CallbackAction::new(move |_, _| {
                *current_font_size.borrow_mut() += 1.0;
                update_font();
                gtk::glib::Propagation::Stop
            });
            let shortcut = gtk::Shortcut::builder()
                .trigger(
                    &gtk::ShortcutTrigger::parse_string(
                        "<Control>plus|<Control>equal|<Control>KP_Add|<Control><Shift>plus|<Control><Shift>equal",
                    )
                    .unwrap(),
                )
                .action(&action)
                .build();
            zoom_controller.add_shortcut(shortcut);
        }

        // Zoom Out
        {
            let update_font = update_font.clone();
            let current_font_size = current_font_size.clone();
            let action = gtk::CallbackAction::new(move |_, _| {
                let mut size = current_font_size.borrow_mut();
                if *size > 4.0 {
                    *size -= 1.0;
                }
                drop(size);
                update_font();
                gtk::glib::Propagation::Stop
            });
            let shortcut = gtk::Shortcut::builder()
                .trigger(
                    &gtk::ShortcutTrigger::parse_string(
                        "<Control>minus|<Control>KP_Subtract|<Control>underscore|<Control><Shift>minus",
                    )
                    .unwrap(),
                )
                .action(&action)
                .build();
            zoom_controller.add_shortcut(shortcut);
        }

        // Reset Zoom
        {
            let update_font = update_font.clone();
            let current_font_size = current_font_size.clone();
            let action = gtk::CallbackAction::new(move |_, _| {
                *current_font_size.borrow_mut() = original_size;
                update_font();
                gtk::glib::Propagation::Stop
            });
            let shortcut = gtk::Shortcut::builder()
                .trigger(&gtk::ShortcutTrigger::parse_string("<Control>0|<Control>KP_0").unwrap())
                .action(&action)
                .build();
            zoom_controller.add_shortcut(shortcut);
        }

        source_view.add_controller(zoom_controller);
    }
}
