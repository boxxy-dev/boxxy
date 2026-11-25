//! E2E Workflow 14:
//! - Collaborative 1-turn `invite_character(..., debate=false)` handoff where the Guest executes
//!   `prepare_character_assets` (Characters v2 `768×432` `portrait.webp` + `256×256` `AVATAR.png`
//!   via `boxxy-assets`), followed by a Turn-2 Host follow-up verifying strict bidirectional
//!   `user`/`assistant` alternation and zero false user-interruption markers.
//! - Phantom Swarm (`summon_headless_worker` + `await_tasks`) profile preservation, zero-DB ghost
//!   pollution, auto-reap cleanup, `--- DETACHED BACKGROUND SHELLS ---` radar injection, and
//!   Ghostty-style live pane drag-and-drop rebinding (`ClawMessage::RebindPane`).

use boxxy_agent::pty::registry::DetachOutcome;
use boxxy_claw_protocol::{
    AgentStatus, ClawEngineEvent, ClawMessage, DiffKind, PersistentClawRow,
};
use boxxy_db::store::Store;
use boxxy_model_selection::{
    CustomModelConfig, ModelProvider, OpenAiApiRoute, OpenAiCompatibleEndpoint,
};
use boxxy_preferences::Settings;
use image::{GenericImageView as _, ImageFormat, Rgb, RgbImage};
use scenario_runner::{MockLlmServer, MockStep, TestWorkspace};
use serde_json::{Value, json};
use serial_test::serial;
use std::io::Cursor;
use std::time::Duration;
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(5);

fn encode_widescreen_png() -> Vec<u8> {
    let img = RgbImage::from_fn(320, 180, |x, y| {
        Rgb([
            u8::try_from((x * 255) / 320).unwrap_or(120),
            u8::try_from((y * 255) / 180).unwrap_or(90),
            180,
        ])
    });
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Png)
        .expect("encode synthetic 16:9 PNG");
    buf.into_inner()
}

fn extract_json_field_from_messages(messages: &[Value], field: &str) -> Option<String> {
    for msg in messages.iter().rev() {
        if let Some(s) = msg.get("content").and_then(Value::as_str)
            && let Ok(parsed) = serde_json::from_str::<Value>(s)
            && let Some(val) = parsed.get(field).and_then(Value::as_str)
            && !val.is_empty()
        {
            return Some(val.to_string());
        }
    }
    None
}

