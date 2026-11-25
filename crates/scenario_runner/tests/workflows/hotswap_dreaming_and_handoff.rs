//! E2E Workflow 13:
//! - Live Model & Per-Character Override Hot-Swapping (`SettingsInvalidated`, `ToggleWebSearch`)
//!   and TUI Modality Warnings (`ForegroundProcessChanged`).
//! - Collaborative 1-Turn `invite_character(..., debate=false)` Handoff with Live Swarm Peer
//!   Home-Pane Context Bridging (`--- YOUR HOME PANE CONTEXT (SWARM PEER BRIDGE) ---`).
//! - Async Map-Reduce (`delegate_task_async` + `await_tasks`), Capability Routing
//!   (`request_assistance`), Multi-Round Scheduled Interval Tasks (`schedule_task` +
//!   `TogglePauseTask` + `CancelTask`), and Full 3-Phase `DreamOrchestrator::run_cycle()`
//!   with Existing Verified Memory Conflict Resolution & Protection Tiers.

use boxxy_claw::memories::dreaming::DreamOrchestrator;
use boxxy_claw_protocol::{AgentStatus, ClawEngineEvent, ClawMessage};
use boxxy_db::store::Store;
use boxxy_model_selection::{
    CustomModelConfig, ModelProvider, OpenAiApiRoute, OpenAiCompatibleEndpoint,
};
use boxxy_preferences::Settings;
use scenario_runner::{MockLlmServer, MockStep, TestWorkspace};
use serde_json::{Value, json};
use serial_test::serial;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
#[serial(boxxy_e2e)]
async fn live_model_and_character_override_hotswap_and_tui_modality_workflow() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    let override_llm = MockLlmServer::start().await.expect("start override server");
    let override_ep_name = format!("OverrideEndpoint-{}", Uuid::new_v4());

    let mut override_ep = OpenAiCompatibleEndpoint::preset_local_server();
    override_ep.name.clone_from(&override_ep_name);
    override_ep.base_url = override_llm.base_url();
    override_ep.zero_cost = false;
    override_ep.models = vec![
        CustomModelConfig {
            id: "mock-override-v1".to_string(),
            display_name: "Mock Override V1".to_string(),
            route: OpenAiApiRoute::ChatCompletions,
            context_window: Some(64_000),
            thinking_levels: Vec::new(),
            input_cost_per_1m: Some(1.0),
            output_cost_per_1m: Some(2.0),
        },
        CustomModelConfig {
            id: "mock-override-v2".to_string(),
            display_name: "Mock Override V2".to_string(),
            route: OpenAiApiRoute::ChatCompletions,
            context_window: Some(128_000),
            thinking_levels: Vec::new(),
            input_cost_per_1m: Some(2.0),
            output_cost_per_1m: Some(4.0),
        },
    ];

    let levi_id = scenario_runner::config_env::seeded_catalog()[1]
        .config
        .id
        .clone();
    Settings::update(|s| {
        s.openai_compatible_endpoints.push(override_ep.clone());
        s.enable_web_search = true;
        s.api_keys
            .insert("Tavily".to_string(), "tvly-synthetic-e2e-key".to_string());
        s.character_models.insert(
            levi_id.clone(),
            ModelProvider::OpenAiCompatible {
                provider_name: override_ep_name.clone(),
                model_id: "mock-override-v1".to_string(),
                thinking: None,
            },
        );
    });

    let mut levi_pane = ws
        .spawn_pane("pane-hotswap-levi", 1)
        .await
        .expect("spawn levi pane");
    levi_pane.wait_ready(TIMEOUT).await.expect("levi ready");

    // 1. Activate TUI modality (`vim`) on Levi's pane and run Turn 1 -> must route exclusively
    //    to `override_llm` (`mock-override-v1`) and inject `--- TUI WARNING ---`!
    levi_pane
        .send(ClawMessage::ForegroundProcessChanged {
            process_name: "vim".to_string(),
        })
        .await
        .expect("set foreground vim");

    override_llm.push_step(
        MockStep::text("Turn 1 via V1 override: vim TUI active.")
            .with_usage(600, 100),
    );
    levi_pane
        .send_query("How should we quit the editor in this pane?")
        .await
        .expect("send turn 1");
    let (r1, _) = levi_pane
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("turn 1 diagnosis");
    assert!(r1.contains("V1 override"));
    assert_eq!(
        ws.llm.main_requests().len(),
        0,
        "character model override must route strictly to override_llm, not global default"
    );

    let v1_reqs = override_llm.main_requests();
    assert_eq!(v1_reqs.len(), 1);
    assert_eq!(v1_reqs[0].model, "mock-override-v1");
    assert!(
        v1_reqs[0]
            .conversation_text()
            .contains("--- TUI WARNING ---")
            && v1_reqs[0]
                .conversation_text()
                .contains("Running: vim"),
        "active foreground TUI must inject --- TUI WARNING ---:\n{}",
        v1_reqs[0].conversation_text()
    );

    // 2. Exit TUI (`process_name: ""`), toggle Web Search ON (`Tavily`), and hot-swap Levi's
    //    override model from `mock-override-v1` (64k) to `mock-override-v2` (128k) via `SettingsInvalidated`!
    levi_pane
        .send(ClawMessage::ForegroundProcessChanged {
            process_name: String::new(),
        })
        .await
        .expect("clear foreground vim");
    levi_pane
        .send(ClawMessage::ToggleWebSearch(true))
        .await
        .expect("enable local web search");
    levi_pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::WebSearchStatusChanged(true) => Some(()),
            _ => None,
        })
        .await
        .expect("WebSearchStatusChanged(true)");

    Settings::update(|s| {
        s.character_models.insert(
            levi_id.clone(),
            ModelProvider::OpenAiCompatible {
                provider_name: override_ep_name.clone(),
                model_id: "mock-override-v2".to_string(),
                thinking: None,
            },
        );
    });
    levi_pane
        .send(ClawMessage::SettingsInvalidated)
        .await
        .expect("notify settings invalidated");

    let updated_window = levi_pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::Identity {
                context_window: Some(128_000),
                total_tokens: 700,
                ..
            } => Some(128_000_u64),
            _ => None,
        })
        .await
        .expect("Identity updated to 128k context_window while preserving 700 total_tokens");
    assert_eq!(updated_window, 128_000);

    let hot_banner = levi_pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::SystemMessage { text } if text.contains("mock-override-v2") => {
                Some(text.clone())
            }
            _ => None,
        })
        .await
        .expect("hot-swap SystemMessage banner");
    assert!(
        hot_banner.contains("Search: Tavily"),
        "banner must reflect per-character web search state: {hot_banner}"
    );

    // 3. Turn 2 runs on the hot-swapped `mock-override-v2` model with `web_search` tool enabled,
    //    retained Turn 1 history, and 0-token silence on `--- TUI WARNING ---`!
    override_llm.push_step(
        MockStep::text("Turn 2 via hot-swapped V2 model with history intact.")
            .with_usage(900, 120),
    );
    levi_pane
        .send_query("Confirm shell returned to normal and summarize our first step.")
        .await
        .expect("send turn 2");
    let (r2, _) = levi_pane
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("turn 2 diagnosis");
    assert!(r2.contains("hot-swapped V2"));

    let all_override_reqs = override_llm.main_requests();
    assert_eq!(all_override_reqs.len(), 2);
    let v2_req = &all_override_reqs[1];
    assert_eq!(v2_req.model, "mock-override-v2");
    assert!(
        v2_req.tool_names().iter().any(|t| t == "web_search"),
        "ToggleWebSearch(true) + Tavily key must rebuild agent with web_search tool"
    );
    assert!(
        v2_req
            .conversation_text()
            .contains("Turn 1 via V1 override: vim TUI active."),
        "hot-swapping models must retain prior conversation history"
    );
    assert!(
        !v2_req.conversation_text().contains("--- TUI WARNING ---"),
        "clearing foreground process must remove TUI WARNING on subsequent turns"
    );

    levi_pane.shutdown().await;
    ws.teardown().await;
}

