use std::collections::HashMap;
use std::hash::BuildHasher;
use std::time::Duration;

use anyhow::{Context, Result};
use rmcp::service::{ClientCacheConfig, RunningService};
use rmcp::transport::child_process::TokioChildProcess;
use rmcp::{RoleClient, serve_client};

use super::error::PermanentConfigError;
use super::handler::BoxxyClientHandler;
use crate::config::MIN_TIMEOUT_MS;

/// Spawns a child-process MCP server over Stdio and completes the `initialize` handshake.
///
/// # Errors
///
/// Returns an error if the command is empty or missing from `PATH` (`PermanentConfigError`),
/// the child process fails to spawn, or the `initialize` handshake fails or times out.
pub async fn build_stdio_client<S: BuildHasher + Sync>(
    handler: BoxxyClientHandler,
    command_name: &str,
    args: &[String],
    env: &HashMap<String, String, S>,
    timeout_ms: u64,
) -> Result<RunningService<RoleClient, BoxxyClientHandler>> {
    let trimmed_cmd = command_name.trim();
    if trimmed_cmd.is_empty() {
        return Err(anyhow::Error::new(PermanentConfigError(
            "Stdio MCP server command cannot be empty".to_string(),
        )));
    }

    let mut cmd = rmcp::transport::which_command(trimmed_cmd).map_err(|e| {
        anyhow::Error::new(PermanentConfigError(format!(
            "Executable not found in PATH '{trimmed_cmd}': {e}"
        )))
    })?;
    cmd.args(args);
    cmd.kill_on_drop(true);

    // Scrub Boxxy internal/dev session markers before applying explicit server env overrides.
    for var_name in boxxy_sys_utils::env::scrubbed_pty_var_names() {
        cmd.env_remove(var_name);
    }
    for (k, v) in env {
        let key = k.trim();
        if !key.is_empty() {
            cmd.env(key, v);
        }
    }

    let transport =
        TokioChildProcess::new(cmd).context("Failed to initialize Stdio MCP transport")?;

    let init_timeout = Duration::from_millis(timeout_ms.max(MIN_TIMEOUT_MS));
    let client = tokio::time::timeout(init_timeout, serve_client(handler, transport))
        .await
        .with_context(|| {
            format!(
                "Timed out after {}ms waiting for Stdio MCP server '{trimmed_cmd}' to initialize",
                init_timeout.as_millis()
            )
        })?
        .map_err(|e| anyhow::anyhow!("Failed to initialize Stdio MCP client: {e}"))?;

    // Surface transport/server errors directly rather than masking them with stale cached pages.
    client
        .peer()
        .set_response_cache_config(ClientCacheConfig::default().with_serve_stale_on_error(false))
        .await;

    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::McpClientManager;
    use std::sync::Arc;

    #[tokio::test]
    async fn rejects_empty_or_whitespace_stdio_command_with_permanent_error() {
        let manager = McpClientManager::new();
        let handler = BoxxyClientHandler::new("empty-cmd", Arc::downgrade(&manager));
        let err = build_stdio_client(handler, "   ", &[], &HashMap::new(), 5_000)
            .await
            .err()
            .expect("empty command must fail");
        assert!(err.downcast_ref::<PermanentConfigError>().is_some());
    }

    #[tokio::test]
    async fn rejects_missing_binary_with_permanent_error() {
        let manager = McpClientManager::new();
        let handler = BoxxyClientHandler::new("missing-bin", Arc::downgrade(&manager));
        let err = build_stdio_client(
            handler,
            "boxxy_nonexistent_mcp_binary_9f8a7b6c",
            &[],
            &HashMap::new(),
            5_000,
        )
        .await
        .err()
        .expect("missing binary must fail");
        assert!(err.downcast_ref::<PermanentConfigError>().is_some());
    }
}
