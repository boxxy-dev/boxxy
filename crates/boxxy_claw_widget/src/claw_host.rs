//! Abstract interface the claw drawer uses to talk to its host (the
//! thing that owns the shell / scrollback / focus target / agent channel).
//!
//! Today the only implementor is `pane::claw_host::PaneClawHost` over a
//! terminal pane. The trait exists to decouple the drawer's UI code from
//! terminal-specific types (`TerminalWidget`, `PaneInner`, `PaneOutput`,
//! `claw_sender`) so the drawer can move into `boxxy_claw_widget` later
//! without dragging those along.
//!
//! When this file moves into `crates/boxxy_claw_widget/`, only the module
//! path changes — the trait shape is stable.
//!
//! Design notes:
//! - `!Send`, `!Sync`. GTK widgets are single-threaded; a `Send` bound
//!   would be a lie and would force `Arc<Mutex<...>>` ceremony in
//!   impls. If a cross-thread variant is ever needed, add it as a
//!   superset trait (`ClawHostThreadSafe: ClawHost + Send + Sync`).
//! - Async methods return `Pin<Box<dyn Future + 'static>>` (not `+
//!   Send`). The event loop uses `glib::spawn_future_local` which runs
//!   local-thread futures.

use boxxy_claw_protocol::{ClawEngineEvent, ClawMessage};
use std::future::Future;
use std::pin::Pin;

pub trait ClawHost: 'static {
    /// Stable id for the host (pane / tab uuid). Used for routing events
    /// back to the window orchestrator.
    fn host_id(&self) -> String;

    /// Inject a line of text into the host (appends `\r` before sending).
    /// The typical use is a user-accepted command going to the shell.
    fn inject_line(&self, text: String);

    /// Execute a (possibly multi-line) named script. The host decides
    /// how — the terminal impl writes to an ephemeral file under
    /// `$XDG_CONFIG_HOME/boxxy-terminal/cache/bookmarks/runs/`, chmods
    /// `+x`, and injects the path. Other hosts could pipe the script
    /// to stdin or invoke a different runner.
    fn execute_script(&self, filename: &str, script: String);

    /// Capture the last `max_lines` lines of host output, starting
    /// `offset_lines` lines above the bottom. Returns `None` if the
    /// host can't currently provide a snapshot.
    fn snapshot(
        &self,
        max_lines: usize,
        offset_lines: usize,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + 'static>>;

    /// Return keyboard focus to the host (e.g. the terminal). Called
    /// after the drawer hides or when the user dismisses a proposal.
    fn grab_focus(&self);

    /// Gate whether the host accepts keyboard focus. Used so that
    /// Escape inside the drawer is consumed by the drawer, not the
    /// underlying terminal.
    fn set_focusable(&self, focusable: bool);

    /// True while a full-screen TUI owns the host (vim, less, etc.).
    /// The drawer uses this to skip side effects that would fight the
    /// TUI — e.g. not injecting `cd` when vim is up.
    fn is_busy(&self) -> bool;

    /// The host's current working directory, if known. Seeds the
    /// agent's CWD context on each user message.
    fn working_dir(&self) -> Option<String>;

    /// Send a `ClawMessage` to the agent session that backs this host.
    fn send_claw(&self, msg: ClawMessage);

    /// Ask the surrounding UI to focus the sidebar-side claw log for
    /// this host (e.g. when the user clicks the bug/inspect icon).
    fn focus_sidebar(&self);

    /// Raise the window and select the tab / pane that contains this host.
    fn focus(&self);

    /// Change the host's working directory to `path`. Host decides the
    /// mechanism — the terminal impl checks `is_busy()`, validates the
    /// path exists, and injects a `cd` command, falling back to a
    /// user-visible notification when either check fails.
    fn cd(&self, path: String);

    /// Surface a transient user-visible notification (typically routed
    /// to a toast or status pill by the surrounding UI).
    fn notify(&self, message: String);

    /// Forward a raw `ClawEngineEvent` upstream — to the window
    /// orchestrator, swarm router, etc. The widget's dispatch loop
    /// calls this after handling every event so non-UI consumers (tab
    /// badges, cross-pane swarms) see the full stream.
    fn forward_event(&self, event: ClawEngineEvent);

    /// Request to assign a specific character to this host.
    /// If no session exists, the host should create one with this character.
    /// If one exists, the host may handle a character swap.
    fn request_character(&self, character_id: String);
}
