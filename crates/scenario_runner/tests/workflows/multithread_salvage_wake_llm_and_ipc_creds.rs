//! E2E Workflow 19:
//! - Multi-threaded Tokio (`flavor = "multi_thread", worker_threads = 2`) mid-turn `Abort` salvage
//!   (`salvage_in_flight_tool_steps` executing deterministically BEFORE `JoinHandle::abort()` drops
//!   `InFlightClearGuard`), `SendQueuedPrompt` unparking with fresh terminal context, and immunity
//!   against hoisting duplicate `--- REFERENCED FILES ---` or `--- UPDATE CHANGELOG CONTEXT ---`
//!   blocks out of `BOXXY_DEBUG_CONTEXT` terminal snapshots.
//! - IPC-only session credentials (`AgentState.api_keys` with empty on-disk `Settings.api_keys`)
//!   propagating across parallel read-only tool batches (`file_read` + `grep_search` + `glob_search`),
//!   Turn-1 title generation, `Sleep` `flush_history`, and `> 3`-command LLM Dreamer wake summaries
//!   (`summarize_wake_delta`), plus HTTP 429 error recovery parking queued follow-up prompts.

use boxxy_claw_protocol::{AgentStatus, ClawEngineEvent, ClawMessage};
use boxxy_db::store::Store;
use boxxy_preferences::Settings;
use scenario_runner::{MockStep, MockToolCall, TestWorkspace, TurnGate};
use serde_json::{Value, json};
use serial_test::serial;
use std::time::Duration;
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial(boxxy_e2e)]
async fn multithreaded_mid_turn_abort_salvage_unpark_and_snapshot_inner_marker_immunity_workflow() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    ws.write_file(
        "src/live_target.rs",
        "pub const MAX_PIPELINE_DEPTH: usize = 64;\n",
    )
    .await
    .expect("write live_target.rs");

    let mut pane = ws
        .spawn_pane("pane-mt-salvage-1", 0)
        .await
        .expect("spawn pane");
    pane.wait_ready(TIMEOUT).await.expect("ready");

    // Seed a terminal snapshot containing `BOXXY_DEBUG_CONTEXT` inner markers (`REFERENCED FILES`
    // and `UPDATE CHANGELOG CONTEXT`) so Turn 2 proves `prepare_query_history` never hoists them
    // out of the snapshot fence as fake top-level blocks.
    let debug_snapshot = concat!(
        "[2026 INFO model-context]\n",
        "--- REFERENCED FILES (PRE-LOADED VIA @MENTION) ---\n",
        "### `@src/ghost_inner.rs` (`/tmp/src/ghost_inner.rs`)\n",
        "```text\n",
        "1: pub const GHOST_INNER_MARKER: u32 = 999;\n",
        "```\n",
        "--- END REFERENCED FILES ---\n\n",
        "--- UPDATE CHANGELOG CONTEXT ---\n",
        "Commits:\n",
        "- fix(parser): synthetic inner log entry\n",
        "thread 'worker' panicked at src/live_target.rs:1\n",
    );
    pane.set_terminal_snapshot(debug_snapshot).await;

    let abort_gate = TurnGate::new();
    ws.llm.push_steps([
        MockStep::tool_call("file_read", &json!({ "path": "src/live_target.rs" })),
        MockStep::gated(
            abort_gate.clone(),
            MockStep::text("Aborted on multi-thread worker before finishing"),
        ),
        MockStep::text("Unparked follow-up confirmed `MAX_PIPELINE_DEPTH` is `64`."),
    ]);

    pane.send_query("Read `src/live_target.rs` and analyze the crash.")
        .await
        .expect("start turn 1");
    abort_gate.wait_entered().await;

    // Enqueue a follow-up prompt so `handle_abort` exercises async queue-parking (`emit_queue_updated`)
    // alongside multi-threaded worker task cancellation.
    let queued_id = Uuid::new_v4();
    pane.send_query_with_id(
        queued_id,
        "Unparked follow-up: what pipeline depth did you read before abort?",
    )
    .await
    .expect("enqueue follow-up");
    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::QueueUpdated {
            items,
            paused: false,
        } if items.len() == 1 && items[0].id == queued_id => Some(()),
        _ => None,
    })
    .await
    .expect("queued item visible");

    // Send `Abort` (`Esc Esc`) on the 2-worker runtime: `salvage_in_flight_tool_steps` MUST run
    // before `handle.abort()` drops `InFlightClearGuard` on the other worker thread!
    pane.send(ClawMessage::Abort)
        .await
        .expect("abort turn 1 on multi-thread runtime");
    abort_gate.release();

    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::QueueUpdated {
            items,
            paused: true,
        } if items.len() == 1 && items[0].id == queued_id => Some(()),
        _ => None,
    })
    .await
    .expect("queued prompt parked on Abort");

    // Update terminal snapshot while parked and click **Send** (`SendQueuedPrompt`) on the parked row!
    pane.set_terminal_snapshot("$ cargo test --lib\ntest result: ok. 4 passed\n")
        .await;
    pane.send(ClawMessage::SendQueuedPrompt {
        id: queued_id,
        snapshot: "$ cargo test --lib\ntest result: ok. 4 passed\n".to_string(),
        cwd: ws.path().to_string_lossy().into_owned(),
    })
    .await
    .expect("unpark and send queued prompt");

    let (unparked_reply, _) = pane
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("unparked turn diagnosis");
    assert!(unparked_reply.contains("64"));

    let main_reqs = ws.llm.main_requests();
    let t2_req = main_reqs.last().expect("unparked turn request");
    let t2_conv = t2_req.conversation_text();

    // 1. Completed Step-1 `file_read` MUST survive multi-threaded `Abort` without amnesia!
    assert!(
        t2_conv.contains("pub const MAX_PIPELINE_DEPTH: usize = 64;"),
        "multi-threaded Abort must salvage completed mid-turn file_read before InFlightClearGuard::drop:\n{t2_conv}"
    );
    // 2. Unparked `SendQueuedPrompt` must carry the fresh terminal snapshot!
    assert!(
        t2_conv.contains("test result: ok. 4 passed"),
        "SendQueuedPrompt must inject updated terminal snapshot:\n{t2_conv}"
    );
    // 3. Turn 1's `BOXXY_DEBUG_CONTEXT` snapshot markers must appear ONLY once (inside Turn 1's
    //    `Terminal Snapshot:` fence), never hoisted into duplicate top-level blocks!
    assert_eq!(
        t2_conv
            .matches("--- REFERENCED FILES (PRE-LOADED VIA @MENTION) ---")
            .count(),
        1,
        "inner REFERENCED FILES marker inside Terminal Snapshot must never be duplicated into a top-level block:\n{t2_conv}"
    );
    assert_eq!(
        t2_conv.matches("--- UPDATE CHANGELOG CONTEXT ---").count(),
        1,
        "inner UPDATE CHANGELOG CONTEXT marker inside Terminal Snapshot must never be duplicated:\n{t2_conv}"
    );

    let non_sys_roles: Vec<&str> = t2_req
        .messages
        .iter()
        .filter_map(|m| m.get("role").and_then(Value::as_str))
        .filter(|&r| r != "system" && r != "developer")
        .collect();
    assert_eq!(
        non_sys_roles,
        vec!["user", "assistant", "tool", "assistant", "user"],
        "salvaged tool step + interruption bridge must serialize with valid roles"
    );

    pane.shutdown().await;
    ws.teardown().await;
}

