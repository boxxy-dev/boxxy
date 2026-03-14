pub mod file_ops;
pub mod memory;
pub mod terminal;
pub mod skills;
pub mod workspace;

use rig::tool::Tool;
use rig::completion::ToolDefinition;
use serde::{Deserialize, Serialize};
use boxxy_agent::ipc::AgentClawProxy;

#[derive(Deserialize)]
pub struct SysShellArgs {
    pub command: String,
}

#[derive(Serialize)]
pub struct SysShellOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Tool for executing host-level commands via `boxxy-agent` IPC
pub struct SysShellTool {
    pub proxy: AgentClawProxy<'static>,
    pub current_dir: String,
}

impl Tool for SysShellTool {
    const NAME: &'static str = "sys_shell_exec";
    
    type Error = std::io::Error;
    type Args = SysShellArgs;
    type Output = SysShellOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Execute non-interactive bash commands on the host system to diagnose or repair issues. DO NOT use interactive commands (like top without -b, or less).".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The exact bash command to execute."
                    }
                },
                "required": ["command"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let command = if self.current_dir.is_empty() {
            args.command
        } else {
            format!("cd '{}' && {}", self.current_dir.replace('\'', "'\\''"), args.command)
        };
        match self.proxy.exec_shell(command).await {
            Ok((exit_code, stdout, stderr)) => {
                Ok(SysShellOutput {
                    stdout,
                    stderr,
                    exit_code,
                })
            }
            Err(e) => {
                Err(std::io::Error::other(format!("IPC Error: {e}")))
            }
        }
    }
}
