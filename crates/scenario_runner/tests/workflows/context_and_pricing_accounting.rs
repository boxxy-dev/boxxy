//! E2E Workflow 19: Sidebar Context Window & Pricing Accounting Parity (`OpenCode` + `rig 0.47`):
//! - Per-call long-context tier isolation vs single-call `> 272k` tier activation & cached-input discounts
//! - Turn-0 `/btw` side-channel cost accumulation without polluting active `context_tokens` / `% used`
//! - Mid-turn billed step salvage (`InFlightTranscriptHook::on_model_turn_finished`) on proposal rejection
//! - Post-compaction `DiagnosisComplete` & `UsageUpdated` live `context_tokens` synchronization
//! - `[SILENT_ACK]` usage delivery, custom endpoint `context_window` hot-reload, Guest Roundtable
//!   Host-window preservation, and Phantom Swarm (`summon_headless_worker`) subagent spend rollup.

use boxxy_claw_protocol::{AgentStatus, ClawEngineEvent, ClawMessage, SessionUsage};
use boxxy_db::store::Store;
use boxxy_model_selection::{
    CustomModelConfig, ModelProvider, OpenAiApiRoute, OpenAiCompatibleEndpoint,
};
use boxxy_preferences::Settings;
use scenario_runner::{MockLlmServer, MockStep, TestWorkspace};
use serde_json::{Value, json};
use serial_test::serial;
use std::time::Duration;
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(6);

fn extract_json_field_from_messages(messages: &[Value], field: &str) -> Option<String> {
    for msg in messages.iter().rev() {
        let mut texts = Vec::new();
        if let Some(s) = msg.get("content").and_then(Value::as_str) {
            texts.push(s);
        } else if let Some(arr) = msg.get("content").and_then(Value::as_array) {
            for item in arr {
                if let Some(s) = item.get("text").and_then(Value::as_str) {
                    texts.push(s);
                }
            }
        }
        for raw in texts {
            if let Ok(parsed) = serde_json::from_str::<Value>(raw)
                && let Some(val) = parsed.get(field).and_then(Value::as_str)
            {
                return Some(val.to_string());
            }
        }
    }
    None
}

