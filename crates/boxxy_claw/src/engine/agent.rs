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

#[derive(Clone)]
pub struct ClawAgent {
    inner: ClawAgentInner,
    preamble: String,
    pub(crate) config: AgentConfig,
}

#[derive(Clone)]
enum ClawAgentInner {
    Ready {
        agent: Agent,
        provider_name: String,
        model_name: String,
    },
    #[allow(dead_code)]
    Error(String),
}

impl ClawAgent {
    pub fn matches_config(&self, candidate: &AgentConfig) -> bool {
        &self.config == candidate
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
        let start = std::time::Instant::now();

        let hook = ModelContextHook {
            preamble: self.preamble.clone(),
        };

        let msg = prompt.into();

        let (agent, provider_name, model_name) = match &self.inner {
            ClawAgentInner::Ready {
                agent,
                provider_name,
                model_name,
            } => (agent, provider_name.clone(), model_name.clone()),
            ClawAgentInner::Error(e) => {
                return Err(rig::completion::PromptError::Provider(
                    rig::error::ProviderError::Provider(e.clone()),
                ));
            }
        };

        let res_result = agent
            .prompt(msg)
            .history(history)
            .add_hook(hook)
            .await;

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
                boxxy_telemetry::track_ai_invocation(&provider_name, &model_name, "claw").await;

                // Track Latency
                boxxy_telemetry::track_ai_latency(
                    &model_name,
                    &provider_name,
                    duration.as_millis() as u64,
                    "claw",
                )
                .await;

                // Track Tokens
                boxxy_telemetry::track_ai_tokens(
                    &model_name,
                    &provider_name,
                    "input",
                    usage.input_tokens.unwrap_or(0),
                    "claw",
                )
                .await;
                boxxy_telemetry::track_ai_tokens(
                    &model_name,
                    &provider_name,
                    "output",
                    usage.output_tokens.unwrap_or(0),
                    "claw",
                )
                .await;

                let is_explicit = std::env::var("BOXXY_DEBUG_CONTEXT")
                    .map(|v| v == "1")
                    .unwrap_or(false);

                if is_explicit {
                    log::info!(
                        target: "model-context",
                        "\n=== MODEL RESPONSE ===\n{}\n======================\n",
                        output
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
                let is_explicit = std::env::var("BOXXY_DEBUG_CONTEXT")
                    .map(|v| v == "1")
                    .unwrap_or(false);

                if is_explicit {
                    log::info!(
                        target: "model-context",
                        "\n=== MODEL ERROR ===\n{:?}\n===================\n",
                        e
                    );
                }
                Err(e)
            }
        }
    }
}

#[must_use]
#[allow(clippy::too_many_arguments)]
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

    let native_search_params = if config.web_search_local_enabled
        && config.web_search_master_enabled
        && config.use_native_search
    {
        boxxy_ai_core::native_search_additional_params(provider)
    } else {
        None
    };

    if config.web_search_local_enabled
        && config.web_search_master_enabled
        && native_search_params.is_none()
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
    tool_server = tool_server.dynamic_tools(mcp_tools);

    let tool_handle = tool_server.run();

    // --- Load Base Prompt ---
    let mut final_preamble =
        load_prompt_fallback("/dev/boxxy/BoxxyTerminal/prompts/claw.md", "claw.md");
    // {{available_skills}} is now injected per-turn in the user message; strip the placeholder
    final_preamble = final_preamble.replace("{{available_skills}}", "");

    if !config.preamble.is_empty() {
        final_preamble.push_str("\n");
        final_preamble.push_str(&config.preamble);
    }

    // --- Inject User Name ---
    if let Some(user_name) = &config.user_name {
        final_preamble.push_str("\n\n[USER INFO]\n");
        final_preamble.push_str(&format!(
            "The user's name is: {}.\nYou should address them by this name when appropriate.",
            user_name
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

    if let Some(params) = native_search_params {
        builder = builder.additional_params(params);
    }

    let agent = builder.build();

    let inner = ClawAgentInner::Ready {
        agent,
        provider_name,
        model_name,
    };

    Ok(ClawAgent {
        inner,
        preamble: final_preamble,
        config: config.clone(),
    })
}
