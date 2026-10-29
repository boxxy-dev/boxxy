use std::env;

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
pub use boxxy_agent::ipc::pty::DetachedSessionInfo;
pub use boxxy_claw_widget::{ClawHost, Proposal};
pub use component::{Direction, TerminalComponent};
pub use events::*;

use crate::agent_manager::AgentManager;
use std::collections::HashMap;
use std::rc::Weak;
use std::sync::LazyLock;
use tokio::sync::broadcast;

thread_local! {
    static LIVE_PANES: std::cell::RefCell<HashMap<String, Weak<std::cell::RefCell<pane::PaneInner>>>> =
        std::cell::RefCell::new(HashMap::new());
}

pub(crate) fn register_live_pane(
    id: String,
    inner: Weak<std::cell::RefCell<pane::PaneInner>>,
) {
    LIVE_PANES.with(|map| {
        map.borrow_mut().insert(id, inner);
    });
}

pub(crate) fn unregister_live_pane(id: &str) {
    LIVE_PANES.with(|map| {
        map.borrow_mut().remove(id);
    });
}

pub(crate) fn find_live_pane(id: &str) -> Option<std::rc::Rc<std::cell::RefCell<pane::PaneInner>>> {
    LIVE_PANES.with(|map| map.borrow().get(id).and_then(|w| w.upgrade()))
}

/// Global broadcast channel — every TerminalComponent writes here;
/// every AppWindow subscribes and filters by the IDs it owns.
pub static TERMINAL_EVENT_BUS: LazyLock<broadcast::Sender<TerminalEvent>> = LazyLock::new(|| {
    let (tx, _) = broadcast::channel(100);
    tx
});

pub static AGENT_MANAGER: tokio::sync::OnceCell<AgentManager> = tokio::sync::OnceCell::const_new();

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
