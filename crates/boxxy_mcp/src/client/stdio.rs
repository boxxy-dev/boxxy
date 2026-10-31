use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use rmcp::service::{ClientCacheConfig, RunningService};
use rmcp::transport::child_process::TokioChildProcess;
use rmcp::{RoleClient, serve_client};

use super::handler::BoxxyClientHandler;
use super::http::PermanentConfigError;

pub async fn build_stdio_client(
    handler: BoxxyClientHandler,
    command_name: &str,
    args: &[String],
    env: &HashMap<String, String>,
    timeout_ms: u64,
) -> Result<RunningService<RoleClient, BoxxyClientHandler>> {
    let mut cmd = rmcp::transport::which_command(command_name).map_err(|e| {
        anyhow::Error::new(PermanentConfigError(format!(
            "Executable not found in PATH '{command_name}': {e}"
        )))
    })?;
    cmd.args(args);
    cmd.kill_on_drop(true);

    for (k, v) in env {
        cmd.env(k, v);
    }

    let transport =
        TokioChildProcess::new(cmd).context("Failed to initialize Stdio MCP transport")?;

    let init_timeout = Duration::from_millis(timeout_ms.max(1_000));
    let client = tokio::time::timeout(init_timeout, serve_client(handler, transport))
        .await
        .with_context(|| {
            format!(
                "Timed out after {}ms waiting for Stdio MCP server '{command_name}' to initialize",
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
