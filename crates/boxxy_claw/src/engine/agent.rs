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
use boxxy_ai_core::{AiCredentials, ModelContextHook, build_provider_model};
use boxxy_core_toolbox::{
    FileDeleteTool, FileReadTool, FileWriteTool, GetClipboardTool, GetSystemInfoTool,
    GlobSearchTool, GrepSearchTool, HttpFetchTool, KillProcessTool, ListDirectoryTool,
    ListProcessesTool, ReplaceInFileTool, RunBackgroundCommandTool, SetClipboardTool,
};
use rig::agent::Agent;
use rig::completion::{CacheRetention, OnUnsupported};
use rig::message::Message;
use rig::tool::server::ToolServer;
use std::sync::Arc;

pub struct TurnStreamContext {
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub state: Arc<tokio::sync::Mutex<SessionState>>,
    pub agent_name: String,
    pub character_id: String,
    pub stream_visible_text: bool,
}

#[derive(Clone)]
pub struct ClawAgent {
    agent: Agent,
    provider_name: String,
    model_name: String,
    preamble: String,
    pub(crate) config: AgentConfig,
    mcp_statuses: Vec<(String, bool)>,
}

impl ClawAgent {
    #[must_use]
    pub fn matches_config(&self, candidate: &AgentConfig) -> bool {
        &self.config == candidate
            && **boxxy_claw_protocol::characters::MCP_STATUS_CACHE.load() == self.mcp_statuses
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

        let hook = ModelContextHook {
            preamble: self.preamble.clone(),
        };

        let msg = prompt.into();

        let res_result: Result<rig::agent::PromptResponse, rig::completion::PromptError> =
            if let Some(sctx) = stream_ctx {
                let mut stream = self
                    .agent
                    .prompt(msg)
                    .history(history)
                    .add_hook(hook)
                    .stream();

                let mut reasoning_parts: std::collections::HashMap<
                    usize,
                    (std::time::Instant, String, Option<String>),
                > = std::collections::HashMap::new();
                let mut step_text = String::new();
                let mut had_streamed_text = false;
                let mut last_ui_push = std::time::Instant::now()
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
                                reasoning_parts.insert(
                                    part.index(),
                                    (std::time::Instant::now(), String::new(), None),
                                );
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
                                        let _ = sctx
                                            .tx_ui
                                            .send(ClawEngineEvent::TurnTraceProgress {
                                                agent_name: sctx.agent_name.clone(),
                                                character_id: sctx.character_id.clone(),
                                                item: None,
                                                active_status: Some(format!(
                                                    "Thinking: {title}…"
                                                )),
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
                                step_text.clear();
                                if had_streamed_text {
                                    had_streamed_text = false;
                                    let _ = sctx
                                        .tx_ui
                                        .send(ClawEngineEvent::AssistantTextDelta {
                                            agent_name: sctx.agent_name.clone(),
                                            character_id: sctx.character_id.clone(),
                                            text: String::new(),
                                        })
                                        .await;
                                }
                                let status_str =
                                    boxxy_claw_protocol::busy_tool_label(tool_name.as_str(), true);
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
                            rig::streaming::StreamEvent::Text { text, .. } => {
                                step_text.push_str(&text);
                                if sctx.stream_visible_text
                                    && last_ui_push.elapsed() >= std::time::Duration::from_millis(45)
                                {
                                    let trimmed_lead = step_text.trim_start();
                                    let speaker_tag_pending = trimmed_lead
                                        .starts_with("[SPEAKER_ID:")
                                        && !trimmed_lead.contains(']');
                                    if !speaker_tag_pending {
                                        let cleaned =
                                            crate::engine::turn::history::strip_speaker_tag(
                                                &step_text,
                                            )
                                            .replace(
                                                crate::engine::session::roundtable::YIELD_TO_USER_MARKER,
                                                "",
                                            );
                                        let trimmed = cleaned.trim();
                                        let is_silent_ack_prefix =
                                            !trimmed.is_empty() && "[SILENT_ACK]".starts_with(trimmed);
                                        if !trimmed.is_empty()
                                            && trimmed != "[SILENT_ACK]"
                                            && !is_silent_ack_prefix
                                        {
                                            had_streamed_text = true;
                                            last_ui_push = std::time::Instant::now();
                                            let _ = sctx
                                                .tx_ui
                                                .send(ClawEngineEvent::AssistantTextDelta {
                                                    agent_name: sctx.agent_name.clone(),
                                                    character_id: sctx.character_id.clone(),
                                                    text: cleaned.trim_start().to_string(),
                                                })
                                                .await;
                                        }
                                    }
                                }
                            }
                            _ => {}
                        },
                        Ok(
                            rig::agent::MultiTurnStreamItem::ModelTurnRetried { .. }
                            | rig::agent::MultiTurnStreamItem::ToolCall { .. }
                            | rig::agent::MultiTurnStreamItem::ToolResult { .. },
                        ) => {
                            step_text.clear();
                            if had_streamed_text {
                                had_streamed_text = false;
                                let _ = sctx
                                    .tx_ui
                                    .send(ClawEngineEvent::AssistantTextDelta {
                                        agent_name: sctx.agent_name.clone(),
                                        character_id: sctx.character_id.clone(),
                                        text: String::new(),
                                    })
                                    .await;
                            }
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
                self.agent.prompt(msg).history(history).add_hook(hook).await
            };

        let duration = start.elapsed();

        match res_result {
            Ok(res) => {
                let raw_output = res.output();
                let output =
                    boxxy_ai_core::append_citations_to_output(&res.messages, &raw_output);
                let context_tokens = boxxy_ai_core::extract_response_context_tokens(&res);
                let endpoints = &boxxy_preferences::Settings::load().openai_compatible_endpoints;
                let context_window = boxxy_ai_core::model_context_window_with_endpoints(
                    &self.config.model,
                    endpoints,
                );
                let usage = boxxy_ai_core::enrich_usage_with_endpoints_cost(
                    res.usage,
                    &self.config.model,
                    endpoints,
                );

                // Track Invocations
                boxxy_telemetry::track_ai_invocation(&self.provider_name, &self.model_name, "claw")
                    .await;

                // Track Latency
                boxxy_telemetry::track_ai_latency(
                    &self.model_name,
                    &self.provider_name,
                    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                    "claw",
                )
                .await;

                // Track Tokens
                boxxy_telemetry::track_ai_tokens(
                    &self.model_name,
                    &self.provider_name,
                    "input",
                    usage.input_tokens.unwrap_or(0),
                    "claw",
                )
                .await;
                boxxy_telemetry::track_ai_tokens(
                    &self.model_name,
                    &self.provider_name,
                    "output",
                    usage.output_tokens.unwrap_or(0),
                    "claw",
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
        .tool(
            crate::engine::tools::orchestration::OrchestrateAgentTool {
                approval: approval_handler.clone(),
            },
        )
        .tool(
            crate::engine::tools::orchestration::RequestAssistanceTool {
                pane_id: pane_id.clone(),
                state: state.clone(),
                approval: approval_handler.clone(),
            },
        )
        .tool(
            crate::engine::tools::orchestration::InviteCharacterTool {
                pane_id: pane_id.clone(),
                state: state.clone(),
                approval: approval_handler.clone(),
            },
        );

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

    if config.web_search_local_enabled
        && config.web_search_master_enabled
        && !use_native_search
    {
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
    let mcp_tools = mcp_manager
        .build_rig_tools(Some(approval_handler.clone()))
        .await;
    let mcp_statuses = mcp_manager.get_server_statuses().await;
    boxxy_claw_protocol::characters::update_mcp_status_cache(mcp_statuses.clone());
    tool_server = tool_server.dynamic_tools(mcp_tools);

    let tool_handle = tool_server.run();

    // --- Load Base Prompt ---
    let mut final_preamble =
        load_prompt_fallback("/dev/boxxy/BoxxyTerminal/prompts/claw.md", "claw.md");
    // {{available_skills}} is now injected per-turn in the user message; strip the placeholder
    final_preamble = final_preamble.replace("{{available_skills}}", "");

    if !config.preamble.is_empty() {
        final_preamble.push('\n');
        final_preamble.push_str(&config.preamble);
    }

    // --- Inject User Name ---
    if let Some(user_name) = &config.user_name {
        final_preamble.push_str("\n\n[USER INFO]\n");
        final_preamble.push_str(&format!(
            "The user's name is: {user_name}.\nYou should address them by this name when appropriate."
        ));
    }

    // --- Inject Location Context (Static Across Turns for Prefix Caching) ---
    // Exact local time is injected per-turn in `## CURRENT TURN CONTEXT` in the user message,
    // keeping `final_preamble` byte-stable across turns and panes.
    let settings = boxxy_preferences::Settings::load();
    if settings.enable_os_context {
        boxxy_sys_utils::fetch_location_context().await;

        if let Some(loc) = boxxy_sys_utils::get_location_context() {
            final_preamble.push_str(&format!(
                "\n\n[ENVIRONMENT CONTEXT]\nLocation: {}, {}\nTimezone: {}",
                loc.city, loc.country, loc.timezone
            ));
        }
    } else {
        let policy = load_prompt_fallback(
            "/dev/boxxy/BoxxyTerminal/prompts/privacy_policy.md",
            "privacy_policy.md",
        );
        final_preamble.push_str("\n\n");
        final_preamble.push_str(&policy);
    }

    let (model, provider_name, model_name, reasoning) = build_provider_model(provider, &creds);

    let mut builder = rig::agent::AgentBuilder::new(model)
        .preamble(&final_preamble)
        .default_max_turns(100)
        .cache(CacheRetention::Short)
        .on_unsupported(OnUnsupported::Ignore)
        .accept_unknown_finish_reasons(true)
        .tool_server_handle(tool_handle);

    if let Some(reasoning) = reasoning {
        builder = builder.reasoning(reasoning);
    }

    if use_native_search {
        builder = boxxy_ai_core::attach_native_search(builder, provider);
    }

    let agent = builder.build();

    Ok(ClawAgent {
        agent,
        provider_name,
        model_name,
        preamble: final_preamble,
        config: config.clone(),
        mcp_statuses,
    })
}
