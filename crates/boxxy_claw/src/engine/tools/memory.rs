use rig::tool::Tool;
use rig::completion::ToolDefinition;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use chrono::Local;

#[derive(Deserialize)]
pub struct MemoryArgs {
    pub fact: String,
}

#[derive(Serialize)]
pub struct MemoryOutput {
    pub success: bool,
    pub message: String,
}

/// Tool for saving persistent facts or preferences to the agent's long-term memory file.
pub struct MemoryTool;

impl Tool for MemoryTool {
    const NAME: &'static str = "remember_fact";
    
    type Error = std::io::Error;
    type Args = MemoryArgs;
    type Output = MemoryOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Save a persistent fact, user preference, or lesson learned to your long-term memory. \
            Use this to remember things across sessions, like preferred tools, directory structures, or custom rules.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "fact": {
                        "type": "string",
                        "description": "The fact or preference to remember (e.g., 'User prefers using dnf over apt' or 'Project X uses a custom build script in ./tools')."
                    }
                },
                "required": ["fact"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
            let config_dir = dirs.config_dir();
            let memory_path = config_dir.join("boxxyclaw").join("CLAW_STATE.md");
            
            // Ensure directory exists
            if let Some(parent) = memory_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&memory_path)?;

            let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
            writeln!(file, "- [{}] {}", timestamp, args.fact)?;

            Ok(MemoryOutput {
                success: true,
                message: format!("Successfully remembered: {}", args.fact),
            })
        } else {
            Err(std::io::Error::other("Could not resolve config directory"))
        }
    }
}
