//! E2E Workflow 1: `@file` pre-loading, `replace_in_file` proposals, zero-token rejection,
//! mid-proposal user steering, approval diff trace, and `/undo` byte-exact restoration.

use boxxy_claw_protocol::{ClawEngineEvent, TurnTraceItem};
use scenario_runner::{MockStep, TestWorkspace};
use serde_json::json;
use serial_test::serial;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
#[serial(boxxy_e2e)]
async fn edit_proposal_reject_steer_approve_and_undo_workflow() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    let initial_source = "pub fn compute_limit() -> u32 {\n    10\n}\n";
    ws.write_file("src/module.rs", initial_source)
        .await
        .expect("write initial source");

    let mut pane = ws.spawn_pane("pane-edit-1", 0).await.expect("spawn pane");
    pane.wait_ready(TIMEOUT).await.expect("ready");

    // --- 1. Turn 1: `@src/module.rs` pre-loads file & hash; user REJECTS proposal (0 extra LLM calls) ---
    ws.llm.push_steps([
        MockStep::tool_call(
            "replace_in_file",
            &json!({
                "path": "src/module.rs",
                "old_string": "    10",
                "new_string": "    20"
            }),
        ),
        // Intentionally push a sentinel step that MUST NOT be consumed when user rejects cleanly!
        MockStep::text("UNEXPECTED_POST_REJECT_STEP"),
    ]);

    pane.send_query("Update @src/module.rs so compute_limit returns 20.")
        .await
        .expect("send turn 1");

    let diff = pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::ProposeFileWrite {
                path,
                diff: Some(d),
                auto_approve: false,
                ..
            } if path.ends_with("src/module.rs") => Some(d.clone()),
            _ => None,
        })
        .await
        .expect("receive ProposeFileWrite");

    assert_eq!(diff.stats.added_lines, 1);
    assert_eq!(diff.stats.removed_lines, 1);

    // Confirm `@src/module.rs` contents were pre-loaded into the Turn 1 prompt context.
    let turn1_reqs = ws.llm.main_requests();
    assert_eq!(turn1_reqs.len(), 1);
    assert!(
        turn1_reqs[0]
            .flattened_message_text()
            .contains("pub fn compute_limit() -> u32")
    );

    // Reject proposal: must abort immediately at 0 extra LLM tokens and leave file untouched.
    pane.reply_file_write(false).await.expect("reject write");
    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::ProposalResolved {
            approved: false, ..
        } => Some(()),
        _ => None,
    })
    .await
    .expect("ProposalResolved(false)");

    assert_eq!(ws.read_file("src/module.rs").await.unwrap(), initial_source);
    assert_eq!(
        ws.llm.remaining_main_steps(),
        1,
        "zero-token rejection must not consume another LLM completion"
    );

    // Pop the unconsumed sentinel by resetting steps via a fresh workspace or draining:
    // Here we replace it by using it? Even better: let's test steering on Turn 2!
    // First consume/replace: let's check Turn 2 with steering!
    let mut ws = TestWorkspace::new().await.expect("workspace 2");
    ws.write_file("src/module.rs", initial_source)
        .await
        .expect("seed file");
    let mut pane2 = ws.spawn_pane("pane-edit-2", 0).await.expect("spawn pane 2");
    pane2.wait_ready(TIMEOUT).await.expect("ready 2");

    // --- 2. Turn 2: Agent proposes `30`, user STEERS mid-proposal with a new message -> `42`, then APPROVES ---
    ws.llm.push_steps([
        // First attempt proposes 30:
        MockStep::tool_call(
            "replace_in_file",
            &json!({
                "path": "src/module.rs",
                "old_string": "    10",
                "new_string": "    30"
            }),
        ),
        // Steered turn proposes 42 and completes:
        MockStep::tool_call(
            "replace_in_file",
            &json!({
                "path": "src/module.rs",
                "old_string": "    10",
                "new_string": "    42"
            }),
        ),
        MockStep::thought_and_text(
            "**Applying limit update**\nVerified replacement.",
            "Done — `compute_limit()` now returns `42`.",
        ),
    ]);

    pane2
        .send_query("Change @src/module.rs limit to 30.")
        .await
        .expect("send initial");

    pane2
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::ProposeFileWrite {
                content,
                auto_approve: false,
                ..
            } if content.contains("    30") => Some(()),
            _ => None,
        })
        .await
        .expect("first proposal (30)");

    // Steer while `pending_file_reply` is open — cancels the pending proposal and starts the steered turn!
    pane2
        .send_query("Wait, make @src/module.rs return 42 instead.")
        .await
        .expect("steer proposal");

    pane2
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::ProposeFileWrite {
                content,
                auto_approve: false,
                ..
            } if content.contains("    42") => Some(()),
            _ => None,
        })
        .await
        .expect("steered proposal (42)");

    pane2.reply_file_write(true).await.expect("approve steered");

    let (diagnosis, trace) = pane2
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("final diagnosis");
    assert!(diagnosis.contains("42"));
    assert!(
        trace
            .iter()
            .any(|item| matches!(item, TurnTraceItem::Edit { auto_approved: false, .. })),
        "approved edit must be recorded in TurnTraceItem::Edit"
    );
    assert_eq!(
        ws.read_file("src/module.rs").await.unwrap(),
        "pub fn compute_limit() -> u32 {\n    42\n}\n"
    );

    // --- 3. `/undo` restores original file byte-for-byte with 0 extra LLM calls ---
    let req_count_before_undo = ws.llm.main_requests().len();
    pane2.send_query("/undo").await.expect("send /undo");

    let sys_msg = pane2
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::SystemMessage { text } => Some(text.clone()),
            _ => None,
        })
        .await
        .expect("undo system message");
    assert!(
        sys_msg.to_lowercase().contains("restored") || sys_msg.contains("module.rs"),
        "unexpected undo message: {sys_msg}"
    );
    assert_eq!(ws.read_file("src/module.rs").await.unwrap(), initial_source);
    assert_eq!(ws.llm.main_requests().len(), req_count_before_undo);

    pane.shutdown().await;
    pane2.shutdown().await;
    ws.teardown().await;
}
