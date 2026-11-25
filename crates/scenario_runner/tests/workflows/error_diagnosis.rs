//! E2E Workflow 2: Origin-aware terminal error diagnosis (`LazyErrorIndicator` at 0 tokens
//! for user-typed failures vs automatic diagnosis for agent-originated commands).

use boxxy_claw_protocol::{ClawEngineEvent, ClawMessage};
use scenario_runner::{MockStep, TestWorkspace};
use serial_test::serial;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
#[serial(boxxy_e2e)]
async fn origin_aware_error_diagnosis_spends_zero_tokens_until_requested() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    let mut pane = ws.spawn_pane("pane-err-1", 0).await.expect("spawn pane");
    pane.wait_ready(TIMEOUT).await.expect("ready");

    // 1. Non-error exit codes (`grep` returning 1 for no matches, `SIGINT` 130) must be ignored completely.
    pane.notify_command_finished(1, "grep needle empty.txt\n")
        .await
        .expect("grep exit 1");
    pane.notify_command_finished(130, "sleep 10\n^C\n")
        .await
        .expect("sigint 130");

    // 2. User-typed failing command (`exit 127`) emits `LazyErrorIndicator { visible: true }` with 0 LLM calls.
    let marker_idx = pane.advance_cursor_to_end();
    pane.notify_command_finished(127, "cargoo check\nbash: cargoo: command not found\n")
        .await
        .expect("user command failure");

    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::LazyErrorIndicator { visible: true, .. } => Some(()),
        _ => None,
    })
    .await
    .expect("LazyErrorIndicator shown");

    // Confirm none of the ignored commands triggered an earlier lazy indicator before `marker_idx`
    // and strictly 0 LLM requests were sent!
    assert_eq!(
        ws.llm.main_requests().len(),
        0,
        "user-typed errors must spend zero tokens before clicking the error pill"
    );
    let pre_events = &pane.all_events()[..marker_idx];
    assert!(
        !pre_events
            .iter()
            .any(|e| matches!(e, ClawEngineEvent::LazyErrorIndicator { visible: true, .. })),
        "grep exit 1 and SIGINT 130 must not trigger LazyErrorIndicator"
    );

    // 3. Clicking the pill (`RequestLazyDiagnosis`) triggers the stashed diagnostic prompt.
    ws.llm.push_step(MockStep::text(
        "Typo in command name — run:\n```bash\ncargo check\n```",
    ));
    pane.send(ClawMessage::RequestLazyDiagnosis)
        .await
        .expect("request lazy diagnosis");

    // Because the diagnostic response includes a ```bash block, the dispatcher emits `InjectCommand`
    // (`pending_inject_command = true` on `SessionState`).
    let suggested_cmd = pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::InjectCommand { command, .. } => Some(command.clone()),
            _ => None,
        })
        .await
        .expect("InjectCommand suggestion");
    assert_eq!(suggested_cmd.trim(), "cargo check");
    assert_eq!(ws.llm.main_requests().len(), 1);

    // 4. Agent-originated command (`InjectCommand`) failing with exit code 101 auto-diagnoses immediately!
    ws.llm.push_step(MockStep::text(
        "The build failed due to a missing semicolon in `src/lib.rs`.",
    ));
    pane.notify_command_finished(
        101,
        "cargo check\nerror: expected `;`, found `}`\n --> src/lib.rs:4:1\n",
    )
    .await
    .expect("agent command failed");

    let (auto_diag, _) = pane
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("automatic diagnosis for agent-originated command");
    assert!(auto_diag.contains("missing semicolon"));
    assert_eq!(ws.llm.main_requests().len(), 2);

    // 5. Rejecting an `InjectCommand` (`CancelPending`) spends 0 extra tokens, clears
    //    `pending_inject_command` so a subsequent user typo is NOT misclassified as agent-owned,
    //    and records the concise command-rejection note in `SessionState.history` for the next turn!
    ws.llm.push_steps([
        MockStep::text("Run the full suite:\n```bash\ncargo test --all\n```"),
        MockStep::text("Narrowed to unit tests only: `cargo test --lib`."),
    ]);
    pane.send_query("Propose running all workspace tests.")
        .await
        .expect("send command proposal turn");
    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::InjectCommand { command, .. } if command.trim() == "cargo test --all" => {
            Some(())
        }
        _ => None,
    })
    .await
    .expect("InjectCommand for cargo test --all");
    assert_eq!(ws.llm.main_requests().len(), 3);

    pane.send(ClawMessage::CancelPending)
        .await
        .expect("reject InjectCommand");
    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::ProposalResolved {
            approved: false, ..
        } => Some(()),
        _ => None,
    })
    .await
    .expect("ProposalResolved(false) on InjectCommand rejection");
    assert_eq!(
        ws.llm.main_requests().len(),
        3,
        "rejecting InjectCommand must spend 0 extra LLM calls"
    );

    // A user-typed failure right after rejecting `InjectCommand` must emit only `LazyErrorIndicator` (0 tokens)!
    pane.notify_command_finished(127, "typoo_cmd\nbash: typoo_cmd: command not found\n")
        .await
        .expect("user failure after rejected InjectCommand");
    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::LazyErrorIndicator { visible: true, .. } => Some(()),
        _ => None,
    })
    .await
    .expect("LazyErrorIndicator after rejected InjectCommand");
    assert_eq!(ws.llm.main_requests().len(), 3);

    // Next user turn sees the concise `[Proposed command `cargo test --all` — rejected by the user]` note!
    pane.send_query("That suite is too slow — suggest only unit tests.")
        .await
        .expect("send follow-up after rejected InjectCommand");
    let (narrow_reply, _) = pane
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("narrow reply");
    assert!(narrow_reply.contains("cargo test --lib"));

    let final_conv = ws
        .llm
        .main_requests()
        .last()
        .expect("final req")
        .conversation_text();
    assert!(
        final_conv.contains("[Proposed command `cargo test --all` — rejected by the user]"),
        "rejected InjectCommand must record concise rejection note in history:\n{final_conv}"
    );
    assert!(
        !final_conv.contains("[Previous task was interrupted by the user]"),
        "appended InjectCommand rejection note must keep clean User/Assistant alternation:\n{final_conv}"
    );

    pane.shutdown().await;
    ws.teardown().await;
}
