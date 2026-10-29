# Boxxy Agent (`boxxy-agent`)

## Role

Host-level daemon that sits between the Boxxy UI and the operating system. It owns every privileged operation (PTY spawning, shell-side tools, clipboard, memory consolidation) and exposes them to the UI process over D-Bus on the user's session bus.

A single instance serves all open UI windows. It is launched by the UI on demand via `ensure_agent_running` and claims `dev.boxxy.BoxxyAgent` on the session bus; a second launch detects the existing instance and exits.

## Binary Entry Points

The `boxxy-agent` binary accepts subcommands in addition to starting the daemon:

- `boxxy-agent` / `boxxy-agent start`: claims the session-bus name and runs the daemon loop.
- `boxxy-agent stop`: talks to a running daemon over D-Bus, flushes telemetry, and requests a clean exit.
- `boxxy-agent restart`: tells a running daemon to `exec()` its own binary in-place (stripping `--background` and ` (deleted)`), picking up a newer version on disk without changing PID or sending `SIGHUP` to child shells.
- `boxxy-agent list-sessions`: prints every detached PTY session the daemon is hosting (`PID`, idle seconds, foreground process, input-wait state via `/proc/<fg_pid>/wchan`, CWD, and pane UUID).
- `boxxy-agent status`: prints daemon version, config directory, power source (AC/Battery), Dream Cycle state, active Claw pane count, and detached PTY count.
- `--background`: daemonizes (double-fork + setsid) before starting the runtime. Called by the UI-side `agent_deployer` when it needs a host-detached process.

The binary also extracts itself from the Flatpak GResource bundle on first launch and writes the host-side copy into `~/.local/boxxy-terminal/boxxy-agent` so it can run outside the sandbox.

## Subsystems

Each D-Bus interface lives in its own module under `src/`. All of them share an `AgentState` (cheap-clone via `Arc`) that carries the process-wide PTY registry, API keys, ollama URL, and power/ghost-mode watchers.

### `pty/` — `dev.boxxy.BoxxyTerminal.Agent.Pty`

Latency-critical terminal I/O. Exposes:

- `create_pty`, `spawn`: open a master PTY, set window size, inject zero-config shell integration (`pty/shell_integration.rs`) for interactive Bash (`--rcfile` with Bash 5.3 `${ ...; }` `PS0` and array `PROMPT_COMMAND` support), Zsh (`ZDOTDIR` shim), Fish (`XDG_DATA_DIRS` `vendor_conf.d` with self-cleaning `XDG_DATA_DIRS`), and Nushell (`XDG_DATA_DIRS` `nushell/vendor/autoload/boxxy.nu`) to emit `OSC 7` and `OSC 133;A/B/C/D` (`cl=line` / `click_events=1`) without modifying user dotfiles, then `fork/exec` the shell. The daemon `dup()`s the master FD at spawn and keeps it in the session record — idle while a UI is attached, activated when `detach()` is called.
- `signal_process_group`, `get_foreground_process`, `get_cwd`, `get_running_processes`, `get_environment_variable`, `set_foreground_tracking`: host-side process introspection used by the UI + Claw.
- `set_persistence(pid, bool)`, `detach(pid)`, `reattach(pid)`, `list_detached_sessions()`, `list_detached_sessions_detailed()`, `get_exit_status(pid)`: persistent-shells and process exit status surface.
- Emits `Exited`, `ForegroundProcessChanged` signals that the UI subscribes to.

**`pty::registry::PtyRegistry`** holds one `PtySession` per spawned shell, keyed by PID. Each session tracks `owner_bus_name` (the D-Bus client currently attached), `initial_cwd`, `state` (`ProcessState::Running` or `ProcessState::Exited { exit_code, exited_at }` retained for 60s so late-subscribing or reconnecting panes can query exit codes), `viewer_count`, `persistence_enabled`, the captured master FD, a reader task (only while detached), a 4 MB byte ring buffer with UTF-8 / line-aligned eviction and SGR reset on overflow (only while detached), and `last_activity`. Detach is **process-aware** (`shell_has_active_process`): idle interactive shells with no running foreground or child processes are terminated on detach rather than kept alive as empty background shells. When a UI client disconnects or crashes, `client_tracker` calls `pty_registry.release_owner(bus_name)` to automatically detach busy shells (or `SIGTERM` idle/non-persistent shells) owned by that client. A 60-second zombie-sweeper task SIGTERMs any detached session idle longer than 4 hours and prunes expired `Exited` records.

