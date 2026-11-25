//! E2E Workflow 16:
//! - Interactive `terminal_exec` Foreground Handoff (`CompleteRunningTerminalCommand` after 800ms
//!   when an accepted command stays running in the terminal like `vim` / `htop`, spending 0 extra
//!   LLM calls and parking queued follow-ups) + RAII `InFlightClearGuard` ensuring `/stop` mid-turn
//!   never leaks stale `[Working on the current turn…]` context into subsequent idle `/btw` queries.
//! - Nested 4-backtick code fence isolation (preventing false `@file` preloads and false `:character`
//!   roundtable triggers inside fenced examples) + Visual Novel `/drama` mode (`typewriter: true`),
//!   mid-roundtable `Abort` (`pause_for_interjection`), and user interjection roundtable resumption.

use boxxy_claw_protocol::{AgentStatus, ClawEngineEvent, ClawMessage};
use boxxy_db::store::Store;
use scenario_runner::{MockStep, TestWorkspace, TurnGate};
use serde_json::json;
use serial_test::serial;
use std::time::Duration;
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
#[serial(boxxy_e2e)]
async fn interactive_terminal_handoff_and_post_stop_btw_context_hygiene_workflow() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    ws.write_file("src/editor.rs", "pub fn open_buffer() -> bool {\n    true\n}\n")
        .await
        .expect("write editor.rs");

    let mut pane = ws
        .spawn_pane("pane-interactive-1", 0)
        .await
        .expect("spawn pane");
    pane.wait_ready(TIMEOUT).await.expect("ready");

    // --- 1. Interactive `terminal_exec` handoff (`vim` stays running in foreground) ---
    ws.llm.push_aux_text("Open Editor Buffer");
    ws.llm.push_steps([
        MockStep::tool_call(
            "terminal_exec",
            &json!({
                "command": "vim src/editor.rs",
                "explanation": "Open src/editor.rs interactively"
            }),
        ),
        // NOTE: No follow-up text step is queued for Turn 1! `CompleteRunningTerminalCommand`
        // MUST yield control directly to the terminal after ~800ms with 0 extra LLM calls!
    ]);

    pane.send_query("Open `src/editor.rs` in vim for me.")
        .await
        .expect("send turn 1");

    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::ProposeTerminalCommand {
            command,
            auto_approve: false,
            target_pane_id: None,
            ..
        } if command == "vim src/editor.rs" => Some(()),
        _ => None,
    })
    .await
    .expect("ProposeTerminalCommand for vim");

    // Enqueue a follow-up prompt before accepting, then mark foreground process as `vim` and accept!
    // Because `CompleteRunningTerminalCommand` hands interactive control to `vim`, the queued prompt
    // MUST park (`paused: true`) rather than auto-firing over the running TUI!
    let parked_id = Uuid::new_v4();
    pane.send(ClawMessage::ForegroundProcessChanged {
        process_name: "vim".to_string(),
    })
    .await
    .expect("set foreground vim");
    pane.send(ClawMessage::TerminalCommandAccepted {
        target_pane_id: None,
    })
    .await
    .expect("accept terminal_exec");
    pane.send_query_with_id(parked_id, "After I close vim, check formatting.")
        .await
        .expect("enqueue behind interactive terminal_exec");

    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::QueueUpdated {
            items,
            paused: true,
        } if items.len() == 1 && items[0].id == parked_id => Some(()),
        _ => None,
    })
    .await
    .expect("interactive terminal_exec handoff (~800ms) completed and parked queued follow-up");

    assert_eq!(
        ws.llm.main_requests().len(),
        1,
        "CompleteRunningTerminalCommand must yield to interactive TUI with 0 follow-up LLM calls"
    );

    // Clear foreground process & parked queue, and verify SQLite persisted the interactive execution note.
    pane.send(ClawMessage::ForegroundProcessChanged {
        process_name: String::new(),
    })
    .await
    .expect("clear foreground vim");
    pane.send(ClawMessage::ClearQueuedPrompts)
        .await
        .expect("clear parked queue");

    let store = Store::new(ws.db.pool());
    let persist_deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        if let Ok(Some(sess)) = store.get_session(pane.session_id()).await
            && sess
                .history_json
                .as_deref()
                .is_some_and(|h| h.contains("command is running interactively in the terminal (`vim`)"))
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < persist_deadline,
            "timed out waiting for CompleteRunningTerminalCommand history persistence"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // --- 2. Start a multi-step Turn 2 (`file_read` -> `TurnGate`), abort via `/stop`, and verify
    //        RAII `InFlightClearGuard` cleared `in_flight_snapshot` before an idle `/btw` query! ---
    let stop_gate = TurnGate::new();
    ws.llm.push_steps([
        MockStep::tool_call("file_read", &json!({ "path": "src/editor.rs" })),
        MockStep::gated(stop_gate.clone(), MockStep::text("Aborted before completion")),
    ]);

    pane.send_query("Read `src/editor.rs` and prepare a refactor.")
        .await
        .expect("start turn 2");
    stop_gate.wait_entered().await;

    // Abort the gated turn via `/stop` (goes through `cancel_active_turn_and_proposals`, NOT `handle_abort`!).
    pane.send_query("/stop").await.expect("send /stop");
    stop_gate.release();

    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::SystemMessage { text } if text.contains("Stopped") => Some(()),
        _ => None,
    })
    .await
    .expect("/stop SystemMessage");
    pane.wait_for_status(&AgentStatus::Waiting, TIMEOUT)
        .await
        .expect("returned to Waiting after /stop");

    // Update terminal snapshot while idle and send `/btw`:
    // If `in_flight_snapshot` had leaked from the aborted turn, `/btw` would falsely emit
    // `[Working on the current turn…]` and omit the live changed Terminal Snapshot!
    pane.set_terminal_snapshot("$ git status --short\n M src/editor.rs\n")
        .await;
    ws.llm.push_btw_step(
        MockStep::text("Idle /btw after /stop: `src/editor.rs` is modified in `git status`.")
            .with_usage(500, 30),
    );

    let btw_id = pane
        .send_btw("What does `git status` show on my terminal right now?")
        .await
        .expect("send idle /btw after /stop");
    let btw_reply = pane
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
        .expect("BtwComplete after /stop");
    assert!(btw_reply.contains("src/editor.rs"));

    let btw_reqs = ws.llm.btw_requests();
    assert_eq!(btw_reqs.len(), 1);
    let btw_conv = btw_reqs[0].conversation_text();
    assert!(
        !btw_conv.contains("[Working on the current turn"),
        "RAII InFlightClearGuard must clear in_flight_snapshot when a turn is aborted via /stop:\n{btw_conv}"
    );
    assert!(
        btw_conv.contains("M src/editor.rs") && btw_conv.contains("CWD: `"),
        "idle /btw after /stop must include live changed Terminal Snapshot and CWD:\n{btw_conv}"
    );
    assert!(
        btw_conv.contains("command is running interactively in the terminal (`vim`)"),
        "settled history from CompleteRunningTerminalCommand must remain visible in context:\n{btw_conv}"
    );

    pane.shutdown().await;
    ws.teardown().await;
}

