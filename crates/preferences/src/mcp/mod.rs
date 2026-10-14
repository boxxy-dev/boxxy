use crate::config::Settings;
use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;

mod mcp_dialog;

pub fn setup_mcp_page(
    builder: &gtk::Builder,
    settings_rc: Rc<RefCell<Settings>>,
    on_change: Rc<dyn Fn(Settings) + 'static>,
) -> Box<dyn Fn(&str) -> bool> {
    let mcp_servers_group: adw::PreferencesGroup = builder.object("mcp_servers_group").unwrap();
    let mcp_add_server_row: adw::ActionRow = builder.object("mcp_add_server_row").unwrap();

    // Track added rows to allow clean removal
    let added_rows = Rc::new(RefCell::new(Vec::<adw::ActionRow>::new()));

    // Shared render function reference
    let render_server_list: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));

    let render_server_list_impl = {
        let group = mcp_servers_group.clone();
        let s_rc = settings_rc.clone();
        let cb = on_change.clone();
        let added_rows_inner = added_rows.clone();
        let render_ref = render_server_list.clone();

        Rc::new(move || {
            // Remove previously added rows
            let mut rows_vec = added_rows_inner.borrow_mut();
            for row in rows_vec.drain(..) {
                group.remove(&row);
            }

            let server_list = {
                let s = s_rc.borrow();
                s.mcp_servers.clone()
            };

            for (idx, server) in server_list.into_iter().enumerate() {
                let row = adw::ActionRow::builder()
                    .title(&server.name)
                    .subtitle(match &server.transport {
                        boxxy_mcp::config::McpTransport::Stdio { command, .. } => {
                            format!("Stdio: {}", command)
                        }
                        boxxy_mcp::config::McpTransport::Http { url, .. } => {
                            format!("HTTP: {}", url)
                        }
                    })
                    .build();

                let switch = gtk::Switch::builder()
                    .active(server.enabled)
                    .valign(gtk::Align::Center)
                    .build();

                let s_rc_inner = s_rc.clone();
                let cb_inner = cb.clone();
                let server_name = server.name.clone();
                switch.connect_active_notify(move |sw| {
                    let mut s = s_rc_inner.borrow_mut();
                    // Still use name for switch because indices might change if we don't re-render everything
                    if let Some(srv) = s.mcp_servers.iter_mut().find(|srv| srv.name == server_name)
                    {
                        if srv.enabled != sw.is_active() {
                            srv.enabled = sw.is_active();
                            s.save();
                            cb_inner(s.clone());
                        }
                    }
                });

                row.add_suffix(&switch);

                // Add Edit button
                let edit_btn = gtk::Button::builder()
                    .icon_name("document-edit-symbolic")
                    .valign(gtk::Align::Center)
                    .css_classes(["flat"])
                    .build();

                let s_rc_inner = s_rc.clone();
                let cb_inner = cb.clone();
                let server_clone = server.clone();
                let old_name = server.name.clone();
                let render_ref_inner = render_ref.clone();

                edit_btn.connect_clicked(move |btn| {
                    let s_rc_inner2 = s_rc_inner.clone();
                    let cb_inner2 = cb_inner.clone();
                    let old_name_clone = old_name.clone();
                    let render_ref_inner2 = render_ref_inner.clone();

                    let dialog = mcp_dialog::build_add_mcp_dialog(
                        Some(&server_clone),
                        move |new_config| {
                            let s_clone = {
                                let mut s = s_rc_inner2.borrow_mut();
                                if let Some(srv) = s
                                    .mcp_servers
                                    .iter_mut()
                                    .find(|srv| srv.name == old_name_clone)
                                {
                                    *srv = new_config.clone();
                                }
                                s.clone()
                            };

                            s_clone.save();
                            cb_inner2(s_clone);

                            // Re-render the whole list to keep everything in sync
                            if let Some(render) = render_ref_inner2.borrow().as_ref() {
                                render();
                            }
                        },
                    );
                    dialog.present(Some(btn));
                });

                row.add_suffix(&edit_btn);

                // Add Delete button
                let del_btn = gtk::Button::builder()
                    .icon_name("user-trash-symbolic")
                    .valign(gtk::Align::Center)
                    .css_classes(["flat", "destructive-action"])
                    .build();

                let s_rc_inner = s_rc.clone();
                let cb_inner = cb.clone();
                let render_ref_inner = render_ref.clone();
                del_btn.connect_clicked(move |_| {
                    let s_clone = {
                        let mut s = s_rc_inner.borrow_mut();
                        // Delete by index to avoid removing duplicates by name
                        if idx < s.mcp_servers.len() {
                            s.mcp_servers.remove(idx);
                        }
                        s.clone()
                    };
                    s_clone.save();
                    cb_inner(s_clone);

                    // Re-render the whole list
                    if let Some(render) = render_ref_inner.borrow().as_ref() {
                        render();
                    }
                });

                row.add_suffix(&del_btn);
                group.add(&row);
                rows_vec.push(row);
            }
        })
    };

    *render_server_list.borrow_mut() = Some(render_server_list_impl.clone());

    // Initial render
    render_server_list_impl();

    let s_rc = settings_rc.clone();
    let cb = on_change.clone();
    let render_ref = render_server_list.clone();
    mcp_add_server_row.connect_activated(move |row| {
        let s_rc_inner = s_rc.clone();
        let cb_inner = cb.clone();
        let render_ref_inner = render_ref.clone();
        let dialog = mcp_dialog::build_add_mcp_dialog(None, move |new_config| {
            let s_clone = {
                let mut s = s_rc_inner.borrow_mut();

                // Prevent duplicate names
                let mut unique_name = new_config.name.clone();
                let mut counter = 1;
                while s.mcp_servers.iter().any(|srv| srv.name == unique_name) {
                    unique_name = format!("{} ({})", new_config.name, counter);
                    counter += 1;
                }

                let mut final_config = new_config.clone();
                final_config.name = unique_name;

                s.mcp_servers.push(final_config);
                s.clone()
            };
            s_clone.save();
            cb_inner(s_clone);

            if let Some(render) = render_ref_inner.borrow().as_ref() {
                render();
            }
        });
        dialog.present(Some(row));
    });

    let mcp_add_server_row_clone = mcp_add_server_row.clone();
    let mcp_servers_group_clone = mcp_servers_group.clone();

    Box::new(move |query: &str| {
        let match_row = |r: &gtk::Widget, text: &str| {
            let m = query.is_empty() || text.to_lowercase().contains(query);
            r.set_visible(m);
            m
        };

        let mcp_visible = match_row(
            mcp_add_server_row_clone.upcast_ref(),
            "mcp servers model context protocol external tools add",
        );

        mcp_servers_group_clone.set_visible(mcp_visible);

        mcp_servers_group_clone.is_visible()
    })
}
