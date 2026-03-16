use anyhow::{Context, Result};
use std::os::unix::io::{FromRawFd, OwnedFd};
use tokio::io::unix::AsyncFd;
use tokio::io::Interest;
use tokio::net::UnixStream;
use tokio::signal::unix::{signal, SignalKind};
use zbus::connection::Builder;
use boxxy_agent::ipc::BoxxyAgent;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter_module("zbus", log::LevelFilter::Warn)
        .filter_module("zvariant", log::LevelFilter::Warn)
        .filter_module("tracing", log::LevelFilter::Warn)
        .filter_module("sqlx", log::LevelFilter::Warn)
        .init();
    log::info!("Boxxy Agent starting...");

    let args: Vec<String> = std::env::args().collect();
    let fd_arg = args.iter().find(|a| a.starts_with("--socket-fd="));

    let stream = if let Some(fd_str) = fd_arg {
        let fd_num: i32 = fd_str.strip_prefix("--socket-fd=").unwrap().parse()
            .context("Failed to parse --socket-fd")?;
        log::info!("Using inherited FD: {}", fd_num);
        
        // Log environment for debugging host escape
        log::info!("Agent HOME: {:?}", std::env::var("HOME"));
        log::info!("Agent SHELL: {:?}", std::env::var("SHELL"));
        log::info!("Agent PATH: {:?}", std::env::var("PATH"));

        let std_stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd_num) };
        std_stream.set_nonblocking(true).context("Failed to set socket non-blocking")?;
        UnixStream::from_std(std_stream).context("Failed to create tokio UnixStream from std")?
    } else {
        // Fallback for native path: UI passes the Unix socket path as the first argument.
        let socket_path = args.get(1).context("Expected --socket-fd or socket path as first argument")?;
        log::info!("Connecting to socket: {}", socket_path);
        UnixStream::connect(socket_path)
            .await
            .with_context(|| format!("Failed to connect to socket: {}", socket_path))?
    };

    // Dup the stream FD before handing it to zbus so we can independently
    // monitor it for closure (detecting when the UI process exits).
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
        .serve_at("/play/mii/Boxxy/Agent", BoxxyAgent)?
        .serve_at("/play/mii/Boxxy/AgentClaw", boxxy_agent::claw::AgentClaw)?
        .build()
        .await
        .context("Failed to build zbus connection")?;

    log::info!("Boxxy Agent serving at /play/mii/Boxxy/Agent and /play/mii/Boxxy/AgentClaw");

    #[cfg(target_os = "linux")]
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
    }

    let mut sigterm = signal(SignalKind::terminate())
        .context("Failed to set up SIGTERM handler")?;
    let mut sighup = signal(SignalKind::hangup())
        .context("Failed to set up SIGHUP handler")?;

    // Wait for shutdown: SIGTERM/SIGHUP from parent death (PDEATHSIG), or socket
    // HUP when the UI closes its end of the socket on exit.
    //
    // We use is_read_closed() to distinguish real connection closure (EPOLLRDHUP)
    // from normal D-Bus traffic (EPOLLIN). When data arrives we clear the ready
    // flag and loop; when the remote end closes we break out and exit.
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
                                // D-Bus traffic made the socket readable — not a close.
                                // Clear the ready flag and wait for the next event.
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

    Ok(())
}
