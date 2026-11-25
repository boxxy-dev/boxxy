use crate::engine::agent_config::AgentConfig;
use crate::engine::session::SessionState;
use crate::engine::tools::set_state::SetAgentStateTool;
use crate::engine::tools::skills::ActivateSkillTool;
use crate::engine::tools::summon_headless::SummonHeadlessWorkerTool;
use crate::engine::tools::tasks::{CancelTaskTool, ListTasksTool, ScheduleTaskTool};
use crate::engine::tools::terminal::TerminalCommandTool;
use crate::engine::tools::workspace::{
    AbortAgentTaskTool, CloseAgentTool, DelegateTaskAsyncTool, DelegateTaskTool,
    ListActiveAgentsTool, ReadPaneTool, RestoreBackgroundShellTool, SendKeystrokesTool,
    SetGlobalIntentTool, SpawnAgentTool,
};
use crate::engine::tools::{ClawApprovalHandler, SysShellTool};
use crate::engine::{ClawEngineEvent, ClawEnvironment};
use crate::utils::load_prompt_fallback;
use boxxy_ai_core::{
    AiCredentials, ModelContextHook, build_provider_model, configure_agent_builder,
};
use boxxy_core_toolbox::{
    FileDeleteTool, FileReadTool, FileWriteTool, GetClipboardTool, GetSystemInfoTool,
    GlobSearchTool, GrepSearchTool, HttpFetchTool, KillProcessTool, ListDirectoryTool,
    ListProcessesTool, ReplaceInFileTool, RunBackgroundCommandTool, SetClipboardTool,
};
use rig::agent::Agent;
use rig::agent::hook::{
    AgentHook, CompletionCallAction, CompletionCallEvent, DispatchAction, DispatchEvent,
    HookContext,
};
use rig::message::Message;
use rig::tool::server::ToolServer;
use rig::wasm_compat::WasmCompatSend;
use std::future::Future;
use std::sync::Arc;

const BTW_STREAM_INTERVAL: std::time::Duration = std::time::Duration::from_millis(45);

/// Captures the exact committed `Arc<Message>` transcript passed to each model step of an
/// active `chat_streaming` turn so concurrent `/btw` side queries see settled tool results
/// and hit the warm provider KV prompt cache without acquiring async locks.
#[derive(Clone)]
struct InFlightTranscriptHook {
    slot: Arc<std::sync::Mutex<crate::engine::session::InFlightSnapshot>>,
}

impl AgentHook for InFlightTranscriptHook {
    fn on_completion_call(
        &self,
        _ctx: &HookContext,
        event: CompletionCallEvent<'_>,
    ) -> impl Future<Output = CompletionCallAction> + WasmCompatSend {
        let mut committed = Vec::with_capacity(event.history.len() + 1);
        committed.extend(event.history.iter().cloned());
        committed.push(Arc::new(event.prompt.clone()));
        {
            let mut guard = self
                .slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.messages = Some(committed);
            guard.active_status = Some("Thinking…".to_string());
        }
        async { CompletionCallAction::Continue }
    }
}

/// Hard-blocks any tool execution during an ephemeral `/btw` side query (defense-in-depth
/// for local gateways such as Ollama that ignore wire-level `ToolChoice::None`).
#[derive(Clone, Copy)]
struct ForbidToolsHook;

impl AgentHook for ForbidToolsHook {
    fn on_dispatch(
        &self,
        _ctx: &HookContext,
        event: DispatchEvent<'_>,
    ) -> impl Future<Output = DispatchAction> + WasmCompatSend {
        let is_tool = event.tool_name().is_some();
        async move {
            if is_tool {
                DispatchAction::skip(
                    "Tools are disabled in /btw side questions. Answer directly in markdown from existing session context.",
                )
            } else {
                DispatchAction::Proceed
            }
        }
    }
}

pub struct TurnStreamContext {
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub state: Arc<tokio::sync::Mutex<SessionState>>,
    pub in_flight_snapshot: Arc<std::sync::Mutex<crate::engine::session::InFlightSnapshot>>,
    pub agent_name: String,
    pub character_id: String,
}

#[derive(Clone)]
pub struct ClawAgent {
    agent: Agent,
    provider_name: String,
    model_name: String,
    pub(crate) config: AgentConfig,
    mcp_statuses: Vec<(String, bool)>,
}

impl ClawAgent {
    #[must_use]
    pub fn matches_config(&self, candidate: &AgentConfig) -> bool {
        &self.config == candidate
            && **boxxy_claw_protocol::characters::MCP_STATUS_CACHE.load() == self.mcp_statuses
    }

