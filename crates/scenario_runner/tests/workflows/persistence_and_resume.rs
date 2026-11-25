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

    // 3. Spawn Pane 2 and `/resume` `session_id` -> Pane 1 must be `Evicted` and Pane 2 receives
    //    `RestoreHistory` with coalesced turn trace + cumulative token counters!
    let mut pane2 = ws.spawn_pane("pane-persist-2", 1).await.expect("pane 2");
    pane2.wait_ready(TIMEOUT).await.expect("ready 2");

    pane2
        .send(ClawMessage::ResumeSession {
            session_id: session_id.clone(),
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

    pane1.shutdown().await;
    pane2.shutdown().await;
    ws.teardown().await;
}
