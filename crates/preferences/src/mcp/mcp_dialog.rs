use adw::prelude::*;
use boxxy_mcp::config::{McpServerConfig, McpTransport};
use gtk4 as gtk;
use libadwaita as adw;
use std::collections::HashMap;
use std::rc::Rc;

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

#[allow(clippy::too_many_lines)]
pub(super) fn build_add_mcp_dialog(
    existing_config: Option<&McpServerConfig>,
    on_save: impl Fn(McpServerConfig) + 'static,
) -> adw::AlertDialog {
    let title = if existing_config.is_some() {
        "Edit MCP Server"
    } else {
        "Add MCP Server"
    };
    let dialog = adw::AlertDialog::new(
        Some(title),
        Some("Configure a Model Context Protocol endpoint."),
    );

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("save", "Save");
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");

    let preserved_enabled = existing_config.is_none_or(|c| c.enabled);
    let preserved_timeout_ms = existing_config.map_or(60_000, |c| c.timeout_ms);
    let preserved_max_retries = existing_config.map_or(3, |c| c.max_retries);
    let preserved_backoff = existing_config
        .map(|c| c.backoff.clone())
        .unwrap_or_default();

    let vbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let name_group = adw::PreferencesGroup::new();
    let name_entry = adw::EntryRow::builder()
        .title("Name")
        .text(existing_config.map_or("My Server", |c| c.name.as_str()))
        .build();
    name_group.add(&name_entry);
    vbox.append(&name_group);

    let transport_group = adw::PreferencesGroup::new();
    let transport_combo = adw::ComboRow::builder().title("Transport").build();
    let string_list = gtk::StringList::new(&["Stdio (Command)", "HTTP (Streamable)"]);
    transport_combo.set_model(Some(&string_list));

    if let Some(config) = existing_config {
        match &config.transport {
            McpTransport::Stdio { .. } => transport_combo.set_selected(0),
            McpTransport::Http { .. } => transport_combo.set_selected(1),
        }
    }

    transport_group.add(&transport_combo);
    vbox.append(&transport_group);

    // Dynamic fields (Stdio)
    let stdio_group = adw::PreferencesGroup::new();
    let (initial_cmd, initial_args, initial_env) = if let Some(McpServerConfig {
        transport: McpTransport::Stdio { command, args, env },
        ..
    }) = existing_config
    {
        let mut pairs: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        pairs.sort();
        (command.as_str(), args.join(", "), pairs.join(", "))
    } else {
        (
            "npx",
            "-y, @modelcontextprotocol/server-everything".to_string(),
            String::new(),
        )
    };

    let command_entry = adw::EntryRow::builder()
        .title("Command")
        .text(initial_cmd)
        .build();
    let args_entry = adw::EntryRow::builder()
        .title("Args (comma-separated)")
        .text(&initial_args)
        .build();
    let env_entry = adw::EntryRow::builder()
        .title("Env (optional, KEY=VAL,...)")
        .text(&initial_env)
        .build();
    stdio_group.add(&command_entry);
    stdio_group.add(&args_entry);
    stdio_group.add(&env_entry);
    vbox.append(&stdio_group);

    // Dynamic fields (HTTP)
    let http_group = adw::PreferencesGroup::new();
    let (initial_url, initial_headers) = if let Some(McpServerConfig {
        transport: McpTransport::Http { url, headers, .. },
        ..
    }) = existing_config
    {
        let mut pairs: Vec<String> = headers.iter().map(|(k, v)| format!("{k}={v}")).collect();
        pairs.sort();
        (url.as_str(), pairs.join(", "))
    } else {
        ("http://localhost:8000/mcp", String::new())
    };

    let url_entry = adw::EntryRow::builder()
        .title("URL (http:// or https://)")
        .text(initial_url)
        .build();
    let headers_entry = adw::EntryRow::builder()
        .title("Headers (optional, KEY=VAL,...)")
        .text(&initial_headers)
        .build();
    http_group.add(&url_entry);
    http_group.add(&headers_entry);
    vbox.append(&http_group);

    let is_stdio = transport_combo.selected() == 0;
    stdio_group.set_visible(is_stdio);
    http_group.set_visible(!is_stdio);

    let validate_form: Rc<dyn Fn()> = {
        let dialog_weak = dialog.downgrade();
        let name_e = name_entry.clone();
        let combo = transport_combo.clone();
        let cmd_e = command_entry.clone();
        let url_e = url_entry.clone();
        Rc::new(move || {
            let Some(dlg) = dialog_weak.upgrade() else {
                return;
            };
            let has_name = !name_e.text().trim().is_empty();
            let transport_ok = if combo.selected() == 0 {
                !cmd_e.text().trim().is_empty()
            } else {
                let u = url_e.text();
                let u = u.trim();
                u.starts_with("http://") || u.starts_with("https://")
            };
            dlg.set_response_enabled("save", has_name && transport_ok);
        })
    };

    {
        let stdio_group = stdio_group;
        let http_group = http_group;
        let validate = validate_form.clone();
        transport_combo.connect_selected_notify(move |combo| {
            let stdio = combo.selected() == 0;
            stdio_group.set_visible(stdio);
            http_group.set_visible(!stdio);
            validate();
        });
    }
    {
        let validate = validate_form.clone();
        name_entry.connect_changed(move |_| validate());
    }
    {
        let validate = validate_form.clone();
        command_entry.connect_changed(move |_| validate());
    }
    {
        let validate = validate_form.clone();
        url_entry.connect_changed(move |_| validate());
    }
    validate_form();

    dialog.set_extra_child(Some(&vbox));

    dialog.connect_response(None, move |_d: &adw::AlertDialog, response| {
        if response != "save" {
            return;
        }
        let name = name_entry.text().trim().to_string();
        if name.is_empty() {
            return;
        }

        let transport = if transport_combo.selected() == 0 {
            let command = command_entry.text().trim().to_string();
            if command.is_empty() {
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
            if !(url.starts_with("http://") || url.starts_with("https://")) {
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
            enabled: preserved_enabled,
            timeout_ms: preserved_timeout_ms,
            max_retries: preserved_max_retries,
            backoff: preserved_backoff.clone(),
        };

        on_save(config);
    });

    dialog
}
