use crate::ui_utils::{SubpageHeader, show_toast};
use adw::prelude::*;
use boxxy_mcp::config::{BackoffConfig, McpServerConfig, McpTransport};
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

type SaveCallback = Box<dyn Fn(McpServerConfig)>;

fn parse_key_value_pairs(raw: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in raw.split(',') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        let key = k.trim();
        if !key.is_empty() {
            map.insert(key.to_string(), v.trim().to_string());
        }
    }
    map
}

fn format_key_value_pairs(map: &HashMap<String, String>) -> String {
    let mut pairs: Vec<String> = map.iter().map(|(k, v)| format!("{k}={v}")).collect();
    pairs.sort();
    pairs.join(", ")
}

#[derive(Clone)]
pub(super) struct McpEditorPage {
    mcp_stack: gtk::Stack,
    header: SubpageHeader,
    name_entry: adw::EntryRow,
    transport_combo: adw::ComboRow,
    stdio_group: adw::PreferencesGroup,
    command_entry: adw::EntryRow,
    args_entry: adw::EntryRow,
    env_entry: adw::EntryRow,
    http_group: adw::PreferencesGroup,
    url_entry: adw::EntryRow,
    headers_entry: adw::EntryRow,
    preserved_enabled: Rc<Cell<bool>>,
    preserved_timeout_ms: Rc<Cell<u64>>,
    preserved_max_retries: Rc<Cell<u32>>,
    preserved_backoff: Rc<RefCell<BackoffConfig>>,
    on_save_slot: Rc<RefCell<Option<SaveCallback>>>,
}

impl McpEditorPage {
    #[allow(clippy::too_many_lines)]
    pub(super) fn new(
        page: &adw::PreferencesPage,
        mcp_stack: &gtk::Stack,
        header: &SubpageHeader,
    ) -> Self {
        let on_save_slot: Rc<RefCell<Option<SaveCallback>>> = Rc::new(RefCell::new(None));
        let preserved_enabled = Rc::new(Cell::new(true));
        let preserved_timeout_ms = Rc::new(Cell::new(60_000));
        let preserved_max_retries = Rc::new(Cell::new(3));
        let preserved_backoff = Rc::new(RefCell::new(BackoffConfig::default()));

        // --- 1. Server Identity & Transport ---
        let server_group = adw::PreferencesGroup::builder()
            .title("Server and Transport")
            .description("Configure a Model Context Protocol endpoint.")
            .build();

        let name_entry = adw::EntryRow::builder().title("Name").build();
        let transport_combo = adw::ComboRow::builder().title("Transport").build();
        let transport_list = gtk::StringList::new(&["Stdio (Command)", "HTTP (Streamable)"]);
        transport_combo.set_model(Some(&transport_list));
        transport_combo.set_selected(0);

        server_group.add(&name_entry);
        server_group.add(&transport_combo);
        page.add(&server_group);

        // --- 2. Stdio Configuration ---
        let stdio_group = adw::PreferencesGroup::builder()
            .title("Stdio Configuration")
            .build();
        let command_entry = adw::EntryRow::builder().title("Command").build();
        let args_entry = adw::EntryRow::builder()
            .title("Args (comma-separated)")
            .build();
        let env_entry = adw::EntryRow::builder()
            .title("Env (optional, KEY=VAL,...)")
            .build();

        stdio_group.add(&command_entry);
        stdio_group.add(&args_entry);
        stdio_group.add(&env_entry);
        page.add(&stdio_group);

        // --- 3. HTTP Configuration ---
        let http_group = adw::PreferencesGroup::builder()
            .title("HTTP Configuration")
            .visible(false)
            .build();
        let url_entry = adw::EntryRow::builder()
            .title("URL (http:// or https://)")
            .build();
        let headers_entry = adw::EntryRow::builder()
            .title("Headers (optional, KEY=VAL,...)")
            .build();

        http_group.add(&url_entry);
        http_group.add(&headers_entry);
        page.add(&http_group);

        // --- 4. Bottom Action Bar (Cancel / Save Server) ---
        let actions_group = adw::PreferencesGroup::new();
        let actions_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .halign(gtk::Align::End)
            .margin_top(6)
            .margin_bottom(12)
            .build();

        let cancel_btn = gtk::Button::builder()
            .label("Cancel")
            .css_classes(["pill"])
            .build();
        let save_btn = gtk::Button::builder()
            .label("Save Server")
            .css_classes(["suggested-action", "pill"])
            .build();
        actions_box.append(&cancel_btn);
        actions_box.append(&save_btn);
        actions_group.add(&actions_box);
        page.add(&actions_group);

        name_entry.connect_changed(|e| e.remove_css_class("error"));
        command_entry.connect_changed(|e| e.remove_css_class("error"));
        url_entry.connect_changed(|e| e.remove_css_class("error"));

        transport_combo.connect_selected_notify({
            let stdio_group = stdio_group.clone();
            let http_group = http_group.clone();
            let command_entry = command_entry.clone();
            let url_entry = url_entry.clone();
            move |combo| {
                let is_stdio = combo.selected() == 0;
                stdio_group.set_visible(is_stdio);
                http_group.set_visible(!is_stdio);
                command_entry.remove_css_class("error");
                url_entry.remove_css_class("error");
            }
        });

        let go_back = {
            let mcp_stack = mcp_stack.clone();
            let header = header.clone();
            Rc::new(move || {
                mcp_stack.set_visible_child_name("list");
                header.leave("MCP");
            })
        };

        cancel_btn.connect_clicked({
            let go_back = go_back.clone();
            move |_| {
                go_back();
            }
        });

        save_btn.connect_clicked({
            let name_entry = name_entry.clone();
            let transport_combo = transport_combo.clone();
            let command_entry = command_entry.clone();
            let args_entry = args_entry.clone();
            let env_entry = env_entry.clone();
            let url_entry = url_entry.clone();
            let headers_entry = headers_entry.clone();
            let preserved_enabled = preserved_enabled.clone();
            let preserved_timeout_ms = preserved_timeout_ms.clone();
            let preserved_max_retries = preserved_max_retries.clone();
            let preserved_backoff = preserved_backoff.clone();
            let on_save_slot = on_save_slot.clone();
            move |btn| {
                let name = name_entry.text().trim().to_string();
                let valid_name = !name.is_empty();
                if !valid_name {
                    name_entry.add_css_class("error");
                }

                let transport = if transport_combo.selected() == 0 {
                    let command = command_entry.text().trim().to_string();
                    let valid_cmd = !command.is_empty();
                    if !valid_cmd {
                        command_entry.add_css_class("error");
                    }
                    if !valid_name || !valid_cmd {
                        show_toast(btn, "Enter a Server Name and a Command to run.");
                        return;
                    }
                    let args: Vec<String> = args_entry
                        .text()
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(ToOwned::to_owned)
                        .collect();
                    let env = parse_key_value_pairs(&env_entry.text());
                    McpTransport::Stdio { command, args, env }
                } else {
                    let url = url_entry.text().trim().to_string();
                    let valid_url = url.starts_with("http://") || url.starts_with("https://");
                    if !valid_url {
                        url_entry.add_css_class("error");
                    }
                    if !valid_name || !valid_url {
                        show_toast(
                            btn,
                            "Enter a Server Name and a valid http:// or https:// URL.",
                        );
                        return;
                    }
                    let headers = parse_key_value_pairs(&headers_entry.text());
                    McpTransport::Http {
                        url,
                        headers,
                        streamable: true,
                    }
                };

                let config = McpServerConfig {
                    name,
                    transport,
                    enabled: preserved_enabled.get(),
                    timeout_ms: preserved_timeout_ms.get(),
                    max_retries: preserved_max_retries.get(),
                    backoff: preserved_backoff.borrow().clone(),
                };

                go_back();
                let cb_opt = on_save_slot.borrow_mut().take();
                if let Some(cb) = cb_opt {
                    cb(config);
                }
            }
        });

        Self {
            mcp_stack: mcp_stack.clone(),
            header: header.clone(),
            name_entry,
            transport_combo,
            stdio_group,
            command_entry,
            args_entry,
            env_entry,
            http_group,
            url_entry,
            headers_entry,
            preserved_enabled,
            preserved_timeout_ms,
            preserved_max_retries,
            preserved_backoff,
            on_save_slot,
        }
    }