#[tokio::test]
#[serial(boxxy_e2e)]
async fn collaborative_one_turn_invite_with_swarm_peer_home_pane_bridge_workflow() {
    let mut ws = TestWorkspace::new().await.expect("workspace");

    // Spawn Host (`Niko Una`, `pane-1`) and live Swarm Peer (`Levi Kujo`, `pane-2`).
    let mut host = ws
        .spawn_pane("pane-collab-host", 0)
        .await
        .expect("spawn host");
    host.wait_ready(TIMEOUT).await.expect("host ready");

    let mut peer_levi = ws
        .spawn_pane("pane-collab-levi-home", 1)
        .await
        .expect("spawn levi home pane");
    peer_levi.wait_ready(TIMEOUT).await.expect("levi ready");
    peer_levi
        .set_terminal_snapshot("$ perf stat ./bench\n     0.042 msec task-clock\n")
        .await;

    // 1. Host calls `invite_character(character="levi-kujo", ..., debate=false)` for a 1-turn handoff.
    // 2. Levi steps into Host's overlay for 1 turn, sees his own `pane-2` live snapshot via
    //    `SWARM PEER BRIDGE`, and even `@mentions` `@niko-una` in his reply — because `debate: false`
    //    grants a strict 1-turn collaborative handoff, the overlay MUST NOT ping-pong back to Niko!
    ws.llm.push_steps([
        MockStep::tool_call(
            "invite_character",
            &json!({
                "character": "levi-kujo",
                "topic": "Inspect your perf stat output and advise on cache locality.",
                "debate": false
            }),
        ),
        MockStep::text("Stepping aside for @levi-kujo to review his perf numbers."),
        MockStep::text(
            "Task-clock is 0.042 msec in `pane-2` — align the hot struct to 64 bytes, @niko-una.",
        ),
    ]);

    host.send_query("Ask Levi to step in and review the perf run from his pane.")
        .await
        .expect("send host query");

    let (host_opening, _) = host
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("host opening reply");
    assert!(host_opening.contains("Stepping aside"));

    let (guest_handoff_reply, _) = host
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("guest 1-turn collaborative reply");
    assert!(guest_handoff_reply.contains("0.042 msec"));

    host.wait_for_status(&AgentStatus::Waiting, TIMEOUT)
        .await
        .expect("1-turn collaborative handoff must settle into Waiting without ping-ponging");

    let reqs = ws.llm.main_requests();
    assert_eq!(
        reqs.len(),
        3,
        "expected exactly 2 host steps (tool + text) + 1 guest step (no ping-pong)"
    );

    let guest_req = &reqs[2];
    assert!(
        guest_req
            .system_prompt_text()
            .contains("Full-Metal Kernel Alchemist"),
        "invited guest must run with its own character system prompt"
    );
    let guest_conv = guest_req.conversation_text();
    assert!(
        guest_conv.contains("[COLLABORATIVE HANDOFF — 1 Turn]"),
        "debate=false invitation must emit [COLLABORATIVE HANDOFF — 1 Turn]:\n{guest_conv}"
    );
    assert!(
        guest_conv.contains("--- YOUR HOME PANE CONTEXT (SWARM PEER BRIDGE) ---")
            && guest_conv.contains("0.042 msec task-clock"),
        "guest actively claimed in pane-2 must receive live home-pane snapshot bridge:\n{guest_conv}"
    );

    host.shutdown().await;
    peer_levi.shutdown().await;
    ws.teardown().await;
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
async fn async_map_reduce_tasks_scheduled_timers_and_dream_consolidation_workflow() {
    let mut ws = TestWorkspace::new().await.expect("workspace");

    // Configure Worker (`Levi Kujo`, character_index 1) with its own `MockLlmServer` so
    // concurrent `delegate_task_async` + `request_assistance` steps never race Leader's queue!
    let worker_llm = MockLlmServer::start().await.expect("worker mock server");
    let worker_ep_name = format!("WorkerEndpoint-{}", Uuid::new_v4());
    let mut worker_ep = OpenAiCompatibleEndpoint::preset_local_server();
    worker_ep.name.clone_from(&worker_ep_name);
    worker_ep.base_url = worker_llm.base_url();
    worker_ep.models = vec![CustomModelConfig {
        id: "mock-worker-model".to_string(),
        display_name: "Mock Worker Model".to_string(),
        route: OpenAiApiRoute::ChatCompletions,
        context_window: Some(64_000),
        thinking_levels: Vec::new(),
        input_cost_per_1m: Some(1.0),
        output_cost_per_1m: Some(2.0),
    }];
    let levi_id = scenario_runner::config_env::seeded_catalog()[1]
        .config
        .id
        .clone();
    Settings::update(|s| {
        s.openai_compatible_endpoints.push(worker_ep.clone());
        s.character_models.insert(
            levi_id.clone(),
            ModelProvider::OpenAiCompatible {
                provider_name: worker_ep_name.clone(),
                model_id: "mock-worker-model".to_string(),
                thinking: None,
            },
        );
    });

    ws.write_skill(
        "oci-specialist",
        "---\nname: OCI Specialist\ndescription: Container layer & manifest specialist\ntriggers:\n  - oci-manifest\n---\nAlways verify sha256 digests.\n",
    )
    .await
    .expect("write oci-specialist skill");

    let mut leader = ws
        .spawn_pane("pane-mapreduce-leader", 0)
        .await
        .expect("spawn leader");
    leader.wait_ready(TIMEOUT).await.expect("leader ready");

    let mut specialist = ws
        .spawn_pane("pane-mapreduce-specialist", 1)
        .await
        .expect("spawn specialist");
    specialist
        .wait_ready(TIMEOUT)
        .await
        .expect("specialist ready");

    // Prime `specialist` (`pane-2`) with active skill `"OCI Specialist"` on Turn 1.
    worker_llm.push_step(MockStep::text("OCI Specialist online."));
    specialist
        .send_query("Load oci-manifest inspection workflow.")
        .await
        .expect("prime specialist skill");
    specialist
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("specialist primed");
    // Drain incoming + outgoing bubbles cleanly on subsequent delegated tasks.
    let _ = specialist.advance_cursor_to_end();

    // Queue 2 responses on `worker_llm`:
    //   1) For `delegate_task_async`
    //   2) For `request_assistance(capability="oci")`
    worker_llm.push_steps([
        MockStep::text("Async Audit Result: 0 unsafe blocks found."),
        MockStep::text("Specialist OCI Result: manifest digest sha256 verified."),
    ]);

    // Queue Leader's multi-step orchestration turn on `ws.llm`:
    //   1) `delegate_task_async` -> `pane-2`
    //   2) `MockStep::dynamic` reads the minted `task_id` from the tool output and calls `await_tasks([task_id])`!
    //   3) `request_assistance(capability="oci", ...)` routes by active skill `"OCI Specialist"` to `pane-2`!
    //   4) `schedule_task` creates a 2-round interval timer (`due_in_seconds: 120`, `rounds: 2`, `break_seconds: 30`)!
    //   5) Final synthesis prose!
    ws.llm.push_steps([
        MockStep::tool_call(
            "delegate_task_async",
            &json!({
                "agent_name": "pane-2",
                "prompt": "Run async unsafe-code audit."
            }),
        ),
        MockStep::dynamic(|req| {
            let task_id = extract_json_field_from_messages(&req.messages, "task_id")
                .expect("extract dynamic task_id from delegate_task_async ToolResult");
            MockStep::tool_call("await_tasks", &json!({ "task_ids": [task_id] }))
        }),
        MockStep::tool_call(
            "request_assistance",
            &json!({
                "capability": "oci",
                "prompt": "Verify the OCI image manifest digest."
            }),
        ),
        MockStep::tool_call(
            "schedule_task",
            &json!({
                "due_in_seconds": 120,
                "task_type": "notification",
                "payload": "Deep Work Sprint",
                "rounds": 2,
                "break_seconds": 30,
                "round_label": "Sprint"
            }),
        ),
        MockStep::text(
            "Combined async audit, OCI specialist check, and scheduled 2-sprint timer.",
        ),
    ]);

    leader
        .send_query("Fan out async audit, consult the OCI specialist, and start a 2-sprint timer.")
        .await
        .expect("send leader map-reduce query");

    let (leader_summary, _) = leader
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("leader map-reduce turn complete");
    assert!(leader_summary.contains("2-sprint timer"));

    let leader_reqs = ws.llm.main_requests();
    let final_leader_conv = leader_reqs.last().expect("final leader req").conversation_text();
    assert!(
        final_leader_conv.contains("Async Audit Result: 0 unsafe blocks found."),
        "await_tasks must return async worker result to Leader:\n{final_leader_conv}"
    );
    assert!(
        final_leader_conv.contains("Specialist OCI Result: manifest digest sha256 verified."),
        "request_assistance must route by active skill to specialist peer:\n{final_leader_conv}"
    );

    // 2. Verify scheduled interval task was emitted and supports urgent Pause + Cancel!
    let scheduled_task = leader
        .all_events()
        .into_iter()
        .rev()
        .find_map(|ev| match ev {
            ClawEngineEvent::TaskStatusChanged { tasks, .. } if tasks.len() == 1 => {
                Some(tasks[0].clone())
            }
            _ => None,
        })
        .expect("TaskStatusChanged with 1 scheduled interval task");
    assert_eq!(scheduled_task.payload, "Deep Work Sprint");
    assert_eq!(
        scheduled_task.interval.as_ref().map(|i| i.total_rounds),
        Some(2)
    );
    let _ = leader.advance_cursor_to_end();

    leader
        .send(ClawMessage::TogglePauseTask {
            task_id: scheduled_task.id,
        })
        .await
        .expect("pause task");
    let paused_task = leader
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::TaskStatusChanged { tasks, .. }
                if tasks.len() == 1 && tasks[0].is_paused() =>
            {
                Some(tasks[0].clone())
            }
            _ => None,
        })
        .await
        .expect("task paused");
    assert!(paused_task.paused_remaining_ms.is_some());

    leader
        .send(ClawMessage::CancelTask {
            task_id: scheduled_task.id,
        })
        .await
        .expect("cancel task");
    leader
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::TaskStatusChanged { tasks, .. } if tasks.is_empty() => Some(()),
            _ => None,
        })
        .await
        .expect("task cancelled");

    // 3. Full 3-Phase `DreamOrchestrator::run_cycle()` against `ws.llm` (`mock-memory-model`):
    //    - Existing candidate `build_linker` (`observation_count = 1`) -> reinforced (`count = 2`, `0.95`) -> promoted to `dreamed` (`verified = true`)!
    //    - Existing verified `dreamed` fact `package_runner` (`npm`) -> flagged in `conflicts` -> demoted & re-candidated (`pnpm`)!
    //    - Protected `preference` fact `editor_choice` (`helix`) -> even if flagged in `conflicts`, protection tier refuses demotion!
    let ws_scope = ws.path().to_string_lossy().into_owned();
    let store = Store::new(ws.db.pool());
    store
        .ensure_session_exists(leader.session_id(), leader.character_id(), "Niko Una", &ws_scope)
        .await
        .expect("ensure session");
    store
        .add_interaction(
            leader.session_id(),
            Some(&ws_scope),
            "Configured mold linker and migrated scripts from npm to pnpm.",
            Some("flush_summary"),
            None,
        )
        .await
        .expect("seed undreamed interaction");
    store
        .upsert_dream_candidate(
            "build_linker",
            Some(&ws_scope),
            "Uses mold fast linker for Rust debug builds",
            0.6,
        )
        .await
        .expect("seed candidate observation 1");

    // Seed a `dreamed` verified fact (via 2 observations + promote) and a protected `preference` fact.
    store
        .upsert_dream_candidate("package_runner", Some(&ws_scope), "Uses npm", 0.9)
        .await
        .expect("seed npm 1");
    store
        .upsert_dream_candidate("package_runner", Some(&ws_scope), "Uses npm", 0.9)
        .await
        .expect("seed npm 2");
    store
        .promote_threshold_memories(2, 0.8)
        .await
        .expect("promote initial npm fact");
    store
        .add_memory(
            "editor_choice",
            Some(&ws_scope),
            "Uses helix editor",
            Some("preference"),
            true,
            false,
        )
        .await
        .expect("seed protected preference");

    let dream_json = serde_json::to_string(&json!({
        "facts": [
            {
                "key": "build_linker",
                "project_path": ws_scope,
                "content": "Uses mold fast linker for Rust debug builds",
                "confidence_score": 0.95
            }
        ],
        "patterns": [
            "Prefers fast incremental Rust linkers"
        ],
        "conflicts": [
            {
                "key": "package_runner",
                "issue": "Migrated workspace from npm to pnpm",
                "resolved_content": "Uses pnpm"
            },
            {
                "key": "editor_choice",
                "issue": "Attempted demotion of protected user preference",
                "resolved_content": "Uses nano"
            }
        ]
    }))
    .unwrap();
    ws.llm.push_aux_text(dream_json);

    let settings_snap = Settings::load();
    let creds = boxxy_ai_core::AiCredentials::new(
        settings_snap.get_effective_api_keys(),
        settings_snap.ollama_base_url.clone(),
    )
    .with_custom_endpoints(settings_snap.openai_compatible_endpoints.clone());
    let orchestrator = DreamOrchestrator::new(
        Arc::new(Mutex::new(Some(ws.db.clone()))),
        creds,
        settings_snap.effective_memory_model(),
    );
    orchestrator
        .run_cycle()
        .await
        .expect("run full 3-phase Dream Cycle");

    // Verify Dream Auditor received `EXISTING VERIFIED MEMORIES` on the wire!
    let aux_reqs = ws.llm.aux_requests();
    let dream_req = aux_reqs
        .iter()
        .rfind(|r| r.system_prompt_text().contains("Boxxy Dream Auditor"))
        .expect("Dream Auditor LLM request recorded");
    let dream_conv = dream_req.conversation_text();
    assert!(
        dream_conv.contains("EXISTING VERIFIED MEMORIES")
            && dream_conv.contains("package_runner: Uses npm")
            && dream_conv.contains("editor_choice: Uses helix editor"),
        "DreamOrchestrator::run_cycle must pass existing verified memories for conflict detection:\n{dream_conv}"
    );

    // Verify promotions, conflict demotion, protection tier, and `MEMORY.md` + `DREAMS.md` audit files!
    let promoted_linker = store
        .get_memory_by_key("build_linker", Some(&ws_scope))
        .await
        .unwrap()
        .expect("build_linker row");
    assert_eq!(promoted_linker.verified, Some(true));
    assert_eq!(promoted_linker.category.as_deref(), Some("dreamed"));
    assert_eq!(promoted_linker.observation_count, 2);

    let demoted_runner = store
        .get_memory_by_key("package_runner", Some(&ws_scope))
        .await
        .unwrap()
        .expect("package_runner row");
    assert_eq!(demoted_runner.verified, Some(false));
    assert_eq!(demoted_runner.content, "Uses pnpm");

    let protected_editor = store
        .get_memory_by_key("editor_choice", Some(&ws_scope))
        .await
        .unwrap()
        .expect("protected editor_choice row");
    assert_eq!(protected_editor.verified, Some(true));
    assert_eq!(protected_editor.category.as_deref(), Some("preference"));
    assert_eq!(
        protected_editor.content, "Uses helix editor",
        "protected preference memories must never be overwritten or demoted by Dream Cycle"
    );

    let dreams_md = tokio::fs::read_to_string(ws.config_root().join("boxxyclaw").join("DREAMS.md"))
        .await
        .expect("read DREAMS.md");
    assert!(
        dreams_md.contains("🛡️ Verified: build_linker"),
        "DREAMS.md must record 🛡️ Verified audit line:\n{dreams_md}"
    );
    let memory_md = tokio::fs::read_to_string(ws.config_root().join("boxxyclaw").join("MEMORY.md"))
        .await
        .expect("read MEMORY.md");
    assert!(
        memory_md.contains("build_linker") && memory_md.contains("editor_choice"),
        "MEMORY.md mirror must sync promoted and active memories"
    );

    leader.shutdown().await;
    specialist.shutdown().await;
    ws.teardown().await;
}
