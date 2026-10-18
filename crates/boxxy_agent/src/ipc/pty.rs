use serde::{Deserialize, Serialize};
use zbus::{OwnedFd, Type, proxy};

#[derive(Debug, Serialize, Deserialize, Type)]
pub struct SpawnOptions {
    pub cwd: String,
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
    /// UUID of the pane that owns this shell. Surfaces in
    /// `list_detached_sessions()` so a reattach can target the right pane.
    pub pane_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct DetachedSessionInfo {
    pub pid: u32,
    pub pane_id: String,
    pub idle_secs: u64,
    pub cwd: String,
    pub foreground_process: String,
    pub waiting_for_input: bool,
}

#[proxy(
    interface = "dev.boxxy.BoxxyTerminal.Agent.Pty",
    default_service = "dev.boxxy.BoxxyAgent",
    default_path = "/dev/boxxy/BoxxyTerminal/Agent/Pty"
)]
pub trait AgentPty {
    async fn get_preferred_shell(&self) -> zbus::Result<String>;
    async fn create_pty(&self) -> zbus::Result<OwnedFd>;
    async fn spawn(&self, pty_master: OwnedFd, options: SpawnOptions) -> zbus::Result<u32>;
    async fn get_cwd(&self, pid: u32) -> zbus::Result<String>;
    async fn get_foreground_process(&self, pid: u32) -> zbus::Result<String>;
    async fn get_running_processes(&self, pid: u32) -> zbus::Result<Vec<(u32, String)>>;
    async fn signal_process_group(&self, pid: u32, signal: i32) -> zbus::Result<()>;
    async fn set_foreground_tracking(&self, pid: u32, enabled: bool) -> zbus::Result<()>;
    async fn get_environment_variable(&self, name: String) -> zbus::Result<String>;

    /// Toggles "keep the shell alive when the last viewer detaches".
    /// When `false`, `detach()` SIGTERMs the process group; when `true`,
    /// the daemon takes over the master FD and buffers output into a
    /// 4 MB ring buffer. Defaults to `false` when a session is spawned.
    async fn set_persistence(&self, pid: u32, enabled: bool) -> zbus::Result<()>;

    /// Decrements the session's viewer count. On the last viewer:
    ///   * persistence off → SIGTERM the process group (returns 0).
    ///   * persistence on  → daemon activates its stored FD dup and
    ///     starts buffering output into a 4 MB ring buffer (returns 1).
    /// No FD parameter: the daemon has held its own dup of the master
    /// since `spawn()`; it was idle until now.
    async fn detach(&self, pid: u32) -> zbus::Result<u32>;

    /// Detaches `pid` while seeding the daemon's 4 MB ring buffer with `initial_snapshot`
    /// (the pre-detach ANSI viewport snapshot from `boxxy-vte`) so line-oriented CLI
    /// tools paused on a prompt restore their full visible screen on reattach.
    async fn detach_with_snapshot(&self, pid: u32, initial_snapshot: Vec<u8>) -> zbus::Result<u32>;

    /// Returns `(pid, pane_id, idle_secs)` for every detached session
    /// the daemon is currently hosting — powers a future "Detached
    /// Sessions" UI view and the `boxxy-agent list-sessions` CLI.
    async fn list_detached_sessions(&self) -> zbus::Result<Vec<(u32, String, u64)>>;

    /// Returns rich metadata `(pid, pane_id, idle_secs, cwd, foreground_process, waiting_for_input)`
    /// for every detached session the daemon is currently hosting.
    async fn list_detached_sessions_detailed(&self) -> zbus::Result<Vec<DetachedSessionInfo>>;

    /// Reclaims a detached session. Returns `(replay_bytes, master_fd)`:
    /// the UI feeds `replay_bytes` into its terminal to restore scrollback,
    /// then resumes live reads on `master_fd`.
    async fn reattach(&self, pid: u32) -> zbus::Result<(Vec<u8>, OwnedFd)>;

    /// Returns `(has_exited, exit_code)` for `pid`. Allows late-subscribing or
    /// reconnecting UI panes to observe exit codes of short-lived processes.
    async fn get_exit_status(&self, pid: u32) -> zbus::Result<(bool, i32)>;

    #[zbus(signal)]
    async fn exited(&self, pid: u32, exit_code: i32) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn foreground_process_changed(&self, pid: u32, process_name: String) -> zbus::Result<()>;
}
