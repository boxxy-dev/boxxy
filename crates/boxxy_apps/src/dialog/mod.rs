use gtk4::prelude::*;
use libadwaita::prelude::*;
use libadwaita as adw;
use gtk4 as gtk;
use std::rc::Rc;
use std::cell::RefCell;
use std::sync::Arc;
use crate::engine::BoxxyAppEngine;
use gtk::glib;
use boxxy_model_selection::ModelProvider;
use std::fs;

#[derive(Clone)]
pub struct CreateAppDialog {
    dialog: adw::Window,
    inner: Rc<RefCell<CreateAppInner>>,
}

struct CreateAppInner {
    engine: Rc<RefCell<BoxxyAppEngine>>,
    prompt_buffer: gtk::EntryBuffer,
    filename_buffer: gtk::EntryBuffer,
    response_view: gtk::Box,
    is_loading: bool,
    model_provider: ModelProvider,
    generated_code: Option<String>,
    generation_task: Option<tokio::task::JoinHandle<()>>,
    spinner: gtk::Spinner,
    action_btn: gtk::Button,
    save_btn: gtk::Button,
    row: adw::ActionRow,
    filename_box: gtk::Box,
}

impl std::fmt::Debug for CreateAppDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateAppDialog").finish()
    }
}

impl CreateAppDialog {
    pub fn new<F: Fn() + 'static>(engine: Rc<RefCell<BoxxyAppEngine>>, on_saved: F) -> Self {
        let on_saved = Arc::new(on_saved);
        
        let ui_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<interface>
  <object class="AdwWindow" id="dialog">
    <property name="default-width">600</property>
    <property name="default-height">600</property>
    <property name="title" translatable="yes">Create Boxxy App</property>
    <property name="modal">True</property>
    <property name="hide-on-close">True</property>
    <property name="content">
      <object class="AdwToolbarView">
        <child type="top">
          <object class="AdwHeaderBar">
            <property name="title-widget">
              <object class="AdwWindowTitle">
                <property name="title" translatable="yes">Create Boxxy App</property>
              </object>
            </property>
          </object>
        </child>
        <property name="content">
          <object class="GtkBox">
            <property name="orientation">vertical</property>
            <property name="margin-top">24</property>
            <property name="margin-bottom">24</property>
            <property name="margin-start">24</property>
            <property name="margin-end">24</property>
            <property name="spacing">12</property>
            <child>
              <object class="GtkLabel">
                <property name="label" translatable="yes">Describe the app you want to create:</property>
                <property name="halign">start</property>
                <style>
                  <class name="heading"/>
                </style>
              </object>
            </child>
            <child>
              <object class="GtkEntry" id="prompt_entry">
                <property name="placeholder-text" translatable="yes">e.g., 'Convert MP3 to Opus using ffmpeg'</property>
              </object>
            </child>
            <child>
              <object class="AdwPreferencesGroup">
                <child>
                  <object class="AdwActionRow" id="action_row">
                    <property name="title" translatable="yes">AI Generation</property>
                    <child type="prefix">
                      <object class="GtkSpinner" id="spinner"/>
                    </child>
                    <child type="suffix">
                      <object class="GtkButton" id="action_btn">
                        <property name="icon-name">paper-plane-symbolic</property>
                        <property name="valign">center</property>
                        <style>
                          <class name="circular"/>
                          <class name="suggested-action"/>
                        </style>
                      </object>
                    </child>
                  </object>
                </child>
              </object>
            </child>
            <child>
              <object class="GtkSeparator"/>
            </child>
            <child>
              <object class="GtkBox" id="filename_box">
                <property name="orientation">horizontal</property>
                <property name="spacing">12</property>
                <property name="visible">False</property>
                <child>
                  <object class="GtkLabel">
                    <property name="label" translatable="yes">Filename:</property>
                  </object>
                </child>
                <child>
                  <object class="GtkEntry" id="filename_entry">
                    <property name="placeholder-text" translatable="yes">app_name</property>
                    <property name="hexpand">True</property>
                  </object>
                </child>
                <child>
                  <object class="GtkLabel">
                    <property name="label" translatable="yes">.lua</property>
                    <style>
                      <class name="dim-label"/>
                    </style>
                  </object>
                </child>
              </object>
            </child>
            <child>
              <object class="GtkLabel">
                <property name="label" translatable="yes">Preview:</property>
                <property name="halign">start</property>
                <style>
                  <class name="heading"/>
                </style>
              </object>
            </child>
            <child>
              <object class="GtkBox" id="response_view">
                <property name="orientation">vertical</property>
                <property name="vexpand">True</property>
                <style>
                  <class name="card"/>
                </style>
              </object>
            </child>
            <child>
              <object class="GtkButton" id="save_btn">
                <property name="label" translatable="yes">Save</property>
                <property name="halign">center</property>
                <property name="margin-top">12</property>
                <property name="sensitive">False</property>
                <style>
                  <class name="suggested-action"/>
                  <class name="pill"/>
                </style>
              </object>
            </child>
          </object>
        </property>
      </object>
    </property>
  </object>
</interface>
"#;

