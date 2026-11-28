//! E2E Workflow 18:
//! - Mid-turn completed tool step salvage (`salvage_in_flight_tool_steps`) across explicit
//!   proposal rejection (`CancelPending` / `FileWriteReply { approved: false }`) and user-initiated
//!   `SleepToggle(true)` (including lock release, follow-up queue parking, `ASSISTANT_TOOLS` flush
//!   visibility, and durable `[WHILE_YOU_SLEPT]` SQLite persistence across `/resume`).
//! - `file_delete` pre-proposal `BLACKLIST.md` guard, tracked-hash cleanup (`0` false
//!   `EXTERNALLY MODIFIED FILES NOTICE` on subsequent turns), and byte-exact `/undo` restoration.
//! - Small-context local model (`16k` window) 75% threshold discipline (never compacting prematurely
//!   at `< 75%`), `BOXXY_DEBUG_CONTEXT` inner-marker snapshot preservation, and high-pressure
//!   few-turn (`<= 6` exchanges at `>= 85%` of `input_limit`) safe exchange-boundary compaction
//!   with proportional `sessions.context_tokens` scaling.

use boxxy_claw_protocol::{AgentStatus, ClawEngineEvent, ClawMessage};
use boxxy_db::store::Store;
use boxxy_model_selection::{
    CustomModelConfig, ModelProvider, OpenAiApiRoute, OpenAiCompatibleEndpoint,
};
use boxxy_preferences::Settings;
use scenario_runner::{MockLlmServer, MockStep, TestWorkspace, TurnGate};
use serde_json::{Value, json};
use serial_test::serial;
use std::time::Duration;
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
#[serial(boxxy_e2e)]
async fn mid_turn_tool_salvage_across_rejection_sleep_and_file_delete_undo_workflow() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    let router_src = "pub const ROUTE_TIMEOUT_MS: u64 = 1500;\n";
    let obsolete_src = "pub fn legacy_adapter() -> bool {\n    true\n}\n";
    ws.write_file("src/router.rs", router_src)
        .await
        .expect("write router.rs");
    ws.write_file("src/obsolete.rs", obsolete_src)
        .await
        .expect("write obsolete.rs");
    ws.write_file("secrets/api_token.txt", "SYNTHETIC_PROTECTED_TOKEN\n")
        .await
        .expect("write blacklisted file");
    ws.set_blacklist_markdown("secrets\n")
        .await
        .expect("write BLACKLIST.md");

    let mut pane1 = ws
        .spawn_pane("pane-salvage-1", 0)
        .await
        .expect("spawn pane 1");
    pane1.wait_ready(TIMEOUT).await.expect("ready 1");
    let session_id = pane1.session_id().to_string();

    // --- 1. Multi-step Turn 1: `file_read("src/router.rs")` succeeds in Step 1, then
    //        `replace_in_file` proposes an edit in Step 2 that the user REJECTS (0 extra LLM calls)! ---
    ws.llm.push_aux_text("Router Timeout And Cleanup");
    ws.llm.push_steps([
        MockStep::tool_call("file_read", &json!({ "path": "src/router.rs" })),
        MockStep::tool_call(
            "replace_in_file",
            &json!({
                "path": "src/router.rs",
                "old_string": "1500",
                "new_string": "3000"
            }),
        ),
        // Turn 2 steps (after Turn 1's Step 2 edit is rejected):
        //   a) Attempt `file_delete` on blacklisted `secrets/api_token.txt` -> blocked BEFORE ProposeFileDelete!
        //   b) Read `src/obsolete.rs` (tracks hash) and delete `src/obsolete.rs` -> approved!
        MockStep::tool_call("file_delete", &json!({ "path": "secrets/api_token.txt" })),
        MockStep::tool_call("file_read", &json!({ "path": "src/obsolete.rs" })),
        MockStep::tool_call("file_delete", &json!({ "path": "src/obsolete.rs" })),
        MockStep::text(
            "`ROUTE_TIMEOUT_MS` was `1500`, `secrets/api_token.txt` stayed protected, and `src/obsolete.rs` was deleted.",
        ),
        // Turn 3 step (verifies 0 false EXTERNALLY MODIFIED FILES NOTICE after file_delete):
        MockStep::text("Workspace clean — no false external-modification warnings."),
    ]);

    pane1
        .send_query("Inspect `src/router.rs` and double the route timeout.")
        .await
        .expect("send turn 1");

    pane1
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::ProposeFileWrite {
                path,
                auto_approve: false,
                ..
            } if path.ends_with("src/router.rs") => Some(()),
            _ => None,
        })
        .await
        .expect("Turn 1 ProposeFileWrite");

    // Reject Step 2's edit proposal: Step 1's completed `file_read("src/router.rs")` MUST be salvaged!
    pane1
        .reply_file_write(false)
        .await
        .expect("reject router edit");
    pane1
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::ProposalResolved {
                approved: false, ..
            } => Some(()),
            _ => None,
        })
        .await
        .expect("ProposalResolved(false)");
    assert_eq!(ws.read_file("src/router.rs").await.unwrap(), router_src);

    // --- 2. Turn 2: Verify Step 1's `file_read` output survived Turn 1's rejection, blacklisted
    //        `file_delete` is blocked without proposing, and approved `file_delete` deletes cleanly! ---
    let ev_before_t2 = pane1.advance_cursor_to_end();
    pane1
        .send_query(
            "What timeout value did you read in `src/router.rs`? Leave it alone and delete `src/obsolete.rs` instead.",
        )
        .await
        .expect("send turn 2");

    let delete_target = pane1
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::ProposeFileDelete { path, .. } => Some(path.clone()),
            _ => None,
        })
        .await
        .expect("ProposeFileDelete for obsolete.rs");
    assert!(
        delete_target.ends_with("src/obsolete.rs"),
        "blacklisted secrets/api_token.txt must be blocked before ProposeFileDelete, got: {delete_target}"
    );
    assert!(
        ws.file_path("secrets/api_token.txt").exists(),
        "blacklisted file must remain untouched on disk"
    );

    pane1
        .reply_file_delete(true)
        .await
        .expect("approve obsolete.rs deletion");
    let (t2_reply, _) = pane1
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("turn 2 diagnosis");
    assert!(t2_reply.contains("1500"));
    assert!(!ws.file_path("src/obsolete.rs").exists());

    assert!(
        !pane1.events_since(ev_before_t2).iter().any(|ev| matches!(
            ev,
            ClawEngineEvent::ProposeFileDelete { path, .. } if path.contains("secrets")
        )),
        "FileDeleteTool must never emit ProposeFileDelete for blacklisted paths"
    );

    // Audit Turn 2's first request (index 2): must retain BOTH salvaged `ROUTE_TIMEOUT_MS: u64 = 1500`
    // AND `[Proposed file edit ... — rejected by the user]` with valid wire role transitions!
    let reqs_after_t2 = ws.llm.main_requests();
    let t2_first_req = &reqs_after_t2[2];
    let t2_first_conv = t2_first_req.conversation_text();
    assert!(
        t2_first_conv.contains("pub const ROUTE_TIMEOUT_MS: u64 = 1500;")
            && t2_first_conv.contains("rejected by the user"),
        "salvage_in_flight_tool_steps must preserve completed Step 1 file_read output alongside Step 2 rejection note:\n{t2_first_conv}"
    );
    let non_sys_roles: Vec<&str> = t2_first_req
        .messages
        .iter()
        .filter_map(|m| m.get("role").and_then(Value::as_str))
        .filter(|&r| r != "system" && r != "developer")
        .collect();
    assert_eq!(
        non_sys_roles,
        vec!["user", "assistant", "tool", "assistant", "user"],
        "salvaged tool step + rejection note must serialize as clean user -> assistant(tool_calls) -> tool -> assistant -> user"
    );

    // --- 3. Turn 3: Verify `file_delete` removed `src/obsolete.rs` from `read_file_hashes` so
    //        `check_externally_modified_files` emits ZERO false external-deletion notices, then `/undo`! ---
    pane1
        .send_query("Confirm everything is clean after deleting `src/obsolete.rs`.")
        .await
        .expect("send turn 3");
    pane1
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("turn 3 diagnosis");

    let t3_conv = ws
        .llm
        .main_requests()
        .last()
        .expect("turn 3 req")
        .conversation_text();
    assert!(
        !t3_conv.contains("EXTERNALLY MODIFIED FILES NOTICE")
            && !t3_conv.contains("deleted or moved on disk"),
        "agent-approved file_delete must untrack deleted path so Turn 3 never emits a false EXTERNALLY MODIFIED FILES NOTICE:\n{t3_conv}"
    );

    // Wait for Turn-2/3 session title generation to settle before `/undo` + `SleepToggle` aux checks!
    let store = Store::new(ws.db.pool());
    let title_deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        if let Ok(Some(s)) = store.get_session(&session_id).await
            && s.title.as_deref() == Some("Router Timeout And Cleanup")
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < title_deadline,
            "timed out waiting for Turn-1 title generation"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Wait until Turn 2's deletion was committed to `last_turn_snapshots`? Note: Turn 3 did NOT
    // modify any files, so `last_turn_snapshots` STILL holds Turn 2's deleted `src/obsolete.rs` baseline!
    // Verify `/undo` restores the deleted file byte-for-byte with 0 LLM tokens!
    pane1.send_query("/undo").await.expect("send /undo");
    let undo_notice = pane1
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::SystemMessage { text } => Some(text.clone()),
            _ => None,
        })
        .await
        .expect("/undo SystemMessage");
    assert!(
        undo_notice.contains("Restored") && undo_notice.contains("obsolete.rs"),
        "expected /undo to restore file deleted via file_delete, got: {undo_notice}"
    );
    assert_eq!(
        ws.read_file("src/obsolete.rs").await.expect("restored file"),
        obsolete_src
    );

    // --- 4. Mid-turn `SleepToggle(true)` while holding a lock + queued follow-up + salvaged tool steps,
    //        followed by Hard Wake (`[WHILE_YOU_SLEPT]`) and cross-pane `/resume`! ---
    let sleep_gate = TurnGate::new();
    ws.llm.push_steps([
        MockStep::tool_call("acquire_lock", &json!({ "resource": "src/router.rs" })),
        MockStep::tool_call("file_read", &json!({ "path": "src/obsolete.rs" })),
        MockStep::gated(
            sleep_gate.clone(),
            MockStep::text("Aborted when user puts session to sleep"),
        ),
        // Post-resume Turn on `pane2` verifying durable `[WHILE_YOU_SLEPT]` + salvaged tool steps:
        MockStep::text("Resumed with slept command delta and salvaged lock/read history intact."),
    ]);

    pane1
        .send_query("Lock `src/router.rs`, inspect restored `src/obsolete.rs`, and plan next step.")
        .await
        .expect("start turn 4");
    sleep_gate.wait_entered().await;

    let queued_id = Uuid::new_v4();
    pane1
        .send_query_with_id(queued_id, "Queued follow-up before sleep.")
        .await
        .expect("enqueue before sleep");
    pane1
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::QueueUpdated {
                items,
                paused: false,
            } if items.len() == 1 => Some(()),
            _ => None,
        })
        .await
        .expect("queued before sleep");

    ws.llm.push_aux_text(
        "{\"facts\":[{\"key\":\"restored_adapter\",\"content\":\"Restored legacy_adapter via undo\"}],\"summary\":\"Restored legacy_adapter and audited router lock.\"}",
    );
    pane1
        .send(ClawMessage::SleepToggle(true))
        .await
        .expect("user SleepToggle(true) mid-turn");
    sleep_gate.release();

    pane1
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::QueueUpdated {
                items,
                paused: true,
            } if items.len() == 1 && items[0].id == queued_id => Some(()),
            _ => None,
        })
        .await
        .expect("SleepToggle(true) mid-turn must park queued follow-ups");
    pane1
        .wait_for_status(&AgentStatus::Sleep, TIMEOUT)
        .await
        .expect("user SleepToggle(true) must abort active turn, release locks, and enter Sleep");

    // Confirm `src/router.rs` lock was released when entering Sleep!
    let ws_reg = boxxy_claw::registry::workspace::global_workspace().await;
    ws_reg
        .acquire_lock("peer-sleep-probe".to_string(), "src/router.rs".to_string())
        .await
        .expect("SleepToggle(true) must release held LockTable locks");
    ws_reg
        .release_lock("peer-sleep-probe".to_string(), "src/router.rs".to_string())
        .await;

    // Wait for `flush_history` on sleep entry and confirm salvaged `ASSISTANT_TOOLS:` reached `memory_model`!
    let ws_scope = ws.path().to_string_lossy().into_owned();
    let flush_deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        let mem_opt = store
            .get_memory_by_key("restored_adapter", Some(&ws_scope))
            .await
            .ok()
            .flatten();
        let flush_req = ws
            .llm
            .aux_requests()
            .into_iter()
            .rfind(|r| r.system_prompt_text().contains("memory extraction system"));
        if let (Some(mem), Some(freq)) = (mem_opt, flush_req) {
            assert_eq!(mem.content, "Restored legacy_adapter via undo");
            assert!(
                freq.conversation_text()
                    .contains("ASSISTANT_TOOLS: acquire_lock(src/router.rs)\nASSISTANT_TOOLS: file_read(src/obsolete.rs)"),
                "sleep flush must include salvaged mid-turn ASSISTANT_TOOLS hints:\n{}",
                freq.conversation_text()
            );
            break;
        }
        assert!(
            tokio::time::Instant::now() < flush_deadline,
            "timed out waiting for mid-turn SleepToggle flush_history"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Run a command while sleeping, Hard Wake (`SleepToggle(false)`), and `/resume` onto `pane2`!
    pane1
        .notify_command_finished(0, "cargo test --lib\ntest result: ok\n")
        .await
        .expect("slept command");
    pane1
        .send(ClawMessage::ClearQueuedPrompts)
        .await
        .expect("clear parked queue");
    pane1
        .send(ClawMessage::SleepToggle(false))
        .await
        .expect("wake from sleep");
    pane1
        .wait_for_status(&AgentStatus::Waiting, TIMEOUT)
        .await
        .expect("awake");

    let mut pane2 = ws
        .spawn_pane("pane-salvage-2", 0)
        .await
        .expect("spawn pane 2");
    pane2.wait_ready(TIMEOUT).await.expect("ready 2");
    pane2
        .send(ClawMessage::ResumeSession {
            session_id,
            character_id: None,
        })
        .await
        .expect("resume on pane 2");
    pane1
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::Evicted => Some(()),
            _ => None,
        })
        .await
        .expect("pane 1 evicted");
    pane2
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::RestoreHistory(rows) if !rows.is_empty() => Some(()),
            _ => None,
        })
        .await
        .expect("pane 2 restored history");

    pane2
        .send_query("Summarize what ran while you slept and what you read before sleeping.")
        .await
        .expect("send post-resume query");
    let (resumed_reply, _) = pane2
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("post-resume diagnosis");
    assert!(resumed_reply.contains("Resumed with slept command delta"));

    let final_conv = ws
        .llm
        .main_requests()
        .last()
        .expect("final post-resume req")
        .conversation_text();
    assert!(
        final_conv.contains("[WHILE_YOU_SLEPT]")
            && final_conv.contains("$ cargo test --lib (exit: 0)"),
        "Hard Wake [WHILE_YOU_SLEPT] summary must persist to SQLite across /resume:\n{final_conv}"
    );
    assert!(
        final_conv.contains("pub fn legacy_adapter() -> bool"),
        "mid-turn salvaged file_read before SleepToggle(true) must persist across /resume:\n{final_conv}"
    );

    pane1.shutdown().await;
    pane2.shutdown().await;
    ws.teardown().await;
}

