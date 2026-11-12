use super::{PaneInner, TerminalPaneComponent};
use crate::PaneOutput;
use boxxy_claw_protocol::{AgentStatus, ClawEngineEvent, ClawMessage, ScheduledTask, SessionUsage};
use boxxy_claw_widget::MsgBarComponent;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

pub(super) type DeferredInnerWeak = Rc<RefCell<Option<Weak<RefCell<PaneInner>>>>>;

#[allow(clippy::too_many_arguments)]
pub(super) fn build_msg_bar(
    id: &str,
    claw_sender: &async_channel::Sender<ClawMessage>,
    event_sender_for_first_msg: async_channel::Sender<ClawEngineEvent>,
    callback: &std::sync::Arc<dyn Fn(PaneOutput) + Send + Sync + 'static>,
    inner_weak_ref: &DeferredInnerWeak,
    pending_char_for_msgbar: Rc<RefCell<String>>,
    is_claw_active: &Rc<Cell<bool>>,
    session_status: &Rc<RefCell<AgentStatus>>,
    is_pinned: &Rc<Cell<bool>>,
    is_web_search: &Rc<Cell<bool>>,
    is_vibe: &Rc<Cell<bool>>,
    agent_name: &Rc<RefCell<String>>,
    session_usage: &Rc<Cell<SessionUsage>>,
    pending_tasks: &Rc<RefCell<Vec<ScheduledTask>>>,
) -> Rc<MsgBarComponent> {
    let tx_msg = claw_sender.clone();
    let tx_claw_toggle = claw_sender.clone();
    let tx_sleep_toggle = claw_sender.clone();
    let tx_pin_toggle = claw_sender.clone();
    let tx_web_search_toggle = claw_sender.clone();
    let tx_vibe_toggle = claw_sender.clone();

    let cb_msg = callback.clone();
    let id_msg = id.to_string();
    let cb_toggle = callback.clone();
    let id_toggle = id.to_string();
    let cb_sleep = callback.clone();
    let id_sleep = id.to_string();
    let cb_pin = callback.clone();
    let id_pin = id.to_string();
    let cb_web_search = callback.clone();
    let id_web_search = id.to_string();
    let cb_vibe = callback.clone();
    let id_vibe = id.to_string();

    let inner_weak_for_msg = inner_weak_ref.clone();
    let inner_weak_for_cancel = inner_weak_ref.clone();
    let inner_weak_for_active = inner_weak_ref.clone();
    let inner_weak_for_sleep = inner_weak_ref.clone();
    let inner_weak_for_pin = inner_weak_ref.clone();
    let inner_weak_for_web_search = inner_weak_ref.clone();
    let inner_weak_for_vibe = inner_weak_ref.clone();

    let is_claw_active_for_msg = is_claw_active.clone();
    let session_status_for_msg = session_status.clone();
    let is_pinned_for_msg = is_pinned.clone();
    let is_web_search_for_msg = is_web_search.clone();
    let is_vibe_for_msg = is_vibe.clone();

    let is_claw_active_for_active = is_claw_active.clone();
    let session_status_for_active = session_status.clone();
    let is_pinned_for_active = is_pinned.clone();
    let is_web_search_for_active = is_web_search.clone();
    let is_vibe_for_active = is_vibe.clone();
    let agent_name_for_active = agent_name.clone();
    let session_usage_for_active = session_usage.clone();
    let pending_tasks_for_active = pending_tasks.clone();

    let is_claw_active_for_sleep = is_claw_active.clone();
    let session_status_for_sleep = session_status.clone();
    let is_pinned_for_sleep = is_pinned.clone();
    let is_web_search_for_sleep = is_web_search.clone();
    let is_vibe_for_sleep = is_vibe.clone();

    let session_status_for_pin = session_status.clone();
    let is_pinned_for_pin = is_pinned.clone();
    let is_web_search_for_pin = is_web_search.clone();
    let is_vibe_for_pin = is_vibe.clone();

    let session_status_for_web_search = session_status.clone();
    let is_pinned_for_web_search = is_pinned.clone();
    let is_web_search_for_web_search = is_web_search.clone();
    let is_vibe_for_web_search = is_vibe.clone();

    let session_status_for_vibe = session_status.clone();
    let is_pinned_for_vibe = is_pinned.clone();
    let is_web_search_for_vibe = is_web_search.clone();
    let is_vibe_for_vibe = is_vibe.clone();

    let pending_char_for_toggle = pending_char_for_msgbar.clone();
    let event_sender_for_toggle = event_sender_for_first_msg.clone();

    Rc::new(MsgBarComponent::new(
        move |(query, images)| {
            let Some(inner_arc) = inner_weak_for_msg
                .borrow()
                .as_ref()
                .and_then(Weak::upgrade)
            else {
                return;
            };

            let pane = inner_arc.borrow().terminal.clone();
            let trimmed_query = query.trim();
            let incognito_inline_opt = if trimmed_query.eq_ignore_ascii_case("/incognito") {
                Some(String::new())
            } else if trimmed_query
                .get(..11)
                .is_some_and(|p| p.eq_ignore_ascii_case("/incognito "))
            {
                Some(trimmed_query[11..].trim().to_string())
            } else {
                None
            };
            let is_resume_cmd = query
                .strip_prefix("/resume ")
                .map(str::trim)
                .is_some_and(|s| !s.is_empty());
            if is_resume_cmd || incognito_inline_opt.as_ref().is_some_and(String::is_empty) {
                inner_arc.borrow().msg_bar.entry.grab_focus();
            }

            if !is_claw_active_for_msg.get() {
                is_claw_active_for_msg.set(true);
                if *session_status_for_msg.borrow() == AgentStatus::Off {
                    *session_status_for_msg.borrow_mut() = AgentStatus::Waiting;
                }
                if let Some(ind) = &inner_arc.borrow().claw_indicator {
                    ind.set_visible(true);
                }
                inner_arc.borrow().msg_bar.update_ui(
                    session_status_for_msg.borrow().clone(),
                    is_pinned_for_msg.get(),
                    is_web_search_for_msg.get(),
                    is_vibe_for_msg.get(),
                );
                cb_msg(PaneOutput::ClawStateChanged(
                    id_msg.clone(),
                    true,
                    matches!(*session_status_for_msg.borrow(), AgentStatus::Sleep),
                ));
            }

            let pending_rc = pending_char_for_msgbar.clone();
            let pane_id = id_msg.clone();
            let (session_id_ref, claw_sub_ref, cwd) = {
                let b = inner_arc.borrow();
                (
                    b.session_id.clone(),
                    b.claw_subscription.clone(),
                    b.working_dir.clone().unwrap_or_default(),
                )
            };
            let event_tx = event_sender_for_first_msg.clone();
            let tx = tx_msg.clone();
            let initial_vibe = is_vibe_for_msg.get();
            let initial_web = is_web_search_for_msg.get();

            glib::spawn_future_local(async move {
                let snapshot = if incognito_inline_opt.is_some() {
                    String::new()
                } else {
                    pane.get_text_snapshot(100, 0).await.unwrap_or_default()
                };
                let has_session = session_id_ref.lock().await.is_some();

                if let Some(inline_incognito_query) = incognito_inline_opt {
                    if has_session {
                        return;
                    }
                    let agent = crate::get_agent().await;
                    let char_id =
                        boxxy_claw_protocol::characters::INCOGNITO_CHARACTER_ID.to_string();
                    *pending_rc.borrow_mut() = char_id.clone();
                    if let Ok((session_id, sub_guard)) = agent
                        .create_claw_session_with_sender(pane_id, char_id, event_tx)
                        .await
                        && !session_id.is_empty()
                    {
                        *claw_sub_ref.lock() = Some(sub_guard);
                        *session_id_ref.lock().await = Some(session_id.clone());
                        let _ = agent
                            .post_claw_message(session_id.clone(), ClawMessage::Initialize)
                            .await;
                        if !inline_incognito_query.is_empty() {
                            let _ = agent
                                .post_claw_message(
                                    session_id,
                                    ClawMessage::ClawQuery {
                                        query: inline_incognito_query,
                                        snapshot: String::new(),
                                        cwd: String::new(),
                                        image_attachments: images,
                                    },
                                )
                                .await;
                        }
                    }
                    return;
                }

                let resume_id_opt = query
                    .strip_prefix("/resume ")
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);

                if let Some(ref resume_id) = resume_id_opt
                    && reattach_or_focus_existing_claim(resume_id, &pane_id).await
                {
                    return;
                }

                if !has_session {
                    let agent = crate::get_agent().await;
                    let resolved_resume_char_id = if let Some(ref resume_id) = resume_id_opt {
                        boxxy_claw_widget::msgbar::autocomplete::resolve_session_character_id(
                            resume_id,
                        )
                        .await
                    } else {
                        None
                    };

                    let char_id = if let Some(resume_char_id) = resolved_resume_char_id {
                        *pending_rc.borrow_mut() = resume_char_id.clone();
                        resume_char_id
                    } else {
                        let pending = pending_rc.borrow().clone();
                        if pending.is_empty() {
                            boxxy_claw_protocol::characters::CHARACTER_CACHE
                                .load()
                                .first()
                                .map(|c| c.config.id.clone())
                                .unwrap_or_default()
                        } else {
                            pending
                        }
                    };

                    if let Ok((session_id, sub_guard)) = agent
                        .create_claw_session_with_sender(pane_id, char_id, event_tx.clone())
                        .await
                        && !session_id.is_empty()
                    {
                        if let Some(resume_id) = resume_id_opt {
                            let resume_guard =
                                agent.subscribe_claw_session(resume_id.clone(), event_tx);
                            *claw_sub_ref.lock() = Some(resume_guard);
                            *session_id_ref.lock().await = Some(resume_id.clone());
                            let _ = agent
                                .post_claw_message(
                                    session_id,
                                    ClawMessage::ResumeSession {
                                        session_id: resume_id,
                                        character_id: None,
                                    },
                                )
                                .await;
                        } else {
                            *claw_sub_ref.lock() = Some(sub_guard);
                            *session_id_ref.lock().await = Some(session_id.clone());
                            if initial_vibe {
                                let _ = agent
                                    .post_claw_message(
                                        session_id.clone(),
                                        ClawMessage::ToggleVibe(true),
                                    )
                                    .await;
                            }
                            let _ = agent
                                .post_claw_message(
                                    session_id.clone(),
                                    ClawMessage::ToggleWebSearch(initial_web),
                                )
                                .await;
                            let _ = agent
                                .post_claw_message(session_id.clone(), ClawMessage::Initialize)
                                .await;
                            let _ = agent
                                .post_claw_message(
                                    session_id,
                                    ClawMessage::ClawQuery {
                                        query,
                                        snapshot,
                                        cwd,
                                        image_attachments: images,
                                    },
                                )
                                .await;
                        }
                    }
                    return;
                }

                if let Some(resume_id) = resume_id_opt {
                    let agent = crate::get_agent().await;
                    let resume_guard = agent.subscribe_claw_session(resume_id.clone(), event_tx);
                    *claw_sub_ref.lock() = Some(resume_guard);
                    let _ = tx
                        .send(ClawMessage::ResumeSession {
                            session_id: resume_id.clone(),
                            character_id: None,
                        })
                        .await;
                    *session_id_ref.lock().await = Some(resume_id);
                    return;
                }

                let _ = tx
                    .send(ClawMessage::ClawQuery {
                        query,
                        snapshot,
                        cwd,
                        image_attachments: images,
                    })
                    .await;
            });
        },
        move || {
            if let Some(inner_arc) = inner_weak_for_cancel
                .borrow()
                .as_ref()
                .and_then(Weak::upgrade)
            {
                let pane = inner_arc.borrow().terminal.clone();
                pane.set_focusable(true);
                pane.grab_focus();
            }
        },
        move |active| {
            if is_claw_active_for_active.get() != active || !active {
                is_claw_active_for_active.set(active);
                let mut session_refs = None;
                if let Some(inner_arc) = inner_weak_for_active
                    .borrow()
                    .as_ref()
                    .and_then(Weak::upgrade)
                {
                    let mut status = session_status_for_active.borrow().clone();
                    if active && status == AgentStatus::Off {
                        status = AgentStatus::Waiting;
                        *session_status_for_active.borrow_mut() = status.clone();
                    } else if !active {
                        status = AgentStatus::Off;
                        *session_status_for_active.borrow_mut() = status.clone();
                        is_pinned_for_active.set(false);
                        is_vibe_for_active.set(false);
                        *agent_name_for_active.borrow_mut() = String::new();
                        *pending_char_for_toggle.borrow_mut() = String::new();
                        session_usage_for_active.set(SessionUsage::default());
                        pending_tasks_for_active.borrow_mut().clear();
                        inner_arc.borrow().msg_bar.clear_character();
                        if let Some(popover) = &inner_arc.borrow().claw_popover {
                            popover.reset_for_exit();
                        }
                        let pane = inner_arc.borrow().terminal.clone();
                        pane.set_focusable(true);
                        pane.grab_focus();
                    }

                    inner_arc.borrow().msg_bar.update_ui(
                        status,
                        is_pinned_for_active.get(),
                        is_web_search_for_active.get(),
                        is_vibe_for_active.get(),
                    );
                    if let Some(ind) = &inner_arc.borrow().claw_indicator {
                        if !active {
                            ind.set_identity("", "");
                            ind.set_has_tasks(false);
                            ind.set_vibe(false);
                            ind.set_mode(AgentStatus::Off);
                            ind.hide();
                        }
                        ind.set_visible(active);
                    }
                    session_refs = Some((
                        inner_arc.borrow().session_id.clone(),
                        inner_arc.borrow().claw_subscription.clone(),
                    ));
                }
                cb_toggle(PaneOutput::ClawStateChanged(
                    id_toggle.clone(),
                    active,
                    matches!(*session_status_for_active.borrow(), AgentStatus::Sleep),
                ));
                let tx = tx_claw_toggle.clone();
                let pane_id = id_toggle.clone();
                if active {
                    let pending = pending_char_for_toggle.borrow().clone();
                    let event_tx = event_sender_for_toggle.clone();
                    glib::spawn_future_local(async move {
                        if let Some((session_id_ref, claw_sub_ref)) = session_refs
                            && session_id_ref.lock().await.is_none()
                        {
                            let agent = crate::get_agent().await;
                            let char_id = if pending.is_empty() {
                                boxxy_claw_protocol::characters::CHARACTER_CACHE
                                    .load()
                                    .first()
                                    .map(|c| c.config.id.clone())
                                    .unwrap_or_default()
                            } else {
                                pending
                            };
                            if let Ok((sid, sub_guard)) = agent
                                .create_claw_session_with_sender(pane_id, char_id, event_tx)
                                .await
                                && !sid.is_empty()
                            {
                                *claw_sub_ref.lock() = Some(sub_guard);
                                *session_id_ref.lock().await = Some(sid.clone());
                                let _ = agent
                                    .post_claw_message(sid, ClawMessage::Initialize)
                                    .await;
                                return;
                            }
                        }
                        let _ = tx.send(ClawMessage::Initialize).await;
                    });
                } else {
                    if let Some((_, ref claw_sub_ref)) = session_refs {
                        *claw_sub_ref.lock() = None;
                    }
                    let cb_after_exit = cb_toggle.clone();
                    glib::spawn_future_local(async move {
                        if let Some((session_id_ref, _)) = session_refs {
                            let _ = session_id_ref.lock().await.take();
                        }
                        let agent = crate::get_agent().await;
                        let _ = agent.release_holder(pane_id.clone()).await;
                        boxxy_claw_widget::msgbar::autocomplete::refresh_resume_sessions_async(
                            None,
                        );
                        cb_after_exit(PaneOutput::ClawStateChanged(pane_id, false, false));
                    });
                }
            }
        },
        move |sleep| {
            let currently_sleeping =
                matches!(*session_status_for_sleep.borrow(), AgentStatus::Sleep);
            if currently_sleeping != sleep {
                let new_status = if sleep {
                    AgentStatus::Sleep
                } else {
                    AgentStatus::Waiting
                };
                *session_status_for_sleep.borrow_mut() = new_status.clone();

                if let Some(inner_arc) = inner_weak_for_sleep
                    .borrow()
                    .as_ref()
                    .and_then(Weak::upgrade)
                {
                    inner_arc.borrow().msg_bar.update_ui(
                        new_status,
                        is_pinned_for_sleep.get(),
                        is_web_search_for_sleep.get(),
                        is_vibe_for_sleep.get(),
                    );
                }
                cb_sleep(PaneOutput::ClawStateChanged(
                    id_sleep.clone(),
                    is_claw_active_for_sleep.get(),
                    sleep,
                ));
                let tx = tx_sleep_toggle.clone();
                glib::spawn_future_local(async move {
                    let _ = tx.send(ClawMessage::SleepToggle(sleep)).await;
                });
            }
        },
        move |pin| {
            if is_pinned_for_pin.get() != pin {
                is_pinned_for_pin.set(pin);
                if let Some(inner_arc) = inner_weak_for_pin
                    .borrow()
                    .as_ref()
                    .and_then(Weak::upgrade)
                {
                    inner_arc.borrow().msg_bar.update_ui(
                        session_status_for_pin.borrow().clone(),
                        pin,
                        is_web_search_for_pin.get(),
                        is_vibe_for_pin.get(),
                    );
                }
                cb_pin(PaneOutput::ClawEvent(
                    id_pin.clone(),
                    ClawEngineEvent::PinStatusChanged(pin),
                ));
                let tx = tx_pin_toggle.clone();
                glib::spawn_future_local(async move {
                    let _ = tx.send(ClawMessage::TogglePin(pin)).await;
                });
            }
        },
        move |web| {
            if is_web_search_for_web_search.get() != web {
                is_web_search_for_web_search.set(web);
                if let Some(inner_arc) = inner_weak_for_web_search
                    .borrow()
                    .as_ref()
                    .and_then(Weak::upgrade)
                {
                    inner_arc.borrow().msg_bar.update_ui(
                        session_status_for_web_search.borrow().clone(),
                        is_pinned_for_web_search.get(),
                        web,
                        is_vibe_for_web_search.get(),
                    );
                }
                cb_web_search(PaneOutput::ClawEvent(
                    id_web_search.clone(),
                    ClawEngineEvent::WebSearchStatusChanged(web),
                ));
                let tx = tx_web_search_toggle.clone();
                glib::spawn_future_local(async move {
                    let _ = tx.send(ClawMessage::ToggleWebSearch(web)).await;
                });
            }
        },
        move |vibe| {
            if is_vibe_for_vibe.get() == vibe {
                return;
            }
            let Some(inner_arc) = inner_weak_for_vibe
                .borrow()
                .as_ref()
                .and_then(Weak::upgrade)
            else {
                return;
            };

            if !vibe {
                set_pane_vibe_enabled(
                    &inner_arc,
                    &is_vibe_for_vibe,
                    &session_status_for_vibe,
                    &is_pinned_for_vibe,
                    &is_web_search_for_vibe,
                    &cb_vibe,
                    &id_vibe,
                    false,
                );
                let tx = tx_vibe_toggle.clone();
                glib::spawn_future_local(async move {
                    let _ = tx.send(ClawMessage::ToggleVibe(false)).await;
                });
                return;
            }

            let cwd = inner_arc.borrow().working_dir.clone().unwrap_or_default();
            let current_settings = boxxy_preferences::Settings::load();

            if current_settings.is_dir_vibe_trusted(&cwd) {
                set_pane_vibe_enabled(
                    &inner_arc,
                    &is_vibe_for_vibe,
                    &session_status_for_vibe,
                    &is_pinned_for_vibe,
                    &is_web_search_for_vibe,
                    &cb_vibe,
                    &id_vibe,
                    true,
                );
                let tx = tx_vibe_toggle.clone();
                glib::spawn_future_local(async move {
                    let _ = tx.send(ClawMessage::ToggleVibe(true)).await;
                });
                return;
            }

            present_vibe_trust_dialog(
                &inner_arc,
                &cwd,
                &is_vibe_for_vibe,
                &session_status_for_vibe,
                &is_pinned_for_vibe,
                &is_web_search_for_vibe,
                &cb_vibe,
                &id_vibe,
                &tx_vibe_toggle,
            );
        },
    ))
}

