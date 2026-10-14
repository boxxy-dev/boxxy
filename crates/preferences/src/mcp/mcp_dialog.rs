use adw::prelude::*;
use boxxy_mcp::config::{McpServerConfig, McpTransport};
use gtk4 as gtk;
use libadwaita as adw;
use std::collections::HashMap;

pub fn build_add_mcp_dialog(
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

    // Box to hold the form
    let vbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    // Name entry
    let name_group = adw::PreferencesGroup::new();
    let name_entry = adw::EntryRow::builder()
        .title("Name")
        .text(
            existing_config
                .map(|c| c.name.as_str())
                .unwrap_or("My Server"),
        )
        .build();
    name_group.add(&name_entry);
    vbox.append(&name_group);

    // Transport combo
    let transport_group = adw::PreferencesGroup::new();
    let transport_combo = adw::ComboRow::builder().title("Transport").build();

    let string_list = gtk::StringList::new(&["Stdio (Command)", "HTTP (Streamable)"]);
    transport_combo.set_model(Some(&string_list));

    // Set initial transport selection based on existing_config
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
        let env_str = env
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join(",");
        (command.as_str(), args.join(","), env_str)
    } else {
        (
            "npx",
            "-y,@modelcontextprotocol/server-everything".to_string(),
            "".to_string(),
        )
    };

    let command_entry = adw::EntryRow::builder()
        .title("Command")
        .text(initial_cmd)
        .build();
    let args_entry = adw::EntryRow::builder()
        .title("Args (comma separated)")
        .text(&initial_args)
        .build();
    let env_entry = adw::EntryRow::builder()
        .title("Env (KEY=VAL,...)")
        .text(&initial_env)
        .build();
    stdio_group.add(&command_entry);
    stdio_group.add(&args_entry);
    stdio_group.add(&env_entry);
    vbox.append(&stdio_group);

    // Dynamic fields (HTTP)
    let http_group = adw::PreferencesGroup::new();

    let (initial_url, initial_headers) = if let Some(config) = existing_config {
        match &config.transport {
            McpTransport::Http { url, headers, .. } => {
                let headers_str = headers
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect::<Vec<_>>()
                    .join(",");
                (url.as_str(), headers_str)
            }
            _ => (
                "http://localhost:8000/sse",
                "Authorization=Bearer YOUR_KEY".to_string(),
            ),
        }
    } else {
        (
            "http://localhost:8000/sse",
            "Authorization=Bearer YOUR_KEY".to_string(),
        )
    };

    let url_entry = adw::EntryRow::builder()
        .title("URL")
        .text(initial_url)
        .build();
    let headers_entry = adw::EntryRow::builder()
        .title("Headers (KEY=VAL,...)")
        .text(&initial_headers)
        .build();
    http_group.add(&url_entry);
    http_group.add(&headers_entry);

    if transport_combo.selected() == 0 {
        stdio_group.set_visible(true);
        http_group.set_visible(false);
    } else {
        stdio_group.set_visible(false);
        http_group.set_visible(true);
    }

    vbox.append(&http_group);

    // Toggle logic
    let stdio_group_clone = stdio_group.clone();
    let http_group_clone = http_group.clone();
    transport_combo.connect_selected_notify(move |combo| {
        if combo.selected() == 0 {
            stdio_group_clone.set_visible(true);
            http_group_clone.set_visible(false);
        } else {
            stdio_group_clone.set_visible(false);
            http_group_clone.set_visible(true);
        }
    });

    dialog.set_extra_child(Some(&vbox));

    dialog.connect_response(None, move |_d: &adw::AlertDialog, response| {
        if response == "save" {
            let transport = if transport_combo.selected() == 0 {
                let args_str = args_entry.text().to_string();
                let args: Vec<String> = if args_str.is_empty() {
                    vec![]
                } else {
                    args_str.split(',').map(|s| s.trim().to_string()).collect()
                };

                let env_str = env_entry.text().to_string();
                let mut env = HashMap::new();
                if !env_str.is_empty() {
                    for pair in env_str.split(',') {
                        let mut parts = pair.splitn(2, '=');
                        if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
                            env.insert(k.trim().to_string(), v.trim().to_string());
                        }
                    }
                }

                McpTransport::Stdio {
                    command: command_entry.text().to_string(),
                    args,
                    env,
                }
            } else {
                let headers_str = headers_entry.text().to_string();
                let mut headers = HashMap::new();
                if !headers_str.is_empty() {
                    for pair in headers_str.split(',') {
                        let mut parts = pair.splitn(2, '=');
                        if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
                            headers.insert(k.trim().to_string(), v.trim().to_string());
                        }
                    }
                }

                McpTransport::Http {
                    url: url_entry.text().to_string(),
                    headers,
                    streamable: true,
                }
            };

            let config = McpServerConfig {
                name: name_entry.text().to_string(),
                transport,
                enabled: true,
                timeout_ms: 60000,
                max_retries: 3,
                backoff: Default::default(),
            };

            on_save(config);
        }
    });

    dialog
}
