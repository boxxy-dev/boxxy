//! Read-only "Claw" sidebar widgets and row factory.
//!
//! This crate hosts everything that's rendered in the Claw page of
//! the right-hand sidebar (see `boxxy_window::ui::build_sidebar`). It
//! is deliberately display-only: approval buttons live in the
//! in-terminal popover. The `add_*_approval_row` helpers only log a
//! formatted diagnosis row; their callback args exist for backward
//! compatibility with `terminal::pane::claw` and are intentionally
//! unused.

use boxxy_claw_protocol::PersistentClawRow;
use boxxy_core_widgets::ObjectExtSafe;
use boxxy_viewer::{BlockRenderer, ContentBlock, StructuredViewer, ViewerRegistry};
use gtk::prelude::*;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::gio;
use libadwaita as adw;
use std::rc::Rc;

pub mod row_object;
pub mod sidebar;

pub use sidebar::ClawSidebarComponent;

use row_object::ClawRowObject;

// ---------------------------------------------------------------------------
// Custom renderer: list_processes
// ---------------------------------------------------------------------------

/// Renders the `list_processes` custom block into a compact columnar list.
/// Two payload schemas are accepted — the object form (new tool output) and
/// the tuple form (legacy persisted rows) — so sessions restored from the DB
/// don't silently fail to render.
pub struct ProcessListRenderer;

impl BlockRenderer for ProcessListRenderer {
    fn can_render(&self, block: &ContentBlock) -> bool {
        matches!(block, ContentBlock::Custom { schema, .. } if schema == "list_processes")
    }

    fn render(&self, block: &ContentBlock, _registry: &ViewerRegistry) -> gtk::Widget {
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);

        let ContentBlock::Custom { raw_payload, .. } = block else {
            return vbox.upcast();
        };

        #[derive(serde::Deserialize)]
        struct ProcessInfo {
            pid: u32,
            name: String,
            cpu_usage: f64,
            memory_bytes: u64,
            read_bytes: u64,
            written_bytes: u64,
        }

        #[derive(serde::Deserialize)]
        struct ProcessListOutput {
            processes: Vec<ProcessInfo>,
        }

        let list_box = gtk::ListBox::new();
        list_box.add_css_class("boxed-list");
        list_box.set_selection_mode(gtk::SelectionMode::None);

        let append_row = |list_box: &gtk::ListBox,
                          pid: u32,
                          name: &str,
                          cpu: f64,
                          mem: u64,
                          read: u64,
                          write: u64| {
            let row = gtk::ListBoxRow::new();
            let item_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            item_box.set_margin_top(4);
            item_box.set_margin_bottom(4);
            item_box.set_margin_start(4);
            item_box.set_margin_end(4);

            let pid_lbl = gtk::Label::new(Some(&format!("{pid}")));
            pid_lbl.set_width_chars(6);
            item_box.append(&pid_lbl);

            let name_lbl = gtk::Label::new(Some(name));
            name_lbl.set_hexpand(true);
            name_lbl.set_halign(gtk::Align::Start);
            name_lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
            item_box.append(&name_lbl);

            // Only show disk I/O when there's actual activity; quiet
            // processes get a cleaner row.
            if read > 0 || write > 0 {
                let io_str = format!("R:{}MB W:{}MB", read / 1_048_576, write / 1_048_576);
                let io_lbl = gtk::Label::new(Some(&io_str));
                io_lbl.add_css_class("caption");
                io_lbl.add_css_class("dim-label");
                io_lbl.set_margin_end(6);
                item_box.append(&io_lbl);
            }

            let cpu_lbl = gtk::Label::new(Some(&format!("{cpu:.1}%")));
            cpu_lbl.add_css_class("caption");
            cpu_lbl.add_css_class("dim-label");
            item_box.append(&cpu_lbl);

            let mem_lbl = gtk::Label::new(Some(&format!("{}MB", mem / (1024 * 1024))));
            mem_lbl.add_css_class("caption");
            mem_lbl.add_css_class("dim-label");
            item_box.append(&mem_lbl);

            row.set_child(Some(&item_box));
            list_box.append(&row);
        };