    #[must_use]
    pub fn display_model_label(&self) -> String {
        self.config
            .model
            .as_ref()
            .map_or_else(|| self.model_name.clone(), |p| p.display_model_name())
    }

    /// Runs an ephemeral single-answer `/btw` side query against the session's warm prefix
    /// with tool execution strictly disabled (`ToolChoice::None` + `ForbidToolsHook`),
    /// streaming reasoning titles and prose deltas to `BtwProgress`.
    #[allow(clippy::too_many_lines)]
    pub async fn generate_side_answer<P: Into<Message> + Send>(
        &self,
        prompt: P,
        history: Vec<Message>,
        request_id: uuid::Uuid,
        agent_name: &str,
        character_id: &str,
        tx_ui: &async_channel::Sender<ClawEngineEvent>,
    ) -> Result<(String, Option<boxxy_claw_protocol::UsageWrapper>), rig::completion::PromptError>
    {
        use futures_util::StreamExt as _;

        let start = std::time::Instant::now();
        let msg = prompt.into();
        let supports_wire_tool_choice_none = !matches!(
            self.config.model,
            Some(
                boxxy_model_selection::ModelProvider::Ollama(_)
                    | boxxy_model_selection::ModelProvider::OpenAiCompatible { .. }
            )
        );

        let mut runner = self
            .agent
            .prompt(msg)
            .history(history)
            .add_hook(ForbidToolsHook);
        if supports_wire_tool_choice_none {
            runner = runner.tool_choice(rig::message::ToolChoice::None).max_turns(1);
        } else {
            runner = runner.max_turns(2);
        }

        let mut stream = runner.stream();
        let mut reasoning_parts: std::collections::HashMap<usize, (String, Option<String>)> =
            std::collections::HashMap::new();
        let mut text_buf = String::new();
        let mut dirty_text = false;
        let mut last_emit = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_millis(100))
            .unwrap_or_else(std::time::Instant::now);
        let mut final_res: Option<rig::agent::PromptResponse> = None;

        while let Some(item_res) = stream.next().await {
            match item_res {
                Ok(rig::agent::MultiTurnStreamItem::StreamAssistantItem(
                    rig::streaming::Item::Event(ev),
                )) => match ev {
                    rig::streaming::StreamEvent::Start {
                        part,
                        kind: rig::streaming::PartKind::Reasoning,
                        ..
                    } => {
                        reasoning_parts.insert(part.index(), (String::new(), None));
                        let _ = tx_ui
                            .try_send(ClawEngineEvent::BtwProgress {
                                request_id,
                                agent_name: agent_name.to_string(),
                                character_id: character_id.to_string(),
                                active_status: Some("Thinking…".to_string()),
                                text_delta: None,
                            });
                    }
                    rig::streaming::StreamEvent::Reasoning { part, text } => {
                        let (buf, last_title) = reasoning_parts
                            .entry(part.index())
                            .or_insert_with(|| (String::new(), None));
                        buf.push_str(&text);
                        if let Some(title) = boxxy_claw_protocol::live_reasoning_title(buf)
                            && last_title.as_deref() != Some(title.as_str())
                        {
                            *last_title = Some(title.clone());
                            let _ = tx_ui.try_send(ClawEngineEvent::BtwProgress {
                                request_id,
                                agent_name: agent_name.to_string(),
                                character_id: character_id.to_string(),
                                active_status: Some(format!("Thinking: {title}…")),
                                text_delta: None,
                            });
                        }
                    }
                    rig::streaming::StreamEvent::Text { text, .. } => {
                        text_buf.push_str(&text);
                        dirty_text = true;
                        if last_emit.elapsed() >= BTW_STREAM_INTERVAL {
                            last_emit = std::time::Instant::now();
                            dirty_text = false;
                            let _ = tx_ui.try_send(ClawEngineEvent::BtwProgress {
                                request_id,
                                agent_name: agent_name.to_string(),
                                character_id: character_id.to_string(),
                                active_status: None,
                                text_delta: Some(text_buf.clone()),
                            });
                        }
                    }
                    _ => {}
                },
                Ok(rig::agent::MultiTurnStreamItem::ModelTurnRetried { .. }) => {
                    reasoning_parts.clear();
                    text_buf.clear();
                    dirty_text = false;
                }
                Ok(rig::agent::MultiTurnStreamItem::FinalResponse(res)) => {
                    final_res = Some(res);
                }
                Ok(_) => {}
                Err(e) => {
                    if boxxy_sys_utils::env::debug_context() {
                        log::info!(
                            target: "model-context",
                            "\n=== BTW MODEL ERROR ===\n{e:?}\n=======================\n"
                        );
                    }
                    return Err(e);
                }
            }
        }