#[allow(clippy::too_many_arguments)]
fn set_pane_vibe_enabled(
    inner_arc: &Rc<RefCell<PaneInner>>,
    is_vibe: &Rc<Cell<bool>>,
    session_status: &Rc<RefCell<AgentStatus>>,
    is_pinned: &Rc<Cell<bool>>,
    is_web_search: &Rc<Cell<bool>>,
    cb: &std::sync::Arc<dyn Fn(PaneOutput) + Send + Sync + 'static>,
    pane_id: &str,
    enabled: bool,
) {
    is_vibe.set(enabled);
    {
        let inner_b = inner_arc.borrow();
        inner_b.msg_bar.update_ui(
            session_status.borrow().clone(),
            is_pinned.get(),
            is_web_search.get(),
            enabled,
        );
        if let Some(ind) = &inner_b.claw_indicator {
            ind.set_vibe(enabled);
        }
        if let Some(pop) = &inner_b.claw_popover {
            pop.refresh_character_selector("");
        }
    }
    cb(PaneOutput::ClawEvent(
        pane_id.to_string(),
        ClawEngineEvent::VibeStatusChanged(enabled),
    ));
}

#[allow(clippy::too_many_arguments)]
fn present_vibe_trust_dialog(
    inner_arc: &Rc<RefCell<PaneInner>>,
    cwd: &str,
    is_vibe: &Rc<Cell<bool>>,
    session_status: &Rc<RefCell<AgentStatus>>,
    is_pinned: &Rc<Cell<bool>>,
    is_web_search: &Rc<Cell<bool>>,
    cb: &std::sync::Arc<dyn Fn(PaneOutput) + Send + Sync + 'static>,
    pane_id: &str,
    tx_vibe_toggle: &async_channel::Sender<ClawMessage>,
) {
    use libadwaita::prelude::AlertDialogExt;

    let parent_widget = inner_arc.borrow().terminal.clone();
    match boxxy_preferences::config::validate_vibe_trusted_dir(cwd) {
        Err(reason) => {
            let dialog = libadwaita::AlertDialog::builder()
                .heading("Cannot Enable Vibe Mode Here")
                .body(format!(
                    "{reason}\n\nNavigate to a project folder (for example, cd ~/Projects or cd ~/test) or manage trusted folders in Preferences → Toolbox & Permissions."
                ))
                .default_response("ok")
                .close_response("ok")
                .build();
            dialog.add_response("ok", "OK");
            libadwaita::prelude::AdwDialogExt::present(&dialog, Some(&parent_widget));
        }
        Ok(valid_dir) => {
            let dialog = libadwaita::AlertDialog::builder()
                .heading("Trust Folder for Vibe Mode?")
                .body(format!(
                    "Allow characters in Vibe Mode to automatically run safe commands and edit files inside:\n\n{valid_dir}\n\n(including its subfolders) without asking each time? You can manage trusted folders anytime in Preferences → Toolbox & Permissions."
                ))
                .default_response("trust")
                .close_response("cancel")
                .build();
            dialog.add_response("cancel", "Cancel");
            dialog.add_response("trust", "Trust & Enable");
            dialog.set_response_appearance("trust", libadwaita::ResponseAppearance::Suggested);

            let inner_for_resp = Rc::downgrade(inner_arc);
            let is_vibe_resp = is_vibe.clone();
            let status_resp = session_status.clone();
            let pin_resp = is_pinned.clone();
            let web_resp = is_web_search.clone();
            let cb_resp = cb.clone();
            let id_resp = pane_id.to_string();
            let tx_resp = tx_vibe_toggle.clone();

            dialog.connect_response(None, move |_, response| {
                if response == "trust"
                    && let Some(inner_rc) = inner_for_resp.upgrade()
                {
                    let dir_to_save = valid_dir.clone();
                    boxxy_preferences::Settings::update(move |s| {
                        if !s.vibe_trusted_dirs.contains(&dir_to_save) {
                            s.vibe_trusted_dirs.push(dir_to_save.clone());
                        }
                    });

                    set_pane_vibe_enabled(
                        &inner_rc,
                        &is_vibe_resp,
                        &status_resp,
                        &pin_resp,
                        &web_resp,
                        &cb_resp,
                        &id_resp,
                        true,
                    );

                    let tx = tx_resp.clone();
                    glib::spawn_future_local(async move {
                        let agent = crate::get_agent().await;
                        let _ = agent.notify_settings_invalidated().await;
                        let _ = tx.send(ClawMessage::ToggleVibe(true)).await;
                    });
                }
            });

            libadwaita::prelude::AdwDialogExt::present(&dialog, Some(&parent_widget));
        }
    }
}

