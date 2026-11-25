//! E2E Workflow 15:
//! - End-to-End Real `/bin/bash` PTY `OSC 133` Semantic Snapshot Pipeline (`[PROMPT]` / `[COMMAND]` /
//!   `[OUTPUT]`): multi-command scrollback `grep` exit-1 filtering, nested 4-backtick snapshot
//!   fence preservation (including inner Rust code output), `InjectCommand` preservation of
//!   explanatory non-bash code blocks, and `Sleep` mode `[COMMAND]` delta extraction.
//! - Hierarchical Workspace Memory & Episodic Interaction Scoping (subdirectory inheritance vs
//!   cross-project isolation) + `GLOBAL RADAR` `last_command` attachment-leak immunity.

use boxxy_claw_protocol::{AgentStatus, ClawEngineEvent, ClawMessage};
use boxxy_db::store::Store;
use scenario_runner::{MockStep, TestWorkspace};
use serial_test::serial;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
#[serial(boxxy_e2e)]
async fn real_pty_osc133_semantic_snapshot_fence_nesting_grep_filter_and_code_block_preservation_workflow()
 {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    ws.write_file("src/sample.rs", "pub fn present_symbol() -> u32 {\n    7\n}\n")
        .await
        .expect("write sample.rs");

    let pty = ws
        .spawn_pty_bash("pane-osc133-pty", ":1.osc133")
        .await
        .expect("spawn PTY bash");

    let mut pane = ws.spawn_pane("pane-osc133-claw", 0).await.expect("spawn pane");
    pane.wait_ready(TIMEOUT).await.expect("ready");

    // Wait for the initial interactive prompt (`OSC 133;A` + `boxxy-e2e$`) before typing Command 1.
    pty.wait_for_output("boxxy-e2e$", TIMEOUT)
        .await
        .expect("initial bash prompt");

    // 1. Execute an initial command (completion #1) followed by `grep` with no matches (completion #2, exit 1)
    //    on the real `/bin/bash` PTY so scrollback contains multiple OSC 133 blocks.
    pty.send_line("printf 'INITIAL_BUILD_STEP_OK\\n'")
        .expect("send cmd 1");
    let (_, exit_1) = pty
        .wait_for_osc133_done(1, TIMEOUT)
        .await
        .expect("osc133 completion 1");
    assert_eq!(exit_1, 0);

    pty.send_line("LC_ALL=C grep ABSENT_TOKEN_XYZ src/sample.rs")
        .expect("send grep exit 1");
    let (grep_semantic_snap, exit_2) = pty
        .wait_for_osc133_done(2, TIMEOUT)
        .await
        .expect("osc133 completion 2");
    assert_eq!(exit_2, 1);
    assert!(
        grep_semantic_snap.contains("[COMMAND]\n```bash\nLC_ALL=C grep ABSENT_TOKEN_XYZ"),
        "expected structured OSC 133 [COMMAND] block in semantic snapshot:\n{grep_semantic_snap}"
    );

    // Feed the real multi-command OSC 133 semantic snapshot to `CommandFinished(1, ...)`:
    // `should_ignore_exit_code` MUST inspect the LAST `[COMMAND]` block (not line 0!) and ignore it!
    let ev_mark = pane.advance_cursor_to_end();
    pane.notify_command_finished(exit_2, &grep_semantic_snap)
        .await
        .expect("notify grep exit 1");

    // 2. Now run a failing command on the real PTY that also prints triple-backtick Rust code in `[OUTPUT]`!
    pty.send_line("bash -c 'printf \"\\140\\140\\140rust\\npub fn broken() {\\n\\140\\140\\140\\nerror[E0061]: missing arg\\n\"; exit 101'")
        .expect("send failing command with inner markdown fence");
    let (fail_semantic_snap, exit_3) = pty
        .wait_for_osc133_done(3, TIMEOUT)
        .await
        .expect("osc133 completion 3");
    assert_eq!(exit_3, 101);

    pane.notify_command_finished(exit_3, &fail_semantic_snap)
        .await
        .expect("notify failing command");

    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::LazyErrorIndicator { visible: true, .. } => Some(()),
        _ => None,
    })
    .await
    .expect("LazyErrorIndicator emitted for exit 101");

    // Verify only ONE `LazyErrorIndicator(true)` arrived since `ev_mark` (meaning `grep` exit 1 was ignored)
    // and 0 LLM tokens were spent before clicking the error pill!
    let lazy_count = pane
        .events_since(ev_mark)
        .iter()
        .filter(|e| matches!(e, ClawEngineEvent::LazyErrorIndicator { visible: true, .. }))
        .count();
    assert_eq!(
        lazy_count, 1,
        "OSC 133 grep exit 1 must be ignored while exit 101 triggers exactly 1 LazyErrorIndicator"
    );
    assert_eq!(ws.llm.main_requests().len(), 0);

    // 3. Request lazy diagnosis: script the model to return BOTH an explanatory ```rust snippet
    //    AND a runnable ```bash fix command!
    ws.llm.push_step(MockStep::text(
        "Close the function body in `src/sample.rs`:\n\n```rust\npub fn broken() {}\n```\n\nThen re-check:\n```bash\ncargo check\n```",
    ));
    pane.send(ClawMessage::RequestLazyDiagnosis)
        .await
        .expect("request lazy diagnosis");

    let (injected_cmd, clean_diag) = pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::InjectCommand {
                command, diagnosis, ..
            } => Some((command.clone(), diagnosis.clone())),
            _ => None,
        })
        .await
        .expect("InjectCommand emitted");
    assert_eq!(injected_cmd.trim(), "cargo check");
    assert!(
        clean_diag.contains("```rust\npub fn broken() {}\n```")
            && !clean_diag.contains("cargo check"),
        "InjectCommand diagnosis must preserve explanatory ```rust blocks while extracting only ```bash:\n{clean_diag}"
    );

    // Audit the wire payload sent on Turn 1: `process_snapshot` MUST preserve 100% of `[PROMPT]`,
    // `[COMMAND]`, `[OUTPUT]`, and `error[E0061]: missing arg` wrapped inside a 4-backtick outer fence!
    let t1_conv = ws.llm.main_requests()[0].conversation_text();
    assert!(
        t1_conv.contains("Terminal Snapshot:\n````\n[PROMPT]")
            && t1_conv.contains("INITIAL_BUILD_STEP_OK")
            && t1_conv.contains("error[E0061]: missing arg"),
        "OSC 133 semantic blocks and terminal output must never be stripped by process_snapshot:\n{t1_conv}"
    );

    // 4. Enter `Sleep`, run a command on the real PTY (completion #4), wake up, and run Turn 2:
    //    verify `[WHILE_YOU_SLEPT]` extracted the real command from `[COMMAND]` AND Turn 1's
    //    4-backtick OSC 133 snapshot stayed intact in `HISTORY`!
    pane.send(ClawMessage::SleepToggle(true))
        .await
        .expect("enter sleep");
    pane.wait_for_status(&AgentStatus::Sleep, TIMEOUT)
        .await
        .expect("sleeping");

    pty.send_line("printf 'SLEPT_STEP_DONE\\n'")
        .expect("send slept command");
    let (slept_semantic_snap, exit_4) = pty
        .wait_for_osc133_done(4, TIMEOUT)
        .await
        .expect("osc133 completion 4");
    assert_eq!(exit_4, 0);
    pane.notify_command_finished(exit_4, &slept_semantic_snap)
        .await
        .expect("notify slept osc133 command");

    pane.send(ClawMessage::SleepToggle(false))
        .await
        .expect("wake from sleep");
    pane.wait_for_status(&AgentStatus::Waiting, TIMEOUT)
        .await
        .expect("awake");

    ws.llm
        .push_step(MockStep::text("Noted `printf 'SLEPT_STEP_DONE\\n'` while asleep."));
    pane.send_query("Summarize the command run while asleep and the earlier error.")
        .await
        .expect("send turn 2");
    pane.wait_for_diagnosis(TIMEOUT)
        .await
        .expect("turn 2 diagnosis");

    let t2_conv = ws
        .llm
        .main_requests()
        .last()
        .expect("turn 2 req")
        .conversation_text();
    assert!(
        t2_conv.contains("[WHILE_YOU_SLEPT]")
            && t2_conv.contains("$ printf 'SLEPT_STEP_DONE\\n' (exit: 0)"),
        "Sleep mode must extract the real command from OSC 133 [COMMAND] blocks:\n{t2_conv}"
    );
    assert!(
        t2_conv.contains("error[E0061]: missing arg"),
        "Turn 1 nested 4-backtick OSC 133 snapshot must remain intact inside 4-turn history window:\n{t2_conv}"
    );
    assert!(
        !t2_conv.contains("\n\n\n"),
        "Turn 2 conversation payload must stay free of triple-newline formatting runs"
    );

    drop(pty);
    pane.shutdown().await;
    ws.teardown().await;
}

