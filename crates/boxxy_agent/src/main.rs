use anyhow::{Context, Result};
use std::os::unix::io::{FromRawFd, OwnedFd};
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::net::UnixStream;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::broadcast;
use zbus::connection::Builder;

use boxxy_agent::core::state::AgentState;
use boxxy_agent::subsystems::claw::ClawSubsystem;
use boxxy_agent::subsystems::maintenance::MaintenanceSubsystem;
use boxxy_agent::subsystems::pty::PtySubsystem;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter_module("zbus", log::LevelFilter::Warn)
        .filter_module("zvariant", log::LevelFilter::Warn)
        .filter_module("tracing", log::LevelFilter::Warn)
        .filter_module("sqlx", log::LevelFilter::Warn)
        .filter_module("h2", log::LevelFilter::Warn)
        .filter_module("hyper", log::LevelFilter::Warn)
        .filter_module("reqwest", log::LevelFilter::Warn)
        .filter_module("rustls", log::LevelFilter::Warn)
        .init();
    log::info!("Boxxy Agent starting...");

    let state = AgentState::new();
    let (shutdown_tx, mut shutdown_rx) = broadcast::channel(1);

    // Spawn background maintenance loop
    let shutdown_rx_maintenance = shutdown_tx.subscribe();
    tokio::spawn(async move {
        boxxy_telemetry::init_db().await;
        boxxy_telemetry::init().await;
        
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(600));
        let mut shutdown_rx = shutdown_rx_maintenance;
        
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    boxxy_telemetry::flush_journal().await;
                }
                _ = shutdown_rx.recv() => {
                    log::info!("Maintenance loop shutting down gracefully.");
                    break;
                }
            }
        }
    });

    let args: Vec<String> = std::env::args().collect();
    let fd_arg = args.iter().find(|a| a.starts_with("--socket-fd="));

    let stream = if let Some(fd_str) = fd_arg {
        let fd_num: i32 = fd_str
            .strip_prefix("--socket-fd=")
            .unwrap()
            .parse()
            .context("Failed to parse --socket-fd")?;
        log::info!("Using inherited FD: {}", fd_num);

        let std_stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd_num) };
        std_stream
            .set_nonblocking(true)
            .context("Failed to set socket non-blocking")?;
        UnixStream::from_std(std_stream).context("Failed to create tokio UnixStream from std")?
    } else {
        let socket_path = args
            .get(1)
            .context("Expected --socket-fd or socket path as first argument")?;
        log::info!("Connecting to socket: {}", socket_path);
        UnixStream::connect(socket_path)
            .await
            .with_context(|| format!("Failed to connect to socket: {}", socket_path))?
    };

    let monitor_fd: Option<OwnedFd> = {
        use std::os::unix::io::AsRawFd;
        let dup = unsafe { libc::dup(stream.as_raw_fd()) };
        if dup >= 0 {
            Some(unsafe { OwnedFd::from_raw_fd(dup) })
        } else {
            log::warn!("dup failed: {}", std::io::Error::last_os_error());
            None
        }
    };

    let _conn = Builder::unix_stream(stream)
        .p2p()
        .serve_at("/dev/boxxy/BoxxyTerminal/Agent/Pty", PtySubsystem::new(state.clone()))?
        .serve_at("/dev/boxxy/BoxxyTerminal/Agent/Claw", ClawSubsystem::new(state.clone()))?
        .serve_at("/dev/boxxy/BoxxyTerminal/Agent/Maintenance", MaintenanceSubsystem::new(state.clone()))?
        .build()
        .await
        .context("Failed to build zbus connection")?;

    log::info!("Boxxy Agent serving at /dev/boxxy/BoxxyTerminal/Agent/*");

    #[cfg(target_os = "linux")]
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
    }

    let mut sigterm = signal(SignalKind::terminate()).context("Failed to set up SIGTERM handler")?;
    let mut sighup = signal(SignalKind::hangup()).context("Failed to set up SIGHUP handler")?;

    match monitor_fd.and_then(|fd| {
        AsyncFd::with_interest(fd, Interest::READABLE)
            .map_err(|e| {
                log::warn!("Failed to set up socket monitor: {}", e);
                e
            })
            .ok()
    }) {
        Some(afd) => {
            loop {
                tokio::select! {
                    _ = sigterm.recv() => {
                        log::info!("SIGTERM received, shutting down.");
                        break;
                    }
                    _ = sighup.recv() => {
                        log::info!("SIGHUP received, shutting down.");
                        break;
                    }
                    guard = afd.readable() => {
                        match guard {
                            Ok(g) if g.ready().is_read_closed() => {
                                log::info!("Socket closed, shutting down.");
                                break;
                            }
                            Ok(mut g) => {
                                g.clear_ready();
                            }
                            Err(_) => {
                                log::info!("Socket error, shutting down.");
                                break;
                            }
                        }
                    }
                }
            }
        }
        None => {
            tokio::select! {
                _ = sigterm.recv() => log::info!("SIGTERM received, shutting down."),
                _ = sighup.recv()  => log::info!("SIGHUP received, shutting down."),
            }
        }
    }

    // Trigger graceful shutdown of subsystems
    let _ = shutdown_tx.send(());

    // Final Telemetry Flush before exit
    log::debug!("Performing final telemetry flush...");
    boxxy_telemetry::seed_dream().await;
    boxxy_telemetry::flush_journal().await;
    boxxy_telemetry::shutdown().await;

    Ok(())
}