async fn reattach_or_focus_existing_claim(resume_id: &str, current_pane_id: &str) -> bool {
    let existing_claim = boxxy_claw_protocol::characters::CLAIMS_CACHE
        .load()
        .iter()
        .find(|c| c.session_id == resume_id && c.holder_id != current_pane_id)
        .cloned();
    let Some(claim) = existing_claim else {
        return false;
    };

    let agent = crate::get_agent().await;
    if claim.owner_bus_name.is_empty() {
        if let Ok(detached) = agent.list_detached_sessions_detailed().await
            && let Some(s) = detached.into_iter().find(|s| {
                s.session_id == resume_id
                    || s.character_id == claim.character_id
                    || s.pane_id == claim.holder_id
            })
        {
            let _ = crate::TERMINAL_EVENT_BUS.send(crate::TerminalEvent {
                id: String::new(),
                kind: crate::TerminalEventKind::RestoreDetached(s.pid, s.cwd),
            });
            return true;
        }
        false
    } else {
        let _ = agent.request_focus_pane(claim.holder_id).await;
        true
    }
}

impl TerminalPaneComponent {
    pub fn set_claw_active(&self, active: bool) {
        if self.is_claw_active.get() == active {
            return;
        }

        self.is_claw_active.set(active);

        if let Some(pid) = self.inner.borrow().pid {
            glib::spawn_future_local(async move {
                let agent = crate::get_agent().await;
                let _ = agent.set_foreground_tracking(pid, true).await;
            });
        }

        self.claw_indicator.set_visible(active);

        let status = if active {
            AgentStatus::Waiting
        } else {
            AgentStatus::Off
        };
        *self.session_status.borrow_mut() = status.clone();

        self.msg_bar.update_ui(
            status,
            self.is_pinned.get(),
            self.is_web_search.get(),
            self.is_vibe.get(),
        );

        let tx = self.claw_sender.clone();
        if active {
            glib::spawn_future_local(async move {
                let _ = tx.send(ClawMessage::Initialize).await;
            });
        } else {
            glib::spawn_future_local(async move {
                let _ = tx.send(ClawMessage::Deactivate).await;
            });
        }
    }

