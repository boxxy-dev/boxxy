use crate::engine::ClawEngineEvent;
use crate::engine::session::SessionState;
use crate::engine::tools::SysShellTool;
use crate::engine::tools::file_ops::{FileReadTool, FileWriteTool};
use crate::engine::tools::skills::ActivateSkillTool;
use crate::engine::tools::terminal::TerminalCommandTool;
use crate::engine::tools::workspace::{
    CloseAgentTool, DelegateTaskTool, ReadPaneTool, SendKeystrokesTool, SetWorkspaceIntentTool,
    SpawnAgentTool,
};
use boxxy_agent::ipc::AgentClawProxy;
use boxxy_model_selection::ModelProvider;
use rig::agent::Agent;
use rig::client::CompletionClient;
use rig::completion::Chat;
use rig::message::Message;
use rig::providers::gemini;
use rig::providers::ollama;

use boxxy_ai_core::AiCredentials;

pub enum ClawAgent {
    Gemini(Agent<gemini::CompletionModel>),
    Ollama(Agent<ollama::CompletionModel>),
    Anthropic(Agent<rig::providers::anthropic::completion::CompletionModel>),
    Error(String),
}

impl ClawAgent {
    pub async fn chat(
        &self,
        prompt: &str,
        history: Vec<Message>,
    ) -> Result<String, rig::completion::PromptError> {
        match self {
            Self::Gemini(agent) => agent.chat(prompt, history).await,
            Self::Ollama(agent) => agent.chat(prompt, history).await,
            Self::Anthropic(agent) => agent.chat(prompt, history).await,
            Self::Error(e) => Err(rig::completion::PromptError::CompletionError(
                rig::completion::CompletionError::ProviderError(e.clone()),
            )),
        }
    }
}