### `claw/` — `dev.boxxy.BoxxyTerminal.Agent.Claw`

Host-side implementation of `ClawEnvironment`: shell execution, file I/O with path blacklisting, list/kill processes, clipboard access, and lazy on-demand `sysinfo 0.40` sampling (`claw::SystemSampler` for CPU/RAM/GPU/disk/thermal system info and per-process CPU/GPU/IO metrics with zero startup overhead). Path blacklist seeds from `~/.config/boxxy-terminal/boxxyclaw/BLACKLIST.md` (falling back to a built-in list that blocks `/etc/shadow`, `.ssh/id_rsa`, etc.).

**`claw::CharacterRegistry`** is the volatile in-memory registry of all live character claims, keyed by `holder_id`. It is the daemon's single source of truth for who holds which character. Claims are never written to disk; they live only for the duration of the daemon process. Key methods: `try_claim(holder_id, holder_kind, character_id, owner_bus_name)` (atomic check-then-set under write lock), `release_holder(holder_id)`, `release_owner(owner_bus_name) -> Vec<CharacterClaim>` (bulk release on client disconnect), `snapshot()`, `subscribe()` (broadcast channel for push updates). The registry also handles orphan character migration at load time: sessions referencing a deleted character UUID are silently mapped to the first catalog entry.

**`claw::notifier`** wraps `org.freedesktop.Notifications` with a tiny zbus proxy so `ClawEngineEvent::PushGlobalNotification`, `TaskCompleted`, and `LongTaskCompleted` turn into real desktop toasts when in Ghost Mode (`ghost.is_ghost()`) — so background AI work that finishes while the UI is detached still reaches the user without duplicating `boxxy-window`'s `gio::Notification` when a window is open.

### `ipc/` — `dev.boxxy.BoxxyTerminal.Agent`

Top-level orchestration interface. Hosts `ClawSession` actors and owns the character claim lifecycle.

**Character claim RPC** (all results serialized as JSON strings over D-Bus):
- `claim_startup_token()` — increments the ghost-mode client counter and returns a `StartupToken` with daemon version, whether the DB was reset, and the current registry revision.
- `get_registry_snapshot()` — returns the full `RegistrySnapshot` (catalog + all claims + revision).
- `try_claim_character(holder_id, holder_kind: u8, character_id)` → `Result<ClaimedSession, ClaimError>`.
- `release_holder(holder_id)` — releases a single claim and cascades swarm lock cleanup for Pane holders.
- `resolve_peer(query_json: PeerQuery)` → `Option<PeerInfo>` — character-aware peer lookup for swarm tools.
- Signal `claims_changed(snapshot_json)` — broadcast after every registry mutation; UI mirrors update `CHARACTER_CACHE` and `CLAIMS_CACHE` from this.

**Session management**: `post_claw_message(session_id, message_json)` routes a `ClawMessage` to the named session; `list_claw_sessions()` returns `[(session_id, pane_id, agent_name)]` for active sessions. Session events flow back as `claw_event(session_id, event_json)` signals. Dead sessions are pruned on every map touch.

**Other**: `update_credentials` (UI → daemon API-key sync), `notify_settings_invalidated` (daemon reloads `settings.json`), `request_reload` / `request_stop`.

