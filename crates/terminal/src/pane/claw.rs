use super::{PaneInner, PendingDiagnosis};
use crate::PaneOutput;
use boxxy_claw_protocol::{AgentStatus, ClawEngineEvent, ClawMessage, SessionUsage};
use boxxy_claw_widget::{
    ClawIndicator, MsgBarComponent, OverlayMode, TerminalOverlay, spawn_dispatch,
};
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub(super) fn setup_claw(
    widget: &gtk::Overlay,
    inner: &Rc<RefCell<PaneInner>>,
    id: String,
    claw_sender: async_channel::Sender<ClawMessage>,
    claw_sender_for_host: async_channel::Sender<ClawEngineEvent>,
    claw_rx: async_channel::Receiver<ClawEngineEvent>,
    msg_bar: Rc<MsgBarComponent>,
    callback: std::sync::Arc<dyn Fn(PaneOutput) + Send + Sync + 'static>,
    spawn_intent: Option<String>,
    is_claw_active: Rc<Cell<bool>>,
    session_usage: Rc<Cell<SessionUsage>>,
    is_pinned: Rc<Cell<bool>>,
    is_web_search: Rc<Cell<bool>>,
    session_status: Rc<RefCell<AgentStatus>>,
    agent_name: Rc<RefCell<String>>,
    claw_indicator: &ClawIndicator,
    pending_character: std::rc::Rc<std::cell::RefCell<String>>,
    pending_tasks: Rc<RefCell<Vec<boxxy_claw_protocol::ScheduledTask>>>,
) -> (TerminalOverlay, PendingDiagnosis) {
    let pending_proactive_diagnosis =
        Rc::new(RefCell::new(None::<(String, crate::TerminalProposal)>));
    let pending_diag_clone = pending_proactive_diagnosis.clone();

    // Provide the initial intent if one was passed in (peer agent spawn).
    // Plain terminal spawns pass `spawn_intent: None` so no character is claimed.
    if let Some(intent) = spawn_intent.filter(|s| !s.trim().is_empty()) {
        let tx = claw_sender.clone();
        let inner_weak = Rc::downgrade(inner);
        let pane_id = id.clone();
        let event_tx = claw_sender_for_host.clone();
        let active_cell = is_claw_active.clone();
        let status_cell = session_status.clone();
        let pin_cell = is_pinned.clone();
        let web_cell = is_web_search.clone();
        let ind = claw_indicator.clone();
        let msg_bar_for_intent = msg_bar.clone();
        let cb_for_intent = callback.clone();

        // Wait for PID to ensure PTY is ready, then claim an available character
        gtk::glib::spawn_future_local(async move {
            let mut check_count = 0;
            loop {
                let Some(inner_rc) = inner_weak.upgrade() else {
                    return;
                };
                let has_pid = inner_rc.borrow().pid.is_some();
                drop(inner_rc);
                if has_pid {
                    break;
                }
                check_count += 1;
                if check_count > 50 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }

            let Some(inner_rc) = inner_weak.upgrade() else {
                return;
            };
            let (pane, cwd, session_id_ref, claw_sub_ref) = {
                let inner_borrow = inner_rc.borrow();
                (
                    inner_borrow.terminal.clone(),
                    inner_borrow.working_dir.clone().unwrap_or_default(),
                    inner_borrow.session_id.clone(),
                    inner_borrow.claw_subscription.clone(),
                )
            };

            let snapshot = pane.get_text_snapshot(100, 0).await.unwrap_or_default();

            if session_id_ref.lock().await.is_none()
                && let Some(chosen) =
                    boxxy_claw_protocol::characters::find_first_available_character(Some(&pane_id))
            {
                let agent = crate::get_agent().await;
                if let Ok((sid, sub_guard)) = agent
                    .create_claw_session_with_sender(pane_id.clone(), chosen.config.id, event_tx)
                    .await
                    && !sid.is_empty()
                {
                    *claw_sub_ref.lock() = Some(sub_guard);
                    *session_id_ref.lock().await = Some(sid.clone());
                    active_cell.set(true);
                    *status_cell.borrow_mut() = AgentStatus::Waiting;
                    ind.set_visible(true);
                    msg_bar_for_intent.update_ui(
                        AgentStatus::Waiting,
                        pin_cell.get(),
                        web_cell.get(),
                    );
                    cb_for_intent(PaneOutput::ClawStateChanged(pane_id, true, false));
                    let _ = agent
                        .post_claw_message(sid.clone(), ClawMessage::Initialize)
                        .await;
                    let _ = agent
                        .post_claw_message(
                            sid,
                            ClawMessage::UserMessage {
                                message: intent,
                                snapshot,
                                cwd,
                                image_attachments: vec![],
                            },
                        )
                        .await;
                    return;
                }
            }

            let _ = tx
                .send(ClawMessage::UserMessage {
                    message: intent,
                    snapshot,
                    cwd,
                    image_attachments: vec![],
                })
                .await;
        });
    }

    // Build the pane-backed host adapter: the drawer's only view into
    // terminal state from here on. All the ad-hoc closures that used to
    // capture `inner`, `claw_sender`, `callback`, and `id` individually
    // are now fields on PaneClawHost.
    let host: Rc<dyn crate::ClawHost> = Rc::new(super::PaneClawHost {
        id: id.clone(),
        inner_weak: Rc::downgrade(inner),
        claw_sender: claw_sender.clone(),
        event_sender: claw_sender_for_host,
        callback: callback.clone(),
    });

    let claw_popover = TerminalOverlay::new(
        claw_indicator.clone(),
        msg_bar.clone(),
        host.clone(),
        pending_character,
    );
    widget.add_overlay(claw_popover.widget());

    // Hide the small top-right badge while the drawer is open — the indicator
    // pill inside the drawer already shows the same info. Show it again when
    // the drawer closes so it remains visible in the terminal background.
    let indicator_for_notify = claw_indicator.clone();
    claw_popover
        .widget()
        .connect_reveal_child_notify(move |rev| {
            indicator_for_notify.set_drawer_open(rev.reveals_child());
        });

    // Indicator callbacks: on-cancel aborts the current agent turn and transitions the
    // drawer back to Idle; on-lazy-click asks the agent for a fresh diagnosis; on-proactive-click
    // drains the queued-up diagnosis (stashed by `show_diagnosis_ready` from the
    // window orchestrator) and pops the drawer.
    let popover_clone = claw_popover.clone();
    let host_lazy = host.clone();
    let host_cancel = host.clone();
    let popover_cancel = claw_popover.clone();
    claw_indicator.set_callbacks(
        move || {
            popover_cancel.set_state(boxxy_claw_widget::OverlayState::Idle);
            host_cancel.send_claw(ClawMessage::Abort);
        },
        move || {
            host_lazy.send_claw(ClawMessage::RequestLazyDiagnosis {});
        },
        move || {
            if let Some((diag, proposal)) = pending_diag_clone.borrow_mut().take() {
                popover_clone.show(
                    OverlayMode::Claw,
                    "Boxxy-Claw",
                    None,
                    &diag,
                    proposal.into(),
                );
            }
        },
    );

    // Hand the agent event stream off to the widget's dispatch loop.
    // Every UI mutation (overlay, indicator, sidebar log) lives there;
    // the pane only owns state cells and the host adapter now.
    spawn_dispatch(
        claw_rx,
        host,
        claw_popover.clone(),
        claw_indicator.clone(),
        msg_bar,
        id.clone(),
        session_status,
        is_pinned,
        is_web_search,
        agent_name,
        session_usage,
        pending_tasks,
    );

    (claw_popover, pending_proactive_diagnosis)
}