#[tokio::test]
#[serial(boxxy_e2e)]
async fn small_context_window_threshold_and_few_turn_high_pressure_compaction_workflow() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    ws.write_file("src/alpha.rs", "pub fn alpha() -> u32 {\n    1\n}\n")
        .await
        .expect("write alpha.rs");
    ws.write_file("src/beta.rs", "pub fn beta() -> u32 {\n    2\n}\n")
        .await
        .expect("write beta.rs");

    // Register a synthetic `16,000`-token local model (`mock-small-16k`) to verify:
    //   1) `4,000` context tokens (`25% < 75% = 12,000`) NEVER triggers premature compaction even
    //      after 7+ exchanges (fixing the old `saturating_add(16_000) >= 16_000` local-model bug).
    //   2) Terminal output containing `BOXXY_DEBUG_CONTEXT` `[TERMINAL_SNAPSHOT: EMPTY]` log lines
    //      is preserved inside the 4-turn freshness window.
    //   3) A heavy multi-tool session hitting `14,500` tokens (`90.6% >= 85%`) in just `3` exchanges
    //      (`<= 6` exchanges!) safely compacts at an exchange boundary and scales `context_tokens`.
    let small_llm = MockLlmServer::start().await.expect("small mock server");
    let small_ep_name = format!("SmallWindowEndpoint-{}", Uuid::new_v4());
    let mut small_ep = OpenAiCompatibleEndpoint::preset_local_server();
    small_ep.name.clone_from(&small_ep_name);
    small_ep.base_url = small_llm.base_url();
    small_ep.models = vec![CustomModelConfig {
        id: "mock-small-16k".to_string(),
        display_name: "Mock Small 16k Local".to_string(),
        route: OpenAiApiRoute::ChatCompletions,
        context_window: Some(16_000),
        thinking_levels: Vec::new(),
        input_cost_per_1m: None,
        output_cost_per_1m: None,
    }];
    Settings::update(|s| {
        s.openai_compatible_endpoints.push(small_ep.clone());
        s.claw_model = Some(ModelProvider::OpenAiCompatible {
            provider_name: small_ep_name.clone(),
            model_id: "mock-small-16k".to_string(),
            thinking: None,
        });
    });

    // --- Part 1: 8 exchanges at `4,000 / 16,000` (`25%`) on `mock-small-16k` ---
    let mut pane_light = ws
        .spawn_pane("pane-small-light", 0)
        .await
        .expect("spawn light pane");
    pane_light.wait_ready(TIMEOUT).await.expect("light ready");
    pane_light
        .set_terminal_snapshot(
            "[2026 INFO model-context]\nTerminal Snapshot:\n```\n[TERMINAL_SNAPSHOT: EMPTY]\n```\nthread 'main' panicked at src/alpha.rs:2\n",
        )
        .await;

    for turn_num in 1..=8 {
        small_llm.push_step(
            MockStep::text(format!("Light turn {turn_num} ok.")).with_usage(3_900, 100),
        );
    }

    for turn_num in 1..=8 {
        pane_light
            .send_query(format!("Light step {turn_num}: check status."))
            .await
            .expect("send light turn");
        pane_light
            .wait_for_diagnosis(TIMEOUT)
            .await
            .expect("light turn done");
    }

    let light_reqs = small_llm.main_requests();
    assert_eq!(light_reqs.len(), 8);
    assert!(
        light_reqs[1]
            .conversation_text()
            .contains("thread 'main' panicked at src/alpha.rs:2"),
        "Turn 2 must retain Turn 1's real terminal crash output even when BOXXY_DEBUG_CONTEXT logs contain `[TERMINAL_SNAPSHOT: EMPTY]`:\n{}",
        light_reqs[1].conversation_text()
    );
    assert!(
        !light_reqs[7]
            .conversation_text()
            .contains("[SESSION_CHECKPOINT]"),
        "16k local model at 4,000 tokens (25% < 75%) must NOT compact prematurely across 8 short turns:\n{}",
        light_reqs[7].conversation_text()
    );
    pane_light.shutdown().await;

    // --- Part 2: High-pressure few-turn compaction (`3` exchanges hitting `14,500 / 16,000` = `90.6%`) ---
    let mut pane_heavy = ws
        .spawn_pane("pane-small-heavy", 1)
        .await
        .expect("spawn heavy pane");
    pane_heavy.wait_ready(TIMEOUT).await.expect("heavy ready");

    small_llm.push_steps([
        // Exchange 1 (multi-tool loop reading `src/alpha.rs` and `src/beta.rs`):
        MockStep::tool_call("file_read", &json!({ "path": "src/alpha.rs" }))
            .with_usage(4_000, 100),
        MockStep::tool_call("file_read", &json!({ "path": "src/beta.rs" }))
            .with_usage(6_500, 100),
        MockStep::text("Exchange 1 complete: audited alpha.rs and beta.rs.")
            .with_usage(7_500, 200),
        // Exchange 2:
        MockStep::text("Exchange 2 complete: verified module boundaries.")
            .with_usage(10_000, 200),
        // Exchange 3: reaches `14,300 + 200 = 14,500` tokens (`90.6% >= 85%` of `16,000` with only 3 exchanges!):
        MockStep::text("Exchange 3 high pressure triggers fallback safe exchange cut.")
            .with_usage(14_300, 200),
        // Exchange 4 (post-compaction):
        MockStep::text("Exchange 4 post-compaction reply.").with_usage(3_200, 100),
    ]);

    pane_heavy
        .send_query("Exchange 1: inspect `src/alpha.rs` and `src/beta.rs`.")
        .await
        .expect("send ex 1");
    pane_heavy
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("ex 1 done");

    pane_heavy
        .send_query("Exchange 2: check module boundaries.")
        .await
        .expect("send ex 2");
    pane_heavy
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("ex 2 done");

    pane_heavy
        .send_query("Exchange 3: push context above 85% in 3 exchanges.")
        .await
        .expect("send ex 3");
    pane_heavy
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("ex 3 done");

    // Inspect SQLite right after Exchange 3's compaction: `context_tokens` must be scaled down (`> 0` and `<= 60% = 9,600`)!
    let store = Store::new(ws.db.pool());
    let sess_after_ex3 = store
        .get_session(pane_heavy.session_id())
        .await
        .expect("query heavy session")
        .expect("heavy session row");
    assert!(
        sess_after_ex3.context_tokens > 0 && sess_after_ex3.context_tokens <= 9_600,
        "post-compaction context_tokens must scale below 75% while staying > 0, got: {}",
        sess_after_ex3.context_tokens
    );

    pane_heavy
        .send_query("Exchange 4: verify checkpoint after 3-exchange high-pressure compaction.")
        .await
        .expect("send ex 4");
    pane_heavy
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("ex 4 done");

    let all_small_reqs = small_llm.main_requests();
    let ex4_req = all_small_reqs.last().expect("ex 4 req");
    let ex4_conv = ex4_req.conversation_text();
    assert!(
        ex4_conv.contains("[SESSION_CHECKPOINT]")
            && ex4_conv.contains("file_read(src/alpha.rs)")
            && ex4_conv.contains("file_read(src/beta.rs)"),
        "high-pressure <= 6-exchange compaction must summarize Exchange 1 tool loop into [SESSION_CHECKPOINT]:\n{ex4_conv}"
    );
    assert!(
        ex4_conv.contains("Exchange 2 complete: verified module boundaries.")
            && ex4_conv.contains("Exchange 3 high pressure triggers fallback safe exchange cut."),
        "fallback `find_safe_exchange_cut(..., 2)` must preserve the 2 newest exchanges intact:\n{ex4_conv}"
    );

    pane_heavy.shutdown().await;
    ws.teardown().await;
}
