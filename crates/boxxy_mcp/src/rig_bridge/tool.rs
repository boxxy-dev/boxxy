use std::fmt::Write as _;
use std::sync::{Arc, Weak};
use std::time::Duration;

use rig::completion::ToolAnnotations;
use rig::message::ToolName;
use rig::tool::{DynamicTool, ToolExecutionError, ToolOutput};
use rmcp::model::{CallToolRequestParams, ErrorCode, JsonObject, Tool as McpToolDefinition};
use rmcp::service::ServiceError;
use serde_json::Value;

use super::output::build_tool_outputs;
pub use super::output::format_call_tool_result;
use super::schema::translate_schema;
use crate::client::error::format_service_error;
use crate::config::MIN_TIMEOUT_MS;
use crate::manager::{McpClientManager, McpClientWrapper};

/// Strict `OpenAI` / Gemini / Anthropic maximum function tool name length.
const MAX_TOOL_NAME_LEN: usize = 64;

/// Observer trait for surfacing MCP tool calls in Boxxy's UI and telemetry.
///
/// Keeps `boxxy-mcp` decoupled from `boxxy-claw` and `boxxy-preferences`.
#[async_trait::async_trait]
pub trait McpToolObserver: Send + Sync {
    async fn on_tool_started(&self, tool_name: String);
    async fn on_tool_result(&self, tool_name: String, result: String);
}

#[derive(Clone)]
struct ToolInvocationContext {
    manager: Option<Arc<McpClientManager>>,
    fallback_client: Weak<McpClientWrapper>,
    server_name: String,
    mcp_tool_name: String,
    normalized_name: String,
    timeout_ms: u64,
    observer: Option<Arc<dyn McpToolObserver>>,
}

pub struct DynamicMcpTool {
    pub manager: Option<Arc<McpClientManager>>,
    /// Weak fallback reference so `DynamicMcpTool` never pins a disconnected
    /// `RunningService` alive when `McpClientManager::update_configs` runs
    /// graceful `close_with_timeout` shutdown.
    pub fallback_client: Weak<McpClientWrapper>,
    pub mcp_tool: McpToolDefinition,
    pub server_name: String,
    pub timeout_ms: u64,
    pub observer: Option<Arc<dyn McpToolObserver>>,
}

impl DynamicMcpTool {
    #[must_use]
    pub fn name(&self) -> String {
        sanitize_tool_name(&self.server_name, &self.mcp_tool.name)
    }

    #[must_use]
    pub fn to_dynamic_tool(&self) -> Option<DynamicTool> {
        let normalized_name = self.name();
        let name = ToolName::new(normalized_name.clone()).ok()?;
        let parameters = translate_schema(self.mcp_tool.schema_as_json_value());
        let annotations = to_rig_annotations(&self.mcp_tool);

        let description = self
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
            .unwrap_or("External MCP tool")
            .to_string();

        let call_ctx = Arc::new(ToolInvocationContext {
            manager: self.manager.clone(),
            fallback_client: self.fallback_client.clone(),
            server_name: self.server_name.clone(),
            mcp_tool_name: self.mcp_tool.name.to_string(),
            normalized_name,
            timeout_ms: self.timeout_ms.max(MIN_TIMEOUT_MS),
            observer: self.observer.clone(),
        });

        Some(
            DynamicTool::new(name, description, parameters, move |args| {
                let ctx = call_ctx.clone();
                Box::pin(async move { execute_mcp_tool_call(&ctx, args).await })
            })
            .with_annotations(annotations),
        )
    }
}