#[tokio::test]
#[serial(boxxy_e2e)]
async fn per_call_tiered_pricing_btw_isolation_salvage_and_headless_rollup_workflow() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    ws.write_file(
        "src/module.rs",
        "pub fn compute_budget(units: u64) -> u64 {\n    units * 2\n}\n",
    )
    .await
    .expect("write module.rs");

    // Configure Host character (`Niko Una`) to route `gpt-6.1-terra` through a local mock gateway
    // WITHOUT custom $/1M overrides so `enrich_prompt_response_usage` resolves `gpt-6.1-terra` from
    // `Catalog::builtin()` (1,050,000 context window; base $1/$5/$0.05 cache; >272k $2/$7.5/$0.10 cache).
    // Configure Guest character (`Levi Kujo`) on a separate 256k custom model so we can also verify
    // Guest Roundtable turns never clobber the Host pane's `context_window`.
    let terra_llm = MockLlmServer::start().await.expect("terra mock server");
    let guest_llm = MockLlmServer::start().await.expect("guest mock server");

    let terra_ep_name = format!("TerraGateway-{}", Uuid::new_v4());
    let mut terra_ep = OpenAiCompatibleEndpoint::preset_local_server();
    terra_ep.name.clone_from(&terra_ep_name);
    terra_ep.base_url = terra_llm.base_url();
    terra_ep.zero_cost = false;
    terra_ep.models = vec![CustomModelConfig {
        id: "gpt-6.1-terra".to_string(),
        display_name: "Synthetic Terra Gateway".to_string(),
        route: OpenAiApiRoute::ChatCompletions,
        context_window: None,
        thinking_levels: Vec::new(),
        input_cost_per_1m: None,
        output_cost_per_1m: None,
    }];

    let guest_ep_name = format!("GuestGateway-{}", Uuid::new_v4());
    let mut guest_ep = OpenAiCompatibleEndpoint::preset_local_server();
    guest_ep.name.clone_from(&guest_ep_name);
    guest_ep.base_url = guest_llm.base_url();
    guest_ep.zero_cost = false;
    guest_ep.models = vec![CustomModelConfig {
        id: "mock-guest-256k".to_string(),
        display_name: "Synthetic Guest 256k".to_string(),
        route: OpenAiApiRoute::ChatCompletions,
        context_window: Some(256_000),
        thinking_levels: Vec::new(),
        input_cost_per_1m: Some(1.0),
        output_cost_per_1m: Some(2.0),
    }];

    let catalog = scenario_runner::config_env::seeded_catalog();
    let niko_id = catalog[0].config.id.clone();
    let levi_id = catalog[1].config.id.clone();

    Settings::update(|s| {
        s.openai_compatible_endpoints.push(terra_ep.clone());
        s.openai_compatible_endpoints.push(guest_ep.clone());
        s.character_models.insert(
            niko_id.clone(),
            ModelProvider::OpenAiCompatible {
                provider_name: terra_ep_name.clone(),
                model_id: "gpt-6.1-terra".to_string(),
                thinking: None,
            },
        );
        s.character_models.insert(
            levi_id.clone(),
            ModelProvider::OpenAiCompatible {
                provider_name: guest_ep_name.clone(),
                model_id: "mock-guest-256k".to_string(),
                thinking: None,
            },
        );
    });

    let mut pane = ws.spawn_pane("pane-pricing-1", 0).await.expect("spawn pane");
    let init_win = pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::Identity { context_window, .. } => Some(*context_window),
            _ => None,
        })
        .await
        .expect("initial Identity");
    assert_eq!(init_win, Some(1_050_000));

    let mut ui_usage = SessionUsage {
        context_window: init_win,
        ..SessionUsage::default()
    };

    // 1. Turn-0 `/btw` side question: accumulates `total_tokens` & `cost_usd` while keeping
    //    `display_tokens() == 0` and `context_percent() == Some(0)`.
    terra_llm.push_btw_step(
        MockStep::text("Side answer: budget doubles units.").with_usage(10_000, 1_000),
    );
    let btw_id = pane
        .send_btw("Quick side check before starting.")
        .await
        .expect("send Turn-0 /btw");
    let btw_usage = pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::BtwComplete {
                request_id,
                usage: Some(u),
                error: None,
                ..
            } if *request_id == btw_id => Some(u.clone()),
            _ => None,
        })
        .await
        .expect("Turn-0 BtwComplete");
    ui_usage.record_side_turn(&btw_usage);
    assert_eq!(ui_usage.total_tokens, 11_000);
    assert_eq!(ui_usage.display_tokens(), 0);
    assert_eq!(ui_usage.context_percent(), Some(0));
    // 10k * $1/M + 1k * $5/M = $0.015
    assert!((ui_usage.cost_usd - 0.015).abs() < 1e-6);

    // 2. Multi-step tool turn where two 160k calls sum to 320k (> 272k tier), with 60k cached on Step 2:
    //    - Step 1: 160k uncached ($0.16) + 1k output ($0.005) = $0.165
    //    - Step 2: 100k uncached ($0.10) + 60k cached ($0.003) + 1k output ($0.005) = $0.108
    //    Combined turn cost MUST be $0.273 (base tier per-call), NOT 2x >272k tier ($0.541),
    //    and `context_tokens` MUST be Step 2's 161,000 (not 322,000 summed)!
    terra_llm.push_steps([
        MockStep::tool_call("file_read", &json!({ "path": "src/module.rs" }))
            .with_usage(160_000, 1_000),
        MockStep::text("Inspected `src/module.rs` across two base-tier calls.")
            .with_usage(160_000, 1_000)
            .with_cached_tokens(60_000),
    ]);
    pane.send_query("Inspect `src/module.rs`.")
        .await
        .expect("send turn 1");

    let diag_usage = pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::DiagnosisComplete { usage: Some(u), .. } => Some(u.clone()),
            _ => None,
        })
        .await
        .expect("turn 1 DiagnosisComplete");
    assert_eq!(diag_usage.context_tokens, Some(161_000));
    assert!((diag_usage.cost_usd.unwrap_or(0.0) - 0.273).abs() < 1e-6);

    let (u1_total, u1_ctx, u1_win, u1_cost) = pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::UsageUpdated {
                total_tokens,
                context_tokens,
                context_window,
                total_cost_usd,
            } => Some((*total_tokens, *context_tokens, *context_window, *total_cost_usd)),
            _ => None,
        })
        .await
        .expect("turn 1 UsageUpdated");
    //Note: Turn-0 `/btw` ran before Turn 1 created the DB row, and in-memory `SessionState` holds both:
    // 11,000 (/btw) + 322,000 (Turn 1) = 333,000; cost = $0.015 + $0.273 = $0.288!
    assert_eq!(u1_total, 333_000);
    assert_eq!(u1_ctx, 161_000);
    assert_eq!(u1_win, Some(1_050_000));
    assert!((u1_cost - 0.288).abs() < 1e-6);

    // 3. Single call exceeding 272k (`300,000` input with `100,000` cached + `2,000` output):
    //    >272k tier applies: 200k * $2/M ($0.40) + 100k * $0.10/M ($0.01) + 2k * $7.5/M ($0.015) = $0.425!
    terra_llm.push_step(
        MockStep::text("Processed large context over 272k threshold.")
            .with_usage(300_000, 2_000)
            .with_cached_tokens(100_000),
    );
    pane.send_query("Analyze large context.")
        .await
        .expect("send turn 2");
    let diag2_usage = pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::DiagnosisComplete { usage: Some(u), .. } => Some(u.clone()),
            _ => None,
        })
        .await
        .expect("turn 2 DiagnosisComplete");
    assert_eq!(diag2_usage.context_tokens, Some(302_000));
    assert!((diag2_usage.cost_usd.unwrap_or(0.0) - 0.425).abs() < 1e-6);
    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::UsageUpdated {
            context_tokens: 302_000,
            ..
        } => Some(()),
        _ => None,
    })
    .await
    .expect("turn 2 UsageUpdated");

    // 4. Mid-turn Proposal Rejection Step Salvage:
    //    Step 1 calls `file_read` (20k input + 1k output = $0.025), Step 2 proposes `replace_in_file`
    //    (25k input + 1k output = $0.030, context = 26,000) and pauses for approval.
    //    User rejects (`FileWriteReply { approved: false }`) -> 0 extra LLM calls, and the 47,000
    //    tokens + $0.055 spend + 26,000 context tokens from Steps 1 & 2 MUST be salvaged into
    //    `UsageUpdated` and SQLite!
    terra_llm.push_steps([
        MockStep::tool_call("file_read", &json!({ "path": "src/module.rs" }))
            .with_usage(20_000, 1_000),
        MockStep::tool_call(
            "replace_in_file",
            &json!({
                "path": "src/module.rs",
                "old_string": "units * 2",
                "new_string": "units * 3"
            }),
        )
        .with_usage(25_000, 1_000),
    ]);
    pane.send_query("Propose updating `src/module.rs` multiplier.")
        .await
        .expect("send turn 3");
    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::ProposeFileWrite {
            auto_approve: false,
            ..
        } => Some(()),
        _ => None,
    })
    .await
    .expect("awaiting file write approval");

    pane.reply_file_write(false)
        .await
        .expect("reject file write");

    let (salvaged_total, salvaged_ctx, salvaged_cost) = pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::UsageUpdated {
                total_tokens,
                context_tokens,
                total_cost_usd,
                ..
            } => Some((*total_tokens, *context_tokens, *total_cost_usd)),
            _ => None,
        })
        .await
        .expect("UsageUpdated emitted on mid-turn proposal rejection salvage");
    // Prior 333,000 + 302,000 = 635,000; + 47,000 salvaged = 682,000!
    // Prior $0.288 + $0.425 = $0.713; + $0.055 salvaged = $0.768!
    assert_eq!(salvaged_total, 682_000);
    assert_eq!(salvaged_ctx, 26_000);
    assert!((salvaged_cost - 0.768).abs() < 1e-6);

    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::ProposalResolved {
            approved: false, ..
        } => Some(()),
        _ => None,
    })
    .await
    .expect("ProposalResolved false");

    let store = Store::new(ws.db.pool());
    let db_after_salvage = store
        .get_session(pane.session_id())
        .await
        .expect("query session")
        .expect("session row");
    assert_eq!(db_after_salvage.total_tokens, 682_000);
    assert_eq!(db_after_salvage.context_tokens, 26_000);
    assert!((db_after_salvage.total_cost_usd - 0.768).abs() < 1e-6);

    // 5. `[SILENT_ACK]`, Custom Endpoint `context_window` Hot-Reload on Same Model,
    //    Guest Roundtable Host-Window Preservation, & Phantom Swarm Subagent Spend Rollup!
    terra_llm.push_step(MockStep::text("[SILENT_ACK]").with_usage(10_000, 200));
    pane.send_query("Acknowledge silently.")
        .await
        .expect("send silent ack turn");
    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::DismissDrawer => Some(()),
        _ => None,
    })
    .await
    .expect("DismissDrawer");
    let (ack_total, ack_ctx, ack_cost) = pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::UsageUpdated {
                total_tokens,
                context_tokens,
                total_cost_usd,
                ..
            } => Some((*total_tokens, *context_tokens, *total_cost_usd)),
            _ => None,
        })
        .await
        .expect("UsageUpdated after [SILENT_ACK]");
    assert_eq!(ack_total, 692_200);
    assert_eq!(ack_ctx, 10_200);
    // + (10k * $1/M + 200 * $5/M = $0.011) -> $0.779
    assert!((ack_cost - 0.779).abs() < 1e-6);

    // Hot-edit `terra_ep`'s `context_window` override (None -> Some(128_000)) WITHOUT changing model_id!
    Settings::update(|s| {
        if let Some(ep) = s
            .openai_compatible_endpoints
            .iter_mut()
            .find(|e| e.name == terra_ep_name)
            && let Some(m) = ep.models.first_mut()
        {
            m.context_window = Some(128_000);
        }
    });
    pane.send(ClawMessage::SettingsInvalidated)
        .await
        .expect("send SettingsInvalidated");
    let hot_win = pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::UsageUpdated {
                context_window: Some(128_000),
                ..
            } => Some(128_000_u64),
            _ => None,
        })
        .await
        .expect("UsageUpdated reflects custom context_window edit on same model");
    assert_eq!(hot_win, 128_000);

    // Invite Guest (`Levi Kujo`, 256k window) for 1-turn collaborative handoff (`debate: false`) —
    // verify `context_window` stays anchored to Host `Niko Una`'s `128_000` window!
    terra_llm.push_steps([
        MockStep::tool_call(
            "invite_character",
            &json!({
                "character": "levi-kujo",
                "topic": "Share a one-line review of `compute_budget`.",
                "debate": false
            }),
        )
        .with_usage(5_000, 200),
        MockStep::text("Handing off to Levi Kujo for a quick review.").with_usage(5_500, 200),
    ]);
    guest_llm.push_step(
        MockStep::text("Levi review: overflow-check `units * 2` if untrusted.")
            .with_usage(6_000, 500),
    );
    pane.send_query("Ask Levi Kujo for a quick one-line review.")
        .await
        .expect("send collab handoff");

    // Wait for Host's DiagnosisComplete, then Levi's Guest DiagnosisComplete!
    let _ = pane
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("host handoff diagnosis");
    let guest_diag_win = pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::DiagnosisComplete {
                diagnosis,
                usage: Some(u),
                ..
            } if diagnosis.contains("Levi review:") => Some(u.context_window),
            _ => None,
        })
        .await
        .expect("guest handoff DiagnosisComplete");
    assert_eq!(
        guest_diag_win,
        Some(128_000),
        "Guest Roundtable turn must preserve Host character's 128k context_window"
    );
    pane.wait_for_status(&AgentStatus::Waiting, TIMEOUT)
        .await
        .expect("settled after 1-turn guest handoff");
    pane.advance_cursor_to_end();

    // Summon a Phantom Swarm headless worker (`ws.llm`: `mock-claw-model` @ $2/M in, $4/M out):
    // Worker spends 10,000 in ($0.02) + 1,000 out ($0.004) = 11,000 tokens / $0.024, which MUST
    // roll up into the parent session's `total_tokens` and `total_cost_usd`!
    let pre_swarm = store
        .get_session(pane.session_id())
        .await
        .unwrap()
        .unwrap();

    ws.llm.push_step(
        MockStep::text("Worker verified arithmetic bounds.").with_usage(10_000, 1_000),
    );
    terra_llm.push_steps([
        MockStep::tool_call(
            "summon_headless_worker",
            &json!({
                "profile": "bounds-checker",
                "prompt": "Check arithmetic bounds in src/module.rs."
            }),
        )
        .with_usage(4_000, 200),
        MockStep::dynamic(|req| {
            let task_id = extract_json_field_from_messages(&req.messages, "task_id")
                .expect("extract headless task_id");
            MockStep::tool_call("await_tasks", &json!({ "task_ids": [task_id] }))
                .with_usage(4_500, 200)
        }),
        MockStep::text("Headless bounds check finished.").with_usage(5_000, 300),
    ]);

    pane.send_query("Run a headless bounds check and wait for its result.")
        .await
        .expect("send summon_headless_worker turn");
    let (final_diag, _) = pane
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("final headless rollup diagnosis");
    assert!(final_diag.contains("Headless bounds check finished"));

    let post_swarm = store
        .get_session(pane.session_id())
        .await
        .unwrap()
        .unwrap();
    // Parent spent (4000+200) + (4500+200) + (5000+300) = 14,200 tokens ($0.0135 + $0.0035 = $0.0170)
    // Worker spent (10000+1000) = 11,000 tokens ($0.020 + $0.004 = $0.0240)
    // Combined delta = +25,200 tokens and +$0.0410!
    assert_eq!(post_swarm.total_tokens - pre_swarm.total_tokens, 25_200);
    assert!(
        ((post_swarm.total_cost_usd - pre_swarm.total_cost_usd) - 0.041).abs() < 1e-6,
        "expected +$0.041 parent + headless worker cost rollup, got {} -> {}",
        pre_swarm.total_cost_usd,
        post_swarm.total_cost_usd
    );

    pane.shutdown().await;
    ws.teardown().await;
}

