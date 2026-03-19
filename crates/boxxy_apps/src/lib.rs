pub mod dialog;
pub mod engine;

use boxxy_model_selection::ModelProvider;
use dialog::CreateAppDialog;
use engine::BoxxyAppEngine;
use gtk::glib;
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;

#[derive(Clone)]
pub struct BoxxyAppsComponent {
    widget: gtk::Box,
    inner: Rc<RefCell<BoxxyAppsInner>>,
}

struct BoxxyAppsInner {
    engine: Rc<RefCell<BoxxyAppEngine>>,
    create_dialog: CreateAppDialog,
    content_box: gtk::Box,
    model_provider: Option<ModelProvider>,
    running_app: Option<PathBuf>,
    app_list_box: gtk::ListBox,
    new_app_btn: gtk::Button,
    close_app_btn: gtk::Button,
}

impl std::fmt::Debug for BoxxyAppsComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoxxyAppsComponent").finish()
    }
}

impl BoxxyAppsComponent {
    pub fn new() -> Self {
        let engine = Rc::new(RefCell::new(BoxxyAppEngine::new()));

        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);

        // Sidebar
        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 12);
        sidebar.set_width_request(250);
        sidebar.set_margin_top(12);
        sidebar.set_margin_bottom(12);
        sidebar.set_margin_start(12);
        sidebar.set_margin_end(12);
        sidebar.add_css_class("boxxy-apps-sidebar");

        let title = gtk::Label::new(Some("Installed Apps"));
        title.add_css_class("title-2");
        title.set_halign(gtk::Align::Start);
        sidebar.append(&title);

        let new_app_btn = gtk::Button::builder()
            .label("New App")
            .icon_name("list-add-symbolic")
            .sensitive(true)
            .tooltip_text("Create new app with AI")
            .build();
        sidebar.append(&new_app_btn);

        let scroll = gtk::ScrolledWindow::new();
        scroll.set_vexpand(true);
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);

        let app_list_box = gtk::ListBox::new();
        app_list_box.set_selection_mode(gtk::SelectionMode::None);
        app_list_box.add_css_class("boxed-list");
        scroll.set_child(Some(&app_list_box));
        sidebar.append(&scroll);

        let warning_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        warning_box.set_margin_top(4);
        let warning_icon = gtk::Image::from_icon_name("dialog-warning-symbolic");
        warning_icon.set_pixel_size(14);
        warning_icon.set_opacity(0.5);
        warning_box.append(&warning_icon);
        let warning_label = gtk::Label::new(Some("Experimental · Review code before run"));
        warning_label.set_wrap(true);
        warning_label.set_xalign(0.0);
        warning_label.add_css_class("caption");
        warning_label.set_opacity(0.5);
        warning_box.append(&warning_label);
        sidebar.append(&warning_box);

        widget.append(&sidebar);

        // Content Area
        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(800);
        clamp.set_hexpand(true);
        clamp.set_vexpand(true);

        let overlay = gtk::Overlay::new();
        let content_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content_box.set_margin_top(12);
        content_box.set_margin_bottom(12);
        content_box.set_margin_start(12);
        content_box.set_margin_end(12);
        content_box.set_hexpand(true);
        content_box.set_vexpand(true);
        overlay.set_child(Some(&content_box));

        let close_app_btn = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(12)
            .margin_end(12)
            .visible(false)
            .tooltip_text("Close App")
            .build();
        close_app_btn.add_css_class("flat");
        close_app_btn.add_css_class("circular");
        overlay.add_overlay(&close_app_btn);

        clamp.set_child(Some(&overlay));
        widget.append(&clamp);

        let initial_model = boxxy_preferences::Settings::load().claw_model;

        let inner = Rc::new(RefCell::new(BoxxyAppsInner {
            engine: engine.clone(),
            create_dialog: CreateAppDialog::new(engine, || {}), // Placeholder
            content_box,
            model_provider: initial_model.clone(),
            running_app: None,
            app_list_box,
            new_app_btn,
            close_app_btn,
        }));

        let comp = Self {
            widget,
            inner: inner.clone(),
        };

        // Listen for global settings changes to sync model across windows
        let comp_clone = comp.clone();
        let mut settings_rx = boxxy_preferences::SETTINGS_EVENT_BUS.subscribe();
        glib::spawn_future_local(async move {
            while let Ok(settings) = settings_rx.recv().await {
                let model = settings.claw_model.clone();
                let mut inner = comp_clone.inner.borrow_mut();
                if inner.model_provider != model {
                    inner.model_provider = model.clone();
                    inner.create_dialog.set_model_provider(model);
                }
            }
        });

        // Re-init dialog with proper callback
        let c = comp.clone();
        let engine_clone = comp.inner.borrow().engine.clone();
        comp.inner.borrow_mut().create_dialog = CreateAppDialog::new(engine_clone, move || {
            c.load_apps();
        });

        let c = comp.clone();
        comp.inner.borrow().new_app_btn.connect_clicked(move |_| {
            c.open_create_dialog();
        });

        let c = comp.clone();
        comp.inner.borrow().close_app_btn.connect_clicked(move |_| {
            c.close_app();
        });

        comp.load_apps();
        if let Some(state_path) = Self::load_state() {
            comp.run_app_file(PathBuf::from(state_path));
        }

        comp
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    pub fn load_apps(&self) {
        let inner = self.inner.borrow();
        while let Some(child) = inner.app_list_box.first_child() {
            inner.app_list_box.remove(&child);
        }

        if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
            let apps_dir = dirs.config_dir().join("apps");
            if apps_dir.exists()
                && let Ok(entries) = fs::read_dir(&apps_dir)
            {
                let mut paths: Vec<PathBuf> = entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|ext| ext == "lua"))
                    .collect();

                let order = Self::load_order();
                paths.sort_by(|a, b| {
                    let a_str = a.to_string_lossy().to_string();
                    let b_str = b.to_string_lossy().to_string();
                    let a_idx = order.iter().position(|o| o == &a_str);
                    let b_idx = order.iter().position(|o| o == &b_str);
                    match (a_idx, b_idx) {
                        (Some(ai), Some(bi)) => ai.cmp(&bi),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => a.cmp(b),
                    }
                });

                for path in paths {
                    let row = self.create_app_row(path);
                    inner.app_list_box.append(&row);
                }
            }
        }
    }

    fn create_app_row(&self, path: PathBuf) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.add_css_class("activatable");

        let label_text = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        hbox.set_margin_top(8);
        hbox.set_margin_bottom(8);
        hbox.set_margin_start(8);
        hbox.set_margin_end(8);

        let label = gtk::Label::new(Some(&label_text));
        label.set_halign(gtk::Align::Start);
        label.set_hexpand(true);
        hbox.append(&label);

        let edit_btn = gtk::Button::from_icon_name("document-edit-symbolic");
        edit_btn.add_css_class("flat");
        edit_btn.add_css_class("circular");
        edit_btn.set_tooltip_text(Some("Open in text editor"));
        let p_clone = path.clone();
        edit_btn.connect_clicked(move |_| {
            let _ = gtk::gio::AppInfo::launch_default_for_uri(
                &format!("file://{}", p_clone.display()),
                None::<&gtk::gio::AppLaunchContext>,
            );
        });
        hbox.append(&edit_btn);

        let delete_btn = gtk::Button::from_icon_name("user-trash-symbolic");
        delete_btn.add_css_class("flat");
        delete_btn.add_css_class("circular");
        delete_btn.set_tooltip_text(Some("Remove app"));
        let p_clone = path.clone();
        let c = self.clone();
        delete_btn.connect_clicked(move |_| {
            let _ = fs::remove_file(&p_clone);
            c.load_apps();
            if c.inner.borrow().running_app.as_ref() == Some(&p_clone) {
                c.close_app();
            }
        });
        hbox.append(&delete_btn);

        row.set_child(Some(&hbox));

        // Drag and Drop
        let drag_source = gtk::DragSource::new();
        drag_source.set_actions(gtk::gdk::DragAction::MOVE);
        let p_str = path.to_string_lossy().to_string();
        drag_source.connect_prepare(move |_, _, _| {
            Some(gtk::gdk::ContentProvider::for_value(&p_str.to_value()))
        });
        row.add_controller(drag_source);

        let drop_target = gtk::DropTarget::new(gtk::glib::Type::STRING, gtk::gdk::DragAction::MOVE);
        let p_target = path.clone();
        let c = self.clone();
        drop_target.connect_drop(move |_, value, _, _| {
            if let Ok(source_str) = value.get::<String>() {
                let source_path = PathBuf::from(source_str);
                if source_path != p_target {
                    c.reorder_app(source_path, p_target.clone());
                }
                true
            } else {
                false
            }
        });
        row.add_controller(drop_target);

        let gesture = gtk::GestureClick::new();
        let p_clone = path.clone();
        let c = self.clone();
        gesture.connect_released(move |_, _, _, _| {
            c.run_app_file(p_clone.clone());
        });
        row.add_controller(gesture);

        row
    }

    fn reorder_app(&self, source: PathBuf, target: PathBuf) {
        let mut order = Self::load_order();
        let s_str = source.to_string_lossy().to_string();
        let t_str = target.to_string_lossy().to_string();

        if let (Some(from_idx), Some(to_idx)) = (
            order.iter().position(|o| o == &s_str),
            order.iter().position(|o| o == &t_str),
        ) {
            let item = order.remove(from_idx);
            order.insert(to_idx, item);
            self.save_order_list(order);
            self.load_apps();
        }
    }

    fn save_order_list(&self, order: Vec<String>) {
        if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
            let order_file = dirs.config_dir().join("apps").join("order.json");
            let _ = fs::write(
                order_file,
                serde_json::to_string(&order).unwrap_or_default(),
            );
        }
    }

    fn load_order() -> Vec<String> {
        directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal")
            .and_then(|dirs| {
                let order_file = dirs.config_dir().join("apps").join("order.json");
                let data = fs::read_to_string(order_file).ok()?;
                serde_json::from_str(&data).ok()
            })
            .unwrap_or_default()
    }

    fn open_create_dialog(&self) {
        let inner = self.inner.borrow();
        while let Some(child) = inner.content_box.first_child() {
            inner.content_box.remove(&child);
        }
        inner.create_dialog.clear();
        inner.create_dialog.present();
    }

    fn run_app_file(&self, path: PathBuf) {
        if let Ok(script) = fs::read_to_string(&path) {
            let mut inner = self.inner.borrow_mut();
            inner.running_app = Some(path.clone());
            inner.close_app_btn.set_visible(true);
            drop(inner);
            self.save_state();

            let inner = self.inner.borrow();
            while let Some(child) = inner.content_box.first_child() {
                inner.content_box.remove(&child);
            }

            let engine = inner.engine.borrow();
            match engine.run_script(&script) {
                Ok(widget) => {
                    inner.content_box.append(&widget);
                }
                Err(e) => {
                    let error_label = gtk::Label::new(Some(&format!("Lua Error: {}", e)));
                    inner.content_box.append(&error_label);
                }
            }
        }
    }

    pub fn close_app(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.running_app = None;
        inner.close_app_btn.set_visible(false);
        while let Some(child) = inner.content_box.first_child() {
            inner.content_box.remove(&child);
        }
        drop(inner);
        self.save_state();
    }

    fn save_state(&self) {
        if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
            let state_file = dirs.config_dir().join("apps").join("state.json");
            let inner = self.inner.borrow();
            let state = inner
                .running_app
                .as_ref()
                .map(|p| p.to_string_lossy().to_string());
            let _ = fs::write(
                state_file,
                serde_json::to_string(&state).unwrap_or_default(),
            );
        }
    }

    fn load_state() -> Option<String> {
        directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal")
            .and_then(|dirs| {
                let state_file = dirs.config_dir().join("apps").join("state.json");
                let data = fs::read_to_string(state_file).ok()?;
                serde_json::from_str(&data).ok()
            })
            .flatten()
    }
}

impl Default for BoxxyAppsComponent {
    fn default() -> Self {
        Self::new()
    }
}