        let builder = gtk::Builder::from_string(ui_xml);
        let dialog: adw::Window = builder.object("dialog").unwrap();
        let prompt_entry: gtk::Entry = builder.object("prompt_entry").unwrap();
        let action_row: adw::ActionRow = builder.object("action_row").unwrap();
        let spinner: gtk::Spinner = builder.object("spinner").unwrap();
        let action_btn: gtk::Button = builder.object("action_btn").unwrap();
        let filename_box: gtk::Box = builder.object("filename_box").unwrap();
        let filename_entry: gtk::Entry = builder.object("filename_entry").unwrap();
        let response_view: gtk::Box = builder.object("response_view").unwrap();
        let save_btn: gtk::Button = builder.object("save_btn").unwrap();

        let inner = Rc::new(RefCell::new(CreateAppInner {
            engine,
            prompt_buffer: prompt_entry.buffer(),
            filename_buffer: filename_entry.buffer(),
            response_view,
            is_loading: false,
            model_provider: ModelProvider::default(),
            generated_code: None,
            generation_task: None,
            spinner,
            action_btn,
            save_btn,
            row: action_row,
            filename_box,
        }));

        let comp = Self { dialog, inner };

        let c = comp.clone();
        comp.inner.borrow().action_btn.connect_clicked(move |_| {
            c.toggle_action();
        });

        let c = comp.clone();
        prompt_entry.connect_activate(move |_| {
            c.toggle_action();
        });

        let c = comp.clone();
        let os = on_saved.clone();
        comp.inner.borrow().save_btn.connect_clicked(move |_| {
            if c.save_app() {
                os();
            }
        });

