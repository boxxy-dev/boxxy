use std::collections::HashMap;
use std::hash::BuildHasher;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use parking_lot::Mutex;
use rmcp::service::{ClientCacheConfig, RunningService};
use rmcp::transport::child_process::TokioChildProcess;
use rmcp::{RoleClient, serve_client};
use tokio::io::AsyncReadExt;
use tokio::process::ChildStderr;

use super::error::{PermanentConfigError, format_client_init_error};
use super::handler::BoxxyClientHandler;
use crate::config::MIN_TIMEOUT_MS;

/// Maximum trailing `stderr` bytes retained during Stdio initialization so startup
/// crashes (missing modules, invalid flags, auth errors) produce actionable diagnostics
/// without unbounded memory growth.
const MAX_STDERR_DIAGNOSTIC_BYTES: usize = 2_048;

struct StderrStartupCapture {
    capturing: AtomicBool,
    tail: Mutex<Vec<u8>>,
}

impl StderrStartupCapture {
    fn spawn(mut stderr: ChildStderr) -> Arc<Self> {
        let state = Arc::new(Self {
            capturing: AtomicBool::new(true),
            tail: Mutex::new(Vec::new()),
        });
        let state_bg = state.clone();
        // Continuously drain child stderr until EOF so chatty MCP servers never block
        // on a full OS pipe buffer and never dump raw output onto the daemon's stderr.
        tokio::spawn(async move {
            let mut buf = [0_u8; 1_024];
            while let Ok(n) = stderr.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                if state_bg.capturing.load(Ordering::Relaxed) {
                    let mut tail = state_bg.tail.lock();
                    tail.extend_from_slice(&buf[..n]);
                    if tail.len() > MAX_STDERR_DIAGNOSTIC_BYTES {
                        let overflow = tail.len() - MAX_STDERR_DIAGNOSTIC_BYTES;
                        tail.drain(..overflow);
                    }
                }
            }
        });
        state
    }

    fn finish_startup(&self) {
        self.capturing.store(false, Ordering::Relaxed);
        *self.tail.lock() = Vec::new();
    }

    fn take_summary(&self) -> Option<String> {
        self.capturing.store(false, Ordering::Relaxed);
        let raw = std::mem::take(&mut *self.tail.lock());
        if raw.is_empty() {
            return None;
        }
        let lossy = String::from_utf8_lossy(&raw);
        let trimmed = lossy.trim_start_matches('\u{FFFD}').trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }
}

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

    let (transport, stderr_opt) = TokioChildProcess::builder(cmd)
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to initialize Stdio MCP transport")?;

    let stderr_capture = stderr_opt.map(StderrStartupCapture::spawn);

    let init_timeout = Duration::from_millis(timeout_ms.max(MIN_TIMEOUT_MS));
    let init_res = tokio::time::timeout(init_timeout, serve_client(handler, transport)).await;

    let client = match init_res {
        Ok(Ok(client)) => {
            if let Some(capture) = &stderr_capture {
                capture.finish_startup();
            }
            client
        }
        Ok(Err(e)) => {
            tokio::task::yield_now().await;
            let detail = format_client_init_error(&e);
            let ctx_msg = stderr_capture
                .as_deref()
                .and_then(StderrStartupCapture::take_summary)
                .map_or_else(
                    || format!("Failed to initialize Stdio MCP client: {detail}"),
                    |stderr_tail| {
                        format!(
                            "Failed to initialize Stdio MCP client ({detail}; stderr: {stderr_tail})"
                        )
                    },
                );
            return Err(anyhow::Error::new(e).context(ctx_msg));
        }
        Err(elapsed) => {
            let ctx_msg = stderr_capture
                .as_deref()
                .and_then(StderrStartupCapture::take_summary)
                .map_or_else(
                    || {
                        format!(
                            "Timed out after {}ms waiting for Stdio MCP server '{trimmed_cmd}' to initialize",
                            init_timeout.as_millis()
                        )
                    },
                    |stderr_tail| {
                        format!(
                            "Timed out after {}ms waiting for Stdio MCP server '{trimmed_cmd}' to initialize (stderr: {stderr_tail})",
                            init_timeout.as_millis()
                        )
                    },
                );
            return Err(anyhow::Error::new(elapsed).context(ctx_msg));
        }
    };

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
    use rmcp::service::ClientInitializeError;

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

    #[cfg(unix)]
    #[tokio::test]
    async fn crashing_stdio_server_surfaces_stderr_diagnostic_and_preserves_typed_error() {
        let manager = McpClientManager::new();
        let handler = BoxxyClientHandler::new("crash-srv", Arc::downgrade(&manager));
        let args = vec![
            "-c".to_string(),
            "echo 'fatal: missing MCP_API_KEY env var' >&2; exit 1".to_string(),
        ];
        let err = build_stdio_client(handler, "sh", &args, &HashMap::new(), 5_000)
            .await
            .err()
            .expect("crashing stdio server must fail");

        assert!(
            err.downcast_ref::<ClientInitializeError>().is_some(),
            "Typed ClientInitializeError must remain downcastable in the anyhow chain"
        );
        let formatted = format!("{err:#}");
        assert!(
            formatted.contains("fatal: missing MCP_API_KEY env var"),
            "Expected captured stderr diagnostic in error, got: {formatted}"
        );
    }
}
