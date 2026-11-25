//! Ephemeral `/btw` side-question execution (`OpenCode` `SessionGenerate`-aligned).
//!
//! Runs concurrently alongside an active or idle `ClawSession` without mutating
//! `SessionState.history` or persisting visual rows to `claw_events`, while reusing
//! the session's warm KV prompt-cache prefix (`ClawAgent` + `in_flight_snapshot`)
//! and recording cumulative session token & USD usage.

use crate::engine::{ClawEngineEvent, ClawEnvironment, session::SessionState};
use boxxy_db::Db;
use std::fmt::Write as _;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

use super::{file_refs, format_turn_error, history, multimodal};

const BTW_INSTRUCTIONS: &str = "\
[SIDE QUESTION (/btw) — EPHEMERAL]\n\
The user is asking a quick side question about the conversation or task so far.\n\
Answer directly, accurately, and concisely in Markdown from what you already know.\n\
Stay naturally in character, do NOT call any tools, do NOT take any actions, and do NOT emit runnable command proposals.";

pub struct BtwQueryParams {
    pub request_id: Uuid,
    pub pane_id: String,
    pub session_id: String,
    pub host_agent_name: String,
    pub question: String,
    pub prior_exchange: Option<(String, String)>,
    pub snapshot: String,
    pub session_ctx: String,
    pub cwd: String,
    pub env: Arc<dyn ClawEnvironment>,
    pub db: Arc<Mutex<Option<Db>>>,
    pub state: Arc<Mutex<SessionState>>,
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
}

#[must_use]
pub fn spawn_btw_query(params: BtwQueryParams) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        run_btw_query(params).await;
    })
}

