use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct ActivateSkillArgs {
    pub name: String,
}

#[derive(Serialize)]
pub struct ActivateSkillOutput {
    pub success: bool,
    pub content: String,
}

/// Tool for dynamically loading the full content of a skill from the Toolbox.
pub struct ActivateSkillTool {
    pub approval: std::sync::Arc<dyn boxxy_core_toolbox::ApprovalHandler>,
}

impl Tool for ActivateSkillTool {
    const NAME: &'static str = "activate_skill";

    type Error = std::io::Error;
    type Args = ActivateSkillArgs;
    type Output = ActivateSkillOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Load the full instructions and specialized tools for a skill in your Toolbox. \
            Use this when you identify a relevant skill in the available skills list that hasn't been fully activated yet. \
            CRITICAL: After activating a skill, you MUST immediately read its instructions and use them to fulfill the user's request. Do not activate more skills unless absolutely necessary to avoid infinite loops."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The unique name of the skill to activate (e.g., 'rust-expert', 'docker-admin')."
                    }
                },
                "required": ["name"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.approval.report_tool_started(Self::NAME.to_string()).await;
        boxxy_telemetry::track_tool_use(Self::NAME).await;
        let registry = crate::registry::skills::global_registry().await;
        let all_skills = registry.get_skills().await;

        if let Some(skill) = all_skills
            .into_iter()
            .find(|s| s.frontmatter.name == args.name)
        {
            Ok(ActivateSkillOutput {
                success: true,
                content: format!(
                    "Skill '{}' activated. FULL INSTRUCTIONS:\n\n{}\n\nCRITICAL DIRECTIVE: You have successfully activated this skill. Stop activating skills and proceed immediately to answer the user's prompt using these instructions.",
                    args.name, skill.content
                ),
            })
        } else {
            Ok(ActivateSkillOutput {
                success: false,
                content: format!(
                    "Skill '{}' not found in registry. Please proceed without this skill.",
                    args.name
                ),
            })
        }
    }
}