        if let Ok(output) = serde_json::from_str::<ProcessListOutput>(raw_payload) {
            for info in output.processes {
                append_row(
                    &list_box,
                    info.pid,
                    &info.name,
                    info.cpu_usage,
                    info.memory_bytes,
                    info.read_bytes,
                    info.written_bytes,
                );
            }
            vbox.append(&list_box);
        } else if let Ok(processes) =
            serde_json::from_str::<Vec<(u32, String, f64, u64, u64, u64)>>(raw_payload)
        {
            // Legacy tuple schema — keep parsing so old persisted sessions
            // restore without a "failed to parse" error.
            for (pid, name, cpu, mem, read, write) in processes {
                append_row(&list_box, pid, &name, cpu, mem, read, write);
            }
            vbox.append(&list_box);
        } else {
            let error_lbl = gtk::Label::new(Some("Failed to parse process list."));
            error_lbl.add_css_class("error");
            vbox.append(&error_lbl);
        }

        vbox.upcast()
    }
}

/// Returns the registry used by every Claw-sidebar `StructuredViewer`:
/// the standard text/code/image renderers plus the custom list_processes
/// renderer. Wrapped in `Rc` because `StructuredViewer::new` takes a handle.
pub fn get_claw_viewer_registry() -> Rc<ViewerRegistry> {
    let mut registry = ViewerRegistry::new_with_defaults();
    registry.register(Box::new(ProcessListRenderer));
    Rc::new(registry)
}

// ---------------------------------------------------------------------------
// Virtual history list
// ---------------------------------------------------------------------------

