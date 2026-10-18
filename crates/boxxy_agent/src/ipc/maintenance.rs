use zbus::proxy;

#[proxy(
    interface = "dev.boxxy.BoxxyTerminal.Agent.Maintenance",
    default_service = "dev.boxxy.BoxxyAgent",
    default_path = "/dev/boxxy/BoxxyTerminal/Agent/Maintenance"
)]
pub trait AgentMaintenance {
    async fn get_maintenance_status(&self) -> zbus::Result<String>;
    async fn is_on_battery(&self) -> zbus::Result<bool>;
    async fn trigger_maintenance_now(&self) -> zbus::Result<()>;
}