#[tokio::test]
#[serial(boxxy_e2e)]
async fn compaction_immediately_syncs_scaled_context_tokens_to_ui_events() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    let small_llm = MockLlmServer::start().await.expect("small 16k mock server");
    let ep_name = format!("CompactSync-{}", Uuid::new_v4());
    let mut ep = OpenAiCompatibleEndpoint::preset_local_server();
    ep.name.clone_from(&ep_name);
    ep.base_url = small_llm.base_url();
    ep.models = vec![CustomModelConfig {
        id: "mock-compact-16k".to_string(),
        display_name: "Synthetic 16k".to_string(),
        route: OpenAiApiRoute::ChatCompletions,
        context_window: Some(16_000),
        thinking_levels: Vec::new(),
        input_cost_per_1m: None,
        output_cost_per_1m: None,
    }];
    Settings::update(|s| {
        s.openai_compatible_endpoints.push(ep.clone());
        s.claw_model = Some(ModelProvider::OpenAiCompatible {
            provider_name: ep_name.clone(),
            model_id: "mock-compact-16k".to_string(),
            thinking: None,
        });
    });

    let mut pane = ws.spawn_pane("pane-compact-sync", 0).await.expect("pane");
    pane.wait_ready(TIMEOUT).await.expect("ready");

    // Exchange 1 & 2 build history; Exchange 3 reports 14,500 tokens (>= 85% of 16,000),
    // triggering high-pressure exchange-boundary compaction inside `perform_persistence`.
    small_llm.push_steps([
        MockStep::text("Exchange 1 baseline summary for module alpha.").with_usage(3_000, 400),
        MockStep::text("Exchange 2 baseline summary for module beta.").with_usage(6_000, 500),
        MockStep::text("Exchange 3 heavy turn crossing 85% compaction threshold.")
            .with_usage(14_000, 500),
    ]);

    pane.send_query("Summarize module alpha.")
        .await
        .expect("ex 1");
    let _ = pane.wait_for_diagnosis(TIMEOUT).await.expect("diag 1");

    pane.send_query("Summarize module beta.")
        .await
        .expect("ex 2");
    let _ = pane.wait_for_diagnosis(TIMEOUT).await.expect("diag 2");

    pane.advance_cursor_to_end();
    pane.send_query("Run full analysis across alpha and beta.")
        .await
        .expect("ex 3");

    let diag_ctx = pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::DiagnosisComplete { usage: Some(u), .. } => u.context_tokens,
            _ => None,
        })
        .await
        .expect("ex 3 DiagnosisComplete");
    let updated_ctx = pane
        .wait_for_next(TIMEOUT, |ev| match ev {
            ClawEngineEvent::UsageUpdated { context_tokens, .. } => Some(*context_tokens),
            _ => None,
        })
        .await
        .expect("ex 3 UsageUpdated");

    let store = Store::new(ws.db.pool());
    let db_sess = store
        .get_session(pane.session_id())
        .await
        .unwrap()
        .unwrap();

    assert!(
        diag_ctx > 0 && diag_ctx <= 9_600,
        "DiagnosisComplete.usage.context_tokens must carry post-compaction scaled count (<= 9,600), got {diag_ctx}"
    );
    assert_eq!(updated_ctx, diag_ctx);
    assert_eq!(u64::try_from(db_sess.context_tokens).unwrap(), updated_ctx);

    pane.shutdown().await;
    ws.teardown().await;
}