/// Builds the Claw sidebar's per-pane history ListView.
///
/// Each row has a single stable widget tree (icon, title, pane label,
/// `StructuredViewer`, command label) set up once in `connect_setup` and
/// rebound per item in `connect_bind`. Widgets are stashed on the row's
/// outer box via `ObjectExtSafe` so we don't leak glib data keys. Recycling
/// is what keeps long histories cheap — 100+ rows all share five live
/// widgets per visible slot, not per item.
pub fn create_claw_message_list() -> (gtk::ListView, gio::ListStore) {
    let store = gio::ListStore::new::<ClawRowObject>();
    let factory = gtk::SignalListItemFactory::new();

    factory.connect_setup(move |_, list_item| {
        let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
        vbox.add_css_class("claw-virtual-row");
        // `claw-row` is always present so consumers (drawer, sidebar)
        // can target "any claw row" generically. The per-variant
        // `claw-row-<kind>` class is rewritten in `connect_bind`.
        vbox.add_css_class("claw-row");

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let icon = gtk::Image::new();
        header.append(&icon);

        let avatar = adw::Avatar::new(24, None, false);
        avatar.set_visible(false);
        header.append(&avatar);

        let title = gtk::Label::new(None);
        title.add_css_class("heading");
        title.set_halign(gtk::Align::Start);
        header.append(&title);

        let pane_lbl = gtk::Label::new(None);
        pane_lbl.add_css_class("caption");
        pane_lbl.add_css_class("dim-label");
        header.append(&pane_lbl);

        vbox.append(&header);

        let viewer = StructuredViewer::new(get_claw_viewer_registry());
        vbox.append(viewer.widget());

        let cmd_label = gtk::Label::new(None);
        cmd_label.set_halign(gtk::Align::Start);
        cmd_label.set_wrap(true);
        cmd_label.set_selectable(true);
        cmd_label.add_css_class("monospace");
        cmd_label.add_css_class("dim-label");
        cmd_label.set_visible(false);
        vbox.append(&cmd_label);

        vbox.set_safe_data("icon", icon);
        vbox.set_safe_data("avatar", avatar);
        vbox.set_safe_data("title", title);
        vbox.set_safe_data("pane_lbl", pane_lbl);
        vbox.set_safe_data("viewer", viewer);
        vbox.set_safe_data("cmd_label", cmd_label);

        list_item.set_child(Some(&vbox));
    });

    factory.connect_bind(move |_, list_item| {
        let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
        let vbox = list_item.child().and_downcast::<gtk::Box>().unwrap();

        let icon = vbox.get_safe_data::<gtk::Image>("icon").unwrap();
        let avatar = vbox.get_safe_data::<adw::Avatar>("avatar").unwrap();
        let title = vbox.get_safe_data::<gtk::Label>("title").unwrap();
        let pane_lbl = vbox.get_safe_data::<gtk::Label>("pane_lbl").unwrap();
        let viewer = vbox.get_safe_data::<StructuredViewer>("viewer").unwrap();
        let cmd_label = vbox.get_safe_data::<gtk::Label>("cmd_label").unwrap();

        let Some(obj) = list_item.item().and_downcast::<ClawRowObject>() else {
            return;
        };

        // Reset per-row state that specific variants may mutate.
        icon.set_visible(true);
        avatar.set_visible(false);
        avatar.set_custom_image(None::<&gdk::Texture>);
        pane_lbl.set_visible(true);
        title.set_visible(true);
        title.remove_css_class("accent");
        icon.remove_css_class("accent");
        icon.remove_css_class("warning");
        icon.remove_css_class("error");
        vbox.remove_css_class("system-message");
        // Strip every per-variant class — rows are recycled across
        // variants, so the new bind picks exactly one below.
        for cls in [
            "claw-row-system",
            "claw-row-diagnosis",
            "claw-row-user",
            "claw-row-suggested",
            "claw-row-process-list",
            "claw-row-tool-call",
            "claw-row-command",
        ] {
            vbox.remove_css_class(cls);
        }

        let agent_or_unknown = |name: Option<String>| -> String {
            name.unwrap_or_else(|| "Unknown Agent".to_string())
        };

        let set_agent_header = |char_id: &Option<String>,
                                fallback_name: &Option<String>,
                                default_icon: &str,
                                icon_class: Option<&str>| {
            let mut name_set = false;
            if let Some(id) = char_id {
                let cache = boxxy_claw_protocol::characters::CHARACTER_CACHE.load();
                if let Some(info) = cache.iter().find(|c| &c.config.id == id) {
                    title.set_label(&info.config.display_name);
                    avatar.set_text(Some(&info.config.display_name));
                    name_set = true;

                    if info.has_avatar {
                        if let Ok(dir) = boxxy_claw_protocol::character_loader::get_characters_dir()
                        {
                            let avatar_path = dir.join(&info.config.name).join("AVATAR.png");
                            if let Ok(texture) = gdk::Texture::from_filename(&avatar_path) {
                                avatar.set_custom_image(Some(&texture));
                                avatar.set_visible(true);
                                icon.set_visible(false);
                            }
                        }
                    }
                }
            }

            if !name_set {
                title.set_label(&agent_or_unknown(fallback_name.clone()));
                avatar.set_visible(false);
                icon.set_visible(true);
            }

            if icon.is_visible() {
                icon.set_icon_name(Some(default_icon));
                if let Some(cls) = icon_class {
                    icon.add_css_class(cls);
                }
            }
        };

        match obj.get_row() {
            PersistentClawRow::SystemMessage { content, .. } => {
                icon.set_visible(false);
                avatar.set_visible(false);
                pane_lbl.set_visible(false);
                title.set_visible(false);
                vbox.add_css_class("system-message");
                vbox.add_css_class("claw-row-system");
                vbox.add_css_class("dimmed");

                viewer.set_content(&content);
                viewer.widget().set_visible(true);
                cmd_label.set_visible(false);
            }
            PersistentClawRow::Diagnosis {
                agent_name,
                character_id,
                content,
                ..
            } => {
                set_agent_header(&character_id, &agent_name, "boxxyclaw", Some("accent"));
                pane_lbl.set_visible(false);
                vbox.add_css_class("claw-row-diagnosis");

                viewer.set_content(&content);
                viewer.widget().set_visible(true);
                cmd_label.set_visible(false);
            }
            PersistentClawRow::User { content, .. } => {
                icon.set_icon_name(Some("boxxy-comic-bubble-symbolic"));
                avatar.set_visible(false);
                title.set_label("You");
                pane_lbl.set_visible(false);
                vbox.add_css_class("claw-row-user");

                viewer.set_content(&content);
                viewer.widget().set_visible(true);
                cmd_label.set_visible(false);
            }
            PersistentClawRow::Suggested {
                agent_name,
                character_id,
                diagnosis,
                command,
                ..
            } => {
                set_agent_header(
                    &character_id,
                    &agent_name,
                    "boxxy-dialog-warning-symbolic",
                    Some("warning"),
                );
                pane_lbl.set_visible(false);
                vbox.add_css_class("claw-row-suggested");

                if diagnosis.is_empty() {
                    viewer.widget().set_visible(false);
                } else {
                    viewer.set_content(&diagnosis);
                    viewer.widget().set_visible(true);
                }

                cmd_label.set_label(&command);
                cmd_label.set_visible(true);
            }
            PersistentClawRow::ProcessList {
                agent_name,
                character_id,
                result_json,
                ..
            } => {
                set_agent_header(&character_id, &agent_name, "boxxyclaw", Some("accent"));
                pane_lbl.set_visible(false);
                vbox.add_css_class("claw-row-process-list");

                viewer.clear();
                viewer.append_custom_block("list_processes", &result_json);
                viewer.widget().set_visible(true);
                cmd_label.set_visible(false);
            }
            PersistentClawRow::ToolCall {
                agent_name,
                character_id,
                tool_name,
                ..
            } => {
                set_agent_header(
                    &character_id,
                    &agent_name,
                    "boxxy-build-circle-symbolic",
                    Some("accent"),
                );
                title.set_label(&format!("Used tool: {tool_name}"));
                pane_lbl.set_visible(false);
                vbox.add_css_class("claw-row-tool-call");

                // Tool results can be huge (whole file contents); show a
                // compact single-row label instead.
                viewer.clear();
                viewer.widget().set_visible(false);
                cmd_label.set_visible(false);
            }
            PersistentClawRow::Command { command, exit_code } => {
                icon.set_icon_name(Some("utilities-terminal-symbolic"));
                avatar.set_visible(false);
                if exit_code == 0 {
                    title.set_label("Command Execution");
                } else {
                    title.set_label(&format!("Command Failed (Exit {exit_code})"));
                    icon.add_css_class("error");
                }
                pane_lbl.set_label("User");
                vbox.add_css_class("claw-row-command");

                viewer.set_content(&command);
                viewer.widget().set_visible(true);
                cmd_label.set_visible(false);
            }
        }
    });

    // Releasing viewer state on unbind stops background highlighter jobs
    // from racing with the next bind on the same recycled widget.
    factory.connect_unbind(move |_, list_item| {
        let list_item = list_item.downcast_ref::<gtk::ListItem>().unwrap();
        let vbox = list_item.child().and_downcast::<gtk::Box>().unwrap();
        if let Some(viewer) = vbox.get_safe_data::<StructuredViewer>("viewer") {
            viewer.clear();
        }
    });

    let selection = gtk::NoSelection::new(Some(store.clone()));
    let list_view = gtk::ListView::new(Some(selection), Some(factory));
    list_view.set_show_separators(false);
    list_view.add_css_class("virtual-history");

    (list_view, store)
}

