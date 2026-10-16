use std::sync::Arc;
use std::time::Duration;

use rig::message::ToolName;
use rig::tool::{DynamicTool, ToolExecutionError, ToolOutput};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, ResourceContents, Tool as McpToolDefinition,
};

use super::schema::translate_schema;
use crate::manager::{McpClientManager, McpClientWrapper};

/// Decoupled observer trait so `boxxy-claw` can surface MCP tool invocations
/// in the Claw indicator badge, sidebar debug log, and telemetry without
/// introducing a circular dependency between `boxxy-mcp` and `boxxy-preferences`.
#[async_trait::async_trait]
pub trait McpToolObserver: Send + Sync {
    async fn on_tool_started(&self, tool_name: String);
    async fn on_tool_result(&self, tool_name: String, result: String);
}

pub struct DynamicMcpTool {
    pub manager: Option<Arc<McpClientManager>>,
    pub client: Arc<McpClientWrapper>,
    pub mcp_tool: McpToolDefinition,
    pub server_name: String,
    pub timeout_ms: u64,
    pub observer: Option<Arc<dyn McpToolObserver>>,
}

impl DynamicMcpTool {
    pub fn name(&self) -> String {
        sanitize_tool_name(&self.server_name, &self.mcp_tool.name)
    }

    pub fn to_dynamic_tool(&self) -> Option<DynamicTool> {
        let normalized_name = self.name();
        let name = ToolName::new(normalized_name.clone()).ok()?;
        let parameters = translate_schema(self.mcp_tool.schema_as_json_value());

        let base_desc = self
            .mcp_tool
            .description
            .as_deref()
            .or(self.mcp_tool.title.as_deref())
            .or_else(|| {
                self.mcp_tool
                    .annotations
                    .as_ref()
                    .and_then(|a| a.title.as_deref())
            })
            .unwrap_or("External MCP tool");
        let description = base_desc.to_string();

        let manager = self.manager.clone();
        let fallback_client = self.client.clone();
        let server_name = self.server_name.clone();
        let mcp_tool_name = self.mcp_tool.name.clone();
        let timeout_ms = self.timeout_ms.max(1_000);
        let observer = self.observer.clone();

        Some(DynamicTool::new(
            name,
            description,
            parameters,
            move |args| {
                let manager = manager.clone();
                let fallback_client = fallback_client.clone();
                let server_name = server_name.clone();
                let mcp_tool_name = mcp_tool_name.clone();
                let normalized_name = normalized_name.clone();
                let observer = observer.clone();

                Box::pin(async move {
                    if let Some(obs) = &observer {
                        obs.on_tool_started(normalized_name.clone()).await;
                    }

                    // Obtain a live client from the manager (reconnecting if the child process
                    // or HTTP session died), falling back to the captured client handle.
                    let client = if let Some(mgr) = &manager {
                        match mgr.ensure_client(&server_name).await {
                            Ok(c) => c,
                            Err(e) => {
                                let err_msg = format!(
                                    "Failed to connect to MCP server '{server_name}': {e}"
                                );
                                if let Some(obs) = &observer {
                                    obs.on_tool_result(normalized_name, err_msg.clone()).await;
                                }
                                return Err(ToolExecutionError::other(err_msg));
                            }
                        }
                    } else {
                        fallback_client
                    };

                    let mut params = CallToolRequestParams::new(mcp_tool_name.clone());
                    if let Some(obj) = args.as_object() {
                        params.arguments = Some(obj.clone());
                    }

                    let call_res = tokio::time::timeout(
                        Duration::from_millis(timeout_ms),
                        client.call_tool(params),
                    )
                    .await;

                    match call_res {
                        Ok(Ok(result)) => {
                            let formatted = format_call_tool_result(&result);
                            if let Some(obs) = &observer {
                                obs.on_tool_result(normalized_name, formatted.clone()).await;
                            }
                            if result.is_error.unwrap_or(false) {
                                Err(ToolExecutionError::other(formatted))
                            } else {
                                Ok(ToolOutput::text(formatted))
                            }
                        }
                        Ok(Err(e)) => {
                            let err_msg = format!("MCP tool '{mcp_tool_name}' failed: {e}");
                            if let Some(obs) = &observer {
                                obs.on_tool_result(normalized_name, err_msg.clone()).await;
                            }
                            Err(ToolExecutionError::other(err_msg))
                        }
                        Err(_) => {
                            let err_msg = format!(
                                "MCP tool '{mcp_tool_name}' on server '{server_name}' timed out after {timeout_ms}ms"
                            );
                            if let Some(obs) = &observer {
                                obs.on_tool_result(normalized_name, err_msg.clone()).await;
                            }
                            Err(ToolExecutionError::other(err_msg))
                        }
                    }
                })
            },
        ))
    }
}

