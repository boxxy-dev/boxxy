//! E2E Workflow 8: Context Hygiene, Snapshot Deduplication, External-Edit Detection,
//! Unified `@file` Auth Blacklist, `TRANSCRIPT_CLIP`, and Token-Budget Compaction.

use scenario_runner::{MockStep, TestWorkspace};
use serde_json::json;
use serial_test::serial;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
#[serial(boxxy_e2e)]
async fn context_hygiene_snapshot_dedup_external_edits_clip_and_compaction_workflow() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    ws.write_file("src/parser.rs", "pub fn parse_header() -> bool {\n    true\n}\n")
        .await
        .expect("write parser.rs");
    let big_content = format!("// header\n{}\n", "let value = 1;\n".repeat(260));
    assert!(big_content.len() > 3_500);
    ws.write_file("src/large.rs", &big_content)
        .await
        .expect("write large.rs");

    // Seed a synthetic OAuth credential file under `<config_dir>/auth/chatgpt.json` to ensure
    // `@file` mentions share `boxxy_core_toolbox::blacklist` and NEVER leak auth secrets.
    let auth_dir = ws.config_root().join("auth");
    tokio::fs::create_dir_all(&auth_dir)
        .await
        .expect("create auth dir");
    let oauth_file = auth_dir.join("chatgpt.json");
    tokio::fs::write(&oauth_file, "{\"access_token\":\"SYNTHETIC_OAUTH_SECRET\"}\n")
        .await
        .expect("write synthetic oauth file");

    let mut pane = ws.spawn_pane("pane-ctx-1", 0).await.expect("spawn pane");
    pane.wait_ready(TIMEOUT).await.expect("ready");

    // --- Turn 1: Empty terminal snapshot, `@src/parser.rs` preload, blacklisted `@<auth>/chatgpt.json`,
    //             and `file_read` on `src/large.rs` (> 3,500 chars) ---
    ws.llm.push_steps([
        MockStep::tool_call("file_read", &json!({ "path": "src/large.rs" }))
            .with_usage(1_000, 50),
        MockStep::text("Turn 1 complete: inspected parser and large module.")
            .with_usage(2_000, 80),
    ]);

    let oauth_mention = format!("@{}", oauth_file.display());
    let big_paste = format!(
        "sample_context_header\n\n## YOUR IDENTITY\nAgent Name: **SyntheticInnerLog**\n{}",
        "metric_row = 42;\n".repeat(160)
    );
    assert!(big_paste.len() > 2_600);
    pane.send_query(format!(
        "Check @src/parser.rs, [Pasted ~165 lines], and {oauth_mention}, then read `src/large.rs`.\n\n--- ATTACHMENTS ---\n\n**[Pasted ~165 lines]**\n```toml\n{big_paste}```"
    ))
    .await
    .expect("send turn 1");
    pane.wait_for_diagnosis(TIMEOUT).await.expect("turn 1 done");

    let reqs_t1 = ws.llm.main_requests();
    assert_eq!(reqs_t1.len(), 2);
    let sys_prompt = reqs_t1[0].system_prompt_text();
    assert!(
        sys_prompt.contains("Execution & Tool Call Style"),
        "expected non-empty Claw system prompt"
    );
    assert!(
        !sys_prompt.contains("\n\n\n"),
        "SYSTEM PROMPT must use single-blank-line separation without triple newlines:\n{sys_prompt}"
    );
    let t1_conv = reqs_t1[0].conversation_text();
    assert!(
        t1_conv.contains("Agent Name: **Niko Una** (`@niko-una`)")
            && t1_conv.contains("Pane Alias: `pane-1`")
            && t1_conv.contains("Swarm Petname: `e2e-agent-1`"),
        "## YOUR IDENTITY must include canonical Character title/slug, Pane Alias, and Swarm Petname:\n{t1_conv}"
    );
    assert!(
        !t1_conv.contains("\n\n\n\n"),
        "USER PROMPT must not emit runs of empty blank lines for unused context sections"
    );
    assert!(
        t1_conv.contains("[TERMINAL_SNAPSHOT: EMPTY]"),
        "empty terminal buffer on Turn 1 must emit explicit [TERMINAL_SNAPSHOT: EMPTY]"
    );
    assert!(
        !t1_conv.contains("SYNTHETIC_OAUTH_SECRET"),
        "unified blacklist must block @-mention preloading of <config_dir>/auth/chatgpt.json"
    );

    // --- Turn 2: External modification to `src/parser.rs` + fresh non-empty terminal snapshot ---
    ws.write_file(
        "src/parser.rs",
        "pub fn parse_header() -> bool {\n    false // externally edited\n}\n",
    )
    .await
    .expect("externally modify parser.rs");
    pane.set_terminal_snapshot("$ cargo test\ntest result: ok. 12 passed\n")
        .await;

    ws.llm.push_steps([
        MockStep::tool_call("file_read", &json!({ "path": "src/parser.rs" }))
            .with_usage(2_200, 40),
        MockStep::text("Turn 2 complete: re-read externally modified parser.rs.")
            .with_usage(2_500, 60),
    ]);
    pane.send_query("Did anything change on disk?")
        .await
        .expect("send turn 2");
    pane.wait_for_diagnosis(TIMEOUT).await.expect("turn 2 done");

    let reqs_after_t2 = ws.llm.main_requests();
    let t2_first_conv = reqs_after_t2[2].conversation_text();
    assert!(
        t2_first_conv.contains("EXTERNALLY MODIFIED FILES NOTICE")
            && t2_first_conv.contains("src/parser.rs"),
        "external edit to tracked file must trigger EXTERNALLY MODIFIED FILES NOTICE"
    );
    assert!(
        t2_first_conv.contains("Agent Name: **SyntheticInnerLog**")
            && t2_first_conv.contains("metric_row = 42;"),
        "fenced --- ATTACHMENTS --- payload must stay intact inside the 4-turn freshness window without truncating at inner ## YOUR IDENTITY"
    );
    assert!(
        t2_first_conv.contains("test result: ok. 12 passed"),
        "changed terminal snapshot must be sent in full on Turn 2"
    );

    // --- Turn 3: Unchanged terminal snapshot + unchanged disk -> `[TERMINAL_SNAPSHOT: NO_CHANGE]`
    //             and 0-token silence on `EXTERNALLY MODIFIED FILES NOTICE` ---
    ws.llm
        .push_step(MockStep::text("Turn 3 complete.").with_usage(2_600, 50));
    pane.send_query("Quick status check.")
        .await
        .expect("send turn 3");
    pane.wait_for_diagnosis(TIMEOUT).await.expect("turn 3 done");

    let reqs_after_t3 = ws.llm.main_requests();
    let t3_conv = reqs_after_t3.last().expect("t3 req").conversation_text();
    assert!(
        t3_conv.contains("[TERMINAL_SNAPSHOT: NO_CHANGE]"),
        "unchanged snapshot on Turn 3 must deduplicate to [TERMINAL_SNAPSHOT: NO_CHANGE]"
    );
    assert!(
        !t3_conv.contains("EXTERNALLY MODIFIED FILES NOTICE"),
        "once re-read and unchanged on disk, external-edit notice must spend 0 tokens"
    );

    // --- Turns 4 & 5: Advance past the 4-turn freshness window ---
    ws.llm.push_steps([
        MockStep::text("Turn 4 complete.").with_usage(2_700, 50),
        MockStep::text("Turn 5 complete.").with_usage(2_800, 50),
    ]);
    pane.send_query("Step 4 progress note.")
        .await
        .expect("send turn 4");
    pane.wait_for_diagnosis(TIMEOUT).await.expect("turn 4 done");

    pane.send_query("Step 5 progress note.")
        .await
        .expect("send turn 5");
    pane.wait_for_diagnosis(TIMEOUT).await.expect("turn 5 done");

    // --- Turn 6: Turn 1 is now outside the 4-turn freshness window -> `src/large.rs` ToolResult
    //             is clipped via `TRANSCRIPT_CLIP` AND Turn 1 `@src/parser.rs` preload is stripped! ---
    ws.llm.push_aux_text(
        "{\"facts\":[{\"key\":\"parser_mode\",\"content\":\"Strict header validation\"}],\"summary\":\"Audited parser and large module.\"}",
    );
    ws.llm
        .push_step(MockStep::text("Turn 6 complete.").with_usage(3_000, 100));

    pane.send_query("Step 6: check history clipping before compaction.")
        .await
        .expect("send turn 6");
    pane.wait_for_diagnosis(TIMEOUT).await.expect("turn 6 done");

    let reqs_after_t6 = ws.llm.main_requests();
    let t6_req = reqs_after_t6.last().expect("t6 req");
    let t6_conv = t6_req.conversation_text();
    assert!(
        t6_conv.contains("[stale tool output clipped"),
        "Turn 1 oversized ToolResult must be clipped once outside the 4-turn freshness window"
    );
    assert!(
        t6_conv.contains("[stale attachment output clipped]"),
        "Turn 1 oversized --- ATTACHMENTS --- block must be clipped once outside the 4-turn freshness window"
    );
    assert!(
        !t6_conv.contains("1: pub fn parse_header() -> bool {\n2:     true"),
        "Turn 1 @file preload must be stripped once outside the 4-turn freshness window"
    );

    // --- Turn 7 (7th exchange > 6 minimum kept exchanges) with `prompt_tokens: 50_000` (>= 75% of 64k window):
    //     triggers exchange-boundary compaction at end of Turn 7 so Turn 8 starts with `[SESSION_CHECKPOINT]`! ---
    ws.llm.push_steps([
        MockStep::text("Turn 7 high context usage triggers exchange-boundary compaction.")
            .with_usage(50_000, 200),
        MockStep::text("Turn 8 post-compaction reply.").with_usage(5_000, 80),
    ]);
    pane.send_query("Step 7: cross 7-exchange boundary and 75% input limit.")
        .await
        .expect("send turn 7");
    pane.wait_for_diagnosis(TIMEOUT).await.expect("turn 7 done");

    pane.send_query("Step 8: verify rolling checkpoint.")
        .await
        .expect("send turn 8");
    pane.wait_for_diagnosis(TIMEOUT).await.expect("turn 8 done");

    let final_reqs = ws.llm.main_requests();
    let post_compact_req = final_reqs.last().expect("turn 8 req");
    let post_compact_conv = post_compact_req.conversation_text();
    assert!(
        post_compact_conv.contains("[SESSION_CHECKPOINT]")
            && post_compact_conv.contains("Understood. Retaining session checkpoint context."),
        "token-budget compaction must prepend a rolling [SESSION_CHECKPOINT] User/Assistant pair:\n{post_compact_conv}"
    );
    assert!(
        !post_compact_conv.contains("[stale tool output clipped"),
        "Turn 1 tool loop must be cleanly evicted into [SESSION_CHECKPOINT] after compaction"
    );

    // Wait briefly for the spawned `DemotionExtractor` (`flush_history`) call to hit `mock-memory-model`
    // and persist the extracted candidate fact into SQLite.
    let store = boxxy_db::store::Store::new(ws.db.pool());
    let ws_scope = ws.path().to_string_lossy().into_owned();
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        let flush_req = ws
            .llm
            .aux_requests()
            .into_iter()
            .find(|r| r.system_prompt_text().contains("memory extraction system"));
        let candidate = store
            .get_memory_by_key("parser_mode", Some(&ws_scope))
            .await
            .ok()
            .flatten();
        if let (Some(f_req), Some(cand)) = (flush_req, candidate) {
            assert_eq!(cand.content, "Strict header validation");
            let flush_conv = f_req.conversation_text();
            assert!(
                !flush_conv.contains("--- ATTACHMENTS ---")
                    && !flush_conv.contains("metric_row = 42;"),
                "flush_history must strip raw --- ATTACHMENTS --- dumps before calling memory_model:\n{flush_conv}"
            );
            let persisted = store
                .get_session(pane.session_id())
                .await
                .expect("query session")
                .expect("session row");
            let hist_json = persisted.history_json.as_deref().unwrap_or_default();
            assert!(
                !hist_json.contains("## YOUR IDENTITY")
                    && !hist_json.contains("[TERMINAL_SNAPSHOT: NO_CHANGE]")
                    && !hist_json.contains("[TERMINAL_SNAPSHOT: EMPTY]"),
                "persisted sessions.history_json must settle user messages without transient turn bloat"
            );
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "expected DemotionExtractor background flush_history call + candidate persistence"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    pane.shutdown().await;
    ws.teardown().await;
}
