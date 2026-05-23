use anyhow::{Context, Result};
use boxxy_agent::daemon;
use clap::{Parser, Subcommand};
use log::info;
use zbus::{Connection, proxy};

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
}

/// Proxy for talking to the running daemon.
#[proxy(
    interface = "dev.boxxy.BoxxyTerminal.Agent",
    default_service = "dev.boxxy.BoxxyAgent",
    default_path = "/dev/boxxy/Agent"
)]
trait AgentControl {
    async fn request_reload(&self) -> zbus::Result<()>;
    async fn request_stop(&self) -> zbus::Result<()>;
}

/// Proxy for the PTY subsystem, used by `list-sessions`.
#[proxy(
    interface = "dev.boxxy.BoxxyTerminal.Agent.Pty",
    default_service = "dev.boxxy.BoxxyAgent",
    default_path = "/dev/boxxy/BoxxyTerminal/Agent/Pty"
)]
trait AgentPtyControl {
    async fn list_detached_sessions(&self) -> zbus::Result<Vec<(u32, String, u64)>>;
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter_module("zbus", log::LevelFilter::Warn)
        .filter_module("zvariant", log::LevelFilter::Warn)
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
                    let proxy = AgentControlProxy::new(&conn).await?;
                    info!("Requesting daemon stop...");
                    proxy.request_stop().await?;
                    Ok(())
                });
            }
            Commands::Restart => {
                return run_async(async {
                    let conn = Connection::session().await?;
                    let proxy = AgentControlProxy::new(&conn).await?;
                    info!("Requesting daemon restart...");
                    proxy.request_reload().await?;
                    Ok(())
                });
            }
            Commands::ListSessions => {
                return run_async(async {
                    let conn = Connection::session().await?;
                    let proxy = AgentPtyControlProxy::new(&conn).await?;
                    let sessions = proxy.list_detached_sessions().await?;
                    if sessions.is_empty() {
                        println!("No detached PTY sessions.");
                    } else {
                        println!("{:>8}  {:>8}  {}", "PID", "IDLE_S", "PANE_ID");
                        for (pid, pane_id, idle_secs) in sessions {
                            println!("{:>8}  {:>8}  {}", pid, idle_secs, pane_id);
                        }
                    }
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
