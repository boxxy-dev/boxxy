use zbus::proxy;

#[proxy(
    interface = "dev.boxxy.BoxxyTerminal.Agent.Claw",
    default_path = "/dev/boxxy/BoxxyTerminal/Agent/Claw",
    gen_blocking = false
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
    async fn list_processes(&self) -> zbus::Result<Vec<(u32, String, f64, u64, u64, u64)>>;
    async fn kill_process(&self, pid: u32, signal: i32) -> zbus::Result<()>;
    async fn get_clipboard(&self) -> zbus::Result<String>;
    async fn set_clipboard(&self, text: String) -> zbus::Result<()>;
}