#[tokio::test]
#[serial(boxxy_e2e)]
async fn collaborative_handoff_character_creator_assets_and_followup_alternation_workflow() {
    let mut ws = TestWorkspace::new().await.expect("workspace");

    // Enable the default-off `boxxy-character-creator` skill so `prepare_character_assets` is registered.
    Settings::update(|s| {
        s.disabled_skills
            .retain(|slug| slug != "boxxy-character-creator");
    });

    let art_path = ws.file_path("art/concept_banner.png");
    tokio::fs::create_dir_all(art_path.parent().unwrap())
        .await
        .expect("mkdir art");
    tokio::fs::write(&art_path, encode_widescreen_png())
        .await
        .expect("write synthetic concept_banner.png");

    let mut host = ws.spawn_pane("pane-cc-host", 0).await.expect("spawn host");
    host.wait_ready(TIMEOUT).await.expect("host ready");

    ws.llm.push_steps([
        // Turn 1 — Host (`Niko Una`) invites `levi-kujo` for a 1-turn collaborative handoff:
        MockStep::tool_call(
            "invite_character",
            &json!({
                "character": "levi-kujo",
                "topic": "Crop art/concept_banner.png into Characters v2 assets for sora-dev.",
                "debate": false
            }),
        ),
        MockStep::text("Handing off to @levi-kujo to frame and crop the character package assets."),
        // Collaborative Handoff — Guest (`Levi Kujo`) runs `prepare_character_assets`:
        MockStep::tool_call(
            "prepare_character_assets",
            &json!({
                "slug": "sora-dev",
                "portrait_source_path": "art/concept_banner.png",
                "avatar_source_path": "art/concept_banner.png",
                "avatar_focus_y": 0.32,
                "avatar_zoom": 1.4
            }),
        ),
        MockStep::text("Prepared `768x432` `portrait.webp` and `256x256` `AVATAR.png` for `sora-dev`."),
        // Turn 2 — Host (`Niko Una`) handles user follow-up on the post-handoff transcript:
        MockStep::text("Ready to write `manifest.toml` and `CHARACTER.toml` for `sora-dev`."),
    ]);

    host.send_query("Ask Levi to crop `art/concept_banner.png` into a new `sora-dev` character package.")
        .await
        .expect("send turn 1");

    let (host_intro, _) = host
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("host handoff intro");
    assert!(host_intro.contains("@levi-kujo"));

    // Approve Guest's binary `prepare_character_assets` write proposal (carrying 2 staged preview images).
    let preview_count = host
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::ProposeFileWrite {
                diff: Some(d),
                auto_approve: false,
                ..
            } if d.kind == DiffKind::Binary => Some(d.preview_images.len()),
            _ => None,
        })
        .await
        .expect("receive binary ProposeFileWrite from prepare_character_assets");
    assert_eq!(
        preview_count, 2,
        "prepare_character_assets must stage both 16:9 portrait and 1:1 avatar previews"
    );

    host.reply_file_write(true)
        .await
        .expect("approve character asset write");

    let (guest_done, _) = host
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("guest handoff complete");
    assert!(guest_done.contains("sora-dev"));
    host.wait_for_status(&AgentStatus::Waiting, TIMEOUT)
        .await
        .expect("settled into Waiting after 1-turn handoff");

    // Verify exact Characters v2 output dimensions on disk (`768x432` WebP + `256x256` PNG).
    let char_pkg_dir = ws
        .config_root()
        .join("boxxyclaw")
        .join("characters")
        .join("sora-dev");
    let portrait_img = image::open(char_pkg_dir.join("portrait.webp")).expect("decode portrait.webp");
    assert_eq!(portrait_img.dimensions(), (768, 432));
    let avatar_img = image::open(char_pkg_dir.join("AVATAR.png")).expect("decode AVATAR.png");
    assert_eq!(avatar_img.dimensions(), (256, 256));

    // Turn 2: Send a follow-up to Host (`Niko Una`) and audit wire alternation after Guest's tool loop!
    host.send_query("Great, now let's plan `manifest.toml` for `sora-dev`.")
        .await
        .expect("send turn 2");
    let (turn2_reply, _) = host
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("turn 2 diagnosis");
    assert!(turn2_reply.contains("manifest.toml"));

    let all_reqs = ws.llm.main_requests();
    let turn2_req = all_reqs.last().expect("turn 2 request");
    let non_sys_roles: Vec<&str> = turn2_req
        .messages
        .iter()
        .filter_map(|m| m.get("role").and_then(Value::as_str))
        .filter(|&r| r != "system" && r != "developer")
        .collect();

    for pair in non_sys_roles.windows(2) {
        if pair[0] != "tool" && pair[1] != "tool" {
            assert_ne!(
                pair[0], pair[1],
                "wire messages must never contain consecutive `{}` roles across guest tool handoffs: {non_sys_roles:?}",
                pair[0]
            );
        }
    }
    let turn2_conv = turn2_req.conversation_text();
    assert!(
        turn2_conv.contains("[Character Levi Kujo said]:")
            && !turn2_conv.contains("[Previous task was interrupted by the user]"),
        "guest tool handoff must perspective-flip cleanly without false user-interruption markers:\n{turn2_conv}"
    );

    let _ = tokio::fs::remove_dir_all(&char_pkg_dir).await;
    host.shutdown().await;
    ws.teardown().await;
}

