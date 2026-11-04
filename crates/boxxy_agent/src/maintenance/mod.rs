use crate::core::state::AgentState;
use crate::daemon::dreaming::DreamStatusCell;
use crate::daemon::power::PowerMonitor;
use zbus::fdo;
use zbus::interface;

pub struct MaintenanceSubsystem {
    #[allow(dead_code)] // may be used by future maintenance tasks
    state: AgentState,
    dream_status: DreamStatusCell,
    power: PowerMonitor,
}

impl MaintenanceSubsystem {
    pub fn new(state: AgentState, dream_status: DreamStatusCell, power: PowerMonitor) -> Self {
        Self {
            state,
            dream_status,
            power,
        }
    }
}

#[interface(name = "dev.boxxy.BoxxyTerminal.Agent.Maintenance")]
impl MaintenanceSubsystem {
    /// Returns the current Dream Cycle state as a lowercase label.
    /// One of: `idle`, `disabled`, `paused_on_battery`, `paused_ui_attached`,
    /// `running`. Useful for telemetry and ad-hoc `busctl` checks while
    /// there's no dedicated UI surface yet.
    async fn get_maintenance_status(&self) -> fdo::Result<String> {
        Ok(self.dream_status.get().await.label().to_string())
    }

    /// `true` if UPower reports the laptop is on battery. Exposed so
    /// the UI / future Dreaming configurators can see what's gating
    /// background work without having to talk to UPower themselves.
    async fn is_on_battery(&self) -> fdo::Result<bool> {
        Ok(self.power.is_on_battery())
    }

    /// Returns `[(server_name, is_connected)]` for all enabled MCP servers.
    /// Synchronizes the global `McpClientManager` with `Settings` and probes
    /// any server not currently in circuit-breaker cooldown so both the Claw
    /// sidebar status list and Turn 1 tool discovery stay warm and accurate.
    async fn get_mcp_statuses(&self) -> fdo::Result<Vec<(String, bool)>> {
        let settings = boxxy_preferences::Settings::load();
        Ok(boxxy_claw::sync_and_probe_mcp_servers(settings.mcp_servers.clone()).await)
    }

    async fn trigger_maintenance_now(&self) -> fdo::Result<()> {
        log::info!("Manual maintenance triggered via D-Bus");
        boxxy_telemetry::flush_journal().await;
        Ok(())
    }
}
