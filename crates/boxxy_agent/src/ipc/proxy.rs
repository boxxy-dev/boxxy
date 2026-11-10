//! Top-level D-Bus client proxy for `dev.boxxy.BoxxyTerminal.Agent`.

#[zbus::proxy(
    interface = "dev.boxxy.BoxxyTerminal.Agent",
    default_service = "dev.boxxy.BoxxyAgent",
    default_path = "/dev/boxxy/Agent"
)]
pub trait Agent {
    async fn get_version(&self) -> zbus::Result<String>;
    async fn get_config_dir(&self) -> zbus::Result<String>;
    async fn update_credentials(
        &self,
        api_keys: std::collections::HashMap<String, String>,
        ollama_url: String,
    ) -> zbus::Result<()>;
    async fn notify_settings_invalidated(&self) -> zbus::Result<()>;
    async fn request_reload(&self) -> zbus::Result<()>;
    async fn request_stop(&self) -> zbus::Result<()>;

    // Claw Session Management (Using JSON for complex types)
    async fn claim_startup_token(&self) -> zbus::Result<String>;
    async fn get_registry_snapshot(&self) -> zbus::Result<String>;
    async fn try_claim_character(
        &self,
        holder_id: String,
        holder_kind: u8,
        character_id: String,
    ) -> zbus::Result<String>;
    async fn release_holder(&self, holder_id: String) -> zbus::Result<()>;
    async fn resolve_peer(&self, query_json: String) -> zbus::Result<String>;

    async fn post_claw_message(&self, session_id: String, message_json: String)
    -> zbus::Result<()>;
    /// Returns `[(session_id, pane_id, agent_name), ...]` for every session
    /// the daemon is currently hosting. Used by a freshly launched UI to
    /// discover sessions it can reattach to.
    async fn list_claw_sessions(&self) -> zbus::Result<Vec<(String, String, String)>>;

    /// Requests that the UI raises the window and focuses the specified pane.
    async fn request_focus_pane(&self, pane_id: String) -> zbus::Result<()>;

    /// Returns a JSON array of `DetachedSessionInfo` objects for all detached
    /// background shells. Used by desktop companion extensions (GNOME Shell, KDE, Waybar).
    async fn get_detached_sessions(&self) -> zbus::Result<String>;

    /// Requests that a detached background shell (`pid`, or `0` for all) be restored
    /// into a live Boxxy Terminal tab. If no Boxxy window is open, queues the PID so
    /// the next window launched automatically restores it.
    async fn request_restore_detached(&self, pid: u32) -> zbus::Result<()>;

    /// Drains any PIDs queued via `request_restore_detached` while no UI window was open.
    async fn take_pending_restores(&self) -> zbus::Result<Vec<(u32, String)>>;

    /// Terminates a detached background shell (`pid`, or `0` for all).
    async fn terminate_detached_session(&self, pid: u32) -> zbus::Result<()>;

    /// Marks `holder_id` (`pane_id`) as detached in the background so its character claim
    /// and `ClawSession` actor survive UI tab/window close.
    async fn detach_holder(&self, holder_id: String) -> zbus::Result<()>;

    /// Rebinds a detached `ClawSession` and `CharacterClaim` from `old_holder_id` to
    /// `new_holder_id`, triggers UI state hydration, and returns `Option<ClaimedSession>` JSON.
    async fn reattach_holder(
        &self,
        old_holder_id: String,
        new_holder_id: String,
    ) -> zbus::Result<String>;

    /// Rebinds a detached `ClawSession` and `CharacterClaim` associated with PTY `pid`
    /// to `new_holder_id`, triggers UI state hydration, and returns `Option<ClaimedSession>` JSON.
    async fn reattach_holder_by_pid(
        &self,
        pid: u32,
        new_holder_id: String,
    ) -> zbus::Result<String>;

    /// Returns a JSON array of `PendingTaskInfo` across all open and detached agent sessions.
    async fn get_pending_tasks(&self) -> zbus::Result<String>;

    /// Cancels a pending scheduled task (`task_id`) in `pane_id`.
    async fn cancel_pending_task(&self, pane_id: String, task_id: String) -> zbus::Result<()>;

    /// Updates a live terminal pane's state in the daemon's `WorkspaceRegistry`
    /// (used for both plain shells and agent-attached panes) and returns its short alias (e.g. `pane-2`).
    async fn update_workspace_pane(
        &self,
        pane_id: String,
        cwd: String,
        last_command: String,
        snapshot: String,
        status: String,
    ) -> zbus::Result<String>;

    /// Unregisters a closed terminal pane from the daemon's `WorkspaceRegistry`.
    async fn unregister_workspace_pane(&self, pane_id: String) -> zbus::Result<()>;

    /// Notifies the daemon's `WorkspaceRegistry` that a shell command finished (`OSC 133;D`)
    /// in `pane_id`, resolving any cross-pane `terminal_exec` waiter and publishing `ProcessExited`
    /// to `subscribe_to_pane` listeners.
    async fn notify_pane_command_finished(
        &self,
        pane_id: String,
        exit_code: i32,
        snapshot: String,
        cwd: String,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn claw_event(&self, session_id: String, event_json: String) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn claims_changed(&self, snapshot_json: String) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn detached_sessions_changed(&self, sessions_json: String) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn pending_tasks_changed(&self, tasks_json: String) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn restore_detached_requested(&self, pid: u32, cwd: String) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn desktop_notification(&self, title: String, message: String) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn mcp_status_changed(&self, statuses_json: String) -> zbus::Result<()>;
}
