//! E2E Workflow 9: Incognito Mode (`/incognito` zero-context & zero-DB guarantee)
//! and Multi-Character Roundtable Debate perspective-flipping (`:levi-kujo`).

use boxxy_claw_protocol::AgentStatus;
use boxxy_db::store::Store;
use boxxy_preferences::Settings;
use scenario_runner::{MockStep, TestWorkspace};
use serial_test::serial;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
#[serial(boxxy_e2e)]
async fn incognito_mode_sends_zero_context_zero_tools_and_writes_nothing_to_db() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    Settings::update(|s| {
        s.user_profile.display_name = "SyntheticUserName".to_string();
    });

    let mut incog = ws.spawn_incognito_pane("pane-incog-1").await;
    incog.wait_ready(TIMEOUT).await.expect("incognito ready");
    incog
        .set_terminal_snapshot("$ export SECRET_TERMINAL_LINE=12345\n")
        .await;

    ws.llm.push_steps([
        MockStep::text("Hello from private chat — turn 1."),
        MockStep::text("Continuing private chat — turn 2."),
    ]);

    incog
        .send_query("Explain ownership in Rust briefly.")
        .await
        .expect("incognito turn 1");
    let (r1, _) = incog
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("incognito reply 1");
    assert!(r1.contains("turn 1"));

    incog
        .send_query("Give one follow-up example.")
        .await
        .expect("incognito turn 2");
    let (r2, _) = incog
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("incognito reply 2");
    assert!(r2.contains("turn 2"));

    let reqs = ws.llm.main_requests();
    assert_eq!(reqs.len(), 2);
    for req in &reqs {
        assert!(
            req.tools.is_empty(),
            "/incognito must send zero tool schemas"
        );
        assert!(
            req.raw_body.get("prompt_cache_key").is_none(),
            "/incognito must suppress session prompt_cache_key"
        );
        let all_text = req.flattened_message_text();
        assert!(!all_text.contains("SyntheticUserName"));
        assert!(!all_text.contains("SECRET_TERMINAL_LINE"));
        assert!(!all_text.contains("## YOUR IDENTITY"));
        assert!(!all_text.contains("## CURRENT TURN CONTEXT"));
        assert!(!all_text.contains(&ws.path().to_string_lossy().into_owned()));
    }
    assert!(
        ws.llm.aux_requests().is_empty(),
        "/incognito must never spawn background title or memory extraction calls"
    );

    let store = Store::new(ws.db.pool());
    assert!(
        store
            .get_session(incog.session_id())
            .await
            .expect("query session")
            .is_none(),
        "/incognito must write zero session rows to SQLite"
    );
    assert!(
        store
            .get_claw_events(incog.session_id())
            .await
            .expect("query events")
            .is_empty(),
        "/incognito must write zero claw_events rows to SQLite"
    );

    incog.shutdown().await;
    ws.teardown().await;
}

#[tokio::test]
#[serial(boxxy_e2e)]
async fn roundtable_debate_perspective_flips_peer_messages_and_honors_turn_limit() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    // Pane 1 is owned by character index 0 (`Niko Una`); `:levi-kujo` invites character index 1 (`Levi Kujo`).
    let mut pane = ws.spawn_pane("pane-debate-1", 0).await.expect("spawn host");
    pane.wait_ready(TIMEOUT).await.expect("ready");

    ws.llm.push_steps([
        // Opening turn by Host (Niko Una):
        MockStep::text("Bounded channels prevent unbounded memory growth, @levi-kujo!"),
        // Autonomous Round 1 by Guest (Levi Kujo):
        MockStep::text("Agreed on backpressure, @niko-una, though ring buffers shine for telemetry."),
        // Autonomous Round 2 (final turn of `for 2 turns`) by Host (Niko Una):
        MockStep::text("Fair point — bounded MPSC for commands and ring buffers for logs it is."),
    ]);

    pane.send_query("/debate :levi-kujo Bounded vs unbounded channels for 2 turns")
        .await
        .expect("start 2-turn debate");

    let (opening_reply, _) = pane
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("opening host reply");
    assert!(opening_reply.contains("Bounded channels"));

    let (guest_reply, _) = pane
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("guest autonomous round 1");
    assert!(guest_reply.contains("ring buffers"));

    let (closing_reply, _) = pane
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("host autonomous round 2 (final)");
    assert!(closing_reply.contains("Fair point"));

    // After hitting `2 turns`, the roundtable must stop autonomously and return to `Waiting`.
    pane.wait_for_status(&AgentStatus::Waiting, TIMEOUT)
        .await
        .expect("return to Waiting after 2 autonomous turns");
    assert_eq!(ws.llm.main_requests().len(), 3);

    // Inspect Request #2 (Guest `Levi Kujo`'s turn):
    // 1. System prompt must inject Levi Kujo's character role/personality (not Niko's tsundere maid prompt).
    // 2. Niko's opening reply must be perspective-flipped into a `user` role `[Character Niko Una said]:` block,
    //    never left as an `assistant` message that would cause persona bleed!
    let reqs = ws.llm.main_requests();
    let levi_req = &reqs[1];
    let levi_sys = levi_req.system_prompt_text();
    assert!(
        levi_sys.contains("--- CHARACTER ROLE ---"),
        "Guest turn must carry a non-empty character system prompt"
    );
    assert!(
        !levi_sys.contains("Tsundere Maid"),
        "Guest turn must use Guest character's own system prompt, not Host's"
    );
    assert!(
        levi_req
            .conversation_text()
            .contains("[Character Niko Una said]:\nBounded channels prevent unbounded memory growth"),
        "Host reply must be perspective-flipped into `[Character Niko Una said]:` for Guest:\n{}",
        levi_req.conversation_text()
    );
    let assistant_msgs_seen_by_levi: Vec<_> = levi_req
        .messages
        .iter()
        .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("assistant"))
        .collect();
    assert!(
        assistant_msgs_seen_by_levi
            .iter()
            .all(|m| !m.to_string().contains("Bounded channels prevent")),
        "Host's words must never appear as `role: assistant` when Guest is speaking"
    );

    pane.shutdown().await;
    ws.teardown().await;
}
