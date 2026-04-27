use std::env;
use std::sync::RwLock;

pub mod agent_manager;
pub mod claw_proposal;
pub mod component;
pub mod events;
pub mod pane;
pub mod preview;
pub mod search_bar;

// The widget crate now owns `ClawHost` + `Proposal`; we re-export them
// at the terminal crate root so existing `crate::ClawHost` /
// `crate::Proposal` references stay valid.
pub use boxxy_claw_widget::{ClawHost, Proposal};
pub use component::{Direction, TerminalComponent};
pub use events::*;

/// Global broadcast channel — every TerminalComponent writes here;
/// every AppWindow subscribes and filters by the IDs it owns.
pub static TERMINAL_EVENTS: RwLock<Option<TerminalEvent>> = RwLock::new(None);
// We need a way to subscribe to events in raw gtk4-rs across the application.
// We'll use a tokio broadcast channel instead.
use crate::agent_manager::AgentManager;
use tokio::sync::broadcast;

lazy_static::lazy_static! {
    pub static ref TERMINAL_EVENT_BUS: broadcast::Sender<TerminalEvent> = {
        let (tx, _) = broadcast::channel(100);
        tx
    };

    pub static ref AGENT_MANAGER: tokio::sync::OnceCell<AgentManager> = tokio::sync::OnceCell::new();
}

pub async fn get_agent() -> &'static AgentManager {
    AGENT_MANAGER
        .get_or_init(|| async {
            AgentManager::new()
                .await
                .expect("Failed to initialize AgentManager")
        })
        .await
}

pub(crate) use boxxy_sys_utils::is_flatpak;

pub(crate) fn get_host_shell() -> String {
    let username = env::var("USER")
        .or_else(|_| env::var("LOGNAME"))
        .unwrap_or_default();

    if !username.is_empty()
        && let Ok(out) = std::process::Command::new("flatpak-spawn")
            .args(["--host", "getent", "passwd", &username])
            .output()
        && let Ok(line) = String::from_utf8(out.stdout)
    {
        let fields: Vec<&str> = line.trim().split(':').collect();
        if fields.len() >= 7 {
            return fields[6].to_string();
        }
    }

    env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into())
}
