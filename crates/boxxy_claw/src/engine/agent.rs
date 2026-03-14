use rig::agent::Agent;
use rig::message::Message;
use rig::providers::gemini;
use rig::providers::ollama;
use rig::completion::Chat;
use rig::client::CompletionClient;
use boxxy_model_selection::ModelProvider;
use boxxy_agent::ipc::AgentClawProxy;
use crate::engine::tools::SysShellTool;
use crate::engine::tools::file_ops::{FileReadTool, FileWriteTool};
use crate::engine::tools::terminal::TerminalCommandTool;
use crate::engine::tools::skills::ActivateSkillTool;
use crate::engine::tools::workspace::{ReadPaneTool, SetWorkspaceIntentTool};
use crate::engine::ClawEngineEvent;
use crate::engine::session::SessionState;

pub enum ClawAgent {
    Gemini(Agent<gemini::CompletionModel>),
    Ollama(Agent<ollama::CompletionModel>),
}

impl ClawAgent {
    pub async fn chat(&self, prompt: &str, history: Vec<Message>) -> Result<String, rig::completion::PromptError> {
        match self {
            Self::Gemini(agent) => agent.chat(prompt, history).await,
            Self::Ollama(agent) => agent.chat(prompt, history).await,
        }
    }
}

#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn create_claw_agent(
    provider: &ModelProvider, 
    api_key: &str, 
    ollama_url: &str,
    system_prompt: &str,
    claw_proxy: &AgentClawProxy<'static>,
    current_dir: &str,
    tx_ui: async_channel::Sender<ClawEngineEvent>,
    state: std::sync::Arc<tokio::sync::Mutex<SessionState>>,
    db: std::sync::Arc<tokio::sync::Mutex<Option<boxxy_db::Db>>>,
) -> ClawAgent {
    match provider {
        ModelProvider::Gemini(model, _thinking) => {
            let client = gemini::Client::new(api_key.trim()).unwrap();
            let gemini_model = client.completion_model(model.api_name());
            
            let agent = rig::agent::AgentBuilder::new(gemini_model)
                .preamble(system_prompt)
                .default_max_turns(5)
                .tool(SysShellTool { proxy: claw_proxy.clone(), current_dir: current_dir.to_string() })
                .tool(FileReadTool { proxy: claw_proxy.clone(), current_dir: current_dir.to_string() })
                .tool(FileWriteTool { proxy: claw_proxy.clone(), current_dir: current_dir.to_string(), tx_ui: tx_ui.clone(), state: state.clone() })
                .tool(crate::memories::MemoryStoreTool { db: db.clone(), current_dir: current_dir.to_string() })
                .tool(TerminalCommandTool { tx_ui: tx_ui.clone(), state: state.clone() })
                .tool(ActivateSkillTool)
                .tool(ReadPaneTool)
                .tool(SetWorkspaceIntentTool { project_path: current_dir.to_string() })
                .build();
                
            ClawAgent::Gemini(agent)
        },
        ModelProvider::Ollama(model_name) => {
            let client: ollama::Client = ollama::Client::builder()
                .api_key(rig::client::Nothing)
                .base_url(ollama_url)
                .build()
                .unwrap();
            let ollama_model = client.completion_model(model_name.as_str());
            
            let agent = rig::agent::AgentBuilder::new(ollama_model)
                .preamble(system_prompt)
                .default_max_turns(5)
                .tool(SysShellTool { proxy: claw_proxy.clone(), current_dir: current_dir.to_string() })
                .tool(FileReadTool { proxy: claw_proxy.clone(), current_dir: current_dir.to_string() })
                .tool(FileWriteTool { proxy: claw_proxy.clone(), current_dir: current_dir.to_string(), tx_ui: tx_ui.clone(), state: state.clone() })
                .tool(crate::memories::MemoryStoreTool { db: db.clone(), current_dir: current_dir.to_string() })
                .tool(TerminalCommandTool { tx_ui: tx_ui.clone(), state: state.clone() })
                .tool(ActivateSkillTool)
                .tool(ReadPaneTool)
                .tool(SetWorkspaceIntentTool { project_path: current_dir.to_string() })
                .build();
                
            ClawAgent::Ollama(agent)
        }
    }
}
