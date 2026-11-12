use super::{PaneInner, TerminalPaneComponent};
use crate::{PaneOutput, is_flatpak};
use boxxy_claw_protocol::{AgentStatus, ClawMessage};
use boxxy_preferences::Settings;
use gtk4::glib;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[allow(clippy::too_many_arguments)]
fn attach_process_watchers(
    inner_rc: &Rc<RefCell<PaneInner>>,
    pane_id: &str,
    exit_rx: async_channel::Receiver<i32>,
    fg_rx: async_channel::Receiver<String>,
    claw_sender: &async_channel::Sender<ClawMessage>,
    is_claw_active_cell: &Rc<Cell<bool>>,
) {
    let inner_weak_exit = Rc::downgrade(inner_rc);
    let id_for_exit = pane_id.to_string();
    let active_for_exit = is_claw_active_cell.clone();
    glib::spawn_future_local(async move {
        if let Ok(exit_code) = exit_rx.recv().await
            && let Some(inner) = inner_weak_exit.upgrade()
        {
            active_for_exit.set(false);
            let (session_opt, hold, cb) = {
                let mut inner_mut = inner.borrow_mut();
                (
                    inner_mut.mark_pty_exited(),
                    inner_mut.hold_on_exit,
                    inner_mut.callback.clone(),
                )
            };
            let Some(session_arc) = session_opt else {
                return;
            };
            let pane_id_for_rpc = id_for_exit.clone();
            glib::spawn_future_local(async move {
                let sid = session_arc.lock().await.take();
                let agent = crate::get_agent().await;
                if sid.is_some() {
                    let _ = agent.release_holder(pane_id_for_rpc).await;
                } else {
                    let _ = agent.unregister_workspace_pane(pane_id_for_rpc).await;
                }
            });
            if hold {
                cb(PaneOutput::ForegroundProcessChanged(
                    id_for_exit,
                    String::new(),
                ));
            } else {
                cb(PaneOutput::Exited(id_for_exit, exit_code));
            }
        }
    });

    let inner_weak_fg = Rc::downgrade(inner_rc);
    let claw_tx = claw_sender.clone();
    let id_for_fg = pane_id.to_string();
    let active_for_fg = is_claw_active_cell.clone();
    glib::spawn_future_local(async move {
        while let Ok(process_name) = fg_rx.recv().await {
            let Some(inner) = inner_weak_fg.upgrade() else {
                break;
            };
            if active_for_fg.get() {
                let _ = claw_tx
                    .send(ClawMessage::ForegroundProcessChanged {
                        process_name: process_name.clone(),
                    })
                    .await;
            } else {
                let status = if process_name.is_empty() {
                    "Idle".to_string()
                } else {
                    format!("Running: {process_name}")
                };
                let pane_id_for_rpc = id_for_fg.clone();
                glib::spawn_future_local(async move {
                    let agent = crate::get_agent().await;
                    let _ = agent
                        .update_workspace_pane(
                            pane_id_for_rpc,
                            String::new(),
                            String::new(),
                            String::new(),
                            status,
                        )
                        .await;
                });
            }

            let cb = {
                let mut inner_mut = inner.borrow_mut();
                let new_fg = (!process_name.is_empty()).then(|| process_name.clone());
                if new_fg.is_none() && inner_mut.foreground_process.is_some() {
                    inner_mut.last_title = None;
                }
                inner_mut.foreground_process = new_fg;
                inner_mut.callback.clone()
            };
            cb(PaneOutput::ForegroundProcessChanged(
                id_for_fg.clone(),
                process_name,
            ));
        }
    });
}