#[tokio::test]
#[serial(boxxy_e2e)]
async fn nested_fences_drama_mode_stop_and_interjection_resumption_workflow() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    ws.write_file(
        "src/unreferenced.rs",
        "pub const SHOULD_NEVER_PRELOAD_FROM_FENCED_EXAMPLE: u8 = 9;\n",
    )
    .await
    .expect("write unreferenced.rs");

    let mut pane = ws.spawn_pane("pane-drama-1", 0).await.expect("spawn pane");
    pane.wait_ready(TIMEOUT).await.expect("ready");

    // --- 1. Nested 4-backtick markdown fence containing inner 3-backtick code block with
    //        `@src/unreferenced.rs` and `:kuro` MUST NOT preload the file or invite Kuro! ---
    ws.llm
        .push_step(MockStep::text("Reviewed your nested markdown example."));
    let nested_fenced_prompt = "Check how this nested markdown renders:\n````markdown\nExample prompt:\n```text\nInspect @src/unreferenced.rs with :kuro please!\n```\n````";
    pane.send_query(nested_fenced_prompt)
        .await
        .expect("send nested fenced prompt");
    pane.wait_for_diagnosis(TIMEOUT)
        .await
        .expect("turn 1 diagnosis");

    assert_eq!(
        ws.llm.main_requests().len(),
        1,
        ":kuro inside nested 4-backtick code fence must not trigger a multi-character roundtable"
    );
    let t1_conv = ws.llm.main_requests()[0].conversation_text();
    assert!(
        !t1_conv.contains("SHOULD_NEVER_PRELOAD_FROM_FENCED_EXAMPLE")
            && !t1_conv.contains("REFERENCED FILES"),
        "@file inside nested 4-backtick code fence must never preload disk contents:\n{t1_conv}"
    );

    // --- 2. Start Visual Novel `/drama :levi-kujo`, verify `typewriter: true`, pause mid-roundtable
    //        via `Abort` (`pause_for_interjection`), and resume with a user interjection! ---
    let levi_gate = TurnGate::new();
    ws.llm.push_steps([
        // Opening Drama line by Host (`Niko Una`):
        MockStep::text("Spaces align everywhere without tab-width surprises, @levi-kujo!"),
        // Gated Round 1 by Guest (`Levi Kujo`) — interrupted by `Abort`:
        MockStep::gated(levi_gate.clone(), MockStep::text("Interrupted guest line")),
        // After user interjects `"What about rustfmt defaults for 1 turn?"`:
        // Host (`Niko Una`) speaks first:
        MockStep::text("`rustfmt` defaults to 4 spaces across the entire Rust ecosystem, @levi-kujo!"),
        // Autonomous Round 1 (1/1 final turn) by Guest (`Levi Kujo`):
        MockStep::text("Agreed — letting `rustfmt` enforce 4 spaces settles the debate cleanly."),
    ]);

    pane.send_query("/drama :levi-kujo Spaces vs tabs")
        .await
        .expect("start /drama roundtable");

    let (drama_opening, is_typewriter) = pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::DiagnosisComplete {
                diagnosis,
                typewriter,
                ..
            } => Some((diagnosis.clone(), *typewriter)),
            _ => None,
        })
        .await
        .expect("Host drama opening");
    assert!(drama_opening.contains("Spaces align"));
    assert!(
        is_typewriter,
        "/drama mode must set `typewriter: true` on DiagnosisComplete"
    );

    // Wait until Guest (`Levi Kujo`) enters his turn (after Drama typewriter pacing), then `Abort`!
    levi_gate.wait_entered().await;
    pane.send(ClawMessage::Abort)
        .await
        .expect("pause roundtable via Abort");
    levi_gate.release();

    // User interjects a follow-up question (`for 1 turn`) — `resume_after_interjection` must
    // automatically queue Host (`Niko Una`) + Guest (`Levi Kujo`) in Drama Mode!
    pane.send_query("What about rustfmt defaults for 1 turn?")
        .await
        .expect("interject into paused roundtable");

    let (host_resumed, host_tw) = pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::DiagnosisComplete {
                agent_name,
                diagnosis,
                typewriter,
                ..
            } if agent_name == "Niko Una" && diagnosis.contains("rustfmt") => {
                Some((diagnosis.clone(), *typewriter))
            }
            _ => None,
        })
        .await
        .expect("Host turn after interjection");
    assert!(host_resumed.contains("4 spaces"));
    assert!(host_tw, "Drama Mode must remain active across interjection");

    let (guest_resumed, guest_tw) = pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::DiagnosisComplete {
                agent_name,
                diagnosis,
                typewriter,
                ..
            } if agent_name == "Levi Kujo" => Some((diagnosis.clone(), *typewriter)),
            _ => None,
        })
        .await
        .expect("Guest turn after interjection");
    assert!(guest_resumed.contains("settles the debate"));
    assert!(guest_tw);

    pane.wait_for_status(&AgentStatus::Waiting, TIMEOUT)
        .await
        .expect("settled into Waiting after 1-turn resumed drama exchange");

    // Audit the final Guest request: must have Levi's system prompt, perspective-flipped Niko lines,
    // Drama Mode instructions, and strictly alternating wire roles!
    let all_reqs = ws.llm.main_requests();
    let final_levi_req = all_reqs.last().expect("final levi req");
    assert!(
        final_levi_req
            .system_prompt_text()
            .contains("Full-Metal Kernel Alchemist"),
        "resumed Guest turn must use Levi Kujo's system prompt"
    );
    let final_levi_conv = final_levi_req.conversation_text();
    assert!(
        final_levi_conv.contains("DRAMA / VISUAL NOVEL MODE IS ACTIVE")
            && final_levi_conv.contains("[Character Niko Una said]:"),
        "resumed Drama Guest turn must carry Drama instructions and perspective-flipped Host dialogue:\n{final_levi_conv}"
    );
    let non_sys_roles: Vec<&str> = final_levi_req
        .messages
        .iter()
        .filter_map(|m| m.get("role").and_then(|v| v.as_str()))
        .filter(|&r| r != "system" && r != "developer")
        .collect();
    for pair in non_sys_roles.windows(2) {
        assert_ne!(
            pair[0], pair[1],
            "resumed roundtable after Abort interjection must maintain strict User/Assistant alternation: {non_sys_roles:?}"
        );
    }

    pane.shutdown().await;
    ws.teardown().await;
}
