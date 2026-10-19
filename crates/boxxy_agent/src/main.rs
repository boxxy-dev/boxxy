use anyhow::{Context, Result};
use boxxy_agent::daemon;
use boxxy_agent::ipc::AgentProxy;
use boxxy_agent::ipc::maintenance::AgentMaintenanceProxy;
use boxxy_agent::ipc::pty::AgentPtyProxy;
use clap::{Parser, Subcommand};
use log::info;
use zbus::Connection;

#[derive(Parser, Debug)]
#[command(
    name = "boxxy-agent",
    version,
    about = "Boxxy host-side maintenance daemon"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Run headlessly: spawn daemon and exit immediately.
    #[arg(long)]
    background: bool,

    /// Skip the singleton check (for debugging only).
    #[arg(long, hide = true)]
    no_singleton: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the daemon (default).
    Start,
    /// Stop the running daemon gracefully.
    Stop,
    /// Restart the running daemon (self-upgrade).
    Restart,
    /// List PTY sessions the daemon has kept alive after the UI closed
    /// (persistence must be enabled in Preferences → Advanced).
    ListSessions,
    /// Show daemon status, power state, Dream Cycle state, and active sessions.
    Status,
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter_module("zbus", log::LevelFilter::Warn)
        .filter_module("tracing", log::LevelFilter::Warn)
        .filter_module("sqlx", log::LevelFilter::Warn)
        .filter_module("rig", log::LevelFilter::Warn)
        .init();

    let cli = Cli::parse();

    // Handle subcommands that don't require the daemon to start
    if let Some(cmd) = &cli.command {
        match cmd {
            Commands::Stop => {
                return run_async(async {
                    let conn = Connection::session().await?;
                    let proxy = AgentProxy::new(&conn).await?;
                    info!("Requesting daemon stop...");
                    proxy.request_stop().await?;
                    Ok(())
                });
            }
            Commands::Restart => {
                return run_async(async {
                    let conn = Connection::session().await?;
                    let proxy = AgentProxy::new(&conn).await?;
                    info!("Requesting daemon restart...");
                    proxy.request_reload().await?;
                    Ok(())
                });
            }
            Commands::ListSessions => {
                return run_async(async {
                    let conn = Connection::session().await?;
                    let proxy = AgentPtyProxy::new(&conn).await?;
                    let sessions = proxy.list_detached_sessions_detailed().await?;
                    if sessions.is_empty() {
                        println!("No detached PTY sessions.");
                    } else {
                        println!(
                            "{:>8}  {:>8}  {:<16}  {:<10}  {:<30}  {}",
                            "PID", "IDLE_S", "PROCESS", "STATE", "CWD", "PANE_ID"
                        );
                        for s in sessions {
                            let proc_label = if s.foreground_process.is_empty() {
                                "(shell)".to_string()
                            } else {
                                s.foreground_process
                            };
                            let state_label = if s.waiting_for_input {
                                "INPUT_WAIT"
                            } else {
                                "DETACHED"
                            };
                            println!(
                                "{:>8}  {:>8}  {:<16}  {:<10}  {:<30}  {}",
                                s.pid, s.idle_secs, proc_label, state_label, s.cwd, s.pane_id
                            );
                        }
                    }
                    Ok(())
                });
            }
            Commands::Status => {
                return run_async(async {
                    let conn = Connection::session().await?;
                    let agent_proxy = AgentProxy::new(&conn).await?;
                    let maint_proxy = AgentMaintenanceProxy::new(&conn).await?;
                    let pty_proxy = AgentPtyProxy::new(&conn).await?;

                    let version = agent_proxy.get_version().await.unwrap_or_default();
                    let config_dir = agent_proxy.get_config_dir().await.unwrap_or_default();
                    let maint_status = maint_proxy
                        .get_maintenance_status()
                        .await
                        .unwrap_or_else(|_| "unknown".to_string());
                    let on_battery = maint_proxy.is_on_battery().await.unwrap_or(false);
                    let claw_sessions = agent_proxy.list_claw_sessions().await.unwrap_or_default();
                    let detached_ptys = pty_proxy
                        .list_detached_sessions_detailed()
                        .await
                        .unwrap_or_default();

                    println!("Boxxy Agent v{version}");
                    println!("  Config Dir:        {config_dir}");
                    println!(
                        "  Power Source:      {}",
                        if on_battery { "Battery" } else { "AC" }
                    );
                    println!("  Dream Cycle:       {maint_status}");
                    println!("  Active Claw Panes: {}", claw_sessions.len());
                    println!("  Detached Shells:   {}", detached_ptys.len());
                    Ok(())
                });
            }
            Commands::Start => {}
        }
    }

    if cli.background {
        info!("Entering background mode...");
        daemonize()?;
    }

    // Hydrate the settings cache from disk BEFORE starting the runtime —
    // the claw engine reads model selection from `Settings::load()`, which
    // is backed by a process-global `OnceLock` cache. Without this init
    // the daemon would always see the default Settings (no model selected),
    // even though the UI process has correctly configured one.
    boxxy_preferences::Settings::init();

    // Now start the async runtime ONLY in the final daemon process
    run_async(async move {
        let resolved_config_dir = boxxy_sys_utils::get_config_dir();
        info!("Boxxy Agent starting (v{})...", env!("CARGO_PKG_VERSION"));
        info!("Using config directory: {:?}", resolved_config_dir);

        if !cli.no_singleton {
            match daemon::singleton::try_claim_or_handoff().await? {
                daemon::singleton::ClaimResult::Claimed => {
                    info!("Singleton claimed — starting daemon");
                }
                daemon::singleton::ClaimResult::HandedOff => {
                    info!("Existing daemon is up-to-date — exiting");
                    return Ok(());
                }
                daemon::singleton::ClaimResult::Upgraded => {
                    info!("Upgraded running daemon — exiting launcher");
                    return Ok(());
                }
            }
        }

        daemon::DaemonCore::run().await
    })
}

/// Helper to run a future to completion using a new Tokio runtime.
fn run_async<F: std::future::Future<Output = Result<()>>>(f: F) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .max_io_events_per_tick(256)
        .max_io_events_per_busy_tick(64)
        .thread_name("boxxy-agent-worker")
        .build()
        .context("Failed to build Tokio runtime")?;
    rt.block_on(f)
}

/// Double-fork + setsid so the process becomes a true daemon.
/// MUST be called before starting any threads (including Tokio runtime).
fn daemonize() -> Result<()> {
    use nix::unistd::{ForkResult, fork, setsid};

    match unsafe { fork()? } {
        ForkResult::Parent { .. } => std::process::exit(0),
        ForkResult::Child => {
            setsid()?;
            match unsafe { fork()? } {
                ForkResult::Parent { .. } => std::process::exit(0),
                ForkResult::Child => {
                    // Redirect standard FDs to /dev/null to be a well-behaved daemon
                    use std::os::unix::io::AsRawFd;
                    if let Ok(dev_null) = std::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open("/dev/null")
                    {
                        let fd = dev_null.as_raw_fd();
                        unsafe {
                            libc::dup2(fd, libc::STDIN_FILENO);
                            libc::dup2(fd, libc::STDOUT_FILENO);
                            libc::dup2(fd, libc::STDERR_FILENO);
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
