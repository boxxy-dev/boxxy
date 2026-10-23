use super::{PaneInner, PendingDiagnosis};
use crate::PaneOutput;
use boxxy_claw_protocol::{AgentStatus, ClawEngineEvent, ClawMessage, SessionUsage};
use boxxy_claw_widget::{
    ClawIndicator, MsgBarComponent, OverlayMode, TerminalOverlay, spawn_dispatch,
};
use gtk4 as gtk;
use gtk4::prelude::*;
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

    // Provide the initial intent if one was passed in
    if let Some(intent) = spawn_intent {
        let tx = claw_sender.clone();
        let inner_weak = Rc::downgrade(inner);

        // Wait for PID to ensure PTY is ready
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
                    // Timeout after 5 seconds
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }

            let Some(inner_rc) = inner_weak.upgrade() else {
                return;
            };
            let pane = inner_rc.borrow().terminal.clone();
            let cwd = inner_rc.borrow().working_dir.clone().unwrap_or_default();
            drop(inner_rc);
            if let Some(snapshot) = pane.get_text_snapshot(100, 0).await {
                let _ = tx
                    .send(ClawMessage::UserMessage {
                        message: intent,
                        snapshot,
                        cwd,
                        image_attachments: vec![],
                    })
                    .await;
            }
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
        claw_indicator.widget().upcast_ref(),
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

    // Indicator callbacks: on-cancel aborts the current agent turn and closes the drawer;
    // on-lazy-click asks the agent for a fresh diagnosis; on-proactive-click drains the
    // queued-up diagnosis (stashed by `show_diagnosis_ready` from the
    // window orchestrator) and pops the drawer.
    let popover_clone = claw_popover.clone();
    let host_lazy = host.clone();
    let host_cancel = host.clone();
    let _popover_cancel = claw_popover.clone();
    claw_indicator.set_callbacks(
        move || {
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