// ---------------------------------------------------------------------------
// Row-append helpers
// ---------------------------------------------------------------------------
//
// The sidebar is a strictly read-only debug log. Interactive approval
// happens in the in-terminal popover. The `add_*_approval_row` helpers
// here exist so callers can log that a proposal was shown — they
// format it as a Diagnosis row and swallow the callbacks their
// signature includes for historical API compatibility.

pub fn add_system_message_row(store: &gio::ListStore, pane_id: String, content: &str) {
    store.append(&ClawRowObject::new(PersistentClawRow::SystemMessage {
        pane_id,
        content: content.to_string(),
    }));
}

pub fn add_diagnosis_row(
    store: &gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    character_id: Option<String>,
    content: &str,
) {
    store.append(&ClawRowObject::new(PersistentClawRow::Diagnosis {
        pane_id,
        agent_name,
        character_id,
        content: content.to_string(),
        usage: None,
    }));
}

pub fn add_user_row(store: &gio::ListStore, pane_id: String, content: &str) {
    store.append(&ClawRowObject::new(PersistentClawRow::User {
        pane_id,
        content: content.to_string(),
    }));
}

pub fn add_suggested_row(
    store: &gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    character_id: Option<String>,
    diagnosis: &str,
    command: &str,
) {
    store.append(&ClawRowObject::new(PersistentClawRow::Suggested {
        pane_id,
        agent_name,
        character_id,
        diagnosis: diagnosis.to_string(),
        command: command.to_string(),
        usage: None,
    }));
}