#[allow(clippy::too_many_lines)]
async fn run_btw_query(params: BtwQueryParams) {
    let BtwQueryParams {
        request_id,
        pane_id,
        session_id,
        host_agent_name,
        question,
        prior_exchange,
        snapshot,
        session_ctx,
        cwd,
        env,
        db,
        state,
        tx_ui,
    } = params;

    let start_time = std::time::Instant::now();
    let trimmed_question = question.trim().to_string();

    let (
        agent_name,
        character_id,
        is_guest_speaker,
        is_incognito,
        in_flight,
        settled_history,
        last_snapshot_hash,
        pending_proposal_summary,
        recent_trace,
        cached_claw_agent,
        mcp_handle,
        web_search_enabled,
        api_keys_lock,
        ollama_url_lock,
        effective_cwd,
    ) = {
        let state_lock = state.lock().await;
        let (spk_name, spk_char_id) = state_lock.effective_speaker();
        let resolved_name = if spk_name.is_empty() {
            host_agent_name.clone()
        } else {
            spk_name
        };
        let is_guest = state_lock
            .active_speaker
            .as_ref()
            .is_some_and(|s| s.is_guest);
        let in_flight = state_lock
            .in_flight_snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let resolved_cwd = if cwd.is_empty() {
            state_lock.current_dir.clone()
        } else {
            cwd
        };

        (
            resolved_name,
            spk_char_id,
            is_guest,
            state_lock.incognito,
            in_flight,
            state_lock.history.clone(),
            state_lock.last_snapshot_hash,
            state_lock.pending_proposal_summary.clone(),
            state_lock.current_turn_trace.clone(),
            state_lock.persistent_agent.clone(),
            state_lock.mcp_handle.clone(),
            state_lock.web_search_enabled,
            state_lock.api_keys.clone(),
            state_lock.ollama_url.clone(),
            resolved_cwd,
        )
    };

    let _ = tx_ui
        .send(ClawEngineEvent::BtwProgress {
            request_id,
            agent_name: agent_name.clone(),
            character_id: character_id.clone(),
            active_status: Some("Thinking…".to_string()),
            text_delta: None,
        })
        .await;

    let settings = boxxy_preferences::Settings::load();
    let mut effective_keys = api_keys_lock.read().await.clone();
    if !effective_keys.values().any(|v| !v.is_empty()) {
        effective_keys = settings.get_effective_api_keys();
    }
    let mut effective_ollama = ollama_url_lock.read().await.clone();
    if effective_ollama.is_empty() {
        effective_ollama.clone_from(&settings.ollama_base_url);
    }
    let creds = boxxy_ai_core::AiCredentials::new(effective_keys, effective_ollama)
        .with_custom_endpoints(settings.openai_compatible_endpoints.clone());

    let mut prompt_body = String::with_capacity(trimmed_question.len() + 384);
    prompt_body.push_str(BTW_INSTRUCTIONS);
    let active_model_hint = if is_incognito {
        settings.claw_model.as_ref().map(|m| m.display_model_name())
    } else {
        settings
            .resolve_character_model(&character_id)
            .map(|m| m.display_model_name())
    };
    if let Some(m_name) = active_model_hint.filter(|s| !s.is_empty()) {
        let _ = write!(prompt_body, "\nActive Model: `{m_name}`");
    }
    if let Some((prev_q, prev_a)) = prior_exchange {
        let _ = write!(
            prompt_body,
            "\n\n[PREVIOUS /btw EXCHANGE]\nUser: {}\nAssistant: {}",
            prev_q.trim(),
            prev_a.trim()
        );
    }
    let _ = write!(prompt_body, "\n\nQuestion: {trimmed_question}");

    let mut btw_tail = String::new();
    let (images, image_meta) = if is_incognito {
        (Vec::new(), Vec::new())
    } else {
        let resolved_refs =
            file_refs::resolve_prompt_file_refs(&trimmed_question, &effective_cwd).await;
        if !resolved_refs.context_block.trim().is_empty() {
            btw_tail.push_str("\n\n");
            btw_tail.push_str(&resolved_refs.context_block);
        }
        (
            resolved_refs.image_attachments,
            resolved_refs.image_attachment_meta,
        )
    };

    let has_session_transcript = !settled_history.is_empty() || in_flight.messages.is_some();

    let (result, model_label) = if is_incognito {
        let preamble = if session_ctx.trim().is_empty() {
            crate::utils::load_prompt_fallback(
                "/dev/boxxy/BoxxyTerminal/prompts/ai_chat.md",
                "ai_chat.md",
            )
        } else {
            session_ctx
        };
        let model_opt = settings.claw_model.clone();
        let label = model_opt
            .as_ref()
            .map_or_else(String::new, boxxy_model_selection::ModelProvider::display_model_name);
        let agent = boxxy_ai_core::create_agent_with_config(
            &model_opt,
            &creds,
            &preamble,
            &boxxy_ai_core::AgentOptions {
                enable_native_search: false,
                stateless: true,
                session_cache_key: None,
            },
        );
        let btw_history = history::build_btw_history(
            in_flight.messages.as_deref(),
            settled_history,
            &character_id,
            &host_agent_name,
            in_flight.active_status.as_deref(),
            None,
            &[],
        );
        let msg = multimodal::build_interleaved_user_message(
            &prompt_body,
            &btw_tail,
            images,
            &image_meta,
        );
        let res = run_boxxy_agent_btw(
            &agent,
            model_opt.as_ref(),
            &settings.openai_compatible_endpoints,
            msg,
            btw_history,
            request_id,
            &agent_name,
            &character_id,
            &tx_ui,
        )
        .await;
        (res, label)
    } else if !has_session_transcript {
        // Brand-new empty pane with 0 history: skip the 25+ tool schemas since no provider
        // prefix cache exists yet, while preserving character identity, CWD, and snapshot.
        let effective_preamble = if is_guest_speaker {
            crate::engine::context::load_session_context(&character_id)
        } else {
            session_ctx
        };
        let mut preamble = String::from(
            "You are an AI companion integrated directly into **Boxxy-Terminal**. \
            Answer the user's `/btw` side question concisely and accurately in Markdown while staying naturally in character.",
        );
        super::append_preamble_section(&mut preamble, &effective_preamble);
        let user_name = settings.user_profile.display_name.trim();
        if !user_name.is_empty() {
            super::append_preamble_section(
                &mut preamble,
                &format!("[USER INFO]\nThe user's name is: {user_name}."),
            );
        }
        if !effective_cwd.is_empty() {
            let _ = write!(btw_tail, "\n\nCWD: `{effective_cwd}`");
        }
        let snap_trimmed = snapshot.trim();
        if !snap_trimmed.is_empty() {
            let snap_block = super::snapshot::format_terminal_snapshot_block(snap_trimmed);
            let _ = write!(btw_tail, "\n\n{snap_block}");
        }

        let model_opt = settings.resolve_character_model(&character_id);
        let label = model_opt
            .as_ref()
            .map_or_else(String::new, boxxy_model_selection::ModelProvider::display_model_name);
        let agent = boxxy_ai_core::create_agent_with_config(
            &model_opt,
            &creds,
            &preamble,
            &boxxy_ai_core::AgentOptions {
                enable_native_search: false,
                stateless: false,
                session_cache_key: Some(session_id.clone()),
            },
        );
        let msg = super::multimodal::build_interleaved_user_message(
            &prompt_body,
            &btw_tail,
            images,
            &image_meta,
        );
        let res = run_boxxy_agent_btw(
            &agent,
            model_opt.as_ref(),
            &settings.openai_compatible_endpoints,
            msg,
            Vec::new(),
            request_id,
            &agent_name,
            &character_id,
            &tx_ui,
        )
        .await;
        (res, label)
    } else {
        // Active or populated session: reuse the exact `ClawAgent` (`system` + `tools` +
        // `session_cache_key`) and settled transcript so providers hit their warm KV cache.
        let effective_preamble = if is_guest_speaker {
            crate::engine::context::load_session_context(&character_id)
        } else {
            session_ctx
        };
        let config = crate::engine::agent_config::AgentConfig::from_env_for_character(
            &settings,
            Some(&character_id),
            web_search_enabled,
            effective_preamble,
        );
        let claw_agent_res = match cached_claw_agent {
            Some(agent) if !is_guest_speaker && agent.matches_config(&config) => Ok(agent),
            _ => {
                crate::engine::agent::create_claw_agent(
                    &config,
                    &env,
                    &effective_cwd,
                    tx_ui.clone(),
                    state.clone(),
                    db.clone(),
                    session_id.clone(),
                    pane_id,
                    mcp_handle,
                )
                .await
            }
        };

        match claw_agent_res {
            Ok(claw_agent) => {
                if !is_guest_speaker {
                    let mut state_lock = state.lock().await;
                    if state_lock.persistent_agent.is_none() {
                        state_lock.persistent_agent = Some(claw_agent.clone());
                    }
                }
                // When `/btw` runs on an idle populated session (`in_flight.messages.is_none()`),
                // include the live CWD and any changed terminal snapshot in the trailing question
                // so `system + tools + btw_history` still gets a 100% warm KV prefix-cache hit!
                if in_flight.messages.is_none() {
                    if !effective_cwd.is_empty() {
                        let _ = write!(btw_tail, "\n\nCWD: `{effective_cwd}`");
                    }
                    let snap_trimmed = snapshot.trim();
                    if !snap_trimmed.is_empty() {
                        let current_hash = super::snapshot::snapshot_hash(&snapshot);
                        if last_snapshot_hash != Some(current_hash) {
                            let snap_block =
                                super::snapshot::format_terminal_snapshot_block(snap_trimmed);
                            let _ = write!(btw_tail, "\n\n{snap_block}");
                        }
                    }
                }
                let label = claw_agent.display_model_label();
                let btw_history = history::build_btw_history(
                    in_flight.messages.as_deref(),
                    settled_history,
                    &character_id,
                    &host_agent_name,
                    in_flight.active_status.as_deref(),
                    pending_proposal_summary.as_deref(),
                    &recent_trace,
                );
                let msg = super::multimodal::build_interleaved_user_message(
                    &prompt_body,
                    &btw_tail,
                    images,
                    &image_meta,
                );
                let res = claw_agent
                    .generate_side_answer(
                        msg,
                        btw_history,
                        request_id,
                        &agent_name,
                        &character_id,
                        &tx_ui,
                    )
                    .await
                    .map_err(|e| format_turn_error(&e));
                (res, label)
            }
            Err(err) => (Err(format!("**Configuration Error:** {err}")), String::new()),
        }
    };

    let duration_ms = u64::try_from(start_time.elapsed().as_millis()).unwrap_or(u64::MAX);

    match result {
        Ok((raw_answer, usage_opt)) => {
            let clean_answer = history::strip_speaker_tag(&raw_answer)
                .replace(crate::engine::session::roundtable::YIELD_TO_USER_MARKER, "")
                .trim()
                .to_string();

            if !is_incognito
                && let Some(ref usage) = usage_opt
            {
                let (new_total_tokens, new_total_cost) = {
                    let mut state_lock = state.lock().await;
                    state_lock.total_tokens =
                        state_lock.total_tokens.saturating_add(usage.total_tokens());
                    if let Some(cost) = usage.cost_usd
                        && cost.is_finite()
                        && cost > 0.0
                    {
                        state_lock.total_cost_usd += cost;
                    }
                    (state_lock.total_tokens, state_lock.total_cost_usd)
                };

                let db_opt = db.lock().await.clone();
                if let Some(db_val) = db_opt {
                    let store = boxxy_db::store::Store::new(db_val.pool());
                    let total_i64 = i64::try_from(new_total_tokens).unwrap_or(i64::MAX);
                    let _ = store
                        .update_session_usage_totals(&session_id, total_i64, new_total_cost)
                        .await;
                }
            }

            if clean_answer.is_empty() {
                let _ = tx_ui
                    .send(ClawEngineEvent::BtwComplete {
                        request_id,
                        agent_name,
                        character_id,
                        question: trimmed_question,
                        answer: String::new(),
                        model_label,
                        duration_ms,
                        usage: usage_opt,
                        error: Some(
                            "The model returned an empty response. Click Retry to ask again."
                                .to_string(),
                        ),
                    })
                    .await;
            } else {
                let _ = tx_ui
                    .send(ClawEngineEvent::BtwComplete {
                        request_id,
                        agent_name,
                        character_id,
                        question: trimmed_question,
                        answer: clean_answer,
                        model_label,
                        duration_ms,
                        usage: usage_opt,
                        error: None,
                    })
                    .await;
            }
        }
        Err(err_msg) => {
            let _ = tx_ui
                .send(ClawEngineEvent::BtwComplete {
                    request_id,
                    agent_name,
                    character_id,
                    question: trimmed_question,
                    answer: String::new(),
                    model_label,
                    duration_ms,
                    usage: None,
                    error: Some(err_msg),
                })
                .await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_boxxy_agent_btw(
    agent: &boxxy_ai_core::BoxxyAgent,
    model_opt: Option<&boxxy_model_selection::ModelProvider>,
    endpoints: &[boxxy_model_selection::OpenAiCompatibleEndpoint],
    msg: rig::message::Message,
    history: Vec<rig::message::Message>,
    request_id: Uuid,
    agent_name: &str,
    character_id: &str,
    tx_ui: &async_channel::Sender<ClawEngineEvent>,
) -> Result<(String, Option<boxxy_claw_protocol::UsageWrapper>), String> {
    let tx_r = tx_ui.clone();
    let name_r = agent_name.to_string();
    let cid_r = character_id.to_string();

    let tx_t = tx_ui.clone();
    let name_t = agent_name.to_string();
    let cid_t = character_id.to_string();

    let owned_model = model_opt.cloned();
    let context_window =
        boxxy_ai_core::model_context_window_with_endpoints(&owned_model, endpoints);

    agent
        .chat_streaming(
            msg,
            history,
            move |raw_thought, _dur_ms| {
                if let Some((Some(summary), _)) =
                    boxxy_claw_protocol::extract_reasoning_summary(&raw_thought)
                {
                    let _ = tx_r.try_send(ClawEngineEvent::BtwProgress {
                        request_id,
                        agent_name: name_r.clone(),
                        character_id: cid_r.clone(),
                        active_status: Some(format!("Thinking: {summary}…")),
                        text_delta: None,
                    });
                }
            },
            move |partial_text| {
                let _ = tx_t.try_send(ClawEngineEvent::BtwProgress {
                    request_id,
                    agent_name: name_t.clone(),
                    character_id: cid_t.clone(),
                    active_status: None,
                    text_delta: Some(partial_text.to_string()),
                });
            },
        )
        .await
        .map(|(output, usage_opt, _)| {
            let wrapper = usage_opt.map(|u| {
                let mut w = boxxy_claw_protocol::UsageWrapper::from(u);
                w.context_window = context_window;
                w
            });
            (output, wrapper)
        })
        .map_err(|e| format_turn_error(&e))
}