/// Strictly normalizes `{server_name}__{tool_name}` to `[a-zA-Z0-9_-]+` for LLM compatibility.
pub fn sanitize_tool_name(server_name: &str, tool_name: &str) -> String {
    let mut name = format!("{server_name}__{tool_name}")
        .replace(' ', "_")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect::<String>();

    // Ensure name doesn't start with a digit (invalid for several LLM function schemas).
    if name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        name = format!("_{name}");
    }
    if name.is_empty() {
        name = "_mcp_tool".to_string();
    }
    name
}

/// Extracts a clean, human- and LLM-readable string from an MCP `CallToolResult`.
///
/// Handles SEP-2106 `structured_content` (any JSON value, including primitives/arrays/null)
/// and avoids wrapping plain text blocks in redundant JSON envelopes.
pub fn format_call_tool_result(result: &CallToolResult) -> String {
    let rendered_blocks: Vec<String> = result
        .content
        .iter()
        .map(format_content_block)
        .filter(|s| !s.is_empty())
        .collect();

    let text_body = rendered_blocks.join("\n\n");

    match (&result.structured_content, text_body.is_empty()) {
        (Some(structured), true) => {
            if let Some(s) = structured.as_str() {
                s.to_string()
            } else {
                serde_json::to_string(structured).unwrap_or_else(|_| structured.to_string())
            }
        }
        (Some(structured), false) if !structured.is_null() => {
            // If `structured` is FastMCP's single-key `{"result": "<string>"}` wrapper around
            // the exact same plain text block, return the plain text directly.
            if let Some(inner_str) = unwrap_single_result_field(structured).and_then(|v| v.as_str())
                && text_body.trim() == inner_str.trim()
            {
                return text_body;
            }

            let structured_str =
                serde_json::to_string(structured).unwrap_or_else(|_| structured.to_string());
            if is_redundant_text_representation(&rendered_blocks, &text_body, structured, &structured_str) {
                structured_str
            } else {
                serde_json::json!({
                    "content": text_body,
                    "structured_content": structured
                })
                .to_string()
            }
        }
        _ => text_body,
    }
}

fn unwrap_single_result_field(val: &serde_json::Value) -> Option<&serde_json::Value> {
    let obj = val.as_object()?;
    if obj.len() == 1 {
        obj.get("result")
    } else {
        None
    }
}

/// Checks whether the MCP `content` text blocks are merely a serialized mirror of `structured_content`
/// (covering standard JSON objects, FastMCP pretty-printed JSON, and FastMCP `{"result": [...]}` list blocks).
fn is_redundant_text_representation(
    rendered_blocks: &[String],
    text_body: &str,
    structured: &serde_json::Value,
    structured_str: &str,
) -> bool {
    let trimmed = text_body.trim();
    if trimmed == structured_str.trim() {
        return true;
    }

    if looks_like_json(trimmed)
        && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(trimmed)
    {
        if &parsed == structured {
            return true;
        }
        if let Some(inner) = unwrap_single_result_field(structured)
            && &parsed == inner
        {
            return true;
        }
    }

    // FastMCP serializes `list[T]` return values as N separate TextContent JSON blocks
    // alongside `structured_content: {"result": [item_0, item_1, ...]}`.
    if let Some(serde_json::Value::Array(arr)) = unwrap_single_result_field(structured)
        && arr.len() == rendered_blocks.len()
        && !arr.is_empty()
        && arr.iter().zip(rendered_blocks.iter()).all(|(expected, block_str)| {
            let b = block_str.trim();
            looks_like_json(b)
                && serde_json::from_str::<serde_json::Value>(b)
                    .ok()
                    .as_ref()
                    == Some(expected)
        })
    {
        return true;
    }

    false
}

fn looks_like_json(s: &str) -> bool {
    matches!(
        s.as_bytes().first(),
        Some(b'{' | b'[' | b'"' | b't' | b'f' | b'n' | b'-' | b'0'..=b'9')
    )
}

