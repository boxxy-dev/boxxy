use zbus::{DBusError, proxy};

/// Domain-specific D-Bus error replies for `dev.boxxy.BoxxyTerminal.Agent.Claw`.
///
/// Preserves structured error categories across the D-Bus boundary (`zbus` 6.5+)
/// instead of collapsing all host tool failures into generic `fdo::Error::Failed`.
#[derive(Debug, Clone, PartialEq, DBusError)]
#[zbus(prefix = "dev.boxxy.BoxxyTerminal.Agent.Error")]
pub enum AgentClawError {
    #[zbus(error)]
    ZBus(zbus::Error),
    BlacklistedPath(String),
    FileNotFound(String),
    InvalidArgs(String),
    ExecutionFailed(String),
}

impl AgentClawError {
    /// Classifies an internal host error into a typed D-Bus domain error.
    pub fn from_anyhow(err: anyhow::Error) -> Self {
        let is_io_not_found = err
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_err| io_err.kind() == std::io::ErrorKind::NotFound);
        let msg = err.to_string();

        if msg.contains("blacklisted for security reasons") {
            Self::BlacklistedPath(msg)
        } else if is_io_not_found
            || msg.starts_with("File not found:")
            || msg.starts_with("Working directory does not exist:")
        {
            Self::FileNotFound(msg)
        } else if msg.starts_with("Invalid PID")
            || msg.starts_with("Refusing to signal")
            || msg.starts_with("Invalid signal number")
            || msg.starts_with("Path is a directory")
        {
            Self::InvalidArgs(msg)
        } else {
            Self::ExecutionFailed(msg)
        }
    }

    /// Unwraps the domain payload message without D-Bus error-name prefixes when available.
    pub fn into_message(self) -> String {
        match self {
            Self::BlacklistedPath(msg)
            | Self::FileNotFound(msg)
            | Self::InvalidArgs(msg)
            | Self::ExecutionFailed(msg) => msg,
            Self::ZBus(err) => err.to_string(),
        }
    }
}

#[proxy(
    interface = "dev.boxxy.BoxxyTerminal.Agent.Claw",
    default_service = "dev.boxxy.BoxxyAgent",
    default_path = "/dev/boxxy/BoxxyTerminal/Agent/Claw"
)]
pub trait AgentClaw {
    async fn exec_shell(&self, command: String) -> zbus::Result<(i32, String, String)>;
    async fn spawn_detached(&self, command: String, cwd: String) -> zbus::Result<u32>;
    async fn read_file(&self, path: String, start_line: u32, end_line: u32)
    -> zbus::Result<String>;
    async fn write_file(&self, path: String, content: String) -> zbus::Result<()>;
    async fn list_directory(&self, path: String) -> zbus::Result<Vec<(String, bool, u64)>>;
    async fn delete_file(&self, path: String) -> zbus::Result<()>;
    async fn get_system_info(&self) -> zbus::Result<String>;
    async fn list_processes(&self) -> zbus::Result<String>;
    async fn kill_process(&self, pid: u32, signal: i32) -> zbus::Result<()>;
    async fn get_clipboard(&self) -> zbus::Result<String>;
    async fn set_clipboard(&self, text: String) -> zbus::Result<()>;
}
