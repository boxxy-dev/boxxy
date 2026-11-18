use crate::config::Settings;
use crate::ui_utils::{PageHooks, SearchRow, SubpageHeader, VoidCallbackSlot};
use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;

mod mcp_dialog;

fn make_unique_mcp_name(
    servers: &[boxxy_mcp::config::McpServerConfig],
    desired: &str,
    skip_index: Option<usize>,
) -> String {
    let base = desired.trim();
    let base = if base.is_empty() { "MCP Server" } else { base };
    let mut candidate = base.to_string();
    let mut counter = 1usize;
    while servers
        .iter()
        .enumerate()
        .any(|(i, srv)| Some(i) != skip_index && srv.name == candidate)
    {
        candidate = format!("{base} ({counter})");
        counter += 1;
    }
    candidate
}

#[allow(clippy::too_many_lines)]
pub(crate) fn setup_mcp_page(builder: &gtk::Builder) -> PageHooks {
    let mcp_stack: gtk::Stack = builder.object("mcp_stack").unwrap();
    let subpage_header = SubpageHeader::from_builder(builder);
    let page_mcp_editor: adw::PreferencesPage = builder.object("page_mcp_editor").unwrap();
    let mcp_servers_group: adw::PreferencesGroup = builder.object("mcp_servers_group").unwrap();
    let mcp_add_server_row: adw::ActionRow = builder.object("mcp_add_server_row").unwrap();

    let editor_page = mcp_dialog::McpEditorPage::new(&page_mcp_editor, &mcp_stack, &subpage_header);

    let added_rows: Rc<RefCell<Vec<(String, adw::ActionRow)>>> = Rc::new(RefCell::new(Vec::new()));
    let last_servers: Rc<RefCell<Vec<boxxy_mcp::config::McpServerConfig>>> =
        Rc::new(RefCell::new(Vec::new()));
    let render_server_list: VoidCallbackSlot = Rc::new(RefCell::new(None));

    let render_server_list_impl: Rc<dyn Fn()> = {
        let group = mcp_servers_group.clone();
        let added_rows_inner = added_rows.clone();
        let last_servers_inner = last_servers.clone();
        let render_ref = render_server_list.clone();
        let editor_page = editor_page.clone();

        Rc::new(move || {
            let mut rows_vec = added_rows_inner.borrow_mut();
            for (_, row) in rows_vec.drain(..) {
                group.remove(&row);
            }

            let server_list = Settings::load().mcp_servers.clone();
            last_servers_inner.borrow_mut().clone_from(&server_list);

            for (idx, server) in server_list.into_iter().enumerate() {
                let subtitle = match &server.transport {
                    boxxy_mcp::config::McpTransport::Stdio { command, .. } => {
                        format!("Stdio: {command}")
                    }
                    boxxy_mcp::config::McpTransport::Http { url, .. } => {
                        format!("HTTP: {url}")
                    }
                };
                let haystack = format!("{} {}", server.name, subtitle).to_lowercase();

                let row = adw::ActionRow::new();
                row.set_use_markup(false);
                row.set_title(&server.name);
                row.set_subtitle(&subtitle);

                let switch = gtk::Switch::builder()
                    .active(server.enabled)
                    .valign(gtk::Align::Center)
                    .build();

                {
                    let server_name = server.name.clone();
                    let last_servers_sw = last_servers_inner.clone();
                    switch.connect_active_notify(move |sw| {
                        let active = sw.is_active();
                        let updated = Settings::update(|s| {
                            if let Some(srv) =
                                s.mcp_servers.iter_mut().find(|srv| srv.name == server_name)
                            {
                                srv.enabled = active;
                            }
                        });
                        last_servers_sw
                            .borrow_mut()
                            .clone_from(&updated.mcp_servers);
                    });
                }
                row.add_suffix(&switch);

                let edit_btn = gtk::Button::builder()
                    .icon_name("document-edit-symbolic")
                    .valign(gtk::Align::Center)
                    .tooltip_text("Edit MCP server")
                    .css_classes(["flat"])
                    .build();
                {
                    let render_ref_inner = render_ref.clone();
                    let editor_page_edit = editor_page.clone();
                    edit_btn.connect_clicked(move |_| {
                        let render_ref_inner2 = render_ref_inner.clone();
                        editor_page_edit.open_editor(Some(&server), move |mut new_config| {
                            Settings::update(|s| {
                                if idx < s.mcp_servers.len() {
                                    new_config.name = make_unique_mcp_name(
                                        &s.mcp_servers,
                                        &new_config.name,
                                        Some(idx),
                                    );
                                    s.mcp_servers[idx] = new_config.clone();
                                }
                            });
                            let render_ref_idle = render_ref_inner2.clone();
                            gtk::glib::idle_add_local_once(move || {
                                if let Some(render) = render_ref_idle.borrow().as_ref() {
                                    render();
                                }
                            });
                        });
                    });
                }
                row.add_suffix(&edit_btn);

                let del_btn = gtk::Button::builder()
                    .icon_name("user-trash-symbolic")
                    .valign(gtk::Align::Center)
                    .tooltip_text("Remove MCP server")
                    .css_classes(["flat", "destructive-action"])
                    .build();
                {
                    let render_ref_inner = render_ref.clone();
                    del_btn.connect_clicked(move |_| {
                        Settings::update(|s| {
                            if idx < s.mcp_servers.len() {
                                s.mcp_servers.remove(idx);
                            }
                        });
                        let render_ref_idle = render_ref_inner.clone();
                        gtk::glib::idle_add_local_once(move || {
                            if let Some(render) = render_ref_idle.borrow().as_ref() {
                                render();
                            }
                        });
                    });
                }
                row.add_suffix(&del_btn);

                group.add(&row);
                rows_vec.push((haystack, row));
            }
        })
    };

    *render_server_list.borrow_mut() = Some(render_server_list_impl.clone());
    render_server_list_impl();

    {
        let render_ref = render_server_list;
        let editor_page_add = editor_page;
        mcp_add_server_row.connect_activated(move |_| {
            let render_ref_inner = render_ref.clone();
            editor_page_add.open_editor(None, move |mut new_config| {
                Settings::update(|s| {
                    new_config.name = make_unique_mcp_name(&s.mcp_servers, &new_config.name, None);
                    s.mcp_servers.push(new_config.clone());
                });
                let render_ref_idle = render_ref_inner.clone();
                gtk::glib::idle_add_local_once(move || {
                    if let Some(render) = render_ref_idle.borrow().as_ref() {
                        render();
                    }
                });
            });
        });
    }

    let s_add = SearchRow::new(
        &mcp_add_server_row,
        "mcp servers model context protocol external tools add stdio http",
    );

    let sync = Box::new(move |s: &Settings| {
        if *last_servers.borrow() != s.mcp_servers {
            render_server_list_impl();
        }
    });

    let filter = Box::new(move |query: &str| {
        let mut vis = s_add.apply(query);
        for (haystack, row) in added_rows.borrow().iter() {
            let m = query.is_empty() || vis || haystack.contains(query);
            row.set_visible(m);
            if m {
                vis = true;
            }
        }
        if vis {
            mcp_add_server_row.set_visible(true);
        }
        mcp_servers_group.set_visible(vis);
        vis
    });

    (sync, filter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use boxxy_mcp::config::{BackoffConfig, McpServerConfig, McpTransport};

    fn sample_server(name: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            transport: McpTransport::Http {
                url: "https://example.com/mcp".to_string(),
                headers: std::collections::HashMap::new(),
                streamable: true,
            },
            enabled: true,
            timeout_ms: 60_000,
            max_retries: 3,
            backoff: BackoffConfig::default(),
        }
    }

    #[test]
    fn unique_mcp_name_disambiguates_duplicates_and_respects_skip_index() {
        let servers = vec![sample_server("docs"), sample_server("docs (1)")];
        assert_eq!(make_unique_mcp_name(&servers, "docs", None), "docs (2)");
        assert_eq!(make_unique_mcp_name(&servers, "docs", Some(0)), "docs");
        assert_eq!(make_unique_mcp_name(&servers, "   ", None), "MCP Server");
    }
}
