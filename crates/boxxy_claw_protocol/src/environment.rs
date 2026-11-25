//! Privileged host environment interface (`ClawEnvironment`) and process telemetry DTOs.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessSample {
    pub pid: u32,
    pub name: String,
    pub cpu_usage: f64,
    pub memory_bytes: u64,
    pub read_bytes: u64,
    pub written_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_usage: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_memory_bytes: Option<u64>,
}

#[async_trait::async_trait]
pub trait ClawEnvironment: Send + Sync + 'static {
    async fn exec_shell(&self, command: String) -> anyhow::Result<(i32, String, String)>;
    async fn spawn_detached(&self, command: String, cwd: String) -> anyhow::Result<u32>;
    async fn read_file(
        &self,
        path: String,
        start_line: u32,
        end_line: u32,
    ) -> anyhow::Result<String>;
    async fn write_file(&self, path: String, content: String) -> anyhow::Result<()>;
    async fn list_directory(&self, path: String) -> anyhow::Result<Vec<(String, bool, u64)>>;
    async fn delete_file(&self, path: String) -> anyhow::Result<()>;
    async fn get_system_info(&self) -> anyhow::Result<String> {
        Ok("{}".to_string())
    }
    async fn list_processes(&self) -> anyhow::Result<Vec<ProcessSample>> {
        Ok(Vec::new())
    }
    async fn kill_process(&self, pid: u32, signal: i32) -> anyhow::Result<()>;
    async fn get_clipboard(&self) -> anyhow::Result<String>;
    async fn set_clipboard(&self, text: String) -> anyhow::Result<()>;

    /// Returns `(pid, pane_id, idle_secs, cwd, foreground_process, waiting_for_input)`
    /// for all detached background PTY shells kept alive in the daemon.
    async fn list_detached_shells(
        &self,
    ) -> anyhow::Result<Vec<(u32, String, u64, String, String, bool)>> {
        Ok(Vec::new())
    }
}

/// Extracts the most recently executed shell command line from a terminal snapshot.
///
/// Supports both structured `boxxy-vte` `OSC 133` semantic snapshots (`[COMMAND]\n```bash\n...``` `)
/// — where up to 100 lines of scrollback precede the latest command — and plain terminal buffers.
#[must_use]
pub fn extract_last_command_from_snapshot(snapshot: &str) -> &str {
    let mut had_semantic_blocks = false;
    for (cmd_idx, _) in snapshot.rmatch_indices("[COMMAND]") {
        had_semantic_blocks = true;
        let after = &snapshot[cmd_idx + "[COMMAND]".len()..];
        let mut saw_fence = false;
        for raw_line in after.lines() {
            let trimmed = raw_line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.starts_with("```") {
                if saw_fence {
                    break;
                }
                saw_fence = true;
                continue;
            }
            return trimmed.strip_prefix("$ ").map_or(trimmed, str::trim);
        }
    }
    if had_semantic_blocks || snapshot.contains("[PROMPT]") {
        return "";
    }

    for raw_line in snapshot.lines().rev() {
        if let Some(after_dollar) = raw_line.trim_start().strip_prefix("$ ") {
            let cmd = after_dollar.trim();
            if !cmd.is_empty() {
                return cmd;
            }
        }
    }

    snapshot
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map_or("", |l| l.strip_prefix("$ ").map_or(l, str::trim))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_sample_omits_absent_gpu_fields_and_roundtrips() {
        let sample = ProcessSample {
            pid: 1001,
            name: "cargo".to_string(),
            cpu_usage: 12.5,
            memory_bytes: 64 * 1024 * 1024,
            read_bytes: 4096,
            written_bytes: 8192,
            gpu_usage: None,
            gpu_memory_bytes: None,
        };
        let json = serde_json::to_string(&sample).unwrap();
        assert!(!json.contains("gpu_usage"));
        let decoded: ProcessSample = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, sample);
    }

    #[test]
    fn extracts_last_command_across_osc133_scrollback_and_plain_snapshots() {
        // Real Bash integration emits `OSC 133;A` + `OSC 133;B` right after `OSC 133;D`, leaving
        // a trailing empty `[COMMAND]` block at the new cursor prompt.
        let osc133_snap = "\n[PROMPT]\n```text\nbox$ \n```\n\n[COMMAND]\n```bash\ncargo build\n```\n\n[OUTPUT]\n```text\nFinished dev\n```\n\n[PROMPT]\n```text\nbox$ \n```\n\n[COMMAND]\n```bash\ngrep needle src/parser.rs\n```\n\n[OUTPUT]\n```text\n```\n\n[PROMPT]\n```text\nbox$ \n```\n\n[COMMAND]\n```bash\n```";
        assert_eq!(
            extract_last_command_from_snapshot(osc133_snap),
            "grep needle src/parser.rs"
        );

        // Fresh prompt with zero executed commands returns `""` (never `"[PROMPT]"`).
        let fresh_prompt_only = "\n[PROMPT]\n```text\nbox$ \n```\n\n[COMMAND]\n```bash\n```";
        assert_eq!(extract_last_command_from_snapshot(fresh_prompt_only), "");

        let multi_prompt_plain = "$ echo first\nfirst\n$ diff -u a.txt b.txt\n--- a.txt\n";
        assert_eq!(
            extract_last_command_from_snapshot(multi_prompt_plain),
            "diff -u a.txt b.txt"
        );

        let bare_two_line = "unknown_cmd --check\nbash: unknown_cmd: not found\n";
        assert_eq!(
            extract_last_command_from_snapshot(bare_two_line),
            "unknown_cmd --check"
        );

        assert_eq!(extract_last_command_from_snapshot("   \n\n  "), "");
    }
}
