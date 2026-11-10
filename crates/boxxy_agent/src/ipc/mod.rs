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
use std::sync::Arc;
use tokio::sync::{Mutex, watch};
use zbus::{Connection, connection::Builder};

pub async fn start_services(
    core: Arc<DaemonCore>,
    client_count_tx: watch::Sender<usize>,
) -> Result<(Connection, Arc<Mutex<ClawSessionManager>>)> {
    let claw_subsystem = Arc::new(crate::claw::ClawSubsystem::new(core.state.clone()));
    let session_manager = Arc::new(Mutex::new(ClawSessionManager::new(claw_subsystem.clone())));

    let conn = Builder::session()
        .name("dev.boxxy.BoxxyAgent")
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
        .serve_at(
            "/dev/boxxy/BoxxyTerminal/Agent/Claw",
            crate::claw::ClawSubsystem::new(core.state.clone()),
        )
        .serve_at(
            "/dev/boxxy/BoxxyTerminal/Agent/Maintenance",
            crate::maintenance::MaintenanceSubsystem::new(
                core.state.clone(),
                core.dream_status.clone(),
                core.power.clone(),
            ),
        )
        .build()
        .await?;

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

        // 3. Pane 2 must NOT inherit Pane 1's stored master PTY FD, and SIGKILL exit reports 137
        let tmp = tempfile::tempdir().expect("tempdir");
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
}