async fn execute_mcp_tool_call(
    ctx: &ToolInvocationContext,
    args: Value,
) -> Result<ToolOutput, ToolExecutionError> {
    if let Some(obs) = &ctx.observer {
        obs.on_tool_started(ctx.normalized_name.clone()).await;
    }

    let arguments = match parse_mcp_arguments(args) {
        Ok(arguments) => arguments,
        Err(bad_kind) => {
            let err_msg = format!(
                "MCP tool '{}' expected a JSON object or null arguments, got {bad_kind}",
                ctx.mcp_tool_name
            );
            notify_result(ctx.observer.as_ref(), &ctx.normalized_name, &err_msg).await;
            return Err(ToolExecutionError::invalid_args(err_msg));
        }
    };

    let client = resolve_live_client(ctx).await?;
    let mut params = CallToolRequestParams::new(ctx.mcp_tool_name.clone());
    params.arguments = arguments;

    let call_res = tokio::time::timeout(
        Duration::from_millis(ctx.timeout_ms),
        client.call_tool(params),
    )
    .await;

    match call_res {
        Ok(Ok(result)) => {
            let is_error = result.is_error.unwrap_or(false);
            let (summary, model_output) = match build_tool_outputs(&result) {
                Ok(pair) => pair,
                Err(err) => {
                    notify_result(
                        ctx.observer.as_ref(),
                        &ctx.normalized_name,
                        &err.to_string(),
                    )
                    .await;
                    return Err(err);
                }
            };

            notify_result(ctx.observer.as_ref(), &ctx.normalized_name, &summary).await;

            if is_error {
                Err(ToolExecutionError::other(summary).with_model_output(model_output))
            } else {
                Ok(model_output)
            }
        }
        Ok(Err(e)) => {
            if (matches!(e, ServiceError::TransportClosed)
                || client.is_closed()
                || client.is_transport_closed())
                && let Some(mgr) = &ctx.manager
            {
                mgr.evict_if_same_client(&ctx.server_name, &client).await;
            }

            let err_msg = format!(
                "MCP tool '{}' failed: {}",
                ctx.mcp_tool_name,
                format_service_error(&e)
            );
            notify_result(ctx.observer.as_ref(), &ctx.normalized_name, &err_msg).await;
            Err(classify_service_error(err_msg, e))
        }
        Err(_) => {
            let err_msg = format!(
                "MCP tool '{}' on server '{}' timed out after {}ms",
                ctx.mcp_tool_name, ctx.server_name, ctx.timeout_ms
            );
            notify_result(ctx.observer.as_ref(), &ctx.normalized_name, &err_msg).await;
            Err(ToolExecutionError::timeout(err_msg))
        }
    }
}

fn classify_service_error(err_msg: String, err: ServiceError) -> ToolExecutionError {
    match &err {
        ServiceError::Timeout { .. } => ToolExecutionError::timeout(err_msg).with_source(err),
        ServiceError::McpError(data) if data.code == ErrorCode::INVALID_PARAMS => {
            ToolExecutionError::invalid_args(err_msg).with_source(err)
        }
        _ => ToolExecutionError::provider(err_msg).with_source(err),
    }
}

async fn resolve_live_client(
    ctx: &ToolInvocationContext,
) -> Result<Arc<McpClientWrapper>, ToolExecutionError> {
    if let Some(mgr) = &ctx.manager {
        return match mgr.ensure_client(&ctx.server_name).await {
            Ok(c) => Ok(c),
            Err(e) => {
                let err_msg = format!("Failed to connect to MCP server '{}': {e}", ctx.server_name);
                notify_result(ctx.observer.as_ref(), &ctx.normalized_name, &err_msg).await;
                Err(ToolExecutionError::provider(err_msg))
            }
        };
    }

    if let Some(c) = ctx.fallback_client.upgrade()
        && !c.is_closed()
        && !c.is_transport_closed()
    {
        return Ok(c);
    }

    let err_msg = format!(
        "MCP connection for server '{}' has been closed",
        ctx.server_name
    );
    notify_result(ctx.observer.as_ref(), &ctx.normalized_name, &err_msg).await;
    Err(ToolExecutionError::provider(err_msg))
}

async fn notify_result(
    observer: Option<&Arc<dyn McpToolObserver>>,
    normalized_name: &str,
    result: &str,
) {
    if let Some(obs) = observer {
        obs.on_tool_result(normalized_name.to_string(), result.to_string())
            .await;
    }
}

fn to_rig_annotations(tool: &McpToolDefinition) -> ToolAnnotations {
    let mut out = ToolAnnotations::default();
    if let Some(hints) = &tool.annotations {
        out.title.clone_from(&hints.title);
        out.read_only_hint = hints.read_only_hint;
        out.destructive_hint = hints.destructive_hint;
        out.idempotent_hint = hints.idempotent_hint;
        out.open_world_hint = hints.open_world_hint;
    }
    out
}

fn parse_mcp_arguments(value: Value) -> Result<Option<JsonObject>, &'static str> {
    match value {
        Value::Null => Ok(None),
        Value::Object(map) => Ok(Some(map)),
        Value::Bool(_) => Err("boolean"),
        Value::Number(_) => Err("number"),
        Value::String(_) => Err("string"),
        Value::Array(_) => Err("array"),
    }
}

