use crate::engine::{ClawEngineEvent, ClawMessage, session::SessionState};
use std::sync::Arc;
use tokio::sync::Mutex;

use super::{history, multimodal};

/// Executes an ephemeral `/incognito` turn:
/// - Sends ONLY the raw user prompt (and any pasted images) + in-memory incognito conversation history.
/// - Strips all system/user/terminal/memory/skill/workspace context (`0` tools, `0` MCP).
/// - Performs zero database writes, zero title generation, and zero memory extraction.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) async fn run_incognito_turn(
    pane_id: String,
    agent_name: String,
    character_id: String,
    prompt: String,
    images: Vec<String>,
    image_meta: Vec<boxxy_claw_protocol::ImageAttachmentMeta>,
    session_ctx: String,
    state: Arc<Mutex<SessionState>>,
    tx_ui: async_channel::Sender<ClawEngineEvent>,
    tx_self: async_channel::Sender<ClawMessage>,
) {
    let settings = boxxy_preferences::Settings::load();
    let preamble = if session_ctx.trim().is_empty() {
        crate::utils::load_prompt_fallback(
            "/dev/boxxy/BoxxyTerminal/prompts/ai_chat.md",
            "ai_chat.md",
        )
    } else {
        session_ctx
    };

    let (history, history_len, api_keys, ollama_url) = {
        let mut state_lock = state.lock().await;
        // Push ONLY the user's raw text — zero identity, CWD, snapshot, memories, or user profile.
        let history_prompt =
            multimodal::annotate_prompt_for_history(&prompt, images.len(), &image_meta);
        state_lock
            .history
            .push(rig::message::Message::user(history_prompt));
        let h = state_lock.history.clone();
        let l = h.len();
        (
            h,
            l,
            state_lock.api_keys.clone(),
            state_lock.ollama_url.clone(),
        )
    };

    let ipc_keys = api_keys.read().await.clone();
    let ipc_ollama = ollama_url.read().await.clone();
    let creds = crate::engine::agent_config::merge_session_credentials(
        &settings,
        &ipc_keys,
        &ipc_ollama,
    );

    let agent = boxxy_ai_core::create_agent_with_config(
        &settings.claw_model,
        &creds,
        &preamble,
        &boxxy_ai_core::AgentOptions {
            enable_native_search: false,
            stateless: true,
            session_cache_key: None,
        },
    );

    let final_history =
        history::prepare_query_history_for_speaker(history, history_len, &character_id, &agent_name);

    let query_for_chat =
        multimodal::build_interleaved_user_message(&prompt, "", images, &image_meta);
    let mut incognito_trace: Vec<boxxy_claw_protocol::TurnTraceItem> = Vec::new();
    let tx_for_thoughts = tx_ui.clone();
    let name_for_thoughts = agent_name.clone();
    let cid_for_thoughts = character_id.clone();

    match agent
        .chat_streaming(
            query_for_chat,
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
                    incognito_trace.push(item.clone());
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
            let _ = tx_ui
                .send(ClawEngineEvent::AgentThinking {
                    agent_name: agent_name.clone(),
                    character_id: character_id.clone(),
                    is_thinking: false,
                })
                .await;

            let clean_response = response.trim().to_string();
            let context_window = boxxy_ai_core::model_context_window_with_endpoints(
                &settings.claw_model,
                &settings.openai_compatible_endpoints,
            );
            let usage_wrapper = usage_opt.map(|u| {
                let mut w = boxxy_claw_protocol::UsageWrapper::from(u);
                w.context_window = context_window;
                w
            });

            {
                let mut state_lock = state.lock().await;
                state_lock
                    .history
                    .push(rig::message::Message::assistant(clean_response.clone()));
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

            if !clean_response.is_empty() || !incognito_trace.is_empty() {
                let _ = tx_ui
                    .send(ClawEngineEvent::DiagnosisComplete {
                        agent_name,
                        character_id,
                        diagnosis: clean_response,
                        trace: incognito_trace,
                        usage: usage_wrapper,
                        typewriter: false,
                    })
                    .await;
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

            log::error!("Pane {pane_id} ({agent_name}): Incognito chat failed: {e}");

            let _ = tx_ui
                .send(ClawEngineEvent::DiagnosisComplete {
                    agent_name,
                    character_id,
                    diagnosis: super::format_turn_error(&e),
                    trace: incognito_trace,
                    usage: None,
                    typewriter: false,
                })
                .await;
        }
    }

    let _ = tx_self.send(ClawMessage::TurnFinished).await;
}