**`ipc/client_tracker.rs`** — `spawn_owner_tracker` subscribes to `org.freedesktop.DBus.NameOwnerChanged`, filters for unique bus names (`:…`) being released (`new_owner` is empty), auto-detaches (or terminates) any PTYs owned by that client via `pty_registry.release_owner`, and for character claims calls `release_owner`, shuts down the corresponding `ClawSession` actors via `ClawSessionManager::shutdown_pane` (flushing unflushed history to SQLite), and cascades `WorkspaceRegistry::release_all_locks` + `unregister_pane` for each released Pane claim.

### `maintenance/` — `dev.boxxy.BoxxyTerminal.Agent.Maintenance`

Background-work status surface. `get_maintenance_status()` returns one of `idle`, `disabled`, `paused_on_battery`, `paused_ui_attached`, `running`. `is_on_battery()` exposes the current UPower reading. `trigger_maintenance_now()` flushes the telemetry journal. The actual maintenance work is driven by modules under `daemon/` (below).

### `daemon/` — lifecycle + scheduling primitives

- **`daemon::power::PowerMonitor`**: subscribes to `org.freedesktop.UPower.OnBattery` and streams `PropertiesChanged` events into a `watch::Receiver<bool>`. Graceful fallback to permanent-AC when UPower isn't reachable (VMs, minimal desktops) so unrelated work isn't accidentally blocked.
- **`daemon::lifecycle::GhostMode`**: derives a `client_count == 0` bool from a `watch::Sender<usize>` incremented by `claim_startup_token()` (each UI process calls this once on connect) and decremented via the `NameOwnerChanged` owner tracker on disconnect. Subsystems clone the receiver cheaply to gate their own work.
- **`daemon::priority`**: `set_current_thread_nice(19)` wrapper around `setpriority(PRIO_PROCESS, 0, ...)`. Called at the entry of the dreaming task and the zombie sweeper.
- **`daemon::dreaming`**: daemon-owned driver for the three-phase Dream Cycle. After a 10 s warm-up it sets `niceness 19`, then polls every minute: runs a cycle only when `enable_auto_dreaming` AND `!on_battery` AND `ghost_mode`. 15-minute cooldown between cycles. Publishes its current state into a shared `DreamStatusCell` read by `MaintenanceSubsystem`.
- **`daemon::singleton`**: implements the name-claim handshake so two agent processes never coexist. The second process either hands off (same version) or upgrades (newer binary).

## Security Posture

- **Sandbox escape**: on Flatpak, the UI spawns `boxxy-agent` via `flatpak-spawn --host`. The binary lives at `~/.local/boxxy-terminal/boxxy-agent` and runs as the host user. The session bus is shared between sandbox and host, so D-Bus is the entire IPC surface.
- **Settings hydration**: the daemon calls `boxxy_preferences::Settings::init()` before starting the runtime so the in-memory cache reads `settings.json` from disk. A `SettingsInvalidated` ClawMessage from the UI triggers `Settings::reload()` so the daemon picks up changes the UI just saved.
- **Credential sync**: `update_credentials(api_keys, ollama_url)` lets the UI push effective credentials to the daemon, and the claw engine also falls back to `Settings::load().get_effective_api_keys()` when the IPC-pushed keys are empty.
- **File I/O blacklist**: every `ClawSubsystem::read_file` / `write_file` / `delete_file` / `list_directory` call checks the path against `load_blacklist()` and rejects matches.

## Public Surface (UI-facing)

- `AgentProxy`: top-level interface — `Agent` (`dev.boxxy.BoxxyTerminal.Agent`).
- `AgentPtyProxy` + `SpawnOptions { cwd, argv, env, cols, rows, pane_id }`: PTY interface.
- `AgentClawProxy`: ClawEnvironment over D-Bus, used by `DbusClawEnvironment` in `boxxy-terminal`.
- `AgentMaintenanceProxy`: maintenance status queries.

## Testing

The crate has unit tests for the pieces that can be exercised without the full daemon loop: the PTY registry's ring buffer + detach/reattach semantics (including a real-PTY end-to-end test), the priority wrapper, power-monitor fallback, and ghost-mode transitions driven by the client-count watch channel. Run with `cargo test -p boxxy-agent --lib`.
