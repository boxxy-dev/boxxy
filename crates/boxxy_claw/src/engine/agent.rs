use crate::utils::load_prompt_fallback;
use crate::engine::ClawEngineEvent;
use crate::engine::session::SessionState;
use crate::engine::tools::skills::ActivateSkillTool;
use crate::engine::tools::tasks::{CancelTaskTool, ListTasksTool, ScheduleTaskTool};
use crate::engine::tools::terminal::TerminalCommandTool;
use crate::engine::tools::workspace::{
    AbortAgentTaskTool, CloseAgentTool, DelegateTaskAsyncTool, DelegateTaskTool, ListActiveAgentsTool,
    ReadPaneTool, SendKeystrokesTool, SetGlobalIntentTool, SpawnAgentTool,
};
use crate::engine::tools::set_state::SetAgentStateTool;
use crate::engine::tools::summon_headless::SummonHeadlessWorkerTool;
use crate::engine::tools::{ClawApprovalHandler, SysShellTool};
use boxxy_agent::ipc::claw::AgentClawProxy;
use boxxy_core_toolbox::{
    FileDeleteTool, FileReadTool, FileWriteTool, GetClipboardTool, GetSystemInfoTool,
    HttpFetchTool, KillProcessTool, ListDirectoryTool, ListProcessesTool, SetClipboardTool,
};
use boxxy_model_selection::ModelProvider;
use rig::agent::Agent;
use rig::client::CompletionClient;
use rig::message::Message;
use rig::providers::gemini;
use rig::providers::ollama;
use rig::providers::openai::responses_api::ResponsesCompletionModel;
use serde_json::json;

use boxxy_ai_core::{AiCredentials, ModelContextHook};

#[derive(Clone)]
pub struct ClawAgent {
    inner: ClawAgentInner,
    preamble: String,
}

#[derive(Clone)]
enum ClawAgentInner {
    Gemini(Agent<gemini::CompletionModel>, String, String), // Agent, Provider Name, Model Name
    Ollama(Agent<ollama::CompletionModel>, String, String),
    Anthropic(
        Agent<rig::providers::anthropic::completion::CompletionModel>,
        String,
        String,
    ),
    OpenAi(Agent<ResponsesCompletionModel>, String, String),
    OpenRouter(Agent<ResponsesCompletionModel>, String, String),
    Error(String),
}