#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn create_claw_agent(
    provider: &Option<ModelProvider>,
    creds: &AiCredentials,
    system_prompt: &str,
    claw_proxy: &AgentClawProxy<'static>,
    current_dir: &str,
    tx_ui: async_channel::Sender<ClawEngineEvent>,
    state: std::sync::Arc<tokio::sync::Mutex<SessionState>>,
    db: std::sync::Arc<tokio::sync::Mutex<Option<boxxy_db::Db>>>,
) -> ClawAgent {
    let provider = match provider {
        Some(p) => p,
        None => {
            return ClawAgent::Error(
                "No Claw model selected. Please configure your models in Settings -> APIs -> Models Selection."
                    .to_string(),
            )
        }
    };

    match provider {
        ModelProvider::Gemini(model, _thinking) => {
            let key = creds.api_keys.get("Gemini").cloned().unwrap_or_default();
            let client = gemini::Client::new(key.trim()).unwrap();
            let gemini_model = client.completion_model(model.api_name());

            let agent = rig::agent::AgentBuilder::new(gemini_model)
                .preamble(system_prompt)
                .default_max_turns(5)
                .tool(SysShellTool {
                    proxy: claw_proxy.clone(),
                    current_dir: current_dir.to_string(),
                })
                .tool(FileReadTool {
                    proxy: claw_proxy.clone(),
                    current_dir: current_dir.to_string(),
                })
                .tool(FileWriteTool {
                    proxy: claw_proxy.clone(),
                    current_dir: current_dir.to_string(),
                    tx_ui: tx_ui.clone(),
                    state: state.clone(),
                })
                .tool(crate::memories::MemoryStoreTool {
                    db: db.clone(),
                    current_dir: current_dir.to_string(),
                })
                .tool(TerminalCommandTool {
                    tx_ui: tx_ui.clone(),
                    state: state.clone(),
                })
                .tool(crate::engine::tools::scrollback::ReadScrollbackTool {
                    tx_ui: tx_ui.clone(),
                    state: state.clone(),
                })
                .tool(ActivateSkillTool)
                .tool(ReadPaneTool)
                .tool(DelegateTaskTool {
                    state: state.clone(),
                })
                .tool(SpawnAgentTool {
                    tx_ui: tx_ui.clone(),
                    state: state.clone(),
                })
                .tool(CloseAgentTool {
                    tx_ui: tx_ui.clone(),
                })
                .tool(SendKeystrokesTool {
                    tx_ui: tx_ui.clone(),
                })
                .tool(SetWorkspaceIntentTool {
                    project_path: current_dir.to_string(),
                })
                .build();

            ClawAgent::Gemini(agent)
        }
        ModelProvider::Ollama(model_name) => {
            let client: ollama::Client = ollama::Client::builder()
                .api_key(rig::client::Nothing)
                .base_url(creds.ollama_url.as_str())
                .build()
                .unwrap();
            let ollama_model = client.completion_model(model_name.as_str());

            let agent = rig::agent::AgentBuilder::new(ollama_model)
                .preamble(system_prompt)
                .default_max_turns(5)
                .tool(SysShellTool {
                    proxy: claw_proxy.clone(),
                    current_dir: current_dir.to_string(),
                })
                .tool(FileReadTool {
                    proxy: claw_proxy.clone(),
                    current_dir: current_dir.to_string(),
                })
                .tool(FileWriteTool {
                    proxy: claw_proxy.clone(),
                    current_dir: current_dir.to_string(),
                    tx_ui: tx_ui.clone(),
                    state: state.clone(),
                })
                .tool(crate::memories::MemoryStoreTool {
                    db: db.clone(),
                    current_dir: current_dir.to_string(),
                })
                .tool(TerminalCommandTool {
                    tx_ui: tx_ui.clone(),
                    state: state.clone(),
                })
                .tool(crate::engine::tools::scrollback::ReadScrollbackTool {
                    tx_ui: tx_ui.clone(),
                    state: state.clone(),
                })
                .tool(ActivateSkillTool)
                .tool(ReadPaneTool)
                .tool(DelegateTaskTool {
                    state: state.clone(),
                })
                .tool(SpawnAgentTool {
                    tx_ui: tx_ui.clone(),
                    state: state.clone(),
                })
                .tool(CloseAgentTool {
                    tx_ui: tx_ui.clone(),
                })
                .tool(SendKeystrokesTool {
                    tx_ui: tx_ui.clone(),
                })
                .tool(SetWorkspaceIntentTool {
                    project_path: current_dir.to_string(),
                })
                .build();

            ClawAgent::Ollama(agent)
        }
        ModelProvider::Anthropic(model) => {
            let key = creds.api_keys.get("Anthropic").cloned().unwrap_or_default();
            let client = rig::providers::anthropic::Client::new(key.trim()).unwrap();
            let anthropic_model = client.completion_model(model.api_name());

            let agent = rig::agent::AgentBuilder::new(anthropic_model)
                .preamble(system_prompt)
                .default_max_turns(5)
                .tool(SysShellTool {
                    proxy: claw_proxy.clone(),
                    current_dir: current_dir.to_string(),
                })
                .tool(FileReadTool {
                    proxy: claw_proxy.clone(),
                    current_dir: current_dir.to_string(),
                })
                .tool(FileWriteTool {
                    proxy: claw_proxy.clone(),
                    current_dir: current_dir.to_string(),
                    tx_ui: tx_ui.clone(),
                    state: state.clone(),
                })
                .tool(crate::memories::MemoryStoreTool {
                    db: db.clone(),
                    current_dir: current_dir.to_string(),
                })
                .tool(TerminalCommandTool {
                    tx_ui: tx_ui.clone(),
                    state: state.clone(),
                })
                .tool(crate::engine::tools::scrollback::ReadScrollbackTool {
                    tx_ui: tx_ui.clone(),
                    state: state.clone(),
                })
                .tool(ActivateSkillTool)
                .tool(ReadPaneTool)
                .tool(DelegateTaskTool {
                    state: state.clone(),
                })
                .tool(SpawnAgentTool {
                    tx_ui: tx_ui.clone(),
                    state: state.clone(),
                })
                .tool(CloseAgentTool {
                    tx_ui: tx_ui.clone(),
                })
                .tool(SendKeystrokesTool {
                    tx_ui: tx_ui.clone(),
                })
                .tool(SetWorkspaceIntentTool {
                    project_path: current_dir.to_string(),
                })
                .build();

            ClawAgent::Anthropic(agent)
        }
    }
}
