//! Host-level desktop notifications.
//!
//! The daemon emits these when a background AI task completes while the UI is
//! detached (Ghost Mode), so the user still hears about it. We talk to
//! `org.freedesktop.Notifications` directly via zbus rather than pulling in
//! `notify-rust` — we already depend on zbus, and the surface we need is one
//! method call.

use std::collections::HashMap;
use zbus::{Connection, Variant};

const APP_NAME: &str = "Boxxy";
const APP_ICON: &str = "dev.boxxy.BoxxyTerminal";
/// Persistent (until the user dismisses). The spec says -1 means
/// "server default"; 0 means "never expire". We pick 0 so long-running
/// results aren't missed if the user steps away.
const EXPIRE_TIMEOUT_MS: i32 = 0;

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait FreedesktopNotifications {
    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: Vec<&str>,
        hints: HashMap<&str, Variant<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;
}

/// Fire-and-forget desktop notification. Errors are logged, never raised —
/// the notification daemon might simply not be running (e.g. headless CI).
pub async fn send(conn: &Connection, title: &str, message: &str) {
    let proxy = match FreedesktopNotificationsProxy::new(conn).await {
        Ok(p) => p,
        Err(e) => {
            log::debug!("notifier: no notification service available: {}", e);
            return;
        }
    };

    if let Err(e) = proxy
        .notify(
            APP_NAME,
            0,
            APP_ICON,
            title,
            message,
            Vec::new(),
            HashMap::new(),
            EXPIRE_TIMEOUT_MS,
        )
        .await
    {
        log::warn!("notifier: Notify() failed: {}", e);
    }
}
