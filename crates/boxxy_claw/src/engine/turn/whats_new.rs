use crate::engine::{
    ClawEngineEvent, ClawMessage, persist_visual_event, session::SessionState,
};
use boxxy_db::Db;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::{history, persistence, summarize_long_task_completion};

/// Executes a lightweight, zero-tool, zero-skill turn when the user clicks
/// **"Read What's New"** in the post-update What's New dialog.
///
/// Why dedicated:
/// - The prompt already embeds the complete commit list and line statistics under
///   `--- UPDATE CHANGELOG CONTEXT ---` and forbids tool calls.
/// - Skipping `SkillRegistry`, memory recall, `@file` resolution, terminal snapshots,
///   and the 25+ tool schemas prevents commit messages (mentioning themes, bugs, or `@file`)
///   from falsely triggering skills (`◇ Skill: ...`) or wasting thousands of input tokens,
///   while still keeping the character's full personality, `[USER INFO]`, session persistence,
///   and background title generation intact for any follow-up questions on Turn 2+.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) async fn run_whats_new_turn(
    pane_id: String,
    session_id: String,
    agent_name: String,
    host_agent_name: String,
    character_id: String,
    speaker_slug: String,
    prompt: String,
    cwd: String,
    is_new_task: bool,
    effective_preamble: String,
    db: Arc<Mutex<Option<Db>>>,
    state: Arc<Mutex<SessionState>>,
    tx_ui: async_channel::Sender<ClawEngineEvent>,
    tx_self: async_channel::Sender<ClawMessage>,
) {
    let settings = boxxy_preferences::Settings::load();

    let mut preamble = String::from(
        "You are an AI companion integrated directly into **Boxxy-Terminal**. \
        Your role and personality are defined by your character assignment below.\n\
        Your actual name is the **Agent Name** in `## YOUR IDENTITY` — stay in character \
        while keeping your changelog summary clear, accurate, and easy to scan.",
    );
    super::append_preamble_section(&mut preamble, &effective_preamble);
    let user_name = settings.user_profile.display_name.trim();
    if !user_name.is_empty() {
        super::append_preamble_section(
            &mut preamble,
            &format!(
                "[USER INFO]\nThe user's name is: {user_name}.\nYou should address them by this name when appropriate."
            ),
        );
    }

    let (drama_mode, history, history_len, full_prompt, api_keys, ollama_url) = {
        let mut state_lock = state.lock().await;
        if is_new_task {
            state_lock.history.clear();
            state_lock.flushed_watermark = 0;
            state_lock.read_file_hashes.clear();
            state_lock.last_snapshot_hash = None;
        }
        state_lock.current_dir.clone_from(&cwd);
        state_lock.current_turn_trace.clear();

        let drama_mode = state_lock.roundtable.drama_mode;
        let mut turn_ctx = format!(
            "## YOUR IDENTITY\nAgent Name: **{agent_name}** (`@{speaker_slug}`)\nUnique ID: `{pane_id}`"
        );
        if drama_mode {
            turn_ctx.push_str(
                "\n\n--- VISUAL NOVEL / DRAMA MODE ACTIVE (OVERRIDE ALL LENGTH & LIST FORMATTING) ---\n\
                1. Speak in ONLY 1 to 2 short, punchy spoken sentences (maximum ~35 words total)!\n\
                2. STRICTLY FORBIDDEN: Do NOT use bullet points, numbered lists, markdown headers, or multi-paragraph essays!\n\
                --------------------------------------------------------------------------------",
            );
        }

        let full_prompt = format!("{prompt}\n\n{turn_ctx}");
        // Store settled user + changelog text without transient `## YOUR IDENTITY` bloat so
        // `sessions.history_json` stays clean while Turn 2-5 follow-ups retain changelog context.
        state_lock
            .history
            .push(rig::message::Message::user(prompt.clone()));

        let h = state_lock.history.clone();
        let l = h.len();
        (
            drama_mode,
            h,
            l,
            full_prompt,
            state_lock.api_keys.clone(),
            state_lock.ollama_url.clone(),
        )
    };

    let mut effective_keys = api_keys.read().await.clone();
    if !effective_keys.values().any(|v| !v.is_empty()) {
        effective_keys = settings.get_effective_api_keys();
    }
    let mut effective_ollama = ollama_url.read().await.clone();
    if effective_ollama.is_empty() {
        effective_ollama.clone_from(&settings.ollama_base_url);
    }
    let creds = boxxy_ai_core::AiCredentials::new(effective_keys, effective_ollama)
        .with_custom_endpoints(settings.openai_compatible_endpoints.clone());

    let effective_model = settings.resolve_character_model(&character_id);
    let agent = boxxy_ai_core::create_agent_with_config(
        &effective_model,
        &creds,
        &preamble,
        &boxxy_ai_core::AgentOptions {
            enable_native_search: false,
            stateless: false,
            session_cache_key: Some(session_id.clone()),
        },
    );

    let final_history = history::prepare_query_history_for_speaker(
        history,
        history_len,
        &character_id,
        &host_agent_name,
    );

    let mut trace: Vec<boxxy_claw_protocol::TurnTraceItem> = Vec::new();
    let tx_for_thoughts = tx_ui.clone();
    let name_for_thoughts = agent_name.clone();
    let cid_for_thoughts = character_id.clone();

    let start_time = std::time::Instant::now();
    match agent
        .chat_streaming(
            rig::message::Message::user(full_prompt),
            final_history,
            |raw_thought, dur_ms| {
                if let Some((summary, clean_body)) =
                    boxxy_claw_protocol::extract_reasoning_summary(&raw_thought)
                {
                    let item = boxxy_claw_protocol::TurnTraceItem::Thought {
                        summary,
                        content: clean_body,
                        duration_ms: Some(dur_ms),
                    };
                    trace.push(item.clone());
                    let _ = tx_for_thoughts.try_send(ClawEngineEvent::TurnTraceProgress {
                        agent_name: name_for_thoughts.clone(),
                        character_id: cid_for_thoughts.clone(),
                        item: Some(item),
                        active_status: None,
                    });
                }
            },
            |_partial_text| {},
        )
        .await
    {
        Ok((response, usage_opt, _messages)) => {
            let duration = start_time.elapsed();
            let _ = tx_ui
                .send(ClawEngineEvent::AgentThinking {
                    agent_name: agent_name.clone(),
                    character_id: character_id.clone(),
                    is_thinking: false,
                })
                .await;

            if duration.as_secs() >= 7 {
                let summary = summarize_long_task_completion(&response);
                let _ = tx_ui
                    .send(ClawEngineEvent::LongTaskCompleted {
                        agent_name: agent_name.clone(),
                        character_id: character_id.clone(),
                        duration_secs: duration.as_secs(),
                        message: summary,
                    })
                    .await;
            }

            let display_response = history::strip_speaker_tag(&response)
                .replace(
                    crate::engine::session::roundtable::YIELD_TO_USER_MARKER,
                    "",
                )
                .trim()
                .to_string();

            let context_window = boxxy_ai_core::model_context_window_with_endpoints(
                &effective_model,
                &settings.openai_compatible_endpoints,
            );
            let usage_wrapper = usage_opt.map(|u| {
                let mut w = boxxy_claw_protocol::UsageWrapper::from(u);
                w.context_window = context_window;
                w
            });

            {
                let mut state_lock = state.lock().await;
                state_lock.history.push(rig::message::Message::assistant(
                    history::tag_assistant_text(&character_id, &agent_name, &display_response),
                ));
                if let Some(ref usage) = usage_wrapper {
                    state_lock.total_tokens += usage.total_tokens();
                    let ctx = usage.effective_context_tokens();
                    if ctx > 0 {
                        state_lock.context_tokens = ctx;
                    }
                    if let Some(cost) = usage.cost_usd
                        && cost.is_finite()
                        && cost > 0.0
                    {
                        state_lock.total_cost_usd += cost;
                    }
                }
            }

            let p_ctx = persistence::PersistenceContext {
                session_id: session_id.clone(),
                agent_name: host_agent_name,
                user_query: prompt,
                cwd,
                settings: (*settings).clone(),
                db: db.clone(),
                tx_self: tx_self.clone(),
            };
            persistence::perform_persistence(p_ctx, state).await;

            if !display_response.is_empty() || !trace.is_empty() {
                let event = ClawEngineEvent::DiagnosisComplete {
                    agent_name,
                    character_id,
                    diagnosis: display_response,
                    trace,
                    usage: usage_wrapper,
                    typewriter: drama_mode,
                };
                persist_visual_event(db, session_id, pane_id, &event);
                let _ = tx_ui.send(event).await;
            }
        }
        Err(e) => {
            state.lock().await.last_turn_paused_queue = true;
            let _ = tx_ui
                .send(ClawEngineEvent::AgentThinking {
                    agent_name: agent_name.clone(),
                    character_id: character_id.clone(),
                    is_thinking: false,
                })
                .await;

            log::error!("Pane {pane_id} ({agent_name}): What's New summary turn failed: {e}");
            let event = ClawEngineEvent::DiagnosisComplete {
                agent_name,
                character_id,
                diagnosis: super::format_turn_error(&e),
                trace,
                usage: None,
                typewriter: false,
            };
            persist_visual_event(db, session_id, pane_id, &event);
            let _ = tx_ui.send(event).await;
        }
    }

    let _ = tx_self.send(ClawMessage::TurnFinished).await;
}