        if dirty_text {
            let _ = tx_ui.try_send(ClawEngineEvent::BtwProgress {
                request_id,
                agent_name: agent_name.to_string(),
                character_id: character_id.to_string(),
                active_status: None,
                text_delta: Some(text_buf),
            });
        }

        let res = final_res.ok_or_else(|| {
            rig::completion::PromptError::Provider(rig::error::ProviderError::Provider(
                "BTW stream terminated before FinalResponse".to_string(),
            ))
        })?;

        let duration = start.elapsed();
        let raw_output = res.output();
        let output = boxxy_ai_core::append_citations_to_output(&res.messages, &raw_output);
        let endpoints = &self.config.openai_compatible_endpoints;
        let context_window =
            boxxy_ai_core::model_context_window_with_endpoints(&self.config.model, endpoints);
        let usage = boxxy_ai_core::enrich_usage_with_endpoints_cost(
            res.usage,
            &self.config.model,
            endpoints,
        );

        boxxy_telemetry::track_ai_turn(
            &self.provider_name,
            &self.model_name,
            "claw_btw",
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
            usage.input_tokens.unwrap_or(0),
            usage.output_tokens.unwrap_or(0),
        )
        .await;

        let mut wrapper = boxxy_claw_protocol::UsageWrapper::from(usage);
        wrapper.context_window = context_window;
        Ok((output, Some(wrapper)))
    }

    pub async fn chat<P: Into<Message> + Send>(
        &self,
        prompt: P,
        history: Vec<Message>,
    ) -> Result<
        (
            String,
            Option<boxxy_claw_protocol::UsageWrapper>,
            Option<Vec<Message>>,
        ),
        rig::completion::PromptError,
    > {
        self.chat_streaming(prompt, history, None).await
    }

    #[allow(clippy::too_many_lines)]
    pub async fn chat_streaming<P: Into<Message> + Send>(
        &self,
        prompt: P,
        history: Vec<Message>,
        stream_ctx: Option<TurnStreamContext>,
    ) -> Result<
        (
            String,
            Option<boxxy_claw_protocol::UsageWrapper>,
            Option<Vec<Message>>,
        ),
        rig::completion::PromptError,
    > {
        use futures_util::StreamExt as _;

        let start = std::time::Instant::now();
        let msg = prompt.into();

        let res_result: Result<rig::agent::PromptResponse, rig::completion::PromptError> =
            if let Some(sctx) = stream_ctx {
                let set_in_flight_status = |status: String| {
                    sctx.in_flight_snapshot
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .active_status = Some(status);
                };
                set_in_flight_status("Thinking…".to_string());

                let mut stream = self
                    .agent
                    .prompt(msg)
                    .history(history)
                    .add_hook(InFlightTranscriptHook {
                        slot: sctx.in_flight_snapshot.clone(),
                    })
                    .tool_concurrency(4)
                    .ordered_by_footprint(true)
                    .max_consecutive_malformed_tool_calls(Some(4))
                    .stream();

                let mut reasoning_parts: std::collections::HashMap<
                    usize,
                    (std::time::Instant, String, Option<String>),
                > = std::collections::HashMap::new();
                // Visible prose is never streamed chunk-by-chunk outside `/drama` typewriter mode;
                // the full response appears at once when the turn completes (`DiagnosisComplete`).
                // We only track whether the model has started composing text so any prior tool
                // status label resets cleanly to `"Thinking…"` once per text step.
                let mut composing_reply = false;
                let mut final_res: Option<rig::agent::PromptResponse> = None;

                while let Some(item_res) = stream.next().await {
                    match item_res {
                        Ok(rig::agent::MultiTurnStreamItem::StreamAssistantItem(
                            rig::streaming::Item::Event(ev),
                        )) => match ev {
                            rig::streaming::StreamEvent::Start {
                                part,
                                kind: rig::streaming::PartKind::Reasoning,
                                ..
                            } => {
                                composing_reply = false;
                                reasoning_parts.insert(
                                    part.index(),
                                    (std::time::Instant::now(), String::new(), None),
                                );
                                set_in_flight_status("Thinking…".to_string());
                                let _ = sctx
                                    .tx_ui
                                    .send(ClawEngineEvent::TurnTraceProgress {
                                        agent_name: sctx.agent_name.clone(),
                                        character_id: sctx.character_id.clone(),
                                        item: None,
                                        active_status: Some("Thinking…".to_string()),
                                    })
                                    .await;
                            }
                            rig::streaming::StreamEvent::Reasoning { part, text } => {
                                if let Some((_, buf, last_title)) =
                                    reasoning_parts.get_mut(&part.index())
                                {
                                    buf.push_str(&text);
                                    if let Some(title) =
                                        boxxy_claw_protocol::live_reasoning_title(buf)
                                        && last_title.as_deref() != Some(title.as_str())
                                    {
                                        *last_title = Some(title.clone());
                                        let status_str = format!("Thinking: {title}…");
                                        set_in_flight_status(status_str.clone());
                                        let _ = sctx
                                            .tx_ui
                                            .send(ClawEngineEvent::TurnTraceProgress {
                                                agent_name: sctx.agent_name.clone(),
                                                character_id: sctx.character_id.clone(),
                                                item: None,
                                                active_status: Some(status_str),
                                            })
                                            .await;
                                    }
                                }
                            }
                            rig::streaming::StreamEvent::End {
                                part,
                                content: rig::message::AssistantContent::Reasoning(r),
                            } => {
                                let duration_ms =
                                    reasoning_parts.remove(&part.index()).map(|(t0, _, _)| {
                                        u64::try_from(t0.elapsed().as_millis()).unwrap_or(u64::MAX)
                                    });
                                if let Some((summary, clean_body)) =
                                    boxxy_claw_protocol::extract_reasoning_summary(&r.text)
                                {
                                    let thought = boxxy_claw_protocol::TurnTraceItem::Thought {
                                        summary,
                                        content: clean_body,
                                        duration_ms,
                                    };
                                    sctx.state
                                        .lock()
                                        .await
                                        .current_turn_trace
                                        .push(thought.clone());
                                    let _ = sctx
                                        .tx_ui
                                        .send(ClawEngineEvent::TurnTraceProgress {
                                            agent_name: sctx.agent_name.clone(),
                                            character_id: sctx.character_id.clone(),
                                            item: Some(thought),
                                            active_status: None,
                                        })
                                        .await;
                                }
                            }
                            rig::streaming::StreamEvent::Start {
                                kind: rig::streaming::PartKind::ToolCall,
                                name: Some(tool_name),
                                ..
                            } => {
                                composing_reply = false;
                                let status_str =
                                    boxxy_claw_protocol::busy_tool_label(tool_name.as_str(), true);
                                set_in_flight_status(status_str.clone());
                                let _ = sctx
                                    .tx_ui
                                    .send(ClawEngineEvent::TurnTraceProgress {
                                        agent_name: sctx.agent_name.clone(),
                                        character_id: sctx.character_id.clone(),
                                        item: None,
                                        active_status: Some(status_str),
                                    })
                                    .await;
                            }
                            rig::streaming::StreamEvent::Start {
                                kind: rig::streaming::PartKind::Text,
                                ..
                            }
                            | rig::streaming::StreamEvent::Text { .. } => {
                                if !composing_reply {
                                    composing_reply = true;
                                    set_in_flight_status("Composing reply…".to_string());
                                    let _ = sctx
                                        .tx_ui
                                        .send(ClawEngineEvent::TurnTraceProgress {
                                            agent_name: sctx.agent_name.clone(),
                                            character_id: sctx.character_id.clone(),
                                            item: None,
                                            active_status: Some("Thinking…".to_string()),
                                        })
                                        .await;
                                }
                            }
                            _ => {}
                        },
                        Ok(rig::agent::MultiTurnStreamItem::ToolExecutionStarted { tool_call }) => {
                            composing_reply = false;
                            let status_str = boxxy_claw_protocol::busy_tool_label(
                                tool_call.function.name.as_str(),
                                false,
                            );
                            set_in_flight_status(status_str.clone());
                            let _ = sctx
                                .tx_ui
                                .send(ClawEngineEvent::TurnTraceProgress {
                                    agent_name: sctx.agent_name.clone(),
                                    character_id: sctx.character_id.clone(),
                                    item: None,
                                    active_status: Some(status_str),
                                })
                                .await;
                        }
                        Ok(rig::agent::MultiTurnStreamItem::ToolResult { .. }) => {
                            composing_reply = false;
                            set_in_flight_status("Thinking…".to_string());
                            let _ = sctx
                                .tx_ui
                                .send(ClawEngineEvent::TurnTraceProgress {
                                    agent_name: sctx.agent_name.clone(),
                                    character_id: sctx.character_id.clone(),
                                    item: None,
                                    active_status: Some("Thinking…".to_string()),
                                })
                                .await;
                        }
                        Ok(
                            rig::agent::MultiTurnStreamItem::ModelTurnRetried { .. }
                            | rig::agent::MultiTurnStreamItem::ToolCall { .. },
                        ) => {
                            composing_reply = false;
                        }
                        Ok(rig::agent::MultiTurnStreamItem::FinalResponse(res)) => {
                            final_res = Some(res);
                        }
                        Ok(_) => {}
                        Err(e) => {
                            return {
                                if boxxy_sys_utils::env::debug_context() {
                                    log::info!(
                                        target: "model-context",
                                        "\n=== MODEL ERROR ===\n{e:?}\n===================\n"
                                    );
                                }
                                Err(e)
                            };
                        }
                    }
                }

                final_res.ok_or_else(|| {
                    rig::completion::PromptError::Provider(rig::error::ProviderError::Provider(
                        "Agent stream terminated before FinalResponse".to_string(),
                    ))
                })
            } else {
                self.agent
                    .prompt(msg)
                    .history(history)
                    .tool_concurrency(4)
                    .ordered_by_footprint(true)
                    .max_consecutive_malformed_tool_calls(Some(4))
                    .await
            };

        let duration = start.elapsed();

        match res_result {
            Ok(res) => {
                let raw_output = res.output();
                let output = boxxy_ai_core::append_citations_to_output(&res.messages, &raw_output);
                let context_tokens = boxxy_ai_core::extract_response_context_tokens(&res);
                let endpoints = &self.config.openai_compatible_endpoints;
                let context_window = boxxy_ai_core::model_context_window_with_endpoints(
                    &self.config.model,
                    endpoints,
                );
                let usage = boxxy_ai_core::enrich_usage_with_endpoints_cost(
                    res.usage,
                    &self.config.model,
                    endpoints,
                );

                // Record invocation, latency, and input/output tokens in a single journal transaction
                boxxy_telemetry::track_ai_turn(
                    &self.provider_name,
                    &self.model_name,
                    "claw",
                    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                    usage.input_tokens.unwrap_or(0),
                    usage.output_tokens.unwrap_or(0),
                )
                .await;

                let is_explicit = boxxy_sys_utils::env::debug_context();

                if is_explicit {
                    log::info!(
                        target: "model-context",
                        "\n=== MODEL RESPONSE ===\n{output}\n======================\n"
                    );
                }

                let mut wrapper = boxxy_claw_protocol::UsageWrapper::from(usage);
                if context_tokens > 0 {
                    wrapper.context_tokens = Some(context_tokens);
                }
                wrapper.context_window = context_window;

                Ok((output, Some(wrapper), Some(res.messages)))
            }
            Err(e) => {
                let is_explicit = boxxy_sys_utils::env::debug_context();

                if is_explicit {
                    log::info!(
                        target: "model-context",
                        "\n=== MODEL ERROR ===\n{e:?}\n===================\n"
                    );
                }
                Err(e)
            }
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub async fn create_claw_agent(
    config: &AgentConfig,
    env: &Arc<dyn ClawEnvironment>,
    current_dir: &str,
    tx_ui: async_channel::Sender<ClawEngineEvent>,
    state: std::sync::Arc<tokio::sync::Mutex<SessionState>>,
    db: std::sync::Arc<tokio::sync::Mutex<Option<boxxy_db::Db>>>,
    session_id: String,
    pane_id: String,
    mcp_handle: Option<std::sync::Arc<boxxy_mcp::manager::McpClientManager>>,
) -> anyhow::Result<ClawAgent> {
    let provider = match &config.model {
        Some(p) => p,
        None => {
            return Err(anyhow::anyhow!(
                "No Claw model selected. Please configure your models in Settings -> APIs -> Models Selection."
            ));
        }
    };

    let (mut api_keys, mut ollama_url) = {
        let state_lock = state.lock().await;
        let keys = state_lock.api_keys.read().await.clone();
        let url = state_lock.ollama_url.read().await.clone();
        (keys, url)
    };

    // Fallback: if the IPC-pushed credentials are empty (common when the
    // engine is hosted in the daemon and the UI hasn't called
    // `update_credentials` yet — e.g. keys are stored in settings.json,
    // not env vars) fall back to the on-disk settings. This also merges
    // in provider env vars via `get_effective_api_keys`.
    let settings_snapshot = boxxy_preferences::Settings::load();
    let is_usable =
        |keys: &std::collections::HashMap<String, String>| keys.values().any(|v| !v.is_empty());
    if !is_usable(&api_keys) {
        api_keys = settings_snapshot.get_effective_api_keys();
        if ollama_url.is_empty() {
            ollama_url = settings_snapshot.ollama_base_url.clone();
        }
    }

    let creds = AiCredentials {
        api_keys,
        ollama_url,
        openai_compatible_endpoints: settings_snapshot.openai_compatible_endpoints.clone(),
    };

    let approval_handler = std::sync::Arc::new(ClawApprovalHandler {
        tx_ui: tx_ui.clone(),
        state: state.clone(),
        db: db.clone(),
        session_id: session_id.clone(),
        pane_id: pane_id.clone(),
    });

    let mut tool_server = ToolServer::new()
        .tool(SysShellTool {
            env: (*env).clone(),
            current_dir: current_dir.to_string(),
            approval: approval_handler.clone(),
        })
        .tool(crate::memories::MemoryStoreTool {
            db: db.clone(),
            current_dir: current_dir.to_string(),
            approval: approval_handler.clone(),
        })
        .tool(crate::memories::MemoryDeleteTool {
            db: db.clone(),
            current_dir: current_dir.to_string(),
            approval: approval_handler.clone(),
        })
        .tool(crate::engine::tools::scrollback::ReadScrollbackTool {
            tx_ui: tx_ui.clone(),
            state: state.clone(),
            approval: approval_handler.clone(),
        })
        .tool(ActivateSkillTool {
            pane_id: pane_id.clone(),
            approval: approval_handler.clone(),
        })
        .tool(TerminalCommandTool {
            tx_ui: tx_ui.clone(),
            state: state.clone(),
            db: db.clone(),
            session_id: session_id.clone(),
            pane_id: pane_id.clone(),
        })
        .tool(ListActiveAgentsTool {
            tx_ui: tx_ui.clone(),
            state: state.clone(),
            approval: approval_handler.clone(),
        })
        .tool(ReadPaneTool {
            tx_ui: tx_ui.clone(),
            state: state.clone(),
            approval: approval_handler.clone(),
        })
        .tool(DelegateTaskTool {
            state: state.clone(),
            tx_ui: tx_ui.clone(),
            approval: approval_handler.clone(),
        })
        .tool(SpawnAgentTool {
            tx_ui: tx_ui.clone(),
            state: state.clone(),
            approval: approval_handler.clone(),
        })
        .tool(CloseAgentTool {
            tx_ui: tx_ui.clone(),
            state: state.clone(),
            approval: approval_handler.clone(),
        })
        .tool(RestoreBackgroundShellTool {
            env: (*env).clone(),
            tx_ui: tx_ui.clone(),
            approval: approval_handler.clone(),
        })
        .tool(AbortAgentTaskTool {
            tx_ui: tx_ui.clone(),
            state: state.clone(),
            approval: approval_handler.clone(),
        })
        .tool(SendKeystrokesTool {
            tx_ui: tx_ui.clone(),
            state: state.clone(),
            approval: approval_handler.clone(),
        })
        .tool(ScheduleTaskTool {
            state: state.clone(),
            tx_ui: tx_ui.clone(),
            approval: approval_handler.clone(),
        })
        .tool(ListTasksTool {
            state: state.clone(),
            tx_ui: tx_ui.clone(),
            approval: approval_handler.clone(),
        })
        .tool(CancelTaskTool {
            state: state.clone(),
            tx_ui: tx_ui.clone(),
            approval: approval_handler.clone(),
        })
        .tool(SetGlobalIntentTool {
            approval: approval_handler.clone(),
        })
        .tool(crate::engine::tools::orchestration::SubscribeToPaneTool {
            pane_id: pane_id.clone(),
            state: state.clone(),
            tx_ui: tx_ui.clone(),
            approval: approval_handler.clone(),
        })
        .tool(crate::engine::tools::orchestration::AcquireLockTool {
            pane_id: pane_id.clone(),
            state: state.clone(),
            tx_ui: tx_ui.clone(),
            approval: approval_handler.clone(),
        })
        .tool(crate::engine::tools::orchestration::ReleaseLockTool {
            pane_id: pane_id.clone(),
            state: state.clone(),
            tx_ui: tx_ui.clone(),
            approval: approval_handler.clone(),
        })
        .tool(crate::engine::tools::orchestration::PublishEventTool {
            state: state.clone(),
            approval: approval_handler.clone(),
        })
        .tool(crate::engine::tools::orchestration::AwaitTasksTool {
            state: state.clone(),
            tx_ui: tx_ui.clone(),
            approval: approval_handler.clone(),
        })
        .tool(DelegateTaskAsyncTool {
            state: state.clone(),
            tx_ui: tx_ui.clone(),
            approval: approval_handler.clone(),
        })
        .tool(SummonHeadlessWorkerTool {
            state: state.clone(),
            env: (*env).clone(),
            tx_ui: tx_ui.clone(),
            approval: approval_handler.clone(),
        })
        .tool(SetAgentStateTool {
            state: state.clone(),
            tx_ui: tx_ui.clone(),
            approval: approval_handler.clone(),
        })
        .tool(crate::engine::tools::orchestration::OrchestrateAgentTool {
            approval: approval_handler.clone(),
        })
        .tool(crate::engine::tools::orchestration::RequestAssistanceTool {
            pane_id: pane_id.clone(),
            state: state.clone(),
            approval: approval_handler.clone(),
        })
        .tool(crate::engine::tools::orchestration::InviteCharacterTool {
            pane_id: pane_id.clone(),
            state: state.clone(),
            approval: approval_handler.clone(),
            self_supports_images: provider.supports_image_generation(),
            image_capable_peers: config.image_capable_peers.clone(),
        });

    if provider.supports_image_generation() {
        tool_server = tool_server.tool(crate::engine::tools::image::GenerateImageTool {
            provider: provider.clone(),
            creds: creds.clone(),
            current_dir: current_dir.to_string(),
            state: state.clone(),
            approval: approval_handler.clone(),
            base_url_override: None,
            generated_dir_override: None,
        });
    }

    if config.character_creator_enabled {
        tool_server = tool_server.tool(
            crate::engine::tools::character_creator::PrepareCharacterAssetsTool {
                current_dir: current_dir.to_string(),
                state: state.clone(),
                approval: approval_handler.clone(),
                characters_dir_override: None,
            },
        );
    }

    // Conditional Core Toolbox tools
    if config.file_tools_enabled {
        tool_server = tool_server
            .tool(FileReadTool {
                env: (*env).clone(),
                current_dir: current_dir.to_string(),
                approval: approval_handler.clone(),
            })
            .tool(FileWriteTool {
                env: (*env).clone(),
                current_dir: current_dir.to_string(),
                approval: approval_handler.clone(),
            })
            .tool(ReplaceInFileTool {
                env: (*env).clone(),
                current_dir: current_dir.to_string(),
                approval: approval_handler.clone(),
            })
            .tool(ListDirectoryTool {
                env: (*env).clone(),
                current_dir: current_dir.to_string(),
                approval: approval_handler.clone(),
            })
            .tool(GlobSearchTool {
                current_dir: current_dir.to_string(),
                approval: approval_handler.clone(),
            })
            .tool(GrepSearchTool {
                current_dir: current_dir.to_string(),
                approval: approval_handler.clone(),
            })
            .tool(FileDeleteTool {
                env: (*env).clone(),
                current_dir: current_dir.to_string(),
                approval: approval_handler.clone(),
            });
    }

    if config.system_tools_enabled {
        tool_server = tool_server
            .tool(GetSystemInfoTool {
                env: (*env).clone(),
                approval: approval_handler.clone(),
            })
            .tool(ListProcessesTool {
                env: (*env).clone(),
                approval: approval_handler.clone(),
            });
    }

    if config.dangerous_tools_enabled {
        tool_server = tool_server
            .tool(KillProcessTool {
                env: (*env).clone(),
                approval: approval_handler.clone(),
            })
            .tool(RunBackgroundCommandTool {
                env: (*env).clone(),
                approval: approval_handler.clone(),
            });
    }

    if config.web_tools_enabled {
        tool_server = tool_server.tool(HttpFetchTool {
            approval: approval_handler.clone(),
        });
    }

    let use_native_search = config.web_search_local_enabled
        && config.web_search_master_enabled
        && config.use_native_search
        && provider.supports_native_search();

    if config.web_search_local_enabled && config.web_search_master_enabled && !use_native_search {
        let tavily_key = creds.api_keys.get("Tavily").cloned().unwrap_or_default();
        if !tavily_key.is_empty() {
            tool_server = tool_server.tool(boxxy_core_toolbox::WebSearchTool {
                provider: Box::new(boxxy_core_toolbox::TavilyProvider::new(tavily_key)),
                approval: approval_handler.clone(),
            });
        }
    }

    if config.clipboard_tools_enabled {
        tool_server = tool_server
            .tool(GetClipboardTool {
                env: (*env).clone(),
                approval: approval_handler.clone(),
            })
            .tool(SetClipboardTool {
                env: (*env).clone(),
                approval: approval_handler.clone(),
            });
    }

    // --- Inject MCP Tools ---
    let mcp_manager = if let Some(mcp) = mcp_handle {
        mcp
    } else {
        boxxy_mcp::manager::global_manager().await
    };
    // Always sync the latest configs before building tools
    mcp_manager.update_configs(config.mcp_servers.clone()).await;
    // Probe server liveness first so loopback servers that came online clear their circuit
    // breaker and stopped servers are evicted before assembling the agent's toolset.
    let mcp_statuses = mcp_manager.get_server_statuses().await;
    boxxy_claw_protocol::characters::update_mcp_status_cache(mcp_statuses.clone());
    let mcp_tools = mcp_manager
        .build_rig_tools(Some(approval_handler.clone()))
        .await;
    tool_server = tool_server.dynamic_tools(mcp_tools);

    let tool_handle = tool_server.run();

    // --- Load Base Prompt ---
    let mut final_preamble =
        load_prompt_fallback("/dev/boxxy/BoxxyTerminal/prompts/claw.md", "claw.md")
            .replace("\n\n{{available_skills}}", "")
            .replace("{{available_skills}}", "");

    crate::engine::turn::append_preamble_section(&mut final_preamble, &config.preamble);

    // --- Inject User Name ---
    if let Some(user_name) = &config.user_name {
        crate::engine::turn::append_preamble_section(
            &mut final_preamble,
            &format!(
                "[USER INFO]\nThe user's name is: {user_name}.\nYou should address them by this name when appropriate."
            ),
        );
    }

    // --- Inject Location Context (Static Across Turns for Prefix Caching) ---
    // Exact local time is injected per-turn in `## CURRENT TURN CONTEXT` in the user message,
    // keeping `final_preamble` byte-stable across turns and panes.
    if settings_snapshot.enable_os_context {
        boxxy_sys_utils::fetch_location_context().await;

        if let Some(loc) = boxxy_sys_utils::get_location_context() {
            crate::engine::turn::append_preamble_section(
                &mut final_preamble,
                &format!(
                    "[ENVIRONMENT CONTEXT]\nLocation: {}, {}\nTimezone: {}",
                    loc.city, loc.country, loc.timezone
                ),
            );
        }
    } else {
        let policy = load_prompt_fallback(
            "/dev/boxxy/BoxxyTerminal/prompts/privacy_policy.md",
            "privacy_policy.md",
        );
        crate::engine::turn::append_preamble_section(&mut final_preamble, &policy);
    }

    if !provider.supports_image_generation() {
        if config.image_capable_peers.is_empty() {
            crate::engine::turn::append_preamble_section(
                &mut final_preamble,
                "[IMAGE GENERATION AVAILABILITY]\n\
                Neither your current model nor any other character in the catalog is currently assigned an image-capable provider. \
                If the user asks to generate, draw, or edit an image, clearly explain that they can assign any character to Gemini, OpenAI, xAI, MiniMax, or OpenRouter in **Models Selection -> Character Overrides**, and do NOT substitute unsolicited ASCII art or SVG/script workarounds unless the user specifically asked for SVG or code.",
            );
        } else {
            let peers: Vec<String> = config
                .image_capable_peers
                .iter()
                .map(|(name, slug, prov)| format!("{name} (`@{slug}`, {prov})"))
                .collect();
            crate::engine::turn::append_preamble_section(
                &mut final_preamble,
                &format!(
                    "[IMAGE GENERATION AVAILABILITY]\n\
                    Your current model cannot generate images directly, but these characters have image generation enabled: {}. \
                    When the user asks to generate, draw, or edit an image, call `invite_character(character=\"...\", topic=\"...\", debate=false)` with a rich visual prompt in `topic` so they can step into the overlay for 1 turn and generate it.",
                    peers.join(", ")
                ),
            );
        }
    }

    let (model, provider_name, model_name, reasoning) = build_provider_model(provider, &creds);

    let mut builder = model
        .into_agent_builder()
        .preamble(&final_preamble)
        .default_max_turns(100)
        .add_hook(ModelContextHook::new(&final_preamble))
        .tool_server_handle(tool_handle);

    builder = configure_agent_builder(
        builder,
        provider,
        reasoning,
        &creds.openai_compatible_endpoints,
        Some(&session_id),
        false,
    );

    if use_native_search {
        builder = boxxy_ai_core::attach_native_search(builder, provider);
    }

    let agent = builder.build();

    Ok(ClawAgent {
        agent,
        provider_name,
        model_name,
        config: config.clone(),
        mcp_statuses,
    })
}