    /// Starts the Claw overlay with the default character and submits `query`,
    /// provided no Claw session is already active on this pane.
    pub fn start_claw_query_with_default_character(&self, query: String) -> bool {
        if self.is_claw_active.get() {
            return false;
        }

        let (pane_id, session_id_ref, claw_sub_ref, event_tx, cwd, cb) = {
            let inner = self.inner.borrow();
            (
                inner.id.clone(),
                inner.session_id.clone(),
                inner.claw_subscription.clone(),
                inner.claw_event_tx.clone(),
                inner.working_dir.clone().unwrap_or_default(),
                inner.callback.clone(),
            )
        };

        let Some(chosen) =
            boxxy_claw_protocol::characters::find_first_available_character(Some(&pane_id))
        else {
            return false;
        };
        let char_id = chosen.config.id.clone();
        let display_name = if chosen.config.display_name.is_empty() {
            "Agent".to_string()
        } else {
            chosen.config.display_name.clone()
        };

        self.is_claw_active.set(true);
        *self.session_status.borrow_mut() = AgentStatus::Waiting;
        self.claw_indicator.set_visible(true);
        self.msg_bar.update_ui(
            AgentStatus::Waiting,
            self.is_pinned.get(),
            self.is_web_search.get(),
            self.is_vibe.get(),
        );
        cb(PaneOutput::ClawStateChanged(pane_id.clone(), true, false));

        let terminal = self.terminal();
        terminal.set_focusable(false);
        self.claw_popover.show_chat_only(&display_name);
        self.claw_popover.enter_waiting_state();
        self.claw_popover.grab_input_focus();

        let tx = self.claw_sender.clone();
        glib::spawn_future_local(async move {
            let snapshot = terminal.get_text_snapshot(100, 0).await.unwrap_or_default();
            let has_session = session_id_ref.lock().await.is_some();

            if !has_session {
                let agent = crate::get_agent().await;
                if let Ok((session_id, sub_guard)) = agent
                    .create_claw_session_with_sender(pane_id, char_id, event_tx)
                    .await
                    && !session_id.is_empty()
                {
                    *claw_sub_ref.lock() = Some(sub_guard);
                    *session_id_ref.lock().await = Some(session_id.clone());
                    let _ = agent
                        .post_claw_message(session_id.clone(), ClawMessage::Initialize)
                        .await;
                    let _ = agent
                        .post_claw_message(
                            session_id,
                            ClawMessage::ClawQuery {
                                query,
                                snapshot,
                                cwd,
                                image_attachments: vec![],
                            },
                        )
                        .await;
                }
            } else {
                let _ = tx
                    .send(ClawMessage::ClawQuery {
                        query,
                        snapshot,
                        cwd,
                        image_attachments: vec![],
                    })
                    .await;
            }
        });

        true
    }

