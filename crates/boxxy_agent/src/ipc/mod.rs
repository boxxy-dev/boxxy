pub mod agent_service;
pub mod broadcasters;
pub mod claw;
pub mod client_tracker;
pub mod maintenance;
pub mod proxy;
pub mod pty;
pub mod session_manager;

pub use agent_service::AgentInterface;
pub use proxy::*;
pub use session_manager::ClawSessionManager;

pub(crate) use broadcasters::{emit_mcp_status_changed, emit_pending_tasks_changed};

use crate::daemon::DaemonCore;
use anyhow::Result;
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, watch};
use zbus::{Connection, connection::Builder};

/// Default method timeout for short-lived daemon control/status D-Bus connections
/// (`+doctor`, `+agent`, `singleton`, and `agent_deployer`) so a wedged process
/// never blocks startup or CLI inspection for the 25-second bus timeout.
pub const CONTROL_METHOD_TIMEOUT: Duration = Duration::from_secs(3);

/// Opens a session D-Bus connection bounded by [`CONTROL_METHOD_TIMEOUT`].
pub async fn connect_control_session() -> zbus::Result<Connection> {
    Builder::session()
        .method_timeout(CONTROL_METHOD_TIMEOUT)
        .build()
        .await
}

/// Returns `true` when `err` indicates that no service currently owns the target
/// bus name or object path (`ServiceUnknown`, `NameHasNoOwner`, `UnknownObject`).
pub fn is_service_unavailable(err: &zbus::Error) -> bool {
    matches!(
        err,
        zbus::Error::FDO(fdo_err)
            if matches!(
                fdo_err.as_ref(),
                zbus::fdo::Error::ServiceUnknown(_)
                    | zbus::fdo::Error::NameHasNoOwner(_)
                    | zbus::fdo::Error::UnknownObject(_)
            )
    )
}

/// Waits for `bus_name` to have no owner on `conn` via `NameOwnerChanged` without polling.
pub async fn wait_for_bus_name_released(conn: &Connection, bus_name: &str, timeout: Duration) {
    let Ok(dbus) = zbus::fdo::DBusProxy::new(conn).await else {
        return;
    };
    let Ok(well_known) = zbus::names::BusName::try_from(bus_name) else {
        return;
    };
    let Ok(mut stream) = dbus
        .receive_name_owner_changed_with_args(&[(0, bus_name), (2, "")])
        .await
    else {
        return;
    };
    if !dbus.name_has_owner(well_known).await.unwrap_or(false) {
        return;
    }
    let _ = tokio::time::timeout(timeout, stream.next()).await;
}

pub async fn connect_agent_proxy(conn: &Connection) -> zbus::Result<AgentProxy<'static>> {
    AgentProxy::builder(conn)
        .destination(boxxy_sys_utils::agent_bus_name())
        .build()
        .await
}

pub async fn connect_pty_proxy(conn: &Connection) -> zbus::Result<pty::AgentPtyProxy<'static>> {
    pty::AgentPtyProxy::builder(conn)
        .destination(boxxy_sys_utils::agent_bus_name())
        .build()
        .await
}

pub async fn connect_claw_proxy(conn: &Connection) -> zbus::Result<claw::AgentClawProxy<'static>> {
    claw::AgentClawProxy::builder(conn)
        .destination(boxxy_sys_utils::agent_bus_name())
        .build()
        .await
}

pub async fn connect_maintenance_proxy(
    conn: &Connection,
) -> zbus::Result<maintenance::AgentMaintenanceProxy<'static>> {
    maintenance::AgentMaintenanceProxy::builder(conn)
        .destination(boxxy_sys_utils::agent_bus_name())
        .build()
        .await
}

pub async fn start_services(
    core: Arc<DaemonCore>,
    client_count_tx: watch::Sender<usize>,
) -> Result<(Connection, Arc<Mutex<ClawSessionManager>>)> {
    Ok(start_services_with_options(core, client_count_tx, true).await?)
}