fn format_content_block(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Text(text) => text.text.clone(),
        ContentBlock::Image(img) => {
            format!("[Image ({}, {} bytes base64)]", img.mime_type, img.data.len())
        }
        ContentBlock::Audio(audio) => {
            format!(
                "[Audio ({}, {} bytes base64)]",
                audio.mime_type,
                audio.data.len()
            )
        }
        ContentBlock::Resource(embedded) => match &embedded.resource {
            ResourceContents::TextResourceContents {
                uri,
                mime_type,
                text,
                ..
            } => {
                let mime = mime_type.as_deref().unwrap_or("text/plain");
                format!("[Resource {uri} ({mime})]\n{text}")
            }
            ResourceContents::BlobResourceContents {
                uri,
                mime_type,
                blob,
                ..
            } => {
                let mime = mime_type.as_deref().unwrap_or("application/octet-stream");
                format!("[Binary Resource {uri} ({mime}, {} bytes base64)]", blob.len())
            }
            _ => serde_json::to_string(&embedded.resource).unwrap_or_default(),
        },
        ContentBlock::ResourceLink(resource) => {
            let desc = resource
                .description
                .as_deref()
                .map(|d| format!(" — {d}"))
                .unwrap_or_default();
            format!("[Resource Link: {} ({}){desc}]", resource.name, resource.uri)
        }
        _ => serde_json::to_string(block).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::tool::ToolContext;
    use rig::tool::server::ToolServer;
    use rmcp::service::{ClientLifecycleMode, serve_client_with_lifecycle};
    use serde_json::json;
    use tokio::sync::Mutex;

    struct RecordingObserver {
        started: Mutex<Vec<String>>,
        results: Mutex<Vec<(String, String)>>,
    }

    impl RecordingObserver {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                started: Mutex::new(Vec::new()),
                results: Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait::async_trait]
    impl McpToolObserver for RecordingObserver {
        async fn on_tool_started(&self, tool_name: String) {
            self.started.lock().await.push(tool_name);
        }

        async fn on_tool_result(&self, tool_name: String, result: String) {
            self.results.lock().await.push((tool_name, result));
        }
    }

    #[test]
    fn sanitize_tool_name_normalizes_invalid_chars() {
        assert_eq!(
            sanitize_tool_name("My Server!", "search.code/v2"),
            "My_Server__searchcodev2"
        );
        assert_eq!(sanitize_tool_name("123server", "run"), "_123server__run");
        assert_eq!(sanitize_tool_name("github", "get_issue"), "github__get_issue");
    }

    #[test]
    fn format_call_tool_result_extracts_plain_text() {
        let res = CallToolResult::success(vec![
            ContentBlock::text("First line"),
            ContentBlock::text("Second line"),
        ]);
        assert_eq!(format_call_tool_result(&res), "First line\n\nSecond line");
    }

    #[test]
    fn format_call_tool_result_formats_errors_cleanly() {
        let res = CallToolResult::error(vec![ContentBlock::text("Permission denied: /etc/shadow")]);
        assert_eq!(
            format_call_tool_result(&res),
            "Permission denied: /etc/shadow"
        );
    }

    #[test]
    fn format_call_tool_result_handles_structured_content() {
        let mut res = CallToolResult::success(vec![]);
        res.structured_content = Some(json!({"count": 42, "items": ["a", "b"]}));
        assert_eq!(
            format_call_tool_result(&res),
            r#"{"count":42,"items":["a","b"]}"#
        );
    }

    #[test]
    fn format_call_tool_result_deduplicates_matching_text_and_structured() {
        let payload = json!({"status": "ok", "items": [1, 2, 3]});
        // Pretty-printed text block (as emitted by FastMCP) vs compact structured_content
        let pretty_text = serde_json::to_string_pretty(&payload).unwrap();
        let mut res = CallToolResult::success(vec![ContentBlock::text(pretty_text)]);
        res.structured_content = Some(payload);
        assert_eq!(
            format_call_tool_result(&res),
            r#"{"status":"ok","items":[1,2,3]}"#
        );

        // FastMCP `list[dict]` return shape: N pretty-printed TextContent blocks + `{"result": [...]}`
        let item1 = json!({"a": 1});
        let item2 = json!({"b": 2});
        let mut list_res = CallToolResult::success(vec![
            ContentBlock::text(serde_json::to_string_pretty(&item1).unwrap()),
            ContentBlock::text(serde_json::to_string_pretty(&item2).unwrap()),
        ]);
        list_res.structured_content = Some(json!({"result": [item1, item2]}));
        assert_eq!(
            format_call_tool_result(&list_res),
            r#"{"result":[{"a":1},{"b":2}]}"#
        );

        // FastMCP `str` return shape: plain text block + `{"result": "hello"}`
        let mut str_res = CallToolResult::success(vec![ContentBlock::text("hello world")]);
        str_res.structured_content = Some(json!({"result": "hello world"}));
        assert_eq!(format_call_tool_result(&str_res), "hello world");
    }

    #[tokio::test]
    async fn dynamic_mcp_tool_executes_and_notifies_observer_over_2026_07_28_discover() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let (server_io, client_io) = tokio::io::duplex(8192);
        let (server_read, mut server_write) = tokio::io::split(server_io);
        let mut reader = BufReader::new(server_read).lines();

        let server_task = tokio::spawn(async move {
            // 1. Handle `server/discover`
            let line = reader.next_line().await.unwrap().expect("discover line");
            let req: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(req["method"], "server/discover");
            assert_eq!(
                req["params"]["_meta"]["io.modelcontextprotocol/clientInfo"]["name"],
                "boxxy-terminal"
            );

            let discover_resp = json!({
                "jsonrpc": "2.0",
                "id": req["id"],
                "result": {
                    "resultType": "complete",
                    "supportedVersions": ["2026-07-28"],
                    "capabilities": { "tools": {} },
                    "ttlMs": 0,
                    "cacheScope": "private"
                }
            });
            server_write
                .write_all(format!("{discover_resp}\n").as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            // 2. Handle `tools/call`
            let call_line = reader.next_line().await.unwrap().expect("tools/call line");
            let call_req: serde_json::Value = serde_json::from_str(&call_line).unwrap();
            assert_eq!(call_req["method"], "tools/call");
            assert_eq!(call_req["params"]["name"], "echo_msg");
            assert_eq!(call_req["params"]["arguments"]["message"], "hello rmcp 3.5.1");

            let call_resp = json!({
                "jsonrpc": "2.0",
                "id": call_req["id"],
                "result": {
                    "resultType": "complete",
                    "content": [
                        { "type": "text", "text": "echo: hello rmcp 3.5.1" }
                    ]
                }
            });
            server_write
                .write_all(format!("{call_resp}\n").as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();
        });

        let manager = McpClientManager::new();
        let handler = crate::client::handler::BoxxyClientHandler::new("demo", Arc::downgrade(&manager));
        let client = serve_client_with_lifecycle(
            handler,
            client_io,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
                legacy_version: Some(rmcp::model::ProtocolVersion::LATEST_WITH_INITIALIZE),
            },
        )
        .await
        .expect("client initialized via server/discover");

        let client_arc = Arc::new(client);
        let schema_obj = json!({
            "type": "object",
            "properties": {
                "message": { "type": "string" }
            },
            "required": ["message"]
        })
        .as_object()
        .unwrap()
        .clone();

        let mcp_tool = McpToolDefinition::new("echo_msg", "Echoes a message", Arc::new(schema_obj));
        let observer = RecordingObserver::new();

        let dyn_mcp_tool = DynamicMcpTool {
            manager: None,
            client: client_arc.clone(),
            mcp_tool,
            server_name: "demo".to_string(),
            timeout_ms: 5_000,
            observer: Some(observer.clone()),
        };

        let rig_tool = dyn_mcp_tool.to_dynamic_tool().expect("valid dynamic tool");
        let tool_handle = ToolServer::new().dynamic_tools(vec![rig_tool]).run();
        let mut ctx = ToolContext::default();
        let result = tool_handle
            .execute(
                "demo__echo_msg",
                r#"{"message":"hello rmcp 3.5.1"}"#,
                &mut ctx,
            )
            .await;

        assert!(!result.is_error());
        assert_eq!(
            observer.started.lock().await.as_slice(),
            &["demo__echo_msg".to_string()]
        );
        assert_eq!(
            observer.results.lock().await.as_slice(),
            &[(
                "demo__echo_msg".to_string(),
                "echo: hello rmcp 3.5.1".to_string()
            )]
        );

        server_task.await.unwrap();
    }
}
