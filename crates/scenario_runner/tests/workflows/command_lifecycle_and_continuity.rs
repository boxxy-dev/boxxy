//! E2E Workflow 17:
//! - `InjectCommand` Lifecycle Hygiene: Proposal-state `/btw` (`[Pending Command Proposal Awaiting User Approval]`),
//!   0-token ghost dismissal on unrelated user-typed commands (`dismissed without running`),
//!   accepted `InjectCommand` (`TerminalCommandAccepted`) immunity against false `"rejected by the user"`
//!   poisoning during concurrent follow-ups, and durable post-`/undo` + post-rejection SQLite
//!   `sessions.history_json` continuity across `/resume`.
//! - Scheduled `Command` Task Execution (`tick_scheduled_tasks` -> `ScheduledTaskTriggered`),
//!   `set_global_intent` (`GLOBAL WORKSPACE INTENT`) + `memory_delete`, `[SILENT_ACK]` (`DismissDrawer` +
//!   queue parking), `[IMAGE GENERATION AVAILABILITY]` teammate discovery, live character personality
//!   hot-reload (`SettingsInvalidated`), and explicit `Deactivate` history flushing.

use boxxy_claw_protocol::characters::{CLAIMS_CACHE, update_caches};
use boxxy_claw_protocol::{ClawEngineEvent, ClawMessage};
use boxxy_db::store::Store;
use boxxy_model_selection::{GeminiModel, ModelProvider, ThinkingLevel};
use boxxy_preferences::Settings;
use scenario_runner::{MockStep, TestWorkspace, TurnGate};
use serde_json::{Value, json};
use serial_test::serial;
use std::time::Duration;
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
#[serial(boxxy_e2e)]
async fn inject_command_accept_vs_ghost_dismiss_btw_and_post_undo_reject_resume_workflow() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    let initial_limits = "pub fn max_batch() -> usize {\n    16\n}\n";
    ws.write_file("src/limits.rs", initial_limits)
        .await
        .expect("write limits.rs");

    let mut pane1 = ws
        .spawn_pane("pane-cmd-life-1", 0)
        .await
        .expect("spawn pane 1");
    pane1.wait_ready(TIMEOUT).await.expect("ready 1");
    let session_id = pane1.session_id().to_string();

    // --- 1. Turn 1: Agent proposes `InjectCommand` (`cargo build --release`) ---
    ws.llm.push_step(MockStep::text(
        "Build the release artifact:\n```bash\ncargo build --release\n```",
    ));
    pane1
        .send_query("Propose building the release artifact.")
        .await
        .expect("send turn 1");

    pane1
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::InjectCommand {
                command,
                auto_approve: false,
                ..
            } if command.trim() == "cargo build --release" => Some(()),
            _ => None,
        })
        .await
        .expect("Turn 1 InjectCommand");

    // 1a. Ask `/btw` while the unaccepted `InjectCommand` card is sitting on screen:
    //     `/btw` must receive `[Pending Command Proposal Awaiting User Approval: ... (not yet executed)]`
    //     while preserving the warm `ClawAgent` tool-schema prefix!
    ws.llm.push_btw_step(
        MockStep::text("Side answer: `cargo build --release` is still awaiting your approval.")
            .with_usage(450, 30),
    );
    let btw_id = pane1
        .send_btw("Has `cargo build --release` executed yet, or is it waiting on me?")
        .await
        .expect("send proposal-state /btw");

    let btw_ans = pane1
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
    assert!(btw_ans.contains("awaiting your approval"));

    let btw_reqs = ws.llm.btw_requests();
    assert_eq!(btw_reqs.len(), 1);
    assert!(
        !btw_reqs[0].tools.is_empty(),
        "proposal-state /btw must reuse warm ClawAgent tool schemas"
    );
    assert!(
        btw_reqs[0].conversation_text().contains(
            "[Pending Command Proposal Awaiting User Approval: `cargo build --release` (not yet executed)]"
        ),
        "proposal-state /btw must explicitly indicate the command proposal has not run yet:\n{}",
        btw_reqs[0].conversation_text()
    );

    // 1b. User ignores the unaccepted `InjectCommand` and manually types an unrelated failing command
    //     (`cargoo fmt`, exit 127) in the terminal ("ghost dismiss"):
    //     MUST emit `LazyErrorIndicator` at 0 LLM tokens (NOT false auto-diagnosis!) and record
    //     `[Proposed command `cargo build --release` — dismissed without running]`!
    let reqs_before_ghost = ws.llm.main_requests().len();
    pane1
        .notify_command_finished(127, "cargoo fmt\nbash: cargoo: command not found\n")
        .await
        .expect("unrelated user command failure");

    pane1
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::LazyErrorIndicator { visible: true, .. } => Some(()),
            _ => None,
        })
        .await
        .expect("ghost-dismissing unaccepted InjectCommand with user typo must emit LazyErrorIndicator");
    assert_eq!(
        ws.llm.main_requests().len(),
        reqs_before_ghost,
        "unrelated user failure over unaccepted InjectCommand must spend 0 LLM calls"
    );

    // --- 2. Turn 2: Propose `cargo test --workspace`, click "Accept & Run" (`TerminalCommandAccepted`),
    //        and send a follow-up query while it is running! ---
    ws.llm.push_steps([
        MockStep::text("Run workspace tests:\n```bash\ncargo test --workspace\n```"),
        MockStep::text("While `cargo test --workspace` runs, `src/limits.rs` looks ready."),
    ]);
    pane1
        .send_query("Propose running workspace tests instead.")
        .await
        .expect("send turn 2");

    pane1
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::InjectCommand {
                command,
                auto_approve: false,
                ..
            } if command.trim() == "cargo test --workspace" => Some(()),
            _ => None,
        })
        .await
        .expect("Turn 2 InjectCommand");

    // User clicks "Accept & Run" in the UI:
    pane1
        .send(ClawMessage::TerminalCommandAccepted {
            target_pane_id: None,
        })
        .await
        .expect("accept InjectCommand");

    let cursor_before_followup = pane1.advance_cursor_to_end();
    pane1
        .send_query("While that runs, quick check on `src/limits.rs`?")
        .await
        .expect("send follow-up while accepted command runs");
    pane1
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("turn 3 diagnosis");

    assert!(
        !pane1
            .events_since(cursor_before_followup)
            .iter()
            .any(|ev| matches!(
                ev,
                ClawEngineEvent::ProposalResolved {
                    approved: false,
                    ..
                }
            )),
        "sending a follow-up after accepting InjectCommand must NOT emit a false rejection event"
    );

    let turn3_conv = ws
        .llm
        .main_requests()
        .last()
        .expect("turn 3 req")
        .conversation_text();
    assert!(
        turn3_conv
            .contains("[Proposed command `cargo build --release` — dismissed without running]"),
        "ghost-dismissed Turn 1 command must be annotated as dismissed without running:\n{turn3_conv}"
    );
    assert!(
        !turn3_conv.contains("[Proposed command `cargo test --workspace` — rejected by the user]"),
        "accepted Turn 2 InjectCommand must NEVER be poisoned as rejected by the user:\n{turn3_conv}"
    );

    // --- 3. Turn 4 (`replace_in_file` -> approve -> `/undo`) + Turn 5 (`replace_in_file` -> Reject)
    //        followed by cross-pane `/resume` onto `pane2`! ---
    ws.llm.push_steps([
        MockStep::tool_call(
            "replace_in_file",
            &json!({
                "path": "src/limits.rs",
                "old_string": "    16",
                "new_string": "    32"
            }),
        ),
        MockStep::text("Updated `max_batch()` to `32`."),
        MockStep::tool_call(
            "replace_in_file",
            &json!({
                "path": "src/limits.rs",
                "old_string": "    16",
                "new_string": "    64"
            }),
        ),
        MockStep::text("Post-resume confirmation: `max_batch()` remains `16` after `/undo` and rejection."),
    ]);

    pane1
        .send_query("Update @src/limits.rs to return 32.")
        .await
        .expect("send turn 4");
    pane1
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::ProposeFileWrite {
                content,
                auto_approve: false,
                ..
            } if content.contains("    32") => Some(()),
            _ => None,
        })
        .await
        .expect("Turn 4 ProposeFileWrite (32)");
    pane1
        .reply_file_write(true)
        .await
        .expect("approve 32 edit");
    pane1
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("turn 4 done");
    assert!(ws.read_file("src/limits.rs").await.unwrap().contains("    32"));

    // Revert Turn 4 via `/undo` (0 tokens) — must persist `/undo` exchange into SQLite `history_json`!
    pane1.send_query("/undo").await.expect("send /undo");
    pane1
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::SystemMessage { text } if text.contains("Reverted file changes") => {
                Some(())
            }
            _ => None,
        })
        .await
        .expect("/undo SystemMessage");
    assert_eq!(ws.read_file("src/limits.rs").await.unwrap(), initial_limits);

    // Turn 5: Propose `64`, then explicitly REJECT (`FileWriteReply { approved: false }`, 0 extra tokens)!
    pane1
        .send_query("Try updating @src/limits.rs to 64.")
        .await
        .expect("send turn 5");
    pane1
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::ProposeFileWrite {
                content,
                auto_approve: false,
                ..
            } if content.contains("    64") => Some(()),
            _ => None,
        })
        .await
        .expect("Turn 5 ProposeFileWrite (64)");
    pane1
        .reply_file_write(false)
        .await
        .expect("reject 64 edit");
    pane1
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::ProposalResolved {
                approved: false, ..
            } => Some(()),
            _ => None,
        })
        .await
        .expect("Turn 5 ProposalResolved(false)");

    // Spawn `pane2` and `/resume` `session_id` immediately without running another turn on `pane1`!
    let mut pane2 = ws
        .spawn_pane("pane-cmd-life-2", 0)
        .await
        .expect("spawn pane 2");
    pane2.wait_ready(TIMEOUT).await.expect("ready 2");
    pane2
        .send(ClawMessage::ResumeSession {
            session_id: session_id.clone(),
            character_id: None,
        })
        .await
        .expect("resume session on pane 2");

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
        .send_query("What is the current state of `src/limits.rs` after my undo and rejection?")
        .await
        .expect("send post-resume query on pane 2");
    let (resumed_ans, _) = pane2
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("post-resume diagnosis");
    assert!(resumed_ans.contains("remains `16`"));

    // Verify `pane2`'s outgoing LLM payload preserved BOTH `/undo` AND the rejected edit note,
    // with strict bidirectional `user`/`assistant` role alternation!
    let all_reqs = ws.llm.main_requests();
    let resumed_req = all_reqs.last().expect("post-resume request");
    let resumed_conv = resumed_req.conversation_text();
    assert!(
        resumed_conv.contains("[System: The user ran /undo to revert file changes from the last turn.]")
            && resumed_conv.contains("Reverted file changes from the last turn"),
        "post-resume LLM context must retain `/undo` revert exchange from SQLite:\n{resumed_conv}"
    );
    assert!(
        resumed_conv.contains("replace_in_file")
            && resumed_conv.contains("rejected by the user"),
        "post-resume LLM context must retain explicit proposal rejection note from SQLite:\n{resumed_conv}"
    );

    let non_sys_roles: Vec<&str> = resumed_req
        .messages
        .iter()
        .filter_map(|m| m.get("role").and_then(Value::as_str))
        .filter(|&r| r != "system" && r != "developer")
        .collect();
    for pair in non_sys_roles.windows(2) {
        if pair[0] != "tool" && pair[1] != "tool" {
            assert_ne!(
                pair[0], pair[1],
                "post-resume history after /undo and rejection must strictly alternate roles: {non_sys_roles:?}"
            );
        }
    }

    pane1.shutdown().await;
    pane2.shutdown().await;
    ws.teardown().await;
}