#[tokio::test]
#[serial(boxxy_e2e)]
async fn phantom_swarm_headless_worker_and_live_pane_rebind_with_detached_radar_workflow() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    ws.write_file("src/engine.rs", "pub fn step() -> usize { 1 }\n")
        .await
        .expect("write engine.rs");

    // Route Host (`Niko Una`, index 0) to a dedicated `leader_llm` so concurrent Phantom Swarm
    // headless worker calls (which use the default `ws.llm` endpoint) never race Leader's queue!
    let leader_llm = MockLlmServer::start().await.expect("leader mock server");
    let leader_ep_name = format!("LeaderEndpoint-{}", Uuid::new_v4());
    let mut leader_ep = OpenAiCompatibleEndpoint::preset_local_server();
    leader_ep.name.clone_from(&leader_ep_name);
    leader_ep.base_url = leader_llm.base_url();
    leader_ep.models = vec![CustomModelConfig {
        id: "mock-leader-model".to_string(),
        display_name: "Mock Leader Model".to_string(),
        route: OpenAiApiRoute::ChatCompletions,
        context_window: Some(64_000),
        thinking_levels: Vec::new(),
        input_cost_per_1m: Some(1.0),
        output_cost_per_1m: Some(2.0),
    }];
    let niko_id = scenario_runner::config_env::seeded_catalog()[0]
        .config
        .id
        .clone();
    Settings::update(|s| {
        s.openai_compatible_endpoints.push(leader_ep.clone());
        s.character_models.insert(
            niko_id.clone(),
            ModelProvider::OpenAiCompatible {
                provider_name: leader_ep_name.clone(),
                model_id: "mock-leader-model".to_string(),
                thinking: None,
            },
        );
    });

    // 1. Create a real detached background shell in `PtyRegistry` so `--- DETACHED BACKGROUND SHELLS ---`
    //    surfaces in Host's turn radar!
    let mut pty = ws
        .spawn_pty_bash("pane-detached-bg", ":1.detached_owner")
        .await
        .expect("spawn PTY");
    pty.set_persistence(true).await;
    pty.send_line("bash -c 'printf \"DETACHED_JOB_%s\\n\" READY; sleep 2'")
        .expect("launch bg job");
    pty.wait_for_output("DETACHED_JOB_READY", TIMEOUT)
        .await
        .expect("bg job running");
    let detach_res = pty.simulate_ui_disconnect().await;
    assert_eq!(detach_res, vec![(pty.pid(), DetachOutcome::Detached)]);

    let mut host = ws
        .spawn_pane("pane-rebind-old", 0)
        .await
        .expect("spawn host");
    host.wait_ready(TIMEOUT).await.expect("host ready");

    // Script the Phantom Swarm headless worker on `ws.llm`:
    // Have the headless worker execute a real relative `file_read("src/engine.rs")` tool call
    // first so we verify it inherited the parent workspace CWD rather than `"/"`!
    ws.llm.push_steps([
        MockStep::tool_call("file_read", &json!({ "path": "src/engine.rs" })),
        MockStep::text("Headless Worker Audit: 0 unsafe blocks in `src/engine.rs`."),
    ]);

    // Script Host (`Niko Una`) on `leader_llm`:
    leader_llm.push_steps([
        MockStep::tool_call("acquire_lock", &json!({ "resource": "src/engine.rs" })),
        MockStep::tool_call(
            "schedule_task",
            &json!({
                "due_in_seconds": 300,
                "task_type": "notification",
                "payload": "Post-Rebind Reminder"
            }),
        ),
        MockStep::tool_call(
            "summon_headless_worker",
            &json!({
                "profile": "RUST_UNSAFE_AUDITOR_PROFILE: inspect FFI and memory safety",
                "prompt": "Audit src/engine.rs for unsafe blocks."
            }),
        ),
        MockStep::dynamic(|req| {
            let task_id = extract_json_field_from_messages(&req.messages, "task_id")
                .expect("extract headless worker task_id");
            MockStep::tool_call("await_tasks", &json!({ "task_ids": [task_id] }))
        }),
        MockStep::text("Locked `src/engine.rs`, scheduled reminder, and verified headless audit."),
        // Turn 2 after `RebindPane`:
        MockStep::text("Confirmed session rebound cleanly to `pane-rebind-new`."),
    ]);

    host.send_query("Lock `src/engine.rs`, set a reminder, and summon a headless safety auditor.")
        .await
        .expect("send turn 1");

    let (turn1_summary, _) = host
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("turn 1 diagnosis");
    assert!(turn1_summary.contains("headless audit"));

    // Verify Host stayed in `Locking` because `src/engine.rs` is still held!
    host.wait_for_status(
        &AgentStatus::Locking {
            resource: "src/engine.rs".to_string(),
        },
        TIMEOUT,
    )
    .await
    .expect("host remains in Locking status");

    // Audit 1: Host's Turn 1 prompt included `--- DETACHED BACKGROUND SHELLS ---`!
    let leader_reqs_t1 = leader_llm.main_requests();
    assert!(
        leader_reqs_t1[0]
            .conversation_text()
            .contains("--- DETACHED BACKGROUND SHELLS ---"),
        "live detached PTY must surface in --- DETACHED BACKGROUND SHELLS ---:\n{}",
        leader_reqs_t1[0].conversation_text()
    );

    // Audit 2: Headless worker received its `RUST_UNSAFE_AUDITOR_PROFILE` in SYSTEM PROMPT
    //          (never overwritten by `ensure_initialized`), inherited the parent workspace `CWD`
    //          (successfully reading relative `src/engine.rs`), observed clean `GLOBAL RADAR`
    //          (`[Full Context]` and `Last command \`idle\``), and wrote ZERO ghost sessions to SQLite!
    let headless_reqs = ws.llm.main_requests();
    assert_eq!(headless_reqs.len(), 2);
    assert!(
        headless_reqs[0]
            .system_prompt_text()
            .contains("RUST_UNSAFE_AUDITOR_PROFILE: inspect FFI and memory safety"),
        "headless worker system prompt must retain its custom profile:\n{}",
        headless_reqs[0].system_prompt_text()
    );
    let ws_dir_str = ws.path().to_string_lossy().into_owned();
    let headless_t1_conv = headless_reqs[0].conversation_text();
    assert!(
        headless_t1_conv.contains("(Worker "),
        "headless worker identity must preserve Worker display name"
    );
    assert!(
        headless_t1_conv.contains(&format!("CWD: `{ws_dir_str}`")),
        "headless worker must inherit parent workspace CWD instead of `/`: \n{headless_t1_conv}"
    );
    assert!(
        headless_t1_conv.contains("Last command `idle` | Status: Waiting [Full Context]"),
        "GLOBAL RADAR must report parent pane `[Full Context]` without leaking user prompts into `Last command`:\n{headless_t1_conv}"
    );
    assert!(
        headless_reqs[1]
            .conversation_text()
            .contains("pub fn step() -> usize { 1 }"),
        "headless worker relative file_read(\"src/engine.rs\") must succeed against inherited CWD"
    );

    let store = Store::new(ws.db.pool());
    let recent_sessions = store
        .get_recent_active_sessions(20)
        .await
        .expect("query recent sessions");
    assert!(
        recent_sessions
            .iter()
            .all(|s| !s.name.contains("(Worker ")),
        "transient headless workers must never pollute SQLite `sessions` table: {recent_sessions:?}"
    );

    // 2. Simulate Ghostty-style pane drag-and-drop across splits/tabs via `ClawMessage::RebindPane`!
    // Wait briefly for Turn-1 background persistence to commit before rebinding.
    let persist_deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        if store
            .get_session(host.session_id())
            .await
            .ok()
            .flatten()
            .is_some()
            && !store
                .get_claw_events(host.session_id())
                .await
                .unwrap_or_default()
                .is_empty()
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < persist_deadline,
            "timed out waiting for Turn-1 session & visual events persistence"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let _ = host.advance_cursor_to_end();
    host.send(ClawMessage::RebindPane {
        new_pane_id: "pane-rebind-new".to_string(),
    })
    .await
    .expect("send RebindPane");
    host.set_local_pane_id("pane-rebind-new");

    host.wait_for_status(
        &AgentStatus::Locking {
            resource: "src/engine.rs".to_string(),
        },
        TIMEOUT,
    )
    .await
    .expect("rebound pane preserves Locking status");

    let rebound_rows = host
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::RestoreHistory(rows) if !rows.is_empty() => Some(rows.clone()),
            _ => None,
        })
        .await
        .expect("RestoreHistory emitted on RebindPane");
    assert!(
        rebound_rows.iter().all(|row| match row {
            PersistentClawRow::Diagnosis { pane_id, .. }
            | PersistentClawRow::User { pane_id, .. }
            | PersistentClawRow::Suggested { pane_id, .. }
            | PersistentClawRow::ProcessList { pane_id, .. }
            | PersistentClawRow::ToolCall { pane_id, .. }
            | PersistentClawRow::SystemMessage { pane_id, .. }
            | PersistentClawRow::FileDiff { pane_id, .. } => pane_id == "pane-rebind-new",
            PersistentClawRow::Command { .. } => true,
        }),
        "RebindPane RestoreHistory must rewrite row pane_ids to `pane-rebind-new`"
    );

    let rebound_tasks = host
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::TaskStatusChanged { tasks, .. } if tasks.len() == 1 => {
                Some(tasks.clone())
            }
            _ => None,
        })
        .await
        .expect("RebindPane preserves scheduled tasks");
    assert_eq!(rebound_tasks[0].payload, "Post-Rebind Reminder");

    // 3. Run Turn 2 on the rebound pane and verify `Unique ID: pane-rebind-new` + `Pane Alias: pane-1`
    //    AND zero lingering headless workers in `GLOBAL RADAR`!
    host.send_query("Verify everything after moving pane to a new split.")
        .await
        .expect("send turn 2 after rebind");
    let (turn2_rebound, _) = host
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("turn 2 diagnosis after rebind");
    assert!(turn2_rebound.contains("pane-rebind-new"));

    let final_leader_conv = leader_llm
        .main_requests()
        .last()
        .expect("final leader req")
        .conversation_text();
    assert!(
        final_leader_conv.contains("Unique ID: `pane-rebind-new`")
            && final_leader_conv.contains("Pane Alias: `pane-1`"),
        "rebound pane must preserve alias `pane-1` under new Unique ID `pane-rebind-new`:\n{final_leader_conv}"
    );
    assert!(
        !final_leader_conv.contains("--- GLOBAL RADAR"),
        "completed Phantom Swarm headless worker must auto-reap and disappear from GLOBAL RADAR:\n{final_leader_conv}"
    );

    drop(pty);
    host.shutdown().await;
    ws.teardown().await;
}
