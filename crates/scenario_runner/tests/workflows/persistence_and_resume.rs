//! E2E Workflow 4: Session SQLite persistence, background Turn-1 title generation,
//! cumulative token & USD cost tracking, `/clear` visual-only reset, and cross-pane `/resume`
//! with `Evicted` + coalesced `RestoreHistory`.

use boxxy_claw_protocol::{ClawEngineEvent, ClawMessage, PersistentClawRow};
use boxxy_db::store::Store;
use scenario_runner::{MockStep, TestWorkspace};
use serde_json::json;
use serial_test::serial;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
#[serial(boxxy_e2e)]
async fn session_persistence_title_clear_and_cross_pane_resume_workflow() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    ws.write_file("src/metrics.rs", "pub fn count() -> usize {\n    0\n}\n")
        .await
        .expect("write metrics.rs");

    let mut pane1 = ws.spawn_pane("pane-persist-1", 0).await.expect("pane 1");
    pane1.wait_ready(TIMEOUT).await.expect("ready 1");
    let session_id = pane1.session_id().to_string();

    // Script a bespoke session title on the background `mock-memory-model` plus a multi-step turn
    // (`file_read` + `sys_shell_exec` + thought + diagnosis) with explicit token counts.
    ws.llm.push_aux_text("Metrics Counter Inspection");
    ws.llm.push_steps([
        MockStep::tool_call("file_read", &json!({ "path": "src/metrics.rs" }))
            .with_usage(1_000, 100),
        MockStep::tool_call("sys_shell_exec", &json!({ "command": "printf 'BUILD_OK'" }))
            .with_usage(1_200, 80),
        MockStep::thought_and_text(
            "**Reviewing metrics module**\nRead file and ran sanity command.",
            "Checked `src/metrics.rs` and verified `BUILD_OK`.",
        )
        .with_usage(1_500, 220),
    ]);

    pane1
        .send_query("Check `src/metrics.rs` and run a quick build check.")
        .await
        .expect("send turn");

    let (diagnosis, trace) = pane1
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("turn diagnosis");
    assert!(diagnosis.contains("BUILD_OK"));
    assert!(
        trace.len() >= 3,
        "expected file_read + sys_shell_exec + thought in trace, got {trace:?}"
    );

    // Wait briefly for the spawned background persistence + title task to commit to SQLite.
    let store = Store::new(ws.db.pool());
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    let persisted_session = loop {
        if let Ok(Some(sess)) = store.get_session(&session_id).await
            && sess.title.as_deref() == Some("Metrics Counter Inspection")
        {
            break sess;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for background session upsert + title generation"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    // Total tokens: (1000+100) + (1200+80) + (1500+220) = 4,100 tokens!
    assert_eq!(persisted_session.total_tokens, 4_100);
    assert!(
        persisted_session.total_cost_usd > 0.0,
        "expected positive catalog-enriched USD cost"
    );

    // 2. `/clear` (`SoftClearHistory`) clears the visible overlay only without deleting SQLite events!
    pane1
        .send(ClawMessage::SoftClearHistory)
        .await
        .expect("soft clear");
    pane1
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::RestoreHistory(rows) if rows.is_empty() => Some(()),
            _ => None,
        })
        .await
        .expect("empty RestoreHistory on /clear");

    let db_events = store
        .get_claw_events(&session_id)
        .await
        .expect("db events still intact");
    assert!(
        !db_events.is_empty(),
        "SoftClearHistory must not delete persisted claw_events from SQLite"
    );

    // 3. Spawn Pane 2, run an initial unrelated turn on Pane 2 (populating `last_snapshot_hash`,
    //    `read_file_hashes`, and `/undo` snapshots), then `/resume` `session_id` with a Character
    //    Switch (`Niko Una` -> `Levi Kujo`). Verify Pane 1 is `Evicted`, Pane 2 restores coalesced
    //    history + `Character Switch: Niko Una → Levi Kujo`, resets stale Pane 2 hashes/undo state,
    //    and sends full terminal snapshot + perspective-flipped history on Turn 2!
    ws.write_file("src/unrelated_old.rs", "pub fn old_val() -> u8 {\n    1\n}\n")
        .await
        .expect("write unrelated_old.rs");

    let mut pane2 = ws.spawn_pane("pane-persist-2", 1).await.expect("pane 2");
    pane2.wait_ready(TIMEOUT).await.expect("ready 2");
    let levi_id = pane2.character_id().to_string();

    pane2
        .set_terminal_snapshot("$ cargo check\nwarning: unused variable `counter`\n")
        .await;
    ws.llm.push_steps([
        MockStep::tool_call(
            "replace_in_file",
            &json!({
                "path": "src/unrelated_old.rs",
                "old_string": "    1",
                "new_string": "    9"
            }),
        ),
        MockStep::text("Updated unrelated_old.rs on Pane 2 prior session."),
    ]);
    pane2
        .send_query("Update @src/unrelated_old.rs to return 9.")
        .await
        .expect("send pre-resume turn on pane 2");
    pane2
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::ProposeFileWrite { .. } => Some(()),
            _ => None,
        })
        .await
        .expect("pre-resume proposal");
    pane2
        .reply_file_write(true)
        .await
        .expect("approve pre-resume edit");
    pane2
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("pre-resume diagnosis");

    // Externally edit `src/unrelated_old.rs` — after `/resume`, Pane 2 must NOT report a stale-file
    // notice for `src/unrelated_old.rs` because it belonged to the pre-resume session!
    ws.write_file(
        "src/unrelated_old.rs",
        "pub fn old_val() -> u8 {\n    99 // modified externally\n}\n",
    )
    .await
    .expect("modify unrelated_old.rs");

    pane2
        .send(ClawMessage::ResumeSession {
            session_id: session_id.clone(),
            character_id: Some(levi_id),
        })
        .await
        .expect("resume on pane 2 as Levi Kujo");

    pane1
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::Evicted => Some(()),
            _ => None,
        })
        .await
        .expect("Pane 1 received Evicted when resumed on Pane 2");

    let restored_rows = pane2
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::RestoreHistory(rows) if !rows.is_empty() => Some(rows.clone()),
            _ => None,
        })
        .await
        .expect("Pane 2 received non-empty RestoreHistory");

    assert!(
        restored_rows
            .iter()
            .any(|row| matches!(row, PersistentClawRow::User { content, .. } if content.contains("metrics.rs"))),
        "restored history must contain original User row"
    );
    assert!(
        restored_rows.iter().any(|row| matches!(
            row,
            PersistentClawRow::Diagnosis { content, trace, .. }
                if content.contains("BUILD_OK") && !trace.is_empty()
        )),
        "restored history must contain coalesced Diagnosis row with non-empty trace"
    );
    assert!(
        restored_rows.iter().any(|row| matches!(
            row,
            PersistentClawRow::SystemMessage { content, .. }
                if content == "Character Switch: Niko Una → Levi Kujo"
        )),
        "resuming a session under a different character must append canonical Character Switch notice, got: {restored_rows:?}"
    );

    let resumed_tokens = pane2
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::Identity { total_tokens, .. } if *total_tokens == 4_100 => {
                Some(*total_tokens)
            }
            _ => None,
        })
        .await
        .expect("Pane 2 Identity reflects persisted total_tokens");
    assert_eq!(resumed_tokens, 4_100);

    // Drain `/resume` models banner SystemMessage before testing `/undo`!
    pane2
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::SystemMessage { text } if text.starts_with("Claw:") => Some(()),
            _ => None,
        })
        .await
        .expect("resume models banner");

    // 4. `/undo` after `/resume` must NOT revert edits from Pane 2's pre-resume session!
    pane2.send_query("/undo").await.expect("send /undo after resume");
    let undo_after_resume = pane2
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::SystemMessage { text } => Some(text.clone()),
            _ => None,
        })
        .await
        .expect("undo notice after resume");
    assert!(
        undo_after_resume.contains("Nothing to undo"),
        "expected /undo buffers to reset across /resume, got: {undo_after_resume}"
    );
    assert!(
        ws.read_file("src/unrelated_old.rs")
            .await
            .unwrap()
            .contains("99"),
        "/undo after /resume must leave pre-resume session files untouched"
    );

    // 5. Send a post-resume follow-up turn on Pane 2 (with the unchanged terminal buffer on Pane 2)
    //    and verify fresh snapshot delivery, Levi persona, Niko perspective-flipping, and 0 stale-file bleed!
    ws.llm.push_step(
        MockStep::text("Resumed as Levi: `count()` is 0 and `counter` warning noted.")
            .with_usage(1_800, 90),
    );
    pane2
        .send_query("Continue from where Niko left off and check the warning on screen.")
        .await
        .expect("send post-resume follow-up");
    let (resumed_reply, _) = pane2
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("post-resume diagnosis");
    assert!(resumed_reply.contains("Resumed as Levi"));

    let reqs = ws.llm.main_requests();
    let post_resume_req = reqs.last().expect("post-resume LLM request");
    assert!(
        post_resume_req
            .system_prompt_text()
            .contains("Full-Metal Kernel Alchemist"),
        "post-resume turn must use Levi Kujo's system prompt"
    );
    let post_resume_conv = post_resume_req.conversation_text();
    assert!(
        post_resume_conv.contains("warning: unused variable `counter`")
            && !post_resume_conv.contains("[TERMINAL_SNAPSHOT: NO_CHANGE]"),
        "first turn after /resume must send full terminal snapshot rather than stale NO_CHANGE:\n{post_resume_conv}"
    );
    assert!(
        !post_resume_conv.contains("unrelated_old.rs")
            && !post_resume_conv.contains("EXTERNALLY MODIFIED FILES NOTICE"),
        "tracked file hashes from Pane 2's pre-resume session must not leak into resumed session:\n{post_resume_conv}"
    );
    assert!(
        post_resume_conv.contains("[Character Niko Una said]:")
            && post_resume_conv.contains("Checked `src/metrics.rs` and verified `BUILD_OK`."),
        "prior character's reply must be perspective-flipped for new character on post-resume turn:\n{post_resume_conv}"
    );

    pane1.shutdown().await;
    pane2.shutdown().await;
    ws.teardown().await;
}