#[tokio::test]
#[serial(boxxy_e2e)]
async fn hierarchical_workspace_memory_scoping_and_radar_attachment_isolation_workflow() {
    // --- Phase 1: Workspace Alpha + Subdirectory Inheritance ---
    let mut ws_alpha = TestWorkspace::new().await.expect("workspace alpha");
    ws_alpha
        .write_file("crates/sub_crate/src/lib.rs", "pub fn sub() {}\n")
        .await
        .expect("write sub_crate");
    let alpha_root = ws_alpha.path().to_string_lossy().into_owned();
    let alpha_subdir = ws_alpha.file_path("crates/sub_crate");

    {
        let store = Store::new(ws_alpha.db.pool());
        store
            .ensure_session_exists("sess-alpha-seed", "seed-char", "Niko Una", &alpha_root)
            .await
            .expect("ensure alpha seed session");
        store
            .add_memory(
                "alpha_pinned_rule",
                Some(&alpha_root),
                "ALPHA_PINNED_SENTINEL: enforce zero unsafe blocks",
                Some("preference"),
                true,
                true,
            )
            .await
            .expect("seed alpha pinned");
        store
            .add_memory(
                "alpha_gateway_port",
                Some(&alpha_root),
                "ALPHA_VERIFIED_SENTINEL: gateway listens on port 9443",
                Some("dreamed"),
                true,
                false,
            )
            .await
            .expect("seed alpha verified");
        store
            .upsert_dream_candidate(
                "alpha_cand_port",
                Some(&alpha_root),
                "ALPHA_CANDIDATE_SENTINEL: metrics port is 9444",
                0.6,
            )
            .await
            .expect("seed alpha candidate");
        store
            .add_interaction(
                "sess-alpha-seed",
                Some(&alpha_root),
                "ALPHA_INTERACTION_SENTINEL: configured gateway port and TLS pipeline.",
                Some("flush_summary"),
                None,
            )
            .await
            .expect("seed alpha interaction");
        let undreamed = store.get_undreamed_interactions().await.unwrap();
        let ids: Vec<i64> = undreamed.into_iter().map(|i| i.id).collect();
        store.mark_interactions_as_dreamed(&ids).await.unwrap();
    }

    // Spawn a pane inside `crates/sub_crate` (subdirectory of `ws_alpha`) and verify all 4 memory
    // tiers inherit cleanly from the parent repository root!
    let mut sub_pane = ws_alpha
        .spawn_pane_in_dir("pane-alpha-sub", 0, alpha_subdir)
        .await
        .expect("spawn sub_pane");
    sub_pane.wait_ready(TIMEOUT).await.expect("sub_pane ready");

    ws_alpha
        .llm
        .push_step(MockStep::text("Subdirectory inherited alpha port 9443."));
    sub_pane
        .send_query("Check gateway port and TLS pipeline configuration.")
        .await
        .expect("send sub_pane query");
    sub_pane
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("sub_pane done");

    let alpha_sub_conv = ws_alpha.llm.main_requests()[0].conversation_text();
    assert!(
        alpha_sub_conv.contains("ALPHA_PINNED_SENTINEL")
            && alpha_sub_conv.contains("ALPHA_VERIFIED_SENTINEL")
            && alpha_sub_conv.contains("ALPHA_CANDIDATE_SENTINEL")
            && alpha_sub_conv.contains("ALPHA_INTERACTION_SENTINEL"),
        "subdirectory pane inside workspace must inherit parent workspace pinned, verified, candidate, and episodic memories:\n{alpha_sub_conv}"
    );

    sub_pane.shutdown().await;
    ws_alpha.teardown().await;

    // --- Phase 2: Separate Workspace Beta — Zero Cross-Project Memory Leak & Clean GLOBAL RADAR ---
    let mut ws_beta = TestWorkspace::new().await.expect("workspace beta");
    let mut beta_pane1 = ws_beta
        .spawn_pane("pane-beta-1", 0)
        .await
        .expect("spawn beta pane 1");
    beta_pane1.wait_ready(TIMEOUT).await.expect("ready 1");

    let mut beta_pane2 = ws_beta
        .spawn_pane("pane-beta-2", 1)
        .await
        .expect("spawn beta pane 2");
    beta_pane2.wait_ready(TIMEOUT).await.expect("ready 2");

    ws_beta.llm.push_steps([
        MockStep::text("Beta pane 1 reviewed pasted config."),
        MockStep::text("Beta pane 2 checked radar cleanly."),
    ]);

    // Pane 1 sends overlapping keywords ("gateway port and TLS pipeline") PLUS an inline attachment!
    beta_pane1
        .send_query(
            "Check gateway port and TLS pipeline in [File 1 · app.toml].\n\n--- ATTACHMENTS ---\n\n**[File 1 · app.toml]**\n```toml\nsecret_token = \"BETA_ATTACHMENT_PAYLOAD_DO_NOT_LEAK_TO_RADAR\"\n```",
        )
        .await
        .expect("send beta pane 1 query");
    beta_pane1
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("beta pane 1 done");

    // Pane 2 runs Turn 1 in the same workspace:
    // 1. Neither Beta pane may recall ANY `ALPHA_*_SENTINEL` from `ws_alpha`!
    // 2. Pane 2's `GLOBAL RADAR` must NOT contain `BETA_ATTACHMENT_PAYLOAD_DO_NOT_LEAK_TO_RADAR`
    //    or Pane 1's chat message in `Last command`!
    beta_pane2
        .send_query("Inspect workspace status and gateway port.")
        .await
        .expect("send beta pane 2 query");
    beta_pane2
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("beta pane 2 done");

    let beta_reqs = ws_beta.llm.main_requests();
    assert_eq!(beta_reqs.len(), 2);
    for (idx, req) in beta_reqs.iter().enumerate() {
        let conv = req.conversation_text();
        assert!(
            !conv.contains("ALPHA_PINNED_SENTINEL")
                && !conv.contains("ALPHA_VERIFIED_SENTINEL")
                && !conv.contains("ALPHA_CANDIDATE_SENTINEL")
                && !conv.contains("ALPHA_INTERACTION_SENTINEL"),
            "Beta request #{idx} leaked project-scoped memories/interactions from unrelated Workspace Alpha:\n{conv}"
        );
    }

    let pane2_conv = beta_reqs[1].conversation_text();
    assert!(
        pane2_conv.contains("--- GLOBAL RADAR")
            && pane2_conv.contains("Last command `idle` | Status: Waiting [Full Context]"),
        "Peer pane in GLOBAL RADAR must display `Last command \\`idle\\`` and `[Full Context]`:\n{pane2_conv}"
    );
    assert!(
        !pane2_conv.contains("BETA_ATTACHMENT_PAYLOAD_DO_NOT_LEAK_TO_RADAR")
            && !pane2_conv.contains("--- ATTACHMENTS ---"),
        "Peer pane attachments and user prompts must NEVER leak into GLOBAL RADAR:\n{pane2_conv}"
    );

    beta_pane1.shutdown().await;
    beta_pane2.shutdown().await;
    ws_beta.teardown().await;
}