pub fn add_tool_call_row(
    store: &gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    character_id: Option<String>,
    tool_name: &str,
    result: &str,
) {
    store.append(&ClawRowObject::new(PersistentClawRow::ToolCall {
        pane_id,
        agent_name,
        character_id,
        tool_name: tool_name.to_string(),
        result: result.to_string(),
        usage: None,
    }));
}

pub fn add_process_list_row(
    store: &gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    character_id: Option<String>,
    result_json: &str,
    _on_kill_request: impl Fn(u32, String) + 'static,
) {
    store.append(&ClawRowObject::new(PersistentClawRow::ProcessList {
        pane_id,
        agent_name,
        character_id,
        result_json: result_json.to_string(),
        usage: None,
    }));
}

/// Logs a proposed shell command as a diagnosis row. The popover handles
/// approval itself; `_on_text_reply` is unused and kept only for signature
/// compatibility with `terminal::pane::claw`.
pub fn add_approval_row(
    store: &gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    character_id: Option<String>,
    command: &str,
    _on_text_reply: impl Fn(String) + 'static,
) {
    add_diagnosis_row(
        store,
        pane_id,
        agent_name,
        character_id,
        &format!("Proposed command:\n```bash\n{command}\n```"),
    );
}

pub fn add_file_write_approval_row(
    store: &gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    character_id: Option<String>,
    path: &str,
    content: &str,
    _on_reply: impl Fn(bool) + 'static,
    _on_text_reply: impl Fn(String) + 'static,
) {
    add_diagnosis_row(
        store,
        pane_id,
        agent_name,
        character_id,
        &format!("Proposed file write to `{path}`:\n```\n{content}\n```"),
    );
}

pub fn add_file_delete_approval_row(
    store: &gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    character_id: Option<String>,
    path: &str,
    _on_reply: impl Fn(bool) + 'static,
    _on_text_reply: impl Fn(String) + 'static,
) {
    add_diagnosis_row(
        store,
        pane_id,
        agent_name,
        character_id,
        &format!("Proposed file deletion: `{path}`"),
    );
}

pub fn add_kill_process_approval_row(
    store: &gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    character_id: Option<String>,
    pid: u32,
    process_name: &str,
    _on_reply: impl Fn(bool) + 'static,
    _on_text_reply: impl Fn(String) + 'static,
) {
    add_diagnosis_row(
        store,
        pane_id,
        agent_name,
        character_id,
        &format!("Proposed killing process: {process_name} (PID: {pid})"),
    );
}

pub fn add_clipboard_get_approval_row(
    store: &gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    character_id: Option<String>,
    _on_reply: impl Fn(bool) + 'static,
    _on_text_reply: impl Fn(String) + 'static,
) {
    add_diagnosis_row(
        store,
        pane_id,
        agent_name,
        character_id,
        "Proposed reading from clipboard.",
    );
}

pub fn add_clipboard_set_approval_row(
    store: &gio::ListStore,
    pane_id: String,
    agent_name: Option<String>,
    character_id: Option<String>,
    text: &str,
    _on_reply: impl Fn(bool) + 'static,
    _on_text_reply: impl Fn(String) + 'static,
) {
    add_diagnosis_row(
        store,
        pane_id,
        agent_name,
        character_id,
        &format!("Proposed writing to clipboard:\n```\n{text}\n```"),
    );
}