#[tokio::test]
#[serial(boxxy_e2e)]
async fn ipc_credentials_parallel_reads_dreamer_llm_wake_summary_and_error_parking_workflow() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    ws.write_file("src/scanner.rs", "pub fn scan_tokens() -> usize {\n    12\n}\n")
        .await
        .expect("write scanner.rs");
    ws.write_file("src/filter.rs", "pub fn filter_noise() -> bool {\n    true\n}\n")
        .await
        .expect("write filter.rs");

    // Clear `Settings.api_keys` on disk and push the endpoint's Bearer key EXCLUSIVELY into
    // `ws.agent_state.api_keys` (simulating Flatpak `Agent.update_credentials` over D-Bus).
    let active_ep_name = Settings::load()
        .openai_compatible_endpoints
        .first()
        .expect("mock endpoint")
        .name
        .clone();
    Settings::update(|s| {
        s.api_keys.clear();
    });
    ws.agent_state.api_keys.write().await.insert(
        active_ep_name,
        "SYNTHETIC_IPC_ONLY_BEARER_KEY".to_string(),
    );

    let mut pane = ws
        .spawn_pane("pane-ipc-wake-1", 0)
        .await
        .expect("spawn pane");
    pane.wait_ready(TIMEOUT).await.expect("ready");
    let session_id = pane.session_id().to_string();

    // --- 1. Parallel read-only tool batch (`file_read` + `grep_search` + `glob_search`) in a single step
    //        + Turn-1 background title generation authenticated via IPC `api_keys`! ---
    ws.llm.push_aux_text("Scanner And Filter Audit");
    ws.llm.push_steps([
        MockStep::tool_calls(vec![
            MockToolCall::new("file_read", &json!({ "path": "src/scanner.rs" })),
            MockToolCall::new("grep_search", &json!({ "pattern": "filter_noise" })),
            MockToolCall::new("glob_search", &json!({ "pattern": "src/*.rs" })),
        ]),
        MockStep::text("Scanned `src/scanner.rs` (`12`) and matched `filter_noise` in `src/filter.rs`."),
    ]);

    pane.send_query("Inspect `src/scanner.rs`, search for `filter_noise`, and list `src/*.rs` in parallel.")
        .await
        .expect("send turn 1");
    let (t1_reply, t1_trace) = pane
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("turn 1 diagnosis");
    assert!(t1_reply.contains("filter_noise"));
    assert!(
        t1_trace.len() >= 3,
        "expected all 3 parallel read-only tools in turn trace, got: {t1_trace:?}"
    );

    let reqs_t1 = ws.llm.main_requests();
    assert_eq!(reqs_t1.len(), 2);
    assert_eq!(
        reqs_t1[0].authorization.as_deref(),
        Some("Bearer SYNTHETIC_IPC_ONLY_BEARER_KEY"),
        "main ClawAgent turn must merge IPC-pushed api_keys"
    );
    let step2_conv = reqs_t1[1].conversation_text();
    assert!(
        step2_conv.contains("pub fn scan_tokens() -> usize")
            && step2_conv.contains("filter_noise")
            && step2_conv.contains("scanner.rs"),
        "all 3 parallel read-only tools must settle cleanly into Step 2 history:\n{step2_conv}"
    );

    // Wait for Turn-1 background title generation and verify it ALSO used the IPC-only Bearer key!
    let store = Store::new(ws.db.pool());
    let title_deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        if let Ok(Some(s)) = store.get_session(&session_id).await
            && s.title.as_deref() == Some("Scanner And Filter Audit")
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < title_deadline,
            "timed out waiting for Turn-1 title generation with IPC-only credentials"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let title_req = ws
        .llm
        .aux_requests()
        .into_iter()
        .find(|r| r.flattened_message_text().contains("conversation title generator"))
        .expect("title aux request");
    assert_eq!(
        title_req.authorization.as_deref(),
        Some("Bearer SYNTHETIC_IPC_ONLY_BEARER_KEY"),
        "Turn-1 background title generator must merge IPC-pushed api_keys"
    );

    // --- 2. Enter `Sleep` (`flush_history` uses IPC key), run `5` commands (`> 3` threshold!),
    //        and Hard Wake (`SleepToggle(false)`) to exercise the LLM-backed Dreamer wake summarizer! ---
    ws.llm.push_aux_text(
        "{\"facts\":[{\"key\":\"scanner_width\",\"content\":\"Scanner emits 12 tokens\"}],\"summary\":\"Audited scanner and filter modules.\"}",
    );
    pane.send(ClawMessage::SleepToggle(true))
        .await
        .expect("enter sleep");
    pane.wait_for_status(&AgentStatus::Sleep, TIMEOUT)
        .await
        .expect("AgentStatus::Sleep");

    let ws_scope = ws.path().to_string_lossy().into_owned();
    let flush_deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        if store
            .get_memory_by_key("scanner_width", Some(&ws_scope))
            .await
            .ok()
            .flatten()
            .is_some()
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < flush_deadline,
            "timed out waiting for Sleep flush_history"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let sleep_flush_req = ws
        .llm
        .aux_requests()
        .into_iter()
        .rfind(|r| r.system_prompt_text().contains("memory extraction system"))
        .expect("sleep flush request");
    assert_eq!(
        sleep_flush_req.authorization.as_deref(),
        Some("Bearer SYNTHETIC_IPC_ONLY_BEARER_KEY"),
        "Sleep flush_unflushed_history must merge IPC-pushed api_keys"
    );

    // Execute 5 commands (> 3 inline limit) while asleep so Hard Wake invokes `summarize_wake_delta`'s LLM path!
    for step_idx in 1..=5 {
        let code = if step_idx == 4 { 101 } else { 0 };
        pane.notify_command_finished(code, format!("cargo check --step-{step_idx}\ndone\n"))
            .await
            .expect("notify slept command");
    }

    ws.llm.push_aux_text(
        "DREAMER_WAKE_SUMMARY: Executed 5 cargo check steps while asleep; step-4 failed with exit 101 before step-5 recovered.",
    );
    pane.send(ClawMessage::SleepToggle(false))
        .await
        .expect("wake from sleep (> 3 commands)");
    pane.wait_for_status(&AgentStatus::Waiting, TIMEOUT)
        .await
        .expect("returned to Waiting after LLM Dreamer wake summary");

    let wake_llm_req = ws
        .llm
        .aux_requests()
        .into_iter()
        .rfind(|r| r.system_prompt_text().contains("context-bridge"))
        .expect("expected > 3 slept commands to invoke Dreamer wake LLM summarizer");
    assert_eq!(
        wake_llm_req.authorization.as_deref(),
        Some("Bearer SYNTHETIC_IPC_ONLY_BEARER_KEY"),
        "Dreamer wake summarizer (> 3 commands) must merge IPC-pushed api_keys"
    );
    assert!(
        wake_llm_req
            .conversation_text()
            .contains("$ cargo check --step-4 (exit: 101)")
            && wake_llm_req
                .conversation_text()
                .contains("$ cargo check --step-5 (exit: 0)"),
        "Dreamer wake prompt must pass all 5 slept commands:\n{}",
        wake_llm_req.conversation_text()
    );

    // --- 3. Provider HTTP 429 Error Turn: verify queued follow-up parks (`paused: true`) instead of
    //        auto-firing over broken state, then recovers cleanly with `[WHILE_YOU_SLEPT]` intact! ---
    let err_gate = TurnGate::new();
    ws.llm.push_steps([
        MockStep::gated(
            err_gate.clone(),
            MockStep::http_error(429, "Upstream rate limit exceeded"),
        ),
        MockStep::text("Recovered after rate limit — noted Dreamer wake summary for step-4."),
    ]);

    pane.send_query("Review what happened while you slept.")
        .await
        .expect("start gated error turn");
    err_gate.wait_entered().await;

    let parked_after_err_id = Uuid::new_v4();
    pane.send_query_with_id(
        parked_after_err_id,
        "Should stay parked when Turn 2 errors!",
    )
    .await
    .expect("enqueue behind failing turn");
    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::QueueUpdated {
            items,
            paused: false,
        } if items.len() == 1 => Some(()),
        _ => None,
    })
    .await
    .expect("queued behind gated error turn");

    err_gate.release();

    let (err_diag, _) = pane
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("error diagnosis card");
    assert!(
        err_diag.contains("Upstream rate limit exceeded") || err_diag.contains("Rate Limited"),
        "expected formatted error diagnostic, got: {err_diag}"
    );
    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::QueueUpdated {
            items,
            paused: true,
        } if items.len() == 1 && items[0].id == parked_after_err_id => Some(()),
        _ => None,
    })
    .await
    .expect("provider error must park queued follow-up prompts (`paused: true`)");

    pane.send(ClawMessage::ClearQueuedPrompts)
        .await
        .expect("clear parked prompt");
    pane.send_query("Retry reviewing what happened while you slept.")
        .await
        .expect("send recovery turn");
    let (rec_reply, _) = pane
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("recovery turn diagnosis");
    assert!(rec_reply.contains("Dreamer wake summary"));

    let all_main_reqs = ws.llm.main_requests();
    let final_req = all_main_reqs.last().expect("recovery req");
    let final_conv = final_req.conversation_text();
    assert!(
        final_conv.contains("[WHILE_YOU_SLEPT]\nDREAMER_WAKE_SUMMARY: Executed 5 cargo check steps"),
        "LLM-summarized [WHILE_YOU_SLEPT] block must appear in post-wake context:\n{final_conv}"
    );
    assert!(
        final_conv.contains("[Previous task was interrupted by the user]"),
        "failed turn followed by retry must bridge consecutive user prompts for alternation:\n{final_conv}"
    );

    pane.shutdown().await;
    ws.teardown().await;
}