#[tokio::test]
#[serial(boxxy_e2e)]
async fn scheduled_command_firing_global_intent_memory_delete_silent_ack_and_character_hot_reload_workflow()
 {
    let mut ws = TestWorkspace::new().await.expect("workspace");

    // Assign `Kuro` (catalog index 2) an image-capable Gemini model with a synthetic test key so
    // text-only `Niko Una` (index 0) discovers `Kuro (@kuro, Gemini)` in `[IMAGE GENERATION AVAILABILITY]`.
    let kuro_id = scenario_runner::config_env::seeded_catalog()[2]
        .config
        .id
        .clone();
    Settings::update(|s| {
        s.api_keys
            .insert("Gemini".to_string(), "gem-synthetic-e2e-key".to_string());
        s.character_models.insert(
            kuro_id.clone(),
            ModelProvider::Gemini(GeminiModel::Flash3_8, Some(ThinkingLevel::Low)),
        );
    });

    // Seed a global preference memory `obsolete_flag` that the agent will prune via `memory_delete`.
    let db = ws.db.clone();
    let store = Store::new(db.pool());
    store
        .add_memory(
            "obsolete_flag",
            None,
            "Pass --legacy-pipeline-v1 to build scripts",
            Some("preference"),
            true,
            true,
        )
        .await
        .expect("seed obsolete_flag memory");

    let mut pane1 = ws
        .spawn_pane("pane-sched-1", 0)
        .await
        .expect("spawn pane 1");
    pane1.wait_ready(TIMEOUT).await.expect("ready 1");

    let mut peer_pane = ws
        .spawn_pane("pane-sched-peer", 1)
        .await
        .expect("spawn peer pane");
    peer_pane.wait_ready(TIMEOUT).await.expect("peer ready");

    // --- 1. Turn 1 on `pane1`: delete `obsolete_flag`, broadcast `set_global_intent`, and schedule
    //        an immediate (`due_in_seconds: 0`) `command` task! ---
    ws.llm.push_aux_text("Pipeline Intent And Cleanup");
    ws.llm.push_steps([
        MockStep::tool_call("memory_delete", &json!({ "key": "obsolete_flag" })),
        MockStep::tool_call(
            "set_global_intent",
            &json!({ "intent": "Stabilizing workspace release pipeline v2" }),
        ),
        MockStep::tool_call(
            "schedule_task",
            &json!({
                "due_in_seconds": 0,
                "task_type": "command",
                "payload": "Run cargo check --workspace"
            }),
        ),
        MockStep::text("Deleted obsolete memory, set global intent, and scheduled workspace check."),
        // Consumed automatically when `tick_scheduled_tasks` fires `[SCHEDULED COMMAND TRIGGERED]`:
        MockStep::text("Scheduled check triggered:\n```bash\ncargo check --workspace\n```"),
    ]);

    pane1
        .send_query("Remove `obsolete_flag` from memory, broadcast release v2 intent, and schedule a workspace check now.")
        .await
        .expect("send turn 1");
    pane1
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("turn 1 diagnosis");

    // Audit Turn 1 SYSTEM PROMPT: must advertise ready image-capable peer `Kuro (@kuro, Gemini)`!
    let t1_sys = ws.llm.main_requests()[0].system_prompt_text();
    assert!(
        t1_sys.contains("[IMAGE GENERATION AVAILABILITY]")
            && t1_sys.contains("Kuro (`@kuro`, Gemini)"),
        "text-only character must discover ready image-capable peer in SYSTEM PROMPT:\n{t1_sys}"
    );

    // Wait deterministically for the 1-second background scheduler tick to trigger the scheduled
    // `Command` task and surface an approval-gated `InjectCommand` (`auto_approve: false`)!
    let scheduled_cmd = pane1
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::InjectCommand {
                command,
                auto_approve: false,
                ..
            } if command.trim() == "cargo check --workspace" => Some(command.clone()),
            _ => None,
        })
        .await
        .expect("scheduled command task fired and emitted approval-gated InjectCommand");
    assert_eq!(scheduled_cmd.trim(), "cargo check --workspace");

    pane1
        .send(ClawMessage::CancelPending)
        .await
        .expect("dismiss scheduled command proposal");
    pane1
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::ProposalResolved {
                approved: false, ..
            } => Some(()),
            _ => None,
        })
        .await
        .expect("scheduled proposal resolved");

    // --- 2. Run a turn on `peer_pane` (`Levi Kujo`): verify `obsolete_flag` was deleted AND
    //        `--- GLOBAL WORKSPACE INTENT ---` appears in `GLOBAL RADAR`! ---
    ws.llm
        .push_step(MockStep::text("Peer confirmed v2 release intent and clean memory."));
    peer_pane
        .send_query("Check current workspace intent and build flags.")
        .await
        .expect("send peer query");
    peer_pane
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("peer diagnosis");

    let peer_conv = ws
        .llm
        .main_requests()
        .last()
        .expect("peer req")
        .conversation_text();
    assert!(
        !peer_conv.contains("obsolete_flag") && !peer_conv.contains("--legacy-pipeline-v1"),
        "memory_delete must remove obsolete_flag from subsequent turn recall:\n{peer_conv}"
    );
    assert!(
        peer_conv.contains("--- GLOBAL WORKSPACE INTENT ---")
            && peer_conv.contains("Stabilizing workspace release pipeline v2"),
        "set_global_intent must broadcast intent across all panes via GLOBAL RADAR:\n{peer_conv}"
    );

    // --- 3. Test `[SILENT_ACK]` (`DismissDrawer` + queued follow-up parking) ---
    let silent_gate = TurnGate::new();
    ws.llm.push_step(MockStep::gated(
        silent_gate.clone(),
        MockStep::text("[SILENT_ACK]"),
    ));
    pane1
        .send_query("Acknowledge rejection silently.")
        .await
        .expect("send silent ack turn");
    silent_gate.wait_entered().await;

    let queued_id = Uuid::new_v4();
    pane1
        .send_query_with_id(queued_id, "Queued follow-up behind silent ack.")
        .await
        .expect("enqueue behind silent ack");
    pane1
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::QueueUpdated {
                items,
                paused: false,
            } if items.len() == 1 => Some(()),
            _ => None,
        })
        .await
        .expect("queued behind silent ack");

    silent_gate.release();

    pane1
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::DismissDrawer => Some(()),
            _ => None,
        })
        .await
        .expect("[SILENT_ACK] must emit DismissDrawer");
    pane1
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::QueueUpdated {
                items,
                paused: true,
            } if items.len() == 1 && items[0].id == queued_id => Some(()),
            _ => None,
        })
        .await
        .expect("[SILENT_ACK] must park queued follow-up prompts");
    pane1
        .send(ClawMessage::ClearQueuedPrompts)
        .await
        .expect("clear parked queue");

    // --- 4. Live Character Personality Hot-Reload (`CHARACTER_CACHE` + `SettingsInvalidated`)
    //        followed by explicit `Deactivate` lifecycle flush ---
    let mut updated_catalog = scenario_runner::config_env::seeded_catalog().to_vec();
    updated_catalog[0].config.personality =
        "HOT_RELOADED_PERSONALITY_SENTINEL: speak like a calm distributed-systemsverifier."
            .to_string();
    let current_claims = (**CLAIMS_CACHE.load()).clone();
    update_caches(updated_catalog, current_claims, 999_999);

    pane1
        .send(ClawMessage::SettingsInvalidated)
        .await
        .expect("notify settings invalidated for character hot-reload");

    ws.llm
        .push_step(MockStep::text("Hot-reloaded character personality active."));
    pane1
        .send_query("Verify your updated character instructions.")
        .await
        .expect("send post-hot-reload query");
    pane1
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("post-hot-reload diagnosis");

    let hot_sys = ws
        .llm
        .main_requests()
        .last()
        .expect("post-hot-reload req")
        .system_prompt_text();
    assert!(
        hot_sys.contains("HOT_RELOADED_PERSONALITY_SENTINEL"),
        "SettingsInvalidated must hot-reload updated character personality on the very next turn:\n{hot_sys}"
    );

    // Wait for Turn-1 title generation to finish before queueing the `Deactivate` flush response.
    let title_deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        if let Ok(Some(sess)) = store.get_session(pane1.session_id()).await
            && sess.title.as_deref() == Some("Pipeline Intent And Cleanup")
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < title_deadline,
            "timed out waiting for Turn-1 title generation"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Explicit `Deactivate` (pane close) must flush unflushed history to `mock-memory-model`!
    ws.llm.push_aux_text(
        "{\"facts\":[{\"key\":\"release_pipeline_ver\",\"content\":\"Uses release pipeline v2\"}],\"summary\":\"Stabilized workspace release pipeline v2.\"}",
    );
    pane1
        .send(ClawMessage::Deactivate)
        .await
        .expect("send explicit Deactivate");

    let ws_scope = ws.path().to_string_lossy().into_owned();
    let flush_deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        if let Ok(Some(cand)) = store
            .get_memory_by_key("release_pipeline_ver", Some(&ws_scope))
            .await
        {
            assert_eq!(cand.content, "Uses release pipeline v2");
            break;
        }
        assert!(
            tokio::time::Instant::now() < flush_deadline,
            "explicit Deactivate must flush unflushed session history"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    pane1.shutdown().await;
    peer_pane.shutdown().await;
    ws.teardown().await;
}