    pub(super) fn open_editor(
        &self,
        existing_config: Option<&McpServerConfig>,
        on_save: impl Fn(McpServerConfig) + 'static,
    ) {
        let is_edit = existing_config.is_some();

        self.preserved_enabled
            .set(existing_config.is_none_or(|c| c.enabled));
        self.preserved_timeout_ms
            .set(existing_config.map_or(60_000, |c| c.timeout_ms));
        self.preserved_max_retries
            .set(existing_config.map_or(3, |c| c.max_retries));
        *self.preserved_backoff.borrow_mut() =
            existing_config.map_or_else(BackoffConfig::default, |c| c.backoff.clone());

        self.name_entry.remove_css_class("error");
        self.command_entry.remove_css_class("error");
        self.url_entry.remove_css_class("error");

        if let Some(cfg) = existing_config {
            self.name_entry.set_text(&cfg.name);
            match &cfg.transport {
                McpTransport::Stdio { command, args, env } => {
                    self.transport_combo.set_selected(0);
                    self.command_entry.set_text(command);
                    self.args_entry.set_text(&args.join(", "));
                    self.env_entry.set_text(&format_key_value_pairs(env));
                    self.url_entry.set_text("");
                    self.headers_entry.set_text("");
                    self.stdio_group.set_visible(true);
                    self.http_group.set_visible(false);
                }
                McpTransport::Http { url, headers, .. } => {
                    self.transport_combo.set_selected(1);
                    self.url_entry.set_text(url);
                    self.headers_entry
                        .set_text(&format_key_value_pairs(headers));
                    self.command_entry.set_text("");
                    self.args_entry.set_text("");
                    self.env_entry.set_text("");
                    self.stdio_group.set_visible(false);
                    self.http_group.set_visible(true);
                }
            }
        } else {
            self.name_entry.set_text("");
            self.transport_combo.set_selected(0);
            self.command_entry.set_text("");
            self.args_entry.set_text("");
            self.env_entry.set_text("");
            self.url_entry.set_text("");
            self.headers_entry.set_text("");
            self.stdio_group.set_visible(true);
            self.http_group.set_visible(false);
        }

        *self.on_save_slot.borrow_mut() = Some(Box::new(on_save));

        let title = if is_edit {
            "Edit MCP Server"
        } else {
            "Add MCP Server"
        };
        self.header.enter("MCP", "Back to MCP", title);
        self.mcp_stack.set_visible_child_name("editor");
        let _ = self.name_entry.grab_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_value_pairs_roundtrip_preserves_embedded_equals() {
        let parsed =
            parse_key_value_pairs(" Authorization = Bearer abc=123 , X-Env = staging , invalid ");
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed.get("Authorization").map(String::as_str),
            Some("Bearer abc=123")
        );
        assert_eq!(parsed.get("X-Env").map(String::as_str), Some("staging"));

        let formatted = format_key_value_pairs(&parsed);
        assert_eq!(formatted, "Authorization=Bearer abc=123, X-Env=staging");
    }
}