/// Normalizes a single name segment to ASCII `[a-zA-Z0-9_-]+`, mapping common
/// separators (` `, `.`, `/`, `:`, `\`) to `_` and collapsing consecutive `_`.
fn sanitize_segment(raw: &str, fallback_prefix: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_underscore = false;

    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' {
            out.push(ch);
            last_underscore = false;
        } else if (ch == '_' || ch.is_whitespace() || matches!(ch, '.' | '/' | ':' | '\\'))
            && !out.is_empty()
            && !last_underscore
        {
            out.push('_');
            last_underscore = true;
        }
    }

    if out.ends_with('_') {
        out.pop();
    }

    if out.is_empty() {
        format!("{fallback_prefix}_{:08x}", fnv1a_32(raw.as_bytes()))
    } else {
        out
    }
}

const fn fnv1a_32_update(mut hash: u32, bytes: &[u8]) -> u32 {
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u32;
        hash = hash.wrapping_mul(0x0100_0193);
        i += 1;
    }
    hash
}

const fn fnv1a_32(bytes: &[u8]) -> u32 {
    fnv1a_32_update(0x811c_9dc5, bytes)
}

const fn fnv1a_32_pair(left: &[u8], right: &[u8]) -> u32 {
    let h = fnv1a_32_update(0x811c_9dc5, left);
    let h = fnv1a_32_update(h, &[0]);
    fnv1a_32_update(h, right)
}

