use super::schema::translate_schema;
use rig::tool::{DynamicTool, ToolExecutionError, ToolOutput};
use rmcp::model::{CallToolRequestParams, Tool as McpToolDefinition};
use rmcp::{RoleClient, service::RunningService};
use std::sync::Arc;

pub struct DynamicMcpTool {
    pub client: Arc<RunningService<RoleClient, ()>>,
    pub mcp_tool: McpToolDefinition,
    pub server_name: String,
}

impl DynamicMcpTool {
    pub fn name(&self) -> String {
        // Strictly normalize the name to [a-zA-Z0-9_-]+ for LLM compatibility
        let mut name = format!("{}__{}", self.server_name, self.mcp_tool.name)
            .replace(' ', "_")
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
            .collect::<String>();

        // Ensure name doesn't start with a number (invalid for many LLM function schemas)
        if name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            name = format!("_{}", name);
        }
        name
    }

    pub fn to_dynamic_tool(&self) -> DynamicTool {
        let name = self.name();
        let schema_map = (*self.mcp_tool.input_schema).clone();
        let schema = serde_json::Value::Object(schema_map);
        let parameters = translate_schema(schema);
        let description = self
            .mcp_tool
            .description
            .clone()
            .unwrap_or_default()
            .to_string();

        let client = self.client.clone();
        let tool_name = self.mcp_tool.name.clone();

        DynamicTool::new(
            name,
            description,
            parameters,
            move |_context, args| {
                let client = client.clone();
                let tool_name = tool_name.clone();
                Box::pin(async move {
                    let mut params = CallToolRequestParams::new(tool_name);
                    if let Some(obj) = args.as_object() {
                        params.arguments = Some(obj.clone());
                    }

                    match client.call_tool(params).await {
                        Ok(result) => {
                            if result.is_error.unwrap_or(false) {
                                let err_msg = format!("{:?}", result.content);
                                Err(ToolExecutionError::other(err_msg))
                            } else {
                                let output = if let Some(structured) = result.structured_content {
                                    if result.content.is_empty() {
                                        structured
                                    } else {
                                        serde_json::json!({
                                            "content": result.content,
                                            "structured_content": structured
                                        })
                                    }
                                } else {
                                    serde_json::to_value(&result.content)
                                        .unwrap_or(serde_json::Value::Null)
                                };
                                Ok(ToolOutput::text(output.to_string()))
                            }
                        }
                        Err(e) => Err(ToolExecutionError::other(e.to_string())),
                    }
                })
            },
        )
    }
}
