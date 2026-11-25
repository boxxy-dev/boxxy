//! E2E Workflow 3: In-flight concurrency — FIFO follow-up prompt queue, recall/edit,
//! concurrent `/btw` side-channel (warm context, `tool_choice: "none"`, 0 history mutation),
//! and `Abort` (`Esc Esc`) queue parking + alternation continuity.

use boxxy_claw_protocol::{ClawEngineEvent, ClawMessage};
use boxxy_db::store::Store;
use scenario_runner::{MockStep, TestWorkspace, TurnGate};
use serde_json::json;
use serial_test::serial;
use std::time::Duration;
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
#[serial(boxxy_e2e)]
async fn follow_up_queue_concurrent_btw_and_abort_parking_workflow() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    ws.write_file("src/config.rs", "pub const PORT: u16 = 8080;\n")
        .await
        .expect("write config.rs");

    let mut pane = ws.spawn_pane("pane-queue-1", 0).await.expect("spawn pane");
    pane.wait_ready(TIMEOUT).await.expect("ready");

    // Gate Turn 1 AFTER a `file_read` tool call so `SessionState.in_flight_snapshot` contains
    // live settled mid-turn tool output when `/btw` runs!
    let turn1_gate = TurnGate::new();
    ws.llm.push_steps([
        MockStep::tool_call("file_read", &json!({ "path": "src/config.rs" }))
            .with_usage(500, 50),
        MockStep::gated(
            turn1_gate.clone(),
            MockStep::text("Inspected `src/config.rs` — default port is 8080.")
                .with_usage(700, 100),
        ),
        // Scripted response for Queued Follow-Up #1 once Turn 1 finishes:
        MockStep::text("Follow-up #1 complete: port confirmed.").with_usage(800, 80),
    ]);
    // Script `/btw` to attempt a rogue tool call FIRST (`file_delete`) — `ForbidToolsHook` must
    // block execution without deleting the file or emitting `ProposeFileDelete`, then return the prose answer!
    ws.llm.push_btw_step(
        MockStep::tool_call("file_delete", &json!({ "path": "src/config.rs" }))
            .with_usage(300, 20),
    );
    ws.llm.push_btw_step(
        MockStep::text("Side answer: `PORT` is `8080` (`u16`).").with_usage(600, 40),
    );

    pane.send_query("Inspect `src/config.rs` and summarize the port.")
        .await
        .expect("start turn 1");

    // Wait deterministically until `file_read` has finished and Turn 1 is paused at `turn1_gate`.
    turn1_gate.wait_entered().await;

    // 1. Enqueue two follow-up prompts while Turn 1 is actively running.
    let q1_id = Uuid::new_v4();
    let q2_id = Uuid::new_v4();
    pane.send_query_with_id(q1_id, "Follow-up 1: double-check the port type.")
        .await
        .expect("enqueue 1");
    pane.send_query_with_id(q2_id, "Follow-up 2: draft to recall.")
        .await
        .expect("enqueue 2");

    let queued_two = pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::QueueUpdated {
                items,
                paused: false,
            } if items.len() == 2 => Some(items.clone()),
            _ => None,
        })
        .await
        .expect("2 items queued");
    assert_eq!(queued_two[0].id, q1_id);
    assert_eq!(queued_two[1].id, q2_id);

    // 2. Run a concurrent `/btw` side question while Turn 1 is still gated!
    let btw_id = pane
        .send_btw("Quick check: what port number did `file_read` just return?")
        .await
        .expect("send /btw");

    let btw_answer = pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::BtwComplete {
                request_id,
                answer,
                error: None,
                ..
            } if *request_id == btw_id => Some(answer.clone()),
            _ => None,
        })
        .await
        .expect("BtwComplete");
    assert!(btw_answer.contains("8080"));

    // Verify `/btw` reused the warm `ClawAgent` tool schemas + `in_flight_snapshot` AND
    // `ForbidToolsHook` blocked the rogue `file_delete` call!
    let btw_reqs = ws.llm.btw_requests();
    assert_eq!(
        btw_reqs.len(),
        2,
        "expected rogue tool attempt blocked by ForbidToolsHook followed by text reply"
    );
    assert!(
        !btw_reqs[0].tools.is_empty(),
        "/btw on active session must preserve tool schemas for KV prefix-cache hit"
    );
    assert!(
        btw_reqs[0]
            .flattened_message_text()
            .contains("pub const PORT: u16 = 8080;"),
        "/btw must reuse warm in_flight_snapshot containing settled mid-turn file_read output"
    );
    assert!(
        btw_reqs[0]
            .flattened_message_text()
            .contains("completed so far in this turn: 1 read; currently active step: Thinking…"),
        "settled tool batch must reset active_status to Thinking… rather than stale Reading file…:\n{}",
        btw_reqs[0].flattened_message_text()
    );
    assert!(
        ws.file_path("src/config.rs").exists(),
        "ForbidToolsHook must prevent /btw from executing file_delete"
    );

    // 3. Recall Follow-Up #2 (`Edit` button in UI) before Turn 1 finishes.
    pane.send(ClawMessage::RemoveQueuedPrompt {
        id: q2_id,
        recall_to_input: true,
    })
    .await
    .expect("recall q2");

    let recalled_text = pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::QueuedPromptRecalled { id, fallback_text } if *id == q2_id => {
                Some(fallback_text.clone())
            }
            _ => None,
        })
        .await
        .expect("QueuedPromptRecalled");
    assert_eq!(recalled_text, "Follow-up 2: draft to recall.");

    // 4. Release Turn 1 gate -> Turn 1 completes AND Follow-Up #1 drains automatically in FIFO order!
    turn1_gate.release();

    let (turn1_reply, _) = pane.wait_for_diagnosis(TIMEOUT).await.expect("turn 1 done");
    assert!(turn1_reply.contains("8080"));

    let (turn2_reply, _) = pane
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("queued follow-up #1 auto-drained");
    assert!(turn2_reply.contains("Follow-up #1 complete"));

    // Verify `/btw` never polluted `SessionState.history` or persisted `claw_events`!
    let store = Store::new(ws.db.pool());
    let persisted_events = store
        .get_claw_events(pane.session_id())
        .await
        .expect("get_claw_events");
    assert!(
        persisted_events
            .iter()
            .all(|raw| !raw.contains("Side answer: `PORT` is `8080`")),
        "/btw side answers must never be written to claw_events"
    );

    // 5. Test `Abort` (`Esc Esc`) parking a queued prompt + alternation continuity bridge!
    let abort_gate = TurnGate::new();
    ws.llm.push_steps([
        MockStep::gated(abort_gate.clone(), MockStep::text("Never reached")),
        MockStep::text("Recovered cleanly after abort."),
    ]);

    pane.send_query("Start a long analysis task.")
        .await
        .expect("start turn 3");
    abort_gate.wait_entered().await;

    let parked_id = Uuid::new_v4();
    pane.send_query_with_id(parked_id, "Run deploy after analysis.")
        .await
        .expect("enqueue behind turn 3");
    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::QueueUpdated {
            items,
            paused: false,
        } if items.len() == 1 => Some(()),
        _ => None,
    })
    .await
    .expect("queued behind turn 3");

    // Interrupt Turn 3: queued prompt MUST park (`paused: true`) rather than auto-firing!
    pane.send(ClawMessage::Abort).await.expect("abort turn 3");
    abort_gate.release();

    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::QueueUpdated {
            items,
            paused: true,
        } if items.len() == 1 && items[0].id == parked_id => Some(()),
        _ => None,
    })
    .await
    .expect("queue parked on Abort");

    // Clear parked item and send a new prompt; verify synthetic interruption bridge preserves alternation!
    pane.send(ClawMessage::ClearQueuedPrompts)
        .await
        .expect("clear parked");
    pane.send_query("Let's try a different approach.")
        .await
        .expect("post-abort prompt");

    let (post_abort_reply, _) = pane
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("post-abort diagnosis");
    assert!(post_abort_reply.contains("Recovered cleanly"));

    let all_main_reqs = ws.llm.main_requests();
    let last_req = all_main_reqs.last().expect("last request");
    assert!(
        last_req
            .flattened_message_text()
            .contains("[Previous task was interrupted by the user]"),
        "consecutive user messages after Abort must be bridged for Anthropic/Gemini alternation"
    );

    pane.shutdown().await;
    ws.teardown().await;
}