/// Strictly normalizes `{server_name}__{tool_name}` to `^[a-zA-Z_][a-zA-Z0-9_-]{0,63}$`
/// for universal LLM function-calling compatibility (`OpenAI`, Gemini, Anthropic).
#[must_use]
pub fn sanitize_tool_name(server_name: &str, tool_name: &str) -> String {
    if server_name.trim().is_empty() && tool_name.trim().is_empty() {
        return "_mcp_tool".to_string();
    }

    let srv = sanitize_segment(server_name, "srv");
    let tool = sanitize_segment(tool_name, "tool");
    let mut name = format!("{srv}__{tool}");

    if name
        .as_bytes()
        .first()
        .is_some_and(|b| b.is_ascii_digit() || *b == b'-')
    {
        name.insert(0, '_');
    }

    if name.len() > MAX_TOOL_NAME_LEN {
        let hash = fnv1a_32_pair(server_name.as_bytes(), tool_name.as_bytes());
        name.truncate(MAX_TOOL_NAME_LEN - 9);
        while name.ends_with('_') && name.len() > 1 {
            name.pop();
        }
        let _ = write!(name, "_{hash:08x}");
    }

    name
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::tool::ToolContext;
    use rig::tool::server::ToolServer;
    use rmcp::service::{ClientLifecycleMode, serve_client_with_lifecycle};
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
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

    fn discover_ok_response(id: &Value) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "resultType": "complete",
                "supportedVersions": ["2026-07-28"],
                "capabilities": { "tools": {} },
                "ttlMs": 0,
                "cacheScope": "private"
            }
        })
    }

    #[test]
    fn sanitize_tool_name_normalizes_separators_unicode_and_64_char_cap() {
        assert_eq!(
            sanitize_tool_name("My Server!", "search.code/v2"),
            "My_Server__search_code_v2"
        );
        assert_eq!(sanitize_tool_name("123server", "run"), "_123server__run");
        assert_eq!(
            sanitize_tool_name("github", "get_issue"),
            "github__get_issue"
        );
        assert_eq!(sanitize_tool_name("", ""), "_mcp_tool");

        let jp_a = sanitize_tool_name("サーバーA", "ping");
        let jp_b = sanitize_tool_name("サーバーB", "ping");
        assert!(
            jp_a.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        );
        assert_ne!(jp_a, jp_b);

        let long_a = sanitize_tool_name(
            "enterprise_observability_cluster_primary",
            "fetch_distributed_trace_span_attributes_by_service_a",
        );
        let long_b = sanitize_tool_name(
            "enterprise_observability_cluster_primary",
            "fetch_distributed_trace_span_attributes_by_service_b",
        );
        assert!(long_a.len() <= MAX_TOOL_NAME_LEN);
        assert!(long_b.len() <= MAX_TOOL_NAME_LEN);
        assert_ne!(long_a, long_b);
    }

    #[tokio::test]
    async fn dynamic_mcp_tool_rejects_non_object_arguments_without_calling_server() {
        let (server_io, client_io) = tokio::io::duplex(8192);
        let (server_read, mut server_write) = tokio::io::split(server_io);
        let mut reader = BufReader::new(server_read).lines();

        let server_task = tokio::spawn(async move {
            let line = reader.next_line().await.unwrap().expect("discover line");
            let req: Value = serde_json::from_str(&line).unwrap();
            let resp = discover_ok_response(&req["id"]);
            server_write
                .write_all(format!("{resp}\n").as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();
        });

        let manager = McpClientManager::new();
        let handler =
            crate::client::handler::BoxxyClientHandler::new("demo", Arc::downgrade(&manager));
        let client = Arc::new(
            serve_client_with_lifecycle(
                handler,
                client_io,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
                },
            )
            .await
            .unwrap(),
        );
        server_task.await.unwrap();

        let schema_obj = json!({ "type": "object", "properties": {} })
            .as_object()
            .unwrap()
            .clone();
        let mcp_tool = McpToolDefinition::new("strict_op", "Strict tool", Arc::new(schema_obj));
        let observer = RecordingObserver::new();

        let dyn_mcp_tool = DynamicMcpTool {
            manager: None,
            fallback_client: Arc::downgrade(&client),
            mcp_tool,
            server_name: "demo".to_string(),
            timeout_ms: 2_000,
            observer: Some(observer.clone()),
        };

        let rig_tool = dyn_mcp_tool.to_dynamic_tool().unwrap();
        let tool_handle = ToolServer::new().dynamic_tools(vec![rig_tool]).run();
        let mut ctx = ToolContext::default();
        let result = tool_handle
            .toolset()
            .execute("demo__strict_op", r#"["unexpected_array"]"#, &mut ctx)
            .await;

        assert!(result.is_error());
        let recorded = observer.results.lock().await.clone();
        assert_eq!(recorded.len(), 1);
        assert!(recorded[0].1.contains("got array"));
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn dynamic_mcp_tool_executes_and_accepts_contentless_results_over_2026_07_28_discover() {
        let (server_io, client_io) = tokio::io::duplex(8192);
        let (server_read, mut server_write) = tokio::io::split(server_io);
        let mut reader = BufReader::new(server_read).lines();

        let server_task = tokio::spawn(async move {
            let line = reader.next_line().await.unwrap().expect("discover line");
            let req: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(req["method"], "server/discover");
            assert_eq!(
                req["params"]["_meta"]["io.modelcontextprotocol/clientInfo"]["name"],
                "boxxy-terminal"
            );

            let discover_resp = discover_ok_response(&req["id"]);
            server_write
                .write_all(format!("{discover_resp}\n").as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            // 1st call: standard text content
            let call_line = reader.next_line().await.unwrap().expect("tools/call line");
            let call_req: Value = serde_json::from_str(&call_line).unwrap();
            assert_eq!(call_req["method"], "tools/call");
            assert_eq!(call_req["params"]["name"], "echo_msg");
            assert_eq!(
                call_req["params"]["arguments"]["message"],
                "hello rmcp 3.11"
            );

            let call_resp = json!({
                "jsonrpc": "2.0",
                "id": call_req["id"],
                "result": {
                    "resultType": "complete",
                    "content": [{ "type": "text", "text": "echo: hello rmcp 3.11" }]
                }
            });
            server_write
                .write_all(format!("{call_resp}\n").as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            // 2nd call: contentless `tools/call` result (`rmcp 3.11` PR #1434)
            let void_line = reader.next_line().await.unwrap().expect("void tools/call");
            let void_req: Value = serde_json::from_str(&void_line).unwrap();
            let void_resp = json!({
                "jsonrpc": "2.0",
                "id": void_req["id"],
                "result": {
                    "resultType": "complete"
                }
            });
            server_write
                .write_all(format!("{void_resp}\n").as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();
        });

        let manager = McpClientManager::new();
        let handler =
            crate::client::handler::BoxxyClientHandler::new("demo", Arc::downgrade(&manager));
        let client_arc = Arc::new(
            serve_client_with_lifecycle(
                handler,
                client_io,
                ClientLifecycleMode::Auto {
                    preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
                    legacy_version: Some(rmcp::model::ProtocolVersion::LATEST_WITH_INITIALIZE),
                },
            )
            .await
            .expect("client initialized via server/discover"),
        );

        let schema_obj = json!({
            "type": "object",
            "properties": { "message": { "type": "string" } }
        })
        .as_object()
        .unwrap()
        .clone();

        let mcp_tool = McpToolDefinition::new("echo_msg", "Echoes a message", Arc::new(schema_obj));
        let observer = RecordingObserver::new();

        let dyn_mcp_tool = DynamicMcpTool {
            manager: None,
            fallback_client: Arc::downgrade(&client_arc),
            mcp_tool,
            server_name: "demo".to_string(),
            timeout_ms: 5_000,
            observer: Some(observer.clone()),
        };

        let rig_tool = dyn_mcp_tool.to_dynamic_tool().expect("valid dynamic tool");
        let tool_handle = ToolServer::new().dynamic_tools(vec![rig_tool]).run();
        let mut ctx = ToolContext::default();
        let result1 = tool_handle
            .toolset()
            .execute(
                "demo__echo_msg",
                r#"{"message":"hello rmcp 3.11"}"#,
                &mut ctx,
            )
            .await;
        assert!(!result1.is_error());

        let result2 = tool_handle
            .toolset()
            .execute("demo__echo_msg", "{}", &mut ctx)
            .await;
        assert!(!result2.is_error());

        assert_eq!(
            observer.started.lock().await.clone(),
            vec!["demo__echo_msg".to_string(), "demo__echo_msg".to_string()]
        );
        assert_eq!(
            observer.results.lock().await.clone(),
            vec![
                (
                    "demo__echo_msg".to_string(),
                    "echo: hello rmcp 3.11".to_string()
                ),
                ("demo__echo_msg".to_string(), String::new()),
            ]
        );

        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn dynamic_mcp_tool_timeout_emits_abandoned_request_cancellation() {
        let (server_io, client_io) = tokio::io::duplex(8192);
        let (server_read, mut server_write) = tokio::io::split(server_io);
        let mut reader = BufReader::new(server_read).lines();

        let server_task = tokio::spawn(async move {
            let line = reader.next_line().await.unwrap().expect("discover line");
            let req: Value = serde_json::from_str(&line).unwrap();
            let discover_resp = discover_ok_response(&req["id"]);
            server_write
                .write_all(format!("{discover_resp}\n").as_bytes())
                .await
                .unwrap();
            server_write.flush().await.unwrap();

            let call_line = reader.next_line().await.unwrap().expect("tools/call line");
            let call_req: Value = serde_json::from_str(&call_line).unwrap();
            assert_eq!(call_req["method"], "tools/call");
            let call_id = call_req["id"].clone();

            let cancel_line = reader
                .next_line()
                .await
                .unwrap()
                .expect("notifications/cancelled line");
            let cancel_notif: Value = serde_json::from_str(&cancel_line).unwrap();
            assert_eq!(cancel_notif["method"], "notifications/cancelled");
            assert_eq!(cancel_notif["params"]["requestId"], call_id);
        });

        let manager = McpClientManager::new();
        let handler =
            crate::client::handler::BoxxyClientHandler::new("demo", Arc::downgrade(&manager));
        let client_arc = Arc::new(
            serve_client_with_lifecycle(
                handler,
                client_io,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
                },
            )
            .await
            .expect("client initialized"),
        );

        let schema_obj = json!({ "type": "object", "properties": {} })
            .as_object()
            .unwrap()
            .clone();
        let mcp_tool = McpToolDefinition::new("slow_op", "Slow tool", Arc::new(schema_obj));
        let observer = RecordingObserver::new();

        let dyn_mcp_tool = DynamicMcpTool {
            manager: None,
            fallback_client: Arc::downgrade(&client_arc),
            mcp_tool,
            server_name: "demo".to_string(),
            timeout_ms: 1_000,
            observer: Some(observer.clone()),
        };

        let rig_tool = dyn_mcp_tool.to_dynamic_tool().expect("valid dynamic tool");
        let tool_handle = ToolServer::new().dynamic_tools(vec![rig_tool]).run();
        let mut ctx = ToolContext::default();
        let result = tool_handle
            .toolset()
            .execute("demo__slow_op", "{}", &mut ctx)
            .await;

        assert!(result.is_error());
        server_task.await.unwrap();
    }
}