impl ClawAgent {
    pub async fn chat(
        &self,
        prompt: &str,
        history: Vec<Message>,
    ) -> Result<(String, Option<rig::completion::Usage>), rig::completion::PromptError> {
        use rig::completion::Prompt;
        let start = std::time::Instant::now();

        let hook = ModelContextHook {
            preamble: self.preamble.clone(),
        };

        let res_result = match &self.inner {
            ClawAgentInner::Gemini(agent, _, _) => {
                agent
                    .prompt(prompt)
                    .with_history(history)
                    .with_hook(hook)
                    .extended_details()
                    .await
            }
            ClawAgentInner::Ollama(agent, _, _) => {
                agent
                    .prompt(prompt)
                    .with_history(history)
                    .with_hook(hook)
                    .extended_details()
                    .await
            }
            ClawAgentInner::Anthropic(agent, _, _) => {
                agent
                    .prompt(prompt)
                    .with_history(history)
                    .with_hook(hook)
                    .extended_details()
                    .await
            }
            ClawAgentInner::OpenAi(agent, _, _) => {
                agent
                    .prompt(prompt)
                    .with_history(history.clone())
                    .with_hook(hook.clone())
                    .extended_details()
                    .await
            }
            ClawAgentInner::OpenRouter(agent, _, _) => {
                agent
                    .prompt(prompt)
                    .with_history(history)
                    .with_hook(hook)
                    .extended_details()
                    .await
            }
            ClawAgentInner::Error(e) => {
                return Err(rig::completion::PromptError::CompletionError(
                    rig::completion::CompletionError::ProviderError(e.clone()),
                ));
            }
        };

        match res_result {
            Ok(res) => {
                let duration = start.elapsed();
                let (provider_name, model_name) = match &self.inner {
                    ClawAgentInner::Gemini(_, p, m) => (p.as_str(), m.as_str()),
                    ClawAgentInner::Ollama(_, p, m) => (p.as_str(), m.as_str()),
                    ClawAgentInner::Anthropic(_, p, m) => (p.as_str(), m.as_str()),
                    ClawAgentInner::OpenAi(_, p, m) => (p.as_str(), m.as_str()),
                    ClawAgentInner::OpenRouter(_, p, m) => (p.as_str(), m.as_str()),
                    _ => ("unknown", "unknown"),
                };

                // Track Invocations
                boxxy_telemetry::track_ai_invocation(provider_name, model_name, "claw").await;

                // Track Latency
                boxxy_telemetry::track_ai_latency(
                    model_name,
                    provider_name,
                    duration.as_millis() as u64,
                    "claw",
                )
                .await;

                // Track Tokens
                boxxy_telemetry::track_ai_tokens(
                    model_name,
                    provider_name,
                    "input",
                    res.usage.input_tokens as u64,
                    "claw",
                )
                .await;
                boxxy_telemetry::track_ai_tokens(
                    model_name,
                    provider_name,
                    "output",
                    res.usage.output_tokens as u64,
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
                        res.output
                    );
                }

                Ok((res.output.clone(), Some(res.usage)))
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
    provider: &Option<ModelProvider>,
    creds: &AiCredentials,
    system_prompt: &str,
    claw_proxy: &AgentClawProxy<'static>,
    current_dir: &str,
    tx_ui: async_channel::Sender<ClawEngineEvent>,
    state: std::sync::Arc<tokio::sync::Mutex<SessionState>>,
    db: std::sync::Arc<tokio::sync::Mutex<Option<boxxy_db::Db>>>,
    settings: &boxxy_preferences::Settings,
    session_id: String,
    pane_id: String,
    web_search_enabled: bool,
) -> ClawAgent {
    let provider = match provider {
        Some(p) => p,
        None => {
            return ClawAgent {
                inner: ClawAgentInner::Error(
                    "No Claw model selected. Please configure your models in Settings -> APIs -> Models Selection."
                        .to_string(),
                ),
                preamble: system_prompt.to_string(),
            }
        }
    };

    let approval_handler = std::sync::Arc::new(ClawApprovalHandler {
        tx_ui: tx_ui.clone(),
        state: state.clone(),
        db: db.clone(),
        session_id: session_id.clone(),
        pane_id: pane_id.clone(),
    });

    let mut tools: Vec<Box<dyn rig::tool::ToolDyn>> = vec![
        Box::new(SysShellTool {
            proxy: claw_proxy.clone(),
            current_dir: current_dir.to_string(),
            approval: approval_handler.clone(),
        }),
        Box::new(crate::memories::MemoryStoreTool {
            db: db.clone(),
            current_dir: current_dir.to_string(),
            approval: approval_handler.clone(),
        }),
        Box::new(crate::memories::MemoryDeleteTool {
            db: db.clone(),
            current_dir: current_dir.to_string(),
            approval: approval_handler.clone(),
        }),
        Box::new(crate::engine::tools::scrollback::ReadScrollbackTool {
            tx_ui: tx_ui.clone(),
            state: state.clone(),
        }),
        Box::new(ActivateSkillTool),
        Box::new(TerminalCommandTool {
            tx_ui: tx_ui.clone(),
            state: state.clone(),
            db: db.clone(),
            session_id: session_id.clone(),
            pane_id: pane_id.clone(),
        }),
        Box::new(ListActiveAgentsTool),
        Box::new(ReadPaneTool),
        Box::new(DelegateTaskTool {
            state: state.clone(),
        }),
        Box::new(SpawnAgentTool {
            tx_ui: tx_ui.clone(),
            state: state.clone(),
        }),
        Box::new(CloseAgentTool {
            tx_ui: tx_ui.clone(),
        }),
        Box::new(AbortAgentTaskTool),
        Box::new(SendKeystrokesTool {
            tx_ui: tx_ui.clone(),
        }),
        Box::new(ListProcessesTool {
            proxy: claw_proxy.clone(),
            approval: approval_handler.clone(),
        }),
        Box::new(ScheduleTaskTool {
            state: state.clone(),
            tx_ui: tx_ui.clone(),
        }),
        Box::new(ListTasksTool {
            state: state.clone(),
        }),
        Box::new(CancelTaskTool {
            state: state.clone(),
            tx_ui: tx_ui.clone(),
        }),
        Box::new(SetGlobalIntentTool),
        Box::new(crate::engine::tools::orchestration::SubscribeToPaneTool {
            pane_id: pane_id.clone(),
            state: state.clone(),
            tx_ui: tx_ui.clone(),
            approval: approval_handler.clone(),
        }),
        Box::new(crate::engine::tools::orchestration::AcquireLockTool {
            pane_id: pane_id.clone(),
            state: state.clone(),
            tx_ui: tx_ui.clone(),
            approval: approval_handler.clone(),
        }),
        Box::new(crate::engine::tools::orchestration::ReleaseLockTool {
            pane_id: pane_id.clone(),
            state: state.clone(),
            tx_ui: tx_ui.clone(),
            approval: approval_handler.clone(),
        }),
        Box::new(crate::engine::tools::orchestration::PublishEventTool {
            state: state.clone(),
            approval: approval_handler.clone(),
        }),
        Box::new(crate::engine::tools::orchestration::AwaitTasksTool {
            state: state.clone(),
            tx_ui: tx_ui.clone(),
            approval: approval_handler.clone(),
        }),
        Box::new(DelegateTaskAsyncTool {
            state: state.clone(),
        }),
        Box::new(SummonHeadlessWorkerTool {
            state: state.clone(),
            claw_proxy: claw_proxy.clone(),
        }),
        Box::new(SetAgentStateTool {
            state: state.clone(),
        }),
        Box::new(crate::engine::tools::orchestration::OrchestrateAgentTool {
            approval: approval_handler.clone(),
        }),
    ];

    // Conditional Core Toolbox tools
    if settings.enable_file_tools {
        tools.push(Box::new(FileReadTool {
            proxy: claw_proxy.clone(),
            current_dir: current_dir.to_string(),
            approval: approval_handler.clone(),
        }));
        tools.push(Box::new(FileWriteTool {
            proxy: claw_proxy.clone(),
            current_dir: current_dir.to_string(),
            approval: approval_handler.clone(),
        }));
        tools.push(Box::new(ListDirectoryTool {
            proxy: claw_proxy.clone(),
            current_dir: current_dir.to_string(),
            approval: approval_handler.clone(),
        }));
        tools.push(Box::new(FileDeleteTool {
            proxy: claw_proxy.clone(),
            current_dir: current_dir.to_string(),
            approval: approval_handler.clone(),
        }));
    }

    if settings.enable_system_tools {
        tools.push(Box::new(GetSystemInfoTool {
            proxy: claw_proxy.clone(),
            approval: approval_handler.clone(),
        }));
    }

    if settings.enable_dangerous_tools {
        tools.push(Box::new(KillProcessTool {
            proxy: claw_proxy.clone(),
            approval: approval_handler.clone(),
        }));
    }

    if settings.enable_web_tools {
        tools.push(Box::new(HttpFetchTool {
            approval: approval_handler.clone(),
        }));
    }

    if web_search_enabled && settings.enable_web_search {
        let tavily_key = creds.api_keys.get("Tavily").cloned().unwrap_or_default();
        if !tavily_key.is_empty() {
            tools.push(Box::new(boxxy_core_toolbox::WebSearchTool {
                provider: Box::new(boxxy_core_toolbox::TavilyProvider::new(tavily_key)),
            }));
        }
    }

    if settings.enable_clipboard_tools {
        tools.push(Box::new(GetClipboardTool {
            proxy: claw_proxy.clone(),
            approval: approval_handler.clone(),
        }));
        tools.push(Box::new(SetClipboardTool {
            proxy: claw_proxy.clone(),
            approval: approval_handler.clone(),
        }));
    }

    // --- Inject MCP Tools ---
    let mcp_manager = boxxy_mcp::manager::global_manager().await;
    // Always sync the latest configs before building tools
    mcp_manager
        .update_configs(settings.mcp_servers.clone())
        .await;
    let mut mcp_tools = mcp_manager.build_rig_tools().await;
    tools.append(&mut mcp_tools);

    for tool in &tools {
        let def = tool.definition("".to_string()).await;
        log::debug!("Injecting tool into Rig: {}", def.name);
    }

    // --- Inject Location & Time Context ---
    let mut final_preamble = system_prompt.to_string();
    if settings.enable_os_context {
        // Trigger fetch (it's cached)
        boxxy_ai_core::utils::fetch_location_context().await;

        let now = chrono::Local::now();
        let time_str = now.format("%Y-%m-%d %H:%M:%S").to_string();

        let mut context_block = format!(
            "\n\n[ENVIRONMENT CONTEXT]\nTime: {}",
            time_str
        );

        if let Some(loc) = boxxy_ai_core::utils::get_location_context() {
            context_block.push_str(&format!(
                "\nLocation: {}, {}\nTimezone: {}",
                loc.city, loc.country, loc.timezone
            ));
        }

        final_preamble.push_str(&context_block);
    } else {
        let policy = load_prompt_fallback(
            "/dev/boxxy/BoxxyTerminal/prompts/privacy_policy.md",
            "privacy_policy.md",
        );
        final_preamble.push_str("\n\n");
        final_preamble.push_str(&policy);
    }

    let inner = match provider {
        ModelProvider::Gemini(model, _thinking) => {
            let key = creds.api_keys.get("Gemini").cloned().unwrap_or_default();
            let client = gemini::Client::new(key.trim()).unwrap();
            let gemini_model = client.completion_model(model.api_name());

            let builder = rig::agent::AgentBuilder::new(gemini_model)
                .preamble(&final_preamble)
                .default_max_turns(100)
                .tools(tools);

            ClawAgentInner::Gemini(
                builder.build(),
                "Gemini".to_string(),
                model.api_name().to_string(),
            )
        }
        ModelProvider::Ollama(model_name) => {
            let client: ollama::Client = ollama::Client::builder()
                .api_key(rig::client::Nothing)
                .base_url(creds.ollama_url.as_str())
                .build()
                .unwrap();
            let ollama_model = client.completion_model(model_name.as_str());

            let builder = rig::agent::AgentBuilder::new(ollama_model)
                .preamble(&final_preamble)
                .default_max_turns(100)
                .tools(tools);

            ClawAgentInner::Ollama(builder.build(), "Ollama".to_string(), model_name.clone())
        }
        ModelProvider::Anthropic(model) => {
            let key = creds.api_keys.get("Anthropic").cloned().unwrap_or_default();
            let client = rig::providers::anthropic::Client::new(key.trim()).unwrap();
            let anthropic_model = client.completion_model(model.api_name());

            let builder = rig::agent::AgentBuilder::new(anthropic_model)
                .preamble(&final_preamble)
                .default_max_turns(100)
                .tools(tools);

            ClawAgentInner::Anthropic(
                builder.build(),
                "Anthropic".to_string(),
                model.api_name().to_string(),
            )
        }
        ModelProvider::OpenAi(model, thinking) => {
            let key = creds.api_keys.get("OpenAI").cloned().unwrap_or_default();
            let client = rig::providers::openai::Client::new(key.trim()).unwrap();
            let openai_model = client.completion_model(model.api_name());

            let mut builder = rig::agent::AgentBuilder::new(openai_model)
                .preamble(&final_preamble)
                .default_max_turns(100)
                .tools(tools);

            if let Some(level) = thinking {
                builder = builder.additional_params(serde_json::json!({
                    "reasoning": { "effort": level.api_name() }
                }));
            }

            ClawAgentInner::OpenAi(
                builder.build(),
                "OpenAI".to_string(),
                model.api_name().to_string(),
            )
        }
        ModelProvider::OpenRouter(model_name) => {
            let key = creds.api_keys.get("OpenRouter").cloned().unwrap_or_default();
            let client = rig::providers::openai::Client::builder()
                .api_key(key.trim())
                .base_url("https://openrouter.ai/api/v1")
                .build()
                .unwrap();
            let openrouter_model = client.completion_model(model_name.as_str());

            let builder = rig::agent::AgentBuilder::new(openrouter_model)
                .preamble(&final_preamble)
                .default_max_turns(100)
                .tools(tools);

            ClawAgentInner::OpenRouter(
                builder.build(),
                "OpenRouter".to_string(),
                model_name.clone(),
            )
        }
    };

    ClawAgent {
        inner,
        preamble: final_preamble,
    }
}