    pub fn resume_session(&self, resume_id: &str, override_character_id: Option<String>) {
        let resume_id = resume_id.trim().to_string();
        if resume_id.is_empty() {
            return;
        }

        let (pane_id, session_id_ref, claw_sub_ref, event_tx) = {
            let inner = self.inner.borrow();
            (
                inner.id.clone(),
                inner.session_id.clone(),
                inner.claw_subscription.clone(),
                inner.claw_event_tx.clone(),
            )
        };

        // Check if `resume_id` is already an active background or open session
        // BEFORE mutating this pane's Claw state so reattaching/focusing doesn't
        // leave the current empty pane in a half-activated state.
        let already_claimed = boxxy_claw_protocol::characters::CLAIMS_CACHE
            .load()
            .iter()
            .any(|c| c.session_id == resume_id && c.holder_id != pane_id);
        if already_claimed {
            glib::spawn_future_local(async move {
                let _ = reattach_or_focus_existing_claim(&resume_id, &pane_id).await;
            });
            return;
        }

        if !self.is_claw_active.get() {
            self.is_claw_active.set(true);
            let mut status = self.session_status.borrow().clone();
            if status == AgentStatus::Off {
                status = AgentStatus::Waiting;
                *self.session_status.borrow_mut() = status.clone();
            }
            self.claw_indicator.set_visible(true);
            self.msg_bar.update_ui(
                status.clone(),
                self.is_pinned.get(),
                self.is_web_search.get(),
                self.is_vibe.get(),
            );
            let (cb, pane_id) = {
                let inner = self.inner.borrow();
                (inner.callback.clone(), inner.id.clone())
            };
            cb(PaneOutput::ClawStateChanged(
                pane_id,
                true,
                matches!(status, AgentStatus::Sleep),
            ));
        }

        let tx = self.claw_sender.clone();
        let claw_popover = self.claw_popover.clone();
        let agent_name_rc = self.agent_name.clone();
        let msg_bar = self.msg_bar.clone();
        let terminal = self.terminal();

        glib::spawn_future_local(async move {
            let has_session = session_id_ref.lock().await.is_some();
            if !has_session {
                let agent = crate::get_agent().await;
                let char_id = if let Some(cid) =
                    override_character_id.as_ref().filter(|s| !s.is_empty())
                {
                    cid.clone()
                } else {
                    boxxy_claw_widget::msgbar::autocomplete::resolve_session_character_id(
                        &resume_id,
                    )
                    .await
                    .unwrap_or_else(|| {
                        boxxy_claw_protocol::characters::CHARACTER_CACHE
                            .load()
                            .first()
                            .map(|c| c.config.id.clone())
                            .unwrap_or_default()
                    })
                };

                if let Ok((session_id, _sub_guard)) = agent
                    .create_claw_session_with_sender(pane_id, char_id, event_tx.clone())
                    .await
                    && !session_id.is_empty()
                {
                    let resume_guard = agent.subscribe_claw_session(resume_id.clone(), event_tx);
                    *claw_sub_ref.lock() = Some(resume_guard);
                    *session_id_ref.lock().await = Some(resume_id.clone());
                    let _ = agent
                        .post_claw_message(
                            session_id,
                            ClawMessage::ResumeSession {
                                session_id: resume_id,
                                character_id: override_character_id,
                            },
                        )
                        .await;
                }
            } else {
                let agent = crate::get_agent().await;
                let resume_guard = agent.subscribe_claw_session(resume_id.clone(), event_tx);
                *claw_sub_ref.lock() = Some(resume_guard);
                let _ = tx
                    .send(ClawMessage::ResumeSession {
                        session_id: resume_id.clone(),
                        character_id: override_character_id,
                    })
                    .await;
                *session_id_ref.lock().await = Some(resume_id);
            }

            glib::timeout_add_local_once(std::time::Duration::from_millis(80), move || {
                terminal.set_focusable(false);
                let aname = agent_name_rc.borrow().clone();
                if !aname.is_empty() {
                    claw_popover.show_input_only(&aname);
                } else {
                    claw_popover.show_chat_only("Agent");
                }
                msg_bar.entry.grab_focus();
            });
        });
    }
}