        comp
    }

    pub fn set_model_provider(&self, provider: ModelProvider) {
        self.inner.borrow_mut().model_provider = provider;
        self.update_ui();
    }

    pub fn present(&self) {
        self.dialog.present();
    }

    pub fn clear(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.prompt_buffer.set_text("");
        inner.filename_buffer.set_text("");
        inner.generated_code = None;
        inner.is_loading = false;
        if let Some(task) = inner.generation_task.take() {
            task.abort();
        }
        while let Some(child) = inner.response_view.first_child() {
            inner.response_view.remove(&child);
        }
        drop(inner);
        self.update_ui();
    }

    fn toggle_action(&self) {
        let is_loading = self.inner.borrow().is_loading;
        if is_loading {
            self.cancel_generation();
        } else {
            self.submit_prompt();
        }
    }

    fn cancel_generation(&self) {
        let mut inner = self.inner.borrow_mut();
        if let Some(task) = inner.generation_task.take() {
            task.abort();
        }
        inner.is_loading = false;
        drop(inner);
        self.update_ui();
    }

    fn submit_prompt(&self) {
        let mut inner = self.inner.borrow_mut();
        let prompt = inner.prompt_buffer.text().to_string();
        if prompt.is_empty() {
            return;
        }

        inner.is_loading = true;
        let provider = inner.model_provider.clone();
        let c = self.clone();
        
        let settings = boxxy_preferences::Settings::load();
        let api_key = settings.gemini_api_key;
        let ollama_url = settings.ollama_base_url;

        let data = gtk::gio::resources_lookup_data("/play/mii/Boxxy/prompts/boxxy_app_generator.md", gtk::gio::ResourceLookupFlags::NONE)
            .expect("Failed to load app generator prompt resource");
        let system_prompt = String::from_utf8(data.to_vec()).expect("Prompt resource is not valid UTF-8");

        let (tx, rx) = tokio::sync::oneshot::channel();

        let handle = tokio::spawn(async move {
            let agent = boxxy_ai_core::create_agent(&provider, &api_key, &ollama_url, &system_prompt);
            let res = agent.prompt(&prompt).await;
            let _ = tx.send(res);
        });

        inner.generation_task = Some(handle);
        drop(inner);
        self.update_ui();

        glib::spawn_future_local(async move {
            if let Ok(res) = rx.await {
                match res {
                    Ok(response) => {
                        let code = response
                            .trim_start_matches("```lua")
                            .trim_start_matches("```")
                            .trim_end_matches("```")
                            .trim()
                            .to_string();
                        c.receive_code(code);
                    }
                    Err(e) => {
                        c.receive_code(format!("-- Error: {}", e));
                    }
                }
            }
        });
    }

    fn receive_code(&self, code: String) {
        let mut inner = self.inner.borrow_mut();
        inner.is_loading = false;
        inner.generated_code = Some(code.clone());

        while let Some(child) = inner.response_view.first_child() {
            inner.response_view.remove(&child);
        }

        {
            let engine = inner.engine.borrow();
            let run_result = engine.run_script(&code);

            match run_result {
                Ok(widget) => {
                    inner.response_view.append(&widget);
                    if inner.filename_buffer.text().is_empty() {
                        inner.filename_buffer.set_text("boxxy_app");
                    }
                }
                Err(e) => {
                    let label = gtk::Label::new(Some(&format!(
                        "Error running generated code:\n{}\n\nCode:\n{}",
                        e, code
                    )));
                    label.set_wrap(true);
                    inner.response_view.append(&label);
                }
            }
        }
        drop(inner);
        self.update_ui();
    }

    fn save_app(&self) -> bool {
        let inner = self.inner.borrow();
        if let Some(code) = &inner.generated_code {
            let filename = inner.filename_buffer.text().to_string();
            let filename = if filename.ends_with(".lua") {
                filename
            } else {
                format!("{}.lua", filename)
            };

            if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
                let apps_dir = dirs.config_dir().join("apps");
                if !apps_dir.exists() {
                    let _ = fs::create_dir_all(&apps_dir);
                }

                let path = apps_dir.join(filename);
                if fs::write(&path, code).is_ok() {
                    self.dialog.set_visible(false);
                    return true;
                }
            }
        }
        false
    }

    fn update_ui(&self) {
        let inner = self.inner.borrow();
        
        inner.spinner.set_spinning(inner.is_loading);
        inner.spinner.set_visible(inner.is_loading);
        
        if inner.is_loading {
            inner.row.set_title("Generating App...");
            inner.action_btn.set_icon_name("media-playback-stop-symbolic");
            inner.action_btn.set_css_classes(&["circular", "destructive-action"]);
            inner.action_btn.set_tooltip_text(Some("Cancel"));
        } else {
            inner.row.set_title("AI Generation");
            inner.action_btn.set_icon_name("paper-plane-symbolic");
            inner.action_btn.set_css_classes(&["circular", "suggested-action"]);
            inner.action_btn.set_tooltip_text(Some("Generate"));
        }

        inner.action_btn.set_sensitive(true);
        inner.filename_box.set_visible(inner.generated_code.is_some());
        inner.save_btn.set_sensitive(!inner.is_loading && inner.generated_code.is_some());
    }
}