pub async fn start_services_with_options(
    core: Arc<DaemonCore>,
    client_count_tx: watch::Sender<usize>,
    claim_bus_name: bool,
) -> zbus::Result<(Connection, Arc<Mutex<ClawSessionManager>>)> {
    let claw_subsystem = crate::claw::ClawSubsystem::new(core.state.clone());
    let session_manager = Arc::new(Mutex::new(ClawSessionManager::new(Arc::new(
        claw_subsystem.clone(),
    ))));

    let mut builder = Builder::session()
        .serve_at(
            "/dev/boxxy/Agent",
            AgentInterface {
                core: core.clone(),
                client_count_tx: client_count_tx.clone(),
                session_manager: session_manager.clone(),
            },
        )
        .serve_at(
            "/dev/boxxy/BoxxyTerminal/Agent/Pty",
            crate::pty::PtySubsystem::new(core.state.clone()),
        )
        .serve_at("/dev/boxxy/BoxxyTerminal/Agent/Claw", claw_subsystem)
        .serve_at(
            "/dev/boxxy/BoxxyTerminal/Agent/Maintenance",
            crate::maintenance::MaintenanceSubsystem::new(
                core.state.clone(),
                core.dream_status.clone(),
                core.power.clone(),
            ),
        );

    if claim_bus_name {
        // Request the well-known name *after* ObjectServer starts (`Builder::serve_at`),
        // without `REPLACE_EXISTING` so concurrent daemon launches resolve atomically
        // via `zbus::Error::NameTaken` instead of stealing an active daemon's name.
        builder = builder
            .name(boxxy_sys_utils::agent_bus_name())
            .replace_existing_names(false);
    }

    let conn = builder.build().await?;

    broadcasters::spawn_signal_broadcasters(&conn, &core, &session_manager);

    Ok((conn, session_manager))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claw::ClawSubsystem;
    use crate::core::state::AgentState;
    use crate::daemon::dreaming::{DreamStatus, DreamStatusCell};
    use crate::daemon::power;
    use crate::ipc::claw::AgentClawProxy;
    use crate::ipc::maintenance::AgentMaintenanceProxy;
    use crate::ipc::pty::AgentPtyProxy;
    use crate::maintenance::MaintenanceSubsystem;
    use crate::pty::PtySubsystem;
    use std::os::fd::AsFd;
    use std::os::unix::io::AsRawFd;
    use std::os::unix::net::UnixStream;
    use zbus::Guid;

    #[tokio::test]
    async fn p2p_subsystems_roundtrip() {
        let (p0, p1) = UnixStream::pair().expect("unix stream pair");
        let state = AgentState::new();
        let dream_status = DreamStatusCell::new(DreamStatus::Idle);
        let power_monitor = power::ac_only();

        let (server_conn, client_conn) = futures_util::try_join!(
            Builder::unix_stream(p0)
                .server(Guid::generate())
                .p2p()
                .serve_at(
                    "/dev/boxxy/BoxxyTerminal/Agent/Pty",
                    PtySubsystem::new(state.clone()),
                )
                .serve_at(
                    "/dev/boxxy/BoxxyTerminal/Agent/Claw",
                    ClawSubsystem::new(state.clone()),
                )
                .serve_at(
                    "/dev/boxxy/BoxxyTerminal/Agent/Maintenance",
                    MaintenanceSubsystem::new(state, dream_status, power_monitor),
                )
                .build(),
            Builder::unix_stream(p1).p2p().build(),
        )
        .expect("p2p connections should build");

        let _server_task = tokio::spawn(async move {
            let _keep = server_conn;
            std::future::pending::<()>().await;
        });

        let pty_proxy = AgentPtyProxy::new(&client_conn).await.expect("pty proxy");
        let master_fd = pty_proxy.create_pty().await.expect("create_pty over p2p");
        assert!(master_fd.as_raw_fd() >= 0);

        let maint_proxy = AgentMaintenanceProxy::new(&client_conn)
            .await
            .expect("maintenance proxy");
        assert_eq!(
            maint_proxy
                .get_maintenance_status()
                .await
                .expect("maintenance status"),
            "idle"
        );
        assert!(!maint_proxy.is_on_battery().await.expect("is_on_battery"));

        let claw_proxy = AgentClawProxy::new(&client_conn).await.expect("claw proxy");
        let (code, stdout, _stderr) = claw_proxy
            .exec_shell("printf boxxy_zbus6".to_string())
            .await
            .expect("exec_shell over p2p");
        assert_eq!(code, 0);
        assert_eq!(stdout, "boxxy_zbus6");

        let tmp = tempfile::tempdir().expect("tempdir");
        let file_path = tmp.path().join("zbus6_test.txt");
        let file_path_str = file_path.to_string_lossy().into_owned();
        claw_proxy
            .write_file(file_path_str.clone(), "line1\nline2\n".to_string())
            .await
            .expect("write_file over p2p");
        let content = claw_proxy
            .read_file(file_path_str.clone(), 1, 10)
            .await
            .expect("read_file over p2p");
        assert!(content.contains("line1"));
        claw_proxy
            .delete_file(file_path_str)
            .await
            .expect("delete_file over p2p");
    }

    #[tokio::test]
    async fn p2p_pty_spawn_and_exited_signal_stream() {
        use crate::ipc::pty::SpawnOptions;
        use futures_util::StreamExt;

        let (p0, p1) = UnixStream::pair().expect("unix stream pair");
        let state = AgentState::new();

        let (server_conn, client_conn) = futures_util::try_join!(
            Builder::unix_stream(p0)
                .server(Guid::generate())
                .p2p()
                .serve_at(
                    "/dev/boxxy/BoxxyTerminal/Agent/Pty",
                    PtySubsystem::new(state),
                )
                .build(),
            Builder::unix_stream(p1).p2p().build(),
        )
        .expect("p2p connections should build");

        let _server_task = tokio::spawn(async move {
            let _keep = server_conn;
            std::future::pending::<()>().await;
        });

        let pty_proxy = AgentPtyProxy::new(&client_conn).await.expect("pty proxy");
        let mut exited_stream = pty_proxy
            .receive_exited()
            .await
            .expect("subscribe exited signal");

        let master_fd = pty_proxy.create_pty().await.expect("create_pty");
        let fd_for_spawn =
            zbus::OwnedFd::from(master_fd.as_fd().try_clone_to_owned().expect("clone master_fd"));

        let tmp = tempfile::tempdir().expect("tempdir");
        let options = SpawnOptions {
            cwd: tmp.path().to_string_lossy().into_owned(),
            argv: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "exit 42".to_string(),
            ],
            env: vec![("TERM".to_string(), "xterm-256color".to_string())],
            cols: 80,
            rows: 24,
            pane_id: "pane-signal-test".to_string(),
        };

        let pid = pty_proxy
            .spawn(fd_for_spawn, options)
            .await
            .expect("spawn over p2p");

        let signal = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            exited_stream.next(),
        )
        .await
        .expect("timed out waiting for Exited signal")
        .expect("stream ended unexpectedly");

        let args = signal.args().expect("signal args");
        assert_eq!(args.pid, pid);
        assert_eq!(args.exit_code, 42);

        let (has_exited, recorded_code) = pty_proxy
            .get_exit_status(pid)
            .await
            .expect("get_exit_status");
        assert!(has_exited);
        assert_eq!(recorded_code, 42);
    }

    #[tokio::test]
    async fn p2p_pty_detach_and_reattach_fd_roundtrip() {
        use crate::ipc::pty::SpawnOptions;

        let (p0, p1) = UnixStream::pair().expect("unix stream pair");
        let state = AgentState::new();

        let (server_conn, client_conn) = futures_util::try_join!(
            Builder::unix_stream(p0)
                .server(Guid::generate())
                .p2p()
                .serve_at(
                    "/dev/boxxy/BoxxyTerminal/Agent/Pty",
                    PtySubsystem::new(state),
                )
                .build(),
            Builder::unix_stream(p1).p2p().build(),
        )
        .expect("p2p connections should build");

        let _server_task = tokio::spawn(async move {
            let _keep = server_conn;
            std::future::pending::<()>().await;
        });

        let pty_proxy = AgentPtyProxy::new(&client_conn).await.expect("pty proxy");
        let master_fd = pty_proxy.create_pty().await.expect("create_pty");
        let fd_for_spawn =
            zbus::OwnedFd::from(master_fd.as_fd().try_clone_to_owned().expect("clone master_fd"));
        // Drop the client's original master_fd so only the daemon holds it once detached
        drop(master_fd);

        let tmp = tempfile::tempdir().expect("tempdir");
        let options = SpawnOptions {
            cwd: tmp.path().to_string_lossy().into_owned(),
            argv: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf 'zbus6_persist_ok'; sleep 10".to_string(),
            ],
            env: vec![],
            cols: 80,
            rows: 24,
            pane_id: "pane-persist-test".to_string(),
        };

        let pid = pty_proxy
            .spawn(fd_for_spawn, options)
            .await
            .expect("spawn over p2p");

        pty_proxy
            .set_persistence(pid, true)
            .await
            .expect("set_persistence");
        let outcome = pty_proxy
            .detach_with_snapshot(pid, b"snapshot_seed:")
            .await
            .expect("detach_with_snapshot");
        assert_eq!(outcome, 1, "expected DetachOutcome::Detached (1)");

        let detached = pty_proxy
            .list_detached_sessions()
            .await
            .expect("list_detached_sessions");
        assert!(
            detached
                .iter()
                .any(|(p, pane, _)| *p == pid && pane == "pane-persist-test")
        );

        // Wait briefly for the detached reader to capture the shell output into the ring buffer
        for _ in 0..20 {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }

        let (replay_bytes, reattached_fd) = pty_proxy.reattach(pid).await.expect("reattach");
        assert!(reattached_fd.as_raw_fd() >= 0);
        let replay_str = String::from_utf8_lossy(&replay_bytes);
        assert!(
            replay_str.starts_with("snapshot_seed:") && replay_str.contains("zbus6_persist_ok"),
            "unexpected replay buffer: {replay_str:?}"
        );

        let _ = pty_proxy
            .signal_process_group(pid, libc::SIGKILL)
            .await;
    }

    #[tokio::test]
    async fn p2p_pty_rejects_empty_argv_dangerous_signals_and_stops_fd_leak_across_panes() {
        use crate::ipc::pty::SpawnOptions;
        use futures_util::StreamExt;

        let (p0, p1) = UnixStream::pair().expect("unix stream pair");
        let state = AgentState::new();

        let (server_conn, client_conn) = futures_util::try_join!(
            Builder::unix_stream(p0)
                .server(Guid::generate())
                .p2p()
                .serve_at(
                    "/dev/boxxy/BoxxyTerminal/Agent/Pty",
                    PtySubsystem::new(state),
                )
                .build(),
            Builder::unix_stream(p1).p2p().build(),
        )
        .expect("p2p connections should build");

        let _server_task = tokio::spawn(async move {
            let _keep = server_conn;
            std::future::pending::<()>().await;
        });

        let pty_proxy = AgentPtyProxy::new(&client_conn).await.expect("pty proxy");

        // 1. Dangerous / untracked signal_process_group calls must fail with InvalidArgs
        assert!(pty_proxy.signal_process_group(0, libc::SIGTERM).await.is_err());
        assert!(pty_proxy.signal_process_group(1, libc::SIGTERM).await.is_err());
        assert!(
            pty_proxy
                .signal_process_group(u32::MAX, libc::SIGTERM)
                .await
                .is_err()
        );
        assert!(
            pty_proxy
                .signal_process_group(99999, libc::SIGTERM)
                .await
                .is_err()
        );

        // 2. Empty argv must fail cleanly without crashing the daemon
        let bad_fd = pty_proxy.create_pty().await.expect("create_pty");
        let empty_opts = SpawnOptions {
            cwd: String::new(),
            argv: vec![],
            env: vec![],
            cols: 80,
            rows: 24,
            pane_id: "pane-empty-argv".to_string(),
        };
        assert!(pty_proxy.spawn(bad_fd, empty_opts).await.is_err());

        let tmp = tempfile::tempdir().expect("tempdir");

        // 2b. Non-existent requested CWD falls back gracefully instead of failing with ENOENT
        let missing_cwd_fd = pty_proxy.create_pty().await.expect("create_pty");
        let missing_cwd_opts = SpawnOptions {
            cwd: tmp
                .path()
                .join("deleted_subdir_404")
                .to_string_lossy()
                .into_owned(),
            argv: vec!["/bin/sh".into(), "-c".into(), "exit 0".into()],
            env: vec![],
            cols: 80,
            rows: 24,
            pane_id: "pane-missing-cwd".to_string(),
        };
        let fallback_pid = pty_proxy
            .spawn(missing_cwd_fd, missing_cwd_opts)
            .await
            .expect("spawn with missing CWD should fall back to HOME");
        assert!(fallback_pid > 0);

        // 3. Pane 2 must NOT inherit Pane 1's stored master PTY FD, and SIGKILL exit reports 137
        let leak_report = tmp.path().join("fds.txt");

        let mut exited_stream = pty_proxy.receive_exited().await.expect("exited stream");

        let master1 = pty_proxy.create_pty().await.expect("create_pty 1");
        let spawn1 = zbus::OwnedFd::from(master1.as_fd().try_clone_to_owned().expect("clone 1"));
        let pid1 = pty_proxy
            .spawn(
                spawn1,
                SpawnOptions {
                    cwd: tmp.path().to_string_lossy().into_owned(),
                    argv: vec!["/bin/sh".into(), "-c".into(), "sleep 30".into()],
                    env: vec![],
                    cols: 80,
                    rows: 24,
                    pane_id: "pane-1".into(),
                },
            )
            .await
            .expect("spawn pane 1");

        // Spawn Pane 2 that inspects its own `/proc/self/fd` symlink targets
        let master2 = pty_proxy.create_pty().await.expect("create_pty 2");
        let spawn2 = zbus::OwnedFd::from(master2.as_fd().try_clone_to_owned().expect("clone 2"));
        let inspect_cmd = format!("ls -l /proc/self/fd > '{}'", leak_report.display());
        let pid2 = pty_proxy
            .spawn(
                spawn2,
                SpawnOptions {
                    cwd: tmp.path().to_string_lossy().into_owned(),
                    argv: vec!["/bin/sh".into(), "-c".into(), inspect_cmd],
                    env: vec![],
                    cols: 80,
                    rows: 24,
                    pane_id: "pane-2".into(),
                },
            )
            .await
            .expect("spawn pane 2");

        // Wait for pid2 to finish, then kill pid1 with SIGKILL and assert exit_code == 137 (128 + 9)
        let mut saw_pid2 = false;
        let mut pid1_code = None;
        pty_proxy
            .signal_process_group(pid1, libc::SIGKILL)
            .await
            .expect("signal_process_group pid1");

        while !saw_pid2 || pid1_code.is_none() {
            let sig = tokio::time::timeout(std::time::Duration::from_secs(5), exited_stream.next())
                .await
                .expect("timeout waiting for exits")
                .expect("stream ended");
            let args = sig.args().expect("args");
            if args.pid == pid2 {
                saw_pid2 = true;
            } else if args.pid == pid1 {
                pid1_code = Some(args.exit_code);
            }
        }

        assert_eq!(pid1_code, Some(128 + libc::SIGKILL));
        let fd_listing = std::fs::read_to_string(&leak_report).expect("read fd listing");
        assert!(
            !fd_listing.contains("/dev/ptmx"),
            "Pane 2 leaked a /dev/ptmx master FD from Pane 1:\n{fd_listing}"
        );
    }

    #[tokio::test]
    async fn p2p_agent_status_and_spawn_character_roundtrip() {
        use crate::claw::CharacterRegistry;
        use crate::daemon::lifecycle;

        let (p0, p1) = UnixStream::pair().expect("unix stream pair");
        let state = AgentState::new();
        let db = boxxy_db::Db::new_in_memory().await.expect("in-memory db");
        let registry = Arc::new(CharacterRegistry::load_or_default(&db).await);
        let (client_tx, client_rx) = watch::channel(0usize);
        let ghost = lifecycle::start(client_rx);
        let dream_status = DreamStatusCell::new(DreamStatus::Idle);
        let power_monitor = power::ac_only();

        let core = Arc::new(DaemonCore {
            state: state.clone(),
            registry: registry.clone(),
            power: power_monitor,
            ghost,
            dream_status,
            db,
        });
        let claw_subsystem = ClawSubsystem::new(state);
        let session_manager = Arc::new(Mutex::new(ClawSessionManager::new(Arc::new(
            claw_subsystem,
        ))));

        let (server_conn, client_conn) = futures_util::try_join!(
            Builder::unix_stream(p0)
                .server(Guid::generate())
                .p2p()
                .serve_at(
                    "/dev/boxxy/Agent",
                    AgentInterface {
                        core,
                        client_count_tx: client_tx,
                        session_manager,
                    },
                )
                .build(),
            Builder::unix_stream(p1).p2p().build(),
        )
        .expect("p2p connections should build");

        let _server_task = tokio::spawn(async move {
            let _keep = server_conn;
            std::future::pending::<()>().await;
        });

        let agent_proxy = AgentProxy::new(&client_conn).await.expect("agent proxy");

        // 1. GetDaemonStatus returns valid JSON with expected fields
        let status_json = agent_proxy
            .get_daemon_status()
            .await
            .expect("get_daemon_status");
        let parsed: agent_service::DaemonStatusSnapshot =
            serde_json::from_str(&status_json).expect("valid status JSON");
        assert_eq!(parsed.dream_cycle, "idle");
        assert_eq!(parsed.power_source, "AC");
        assert!(parsed.is_ghost);

        // 2. RequestSpawnCharacter while in Ghost Mode queues character for TakePendingCharacterSpawns
        let snap_json = agent_proxy
            .get_registry_snapshot()
            .await
            .expect("get_registry_snapshot");
        let snap: boxxy_claw_protocol::characters::RegistrySnapshot =
            serde_json::from_str(&snap_json).expect("snapshot json");
        let first_char = snap.catalog.first().expect("at least one character");

        agent_proxy
            .request_spawn_character(first_char.config.name.clone(), "/tmp".to_string())
            .await
            .expect("request_spawn_character");

        let queued = agent_proxy
            .take_pending_character_spawns()
            .await
            .expect("take_pending_character_spawns");
        assert_eq!(queued, vec![(first_char.config.id.clone(), "/tmp".to_string())]);

        // Second drain is empty
        let empty = agent_proxy
            .take_pending_character_spawns()
            .await
            .expect("second drain");
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn p2p_request_stop_dispatches_reply_before_closing_connection() {
        use crate::claw::CharacterRegistry;
        use crate::daemon::lifecycle;

        let (p0, p1) = UnixStream::pair().expect("unix stream pair");
        let state = AgentState::new();
        let db = boxxy_db::Db::new_in_memory().await.expect("in-memory db");
        let registry = Arc::new(CharacterRegistry::load_or_default(&db).await);
        let (client_tx, client_rx) = watch::channel(0usize);
        let ghost = lifecycle::start(client_rx);
        let dream_status = DreamStatusCell::new(DreamStatus::Idle);
        let power_monitor = power::ac_only();

        let core = Arc::new(DaemonCore {
            state: state.clone(),
            registry,
            power: power_monitor,
            ghost,
            dream_status,
            db,
        });
        let claw_subsystem = ClawSubsystem::new(state);
        let session_manager = Arc::new(Mutex::new(ClawSessionManager::new(Arc::new(
            claw_subsystem,
        ))));

        let (server_conn, client_conn) = futures_util::try_join!(
            Builder::unix_stream(p0)
                .server(Guid::generate())
                .p2p()
                .serve_at(
                    "/dev/boxxy/Agent",
                    AgentInterface {
                        core,
                        client_count_tx: client_tx,
                        session_manager,
                    },
                )
                .build(),
            Builder::unix_stream(p1).p2p().build(),
        )
        .expect("p2p connections should build");

        let agent_proxy = AgentProxy::new(&client_conn).await.expect("agent proxy");

        // `ResponseDispatchNotifier::followed_by` must deliver `Ok(())` to the caller
        // *before* closing the server connection, and then wake `closed().await` on both ends.
        agent_proxy
            .request_stop()
            .await
            .expect("request_stop should return Ok(()) cleanly before socket close");

        tokio::time::timeout(std::time::Duration::from_secs(2), server_conn.closed())
            .await
            .expect("server_conn.closed() should resolve promptly after request_stop");
        assert!(server_conn.is_closed());

        tokio::time::timeout(std::time::Duration::from_secs(2), client_conn.closed())
            .await
            .expect("client_conn.closed() should observe peer EOF");
        assert!(client_conn.is_closed());
    }

    #[tokio::test]
    async fn p2p_claw_domain_errors_preserve_typed_dbus_error_names() {
        use crate::ipc::claw::AgentClawError;
        use zbus::DBusError;

        let (p0, p1) = UnixStream::pair().expect("unix stream pair");
        let state = AgentState::new();

        let (server_conn, client_conn) = futures_util::try_join!(
            Builder::unix_stream(p0)
                .server(Guid::generate())
                .p2p()
                .serve_at(
                    "/dev/boxxy/BoxxyTerminal/Agent/Claw",
                    ClawSubsystem::new(state),
                )
                .build(),
            Builder::unix_stream(p1).p2p().build(),
        )
        .expect("p2p connections should build");

        let _server_task = tokio::spawn(async move {
            let _keep = server_conn;
            std::future::pending::<()>().await;
        });

        let claw_proxy = AgentClawProxy::new(&client_conn).await.expect("claw proxy");

        // 1. Built-in blacklisted path maps to `dev.boxxy.BoxxyTerminal.Agent.Error.BlacklistedPath`
        let bl_err = claw_proxy
            .read_file("/etc/shadow".to_string(), 1, 10)
            .await
            .expect_err("blacklisted read must fail");
        let typed_bl = AgentClawError::from(bl_err);
        assert_eq!(
            typed_bl.name().as_str(),
            "dev.boxxy.BoxxyTerminal.Agent.Error.BlacklistedPath"
        );
        assert!(matches!(typed_bl, AgentClawError::BlacklistedPath(_)));

        // 2. Missing file maps to `dev.boxxy.BoxxyTerminal.Agent.Error.FileNotFound`
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp
            .path()
            .join("missing_fixture.txt")
            .to_string_lossy()
            .into_owned();
        let nf_err = claw_proxy
            .read_file(missing, 1, 10)
            .await
            .expect_err("missing file read must fail");
        let typed_nf = AgentClawError::from(nf_err);
        assert_eq!(
            typed_nf.name().as_str(),
            "dev.boxxy.BoxxyTerminal.Agent.Error.FileNotFound"
        );
        assert!(matches!(typed_nf, AgentClawError::FileNotFound(_)));

        // 3. Dangerous PID maps to `dev.boxxy.BoxxyTerminal.Agent.Error.InvalidArgs`
        let pid_err = claw_proxy
            .kill_process(0, libc::SIGTERM)
            .await
            .expect_err("invalid pid must fail");
        let typed_pid = AgentClawError::from(pid_err);
        assert_eq!(
            typed_pid.name().as_str(),
            "dev.boxxy.BoxxyTerminal.Agent.Error.InvalidArgs"
        );
        assert!(matches!(typed_pid, AgentClawError::InvalidArgs(_)));
    }

    #[test]
    fn is_service_unavailable_recognizes_fdo_errors() {
        let svc_unknown =
            zbus::Error::FDO(Box::new(zbus::fdo::Error::ServiceUnknown("gone".into())));
        let no_owner =
            zbus::Error::FDO(Box::new(zbus::fdo::Error::NameHasNoOwner("none".into())));
        let unknown_obj =
            zbus::Error::FDO(Box::new(zbus::fdo::Error::UnknownObject("no path".into())));
        let other_fdo = zbus::Error::FDO(Box::new(zbus::fdo::Error::InvalidArgs("bad".into())));

        assert!(is_service_unavailable(&svc_unknown));
        assert!(is_service_unavailable(&no_owner));
        assert!(is_service_unavailable(&unknown_obj));
        assert!(!is_service_unavailable(&other_fdo));
        assert!(!is_service_unavailable(&zbus::Error::InvalidReply));
    }

    #[test]
    fn prepare_reload_exec_args_strips_deleted_suffix_and_transient_flags() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        use std::path::PathBuf;

        let swapped_exe = PathBuf::from("/usr/local/bin/boxxy-agent (deleted)");
        let args = vec![
            OsString::from("boxxy-agent"),
            OsString::from("--background"),
            OsString::from("--dev"),
            OsString::from("--sync-settings"),
            OsString::from("--config-dir"),
            OsString::from_vec(b"/tmp/cfg-\xff".to_vec()),
        ];

        let (c_exe, c_args) =
            agent_service::prepare_reload_exec_args(Some(swapped_exe), args).expect("reload spec");
        assert_eq!(c_exe.as_bytes(), b"/usr/local/bin/boxxy-agent");
        let arg_bytes: Vec<&[u8]> = c_args.iter().map(|c| c.as_bytes()).collect();
        assert_eq!(
            arg_bytes,
            vec![
                &b"boxxy-agent"[..],
                &b"--dev"[..],
                &b"--config-dir"[..],
                &b"/tmp/cfg-\xff"[..],
            ]
        );

        // Empty argv falls back to executable path as argv[0], and missing exe errors cleanly.
        let (_, fallback_args) = agent_service::prepare_reload_exec_args(
            Some(PathBuf::from("/usr/bin/boxxy-agent")),
            Vec::<OsString>::new(),
        )
        .expect("fallback argv[0]");
        assert_eq!(fallback_args.len(), 1);
        assert_eq!(fallback_args[0].as_bytes(), b"/usr/bin/boxxy-agent");
        assert!(agent_service::prepare_reload_exec_args(None, Vec::<OsString>::new()).is_err());
    }
}
