//! E2E Workflow 1: `@file` pre-loading, `replace_in_file` proposals, zero-token rejection
//! with same-pane continuity, mid-proposal user steering with `@file` preload, approval diff trace,
//! and `/undo` byte-exact restoration.

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
    let initial_module = "pub fn compute_limit() -> u32 {\n    10\n}\n";
    let initial_other = "pub fn fallback_limit() -> u32 {\n    5\n}\n";
    ws.write_file("src/module.rs", initial_module)
        .await
        .expect("write initial module.rs");
    ws.write_file("src/other.rs", initial_other)
        .await
        .expect("write initial other.rs");

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
        // Next step is consumed by Turn 2 on the SAME pane (never by the rejected Turn 1!).
        MockStep::tool_call(
            "replace_in_file",
            &json!({
                "path": "src/module.rs",
                "old_string": "    10",
                "new_string": "    30"
            }),
        ),
        // Steered step inside Turn 2 edits `src/other.rs` (pre-loaded via `@src/other.rs` in steering msg):
        MockStep::tool_call(
            "replace_in_file",
            &json!({
                "path": "src/other.rs",
                "old_string": "    5",
                "new_string": "    42"
            }),
        ),
        MockStep::thought_and_text(
            "**Applying steered limit update**\nUpdated fallback_limit.",
            "Done — `fallback_limit()` in `src/other.rs` now returns `42`.",
        ),
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
            .conversation_text()
            .contains("1: pub fn compute_limit() -> u32 {")
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

    assert_eq!(ws.read_file("src/module.rs").await.unwrap(), initial_module);
    assert_eq!(
        ws.llm.remaining_main_steps(),
        3,
        "zero-token rejection must not consume another LLM completion"
    );

    // --- 2. Turn 2 on SAME pane: verifies post-reject history continuity + `@file` retention,
    //        then STEERS mid-proposal to `@src/other.rs` -> `42`, then APPROVES ---
    pane.send_query("Try updating `src/module.rs` to 30 instead.")
        .await
        .expect("send turn 2 on same pane");

    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::ProposeFileWrite {
            content,
            auto_approve: false,
            ..
        } if content.contains("    30") => Some(()),
        _ => None,
    })
    .await
    .expect("first proposal on turn 2 (30)");

    // Verify Turn 2's wire payload retained Turn 1's `@src/module.rs` preload AND the concise rejection note!
    let reqs_after_t2_start = ws.llm.main_requests();
    assert_eq!(reqs_after_t2_start.len(), 2);
    let t2_conv = reqs_after_t2_start[1].conversation_text();
    assert!(
        t2_conv.contains("1: pub fn compute_limit() -> u32 {"),
        "Turn 1 @file preload must stay in history inside the 4-turn freshness window"
    );
    assert!(
        t2_conv.contains("rejected by the user"),
        "Zero-token rejection must record concise rejection note in assistant history"
    );
    assert!(
        !t2_conv.contains("[Previous task was interrupted by the user]"),
        "Concise rejection note already satisfies User/Assistant alternation without fallback placeholder"
    );

    // Steer while `pending_file_reply` is open, referencing `@src/other.rs` for the first time!
    pane.send_query("Wait, leave module.rs alone and make @src/other.rs return 42 instead.")
        .await
        .expect("steer proposal to @src/other.rs");

    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::ProposeFileWrite {
            path,
            content,
            auto_approve: false,
            ..
        } if path.ends_with("src/other.rs") && content.contains("    42") => Some(()),
        _ => None,
    })
    .await
    .expect("steered proposal on src/other.rs (42)");

    // Confirm `@src/other.rs` was pre-loaded into the `[USER_INTERRUPTION]` tool result!
    let steered_reqs = ws.llm.main_requests();
    assert_eq!(steered_reqs.len(), 3);
    assert!(
        steered_reqs[2]
            .conversation_text()
            .contains("1: pub fn fallback_limit() -> u32 {"),
        "mid-proposal steering must pre-load newly @-mentioned files"
    );

    pane.reply_file_write(true).await.expect("approve steered");

    let (diagnosis, trace) = pane
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
    assert_eq!(ws.read_file("src/module.rs").await.unwrap(), initial_module);
    assert_eq!(
        ws.read_file("src/other.rs").await.unwrap(),
        "pub fn fallback_limit() -> u32 {\n    42\n}\n"
    );

    // --- 3. `/undo` restores `src/other.rs` byte-for-byte with 0 extra LLM calls ---
    let req_count_before_undo = ws.llm.main_requests().len();
    pane.send_query("/undo").await.expect("send /undo");

    let sys_msg = pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::SystemMessage { text } => Some(text.clone()),
            _ => None,
        })
        .await
        .expect("undo system message");
    assert!(
        sys_msg.to_lowercase().contains("restored") || sys_msg.contains("other.rs"),
        "unexpected undo message: {sys_msg}"
    );
    assert_eq!(ws.read_file("src/other.rs").await.unwrap(), initial_other);
    assert_eq!(ws.llm.main_requests().len(), req_count_before_undo);

    pane.shutdown().await;
    ws.teardown().await;
}