impl TerminalPaneComponent {
    pub fn spawn(&self) {
        let inner_rc = self.inner.clone();
        let id = self.id();
        let claw_sender = self.claw_sender.clone();
        let is_claw_active_cell = self.is_claw_active.clone();
        let session_status_cell = self.session_status.clone();
        let is_pinned_cell = self.is_pinned.clone();
        let is_web_search_cell = self.is_web_search.clone();
        let is_vibe_cell = self.is_vibe.clone();
        let claw_popover_reattach = self.claw_popover.clone();
        let agent_name_reattach = self.agent_name.clone();

        glib::spawn_future_local(async move {
            let (reattach_pid, command_override, layout_tag, settings, working_dir) = {
                let mut inner = inner_rc.borrow_mut();
                let settings = inner
                    .current_settings
                    .clone()
                    .unwrap_or_else(|| (*Settings::load()).clone());
                (
                    inner.reattach_pid.take(),
                    inner.command_override.take(),
                    inner.layout_tag.clone(),
                    settings,
                    inner.working_dir.clone(),
                )
            };

            let agent = crate::get_agent().await;

            // Reattach path: restore buffered output and attach existing PTY master FD.
            if let Some(pid) = reattach_pid {
                match agent.reattach(pid).await {
                    Ok((replay_bytes, master_fd)) => {
                        inner_rc.borrow_mut().pid = Some(pid);
                        let _ = agent.set_pane_id(pid, layout_tag).await;

                        // Rebind any backgrounded ClawSession associated with this detached PTY.
                        let (event_tx, session_id_ref, claw_sub_ref) = {
                            let inner = inner_rc.borrow();
                            (
                                inner.claw_event_tx.clone(),
                                inner.session_id.clone(),
                                inner.claw_subscription.clone(),
                            )
                        };
                        if let Ok(Some((sid, sub_guard))) = agent
                            .reattach_holder_by_pid_with_sender(pid, id.clone(), event_tx)
                            .await
                        {
                            *claw_sub_ref.lock() = Some(sub_guard);
                            *session_id_ref.lock().await = Some(sid);
                            is_claw_active_cell.set(true);
                            *session_status_cell.borrow_mut() = AgentStatus::Waiting;
                            {
                                let inner = inner_rc.borrow();
                                if let Some(ind) = &inner.claw_indicator {
                                    ind.set_visible(true);
                                }
                                inner.msg_bar.update_ui(
                                    AgentStatus::Waiting,
                                    is_pinned_cell.get(),
                                    is_web_search_cell.get(),
                                    is_vibe_cell.get(),
                                );
                                (inner.callback)(PaneOutput::ClawStateChanged(
                                    id.clone(),
                                    true,
                                    false,
                                ));
                            }
                            let pop = claw_popover_reattach.clone();
                            let aname = agent_name_reattach.clone();
                            glib::timeout_add_local_once(
                                std::time::Duration::from_millis(60),
                                move || {
                                    let name = aname.borrow().clone();
                                    if !name.is_empty() && !pop.is_visible() {
                                        pop.show_chat_only(&name);
                                    }
                                },
                            );
                        }

                        let has_osc_title = replay_bytes
                            .windows(4)
                            .any(|w| w == b"\x1b]0;" || w == b"\x1b]2;");

                        inner_rc
                            .borrow()
                            .terminal
                            .attach_pty_with_replay(master_fd, replay_bytes);

                        if let Ok(cwd) = agent.get_cwd(pid).await
                            && !cwd.is_empty()
                        {
                            let cb = {
                                let mut inner = inner_rc.borrow_mut();
                                inner.working_dir = Some(cwd.clone());
                                inner.msg_bar.set_working_dir(Some(cwd.clone()));
                                inner.callback.clone()
                            };
                            cb(PaneOutput::DirectoryChanged(id.clone(), cwd.clone()));

                            if !has_osc_title {
                                let pretty_cwd = if let Ok(home) = std::env::var("HOME")
                                    && !home.is_empty()
                                    && cwd.starts_with(&home)
                                {
                                    format!("~{}", &cwd[home.len()..])
                                } else {
                                    cwd.clone()
                                };
                                inner_rc.borrow_mut().last_title = Some(pretty_cwd.clone());
                                cb(PaneOutput::TitleChanged(id.clone(), pretty_cwd));
                            }
                            let _ = agent
                                .update_workspace_pane(
                                    id.clone(),
                                    cwd,
                                    String::new(),
                                    String::new(),
                                    String::new(),
                                )
                                .await;
                        } else {
                            let _ = agent
                                .update_workspace_pane(
                                    id.clone(),
                                    working_dir.unwrap_or_default(),
                                    String::new(),
                                    String::new(),
                                    String::new(),
                                )
                                .await;
                        }

                        let (exit_rx, exit_guard) = agent.subscribe_process_exit(pid).await;
                        let (fg_rx, fg_guard) = agent.subscribe_foreground_process(pid);
                        {
                            let mut inner = inner_rc.borrow_mut();
                            inner.process_subscriptions.push(exit_guard);
                            inner.process_subscriptions.push(fg_guard);
                        }

                        attach_process_watchers(
                            &inner_rc,
                            &id,
                            exit_rx,
                            fg_rx,
                            &claw_sender,
                            &is_claw_active_cell,
                        );

                        let _ = agent.set_foreground_tracking(pid, true).await;
                        return;
                    }
                    Err(e) => {
                        log::warn!(
                            "Failed to reattach to detached pid={pid}: {e:#}; spawning fresh shell"
                        );
                    }
                }
            }

            let (shell_res, pty_res) =
                tokio::join!(agent.get_preferred_shell(), agent.create_pty());

            let shell = match shell_res {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("get_preferred_shell failed: {e:#}, falling back");
                    crate::get_host_shell()
                }
            };

            let cmd = if let Some(cli_argv) = command_override.filter(|v| !v.is_empty()) {
                // If `-e` received a single shell-like string containing whitespace (e.g.
                // `-e "cargo test && echo done"`) and no literal file with that name exists,
                // evaluate it via `$SHELL -c`; otherwise execute `argv` directly.
                if cli_argv.len() == 1
                    && cli_argv[0].chars().any(char::is_whitespace)
                    && !std::path::Path::new(&cli_argv[0]).is_file()
                {
                    vec![shell, "-c".into(), cli_argv[0].clone()]
                } else {
                    cli_argv
                }
            } else if settings.use_custom_command && !settings.custom_command.trim().is_empty() {
                match glib::shell_parse_argv(&settings.custom_command) {
                    Ok(argv) if !argv.is_empty() => argv
                        .into_iter()
                        .map(|os| os.to_string_lossy().into_owned())
                        .collect(),
                    _ => vec![
                        "/bin/sh".into(),
                        "-c".into(),
                        settings.custom_command.clone(),
                    ],
                }
            } else {
                let mut c = vec![shell];
                if settings.login_shell {
                    c.push("--login".into());
                }
                c
            };

            match pty_res {
                Ok(master_fd) => {
                    // Safe FD duplication via `F_DUPFD_CLOEXEC` (replaces raw unchecked `libc::dup`).
                    let master_fd_for_agent =
                        match std::os::fd::AsFd::as_fd(&master_fd).try_clone_to_owned() {
                            Ok(cloned) => zbus::OwnedFd::from(cloned),
                            Err(e) => {
                                log::error!("Failed to clone PTY master FD: {e}");
                                return;
                            }
                        };

                    let mut env: Vec<(String, String)> = if is_flatpak() {
                        Vec::new()
                    } else {
                        std::env::vars().collect()
                    };
                    env.push(("TERM".to_string(), "xterm-256color".to_string()));
                    env.push(("COLORTERM".to_string(), "truecolor".to_string()));
                    env.push(("TERM_PROGRAM".to_string(), "Boxxy".to_string()));
                    env.push((
                        "TERM_PROGRAM_VERSION".to_string(),
                        env!("CARGO_PKG_VERSION").to_string(),
                    ));
                    env.push(("VTE_VERSION".to_string(), "7600".to_string()));

                    let (cols, rows) = {
                        let inner = inner_rc.borrow();
                        let c = if inner.n_columns > 0 {
                            inner.n_columns as u16
                        } else {
                            80
                        };
                        let r = if inner.n_rows > 0 {
                            inner.n_rows as u16
                        } else {
                            24
                        };
                        (c, r)
                    };

                    let initial_cwd = working_dir.unwrap_or_default();
                    let options = boxxy_agent::ipc::pty::SpawnOptions {
                        cwd: initial_cwd.clone(),
                        argv: cmd,
                        env,
                        cols,
                        rows,
                        pane_id: id.clone(),
                    };

                    match agent.spawn_process(master_fd_for_agent, options).await {
                        Ok(pid) => {
                            {
                                let mut inner = inner_rc.borrow_mut();
                                inner.pid = Some(pid);
                                inner.terminal.attach_pty(master_fd);
                            }

                            let effective_cwd = if !initial_cwd.is_empty() {
                                initial_cwd
                            } else {
                                agent
                                    .get_cwd(pid)
                                    .await
                                    .ok()
                                    .filter(|c| !c.is_empty())
                                    .or_else(|| std::env::var("HOME").ok())
                                    .unwrap_or_default()
                            };
                            {
                                let mut inner = inner_rc.borrow_mut();
                                if inner.working_dir.is_none() && !effective_cwd.is_empty() {
                                    inner.working_dir = Some(effective_cwd.clone());
                                    inner.msg_bar.set_working_dir(Some(effective_cwd.clone()));
                                }
                            }
                            if !effective_cwd.is_empty() {
                                let cb = inner_rc.borrow().callback.clone();
                                cb(PaneOutput::DirectoryChanged(
                                    id.clone(),
                                    effective_cwd.clone(),
                                ));
                            }
                            let _ = tokio::join!(
                                agent.set_pane_id(pid, layout_tag),
                                agent.update_workspace_pane(
                                    id.clone(),
                                    effective_cwd,
                                    String::new(),
                                    String::new(),
                                    String::new(),
                                ),
                            );

                            let (exit_rx, exit_guard) = agent.subscribe_process_exit(pid).await;
                            let (fg_rx, fg_guard) = agent.subscribe_foreground_process(pid);
                            {
                                let mut inner = inner_rc.borrow_mut();
                                inner.process_subscriptions.push(exit_guard);
                                inner.process_subscriptions.push(fg_guard);
                            }

                            attach_process_watchers(
                                &inner_rc,
                                &id,
                                exit_rx,
                                fg_rx,
                                &claw_sender,
                                &is_claw_active_cell,
                            );

                            let _ = agent.set_foreground_tracking(pid, true).await;
                        }
                        Err(e) => {
                            log::error!("Failed to spawn process via agent: {e:#}");
                            let msg = format!(
                                "\x1b[1;31mFailed to execute command:\x1b[0m {e}\r\n"
                            );
                            let mut inner = inner_rc.borrow_mut();
                            inner.hold_on_exit = true;
                            inner
                                .terminal
                                .attach_pty_with_replay(master_fd, msg.into_bytes());
                        }
                    }
                }
                Err(e) => log::error!("Failed to create PTY via agent: {e:#}"),
            }
        });
    }
}
