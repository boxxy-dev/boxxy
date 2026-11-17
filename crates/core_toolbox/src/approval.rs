//! User approval protocol and shared tool lifecycle helpers.

use boxxy_claw_protocol::FileDiff;
use serde::Serialize;
use std::future::Future;

/// Result of a user approval prompt for a sensitive tool action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalOutcome {
    /// User clicked Approve.
    Approved,
    /// User clicked Reject (or dismissed the proposal).
    Rejected,
    /// User typed a new message/instruction instead of approving or rejecting.
    Interrupted(String),
}

impl ApprovalOutcome {
    #[inline]
    #[must_use]
    pub const fn is_approved(&self) -> bool {
        matches!(self, Self::Approved)
    }

    /// Formats the error string returned to the LLM when the action was not approved.
    #[must_use]
    pub fn rejection_message(&self) -> String {
        match self {
            Self::Approved => String::new(),
            Self::Rejected => "[USER_EXPLICIT_REJECT]".to_string(),
            Self::Interrupted(msg) => format!(
                "[USER_INTERRUPTION]: The user did not approve this proposal and instead sent a new instruction: \"{msg}\". Do NOT retry the rejected proposal; address the user's new instruction immediately."
            ),
        }
    }
}

/// Trait for handling user approvals for sensitive tools.
/// Implemented by `boxxy-claw` to bridge with the UI.
#[async_trait::async_trait]
pub trait ApprovalHandler: Send + Sync {
    /// Propose writing or editing a file on the host system, supplying a pre-computed `FileDiff`.
    async fn propose_file_write(
        &self,
        path: String,
        content: String,
        diff: FileDiff,
    ) -> ApprovalOutcome;
    /// Propose deleting a file from the host system.
    async fn propose_file_delete(&self, path: String) -> ApprovalOutcome;
    /// Propose running a command in the background.
    async fn propose_background_command(
        &self,
        command: String,
        explanation: String,
    ) -> ApprovalOutcome;
    /// Propose killing a process on the host system.
    async fn propose_kill_process(&self, pid: u32, process_name: String) -> ApprovalOutcome;
    /// Propose reading the system clipboard.
    async fn propose_get_clipboard(&self) -> ApprovalOutcome;
    /// Propose setting the system clipboard.
    async fn propose_set_clipboard(&self, text: String) -> ApprovalOutcome;
    /// Report a structured tool result (e.g. process list).
    async fn report_tool_result(&self, tool_name: String, result: String);
    /// Report that a tool call has started.
    async fn report_tool_started(&self, tool_name: String);
    /// Update the agent's thinking status in the UI.
    async fn set_thinking(&self, thinking: bool);
    /// Return the live working directory for the current session/pane, if available.
    async fn current_dir(&self) -> Option<String> {
        None
    }
    /// Record the content hash of a file read by `file_read` so subsequent writes can
    /// detect external manual modifications on disk.
    async fn record_file_read(&self, _path: String, _content_hash: u64) {}
    /// Check if `path` has been modified externally on disk since the session last
    /// read or wrote it. Returns `Some(error_message)` if the file is stale.
    async fn check_stale_file_before_write(
        &self,
        _path: &str,
        _disk_content: Option<&str>,
    ) -> Option<String> {
        None
    }
    /// Record the pre-write content of `path` (or `None` if newly created) for `/undo`
    /// rollback and update the session's known content hash to `new_content_hash`.
    async fn record_file_snapshot_before_write(
        &self,
        _path: String,
        _previous_content: Option<String>,
        _new_content_hash: u64,
    ) {
    }
}

/// Announces tool start to the UI indicator and records anonymous tool telemetry.
pub(crate) async fn begin_tool(approval: &dyn ApprovalHandler, tool_name: &'static str) {
    approval.report_tool_started(tool_name.to_string()).await;
    boxxy_telemetry::track_tool_use(tool_name).await;
}

/// Serializes `output` once and reports the structured result to the UI and trace log.
pub(crate) async fn finish_tool<T: Serialize + Sync>(
    approval: &dyn ApprovalHandler,
    tool_name: &'static str,
    output: &T,
) {
    let payload = serde_json::to_string(output).unwrap_or_default();
    approval
        .report_tool_result(tool_name.to_string(), payload)
        .await;
}

/// Reports a failed tool call when returning `Err(...)` directly so `report_tool_started`
/// is always paired with a matching `report_tool_result` event.
pub(crate) async fn report_error(
    approval: &dyn ApprovalHandler,
    tool_name: &'static str,
    error_message: &str,
) {
    let payload = serde_json::json!({ "error": error_message }).to_string();
    approval
        .report_tool_result(tool_name.to_string(), payload)
        .await;
}

/// Pauses the thinking indicator while awaiting interactive user approval, then resumes it.
pub(crate) async fn await_proposal<F>(approval: &dyn ApprovalHandler, proposal: F) -> ApprovalOutcome
where
    F: Future<Output = ApprovalOutcome>,
{
    approval.set_thinking(false).await;
    let outcome = proposal.await;
    approval.set_thinking(true).await;
    outcome
}

/// Resolves the active pane's live working directory, falling back to `fallback` if unset.
pub(crate) async fn resolve_live_cwd(approval: &dyn ApprovalHandler, fallback: &str) -> String {
    approval
        .current_dir()
        .await
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}
