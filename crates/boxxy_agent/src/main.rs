use anyhow::{Context, Result};
use boxxy_agent::{cli_ops, daemon};
use clap::{Parser, Subcommand};
use log::info;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "boxxy-agent",
    version = concat!(env!("CARGO_PKG_VERSION"), "+", env!("GIT_HASH")),
    about = "Host-side privileged daemon for Boxxy Terminal",
    long_about = "Host-side privileged daemon for Boxxy Terminal.\n\n\
        Normally started and upgraded automatically by `boxxy-terminal`.\n\
        For everyday inspection and control from the CLI, prefer `boxxy-terminal +agent <subcommand>`."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Run headlessly: double-fork into the background and exit immediately.
    #[arg(long)]
    background: bool,

    /// Run as an isolated development instance (`dev.boxxy.BoxxyAgent.Devel`).
    #[arg(long, global = true)]
    dev: bool,

    /// Run as a named isolated instance (`dev.boxxy.BoxxyAgent.Devel.i_<slug>`).
    #[arg(long, global = true, value_name = "SLUG")]
    instance: Option<String>,

    /// Use a clean ephemeral configuration directory with factory defaults.
    #[arg(long, global = true)]
    ephemeral: bool,

    /// Refresh dev configuration files from the stable Boxxy installation before starting.
    #[arg(long, global = true)]
    sync_settings: bool,

    /// Override the active configuration and SQLite directory (must be an absolute path).
    #[arg(long, global = true, value_name = "ABS_PATH")]
    config_dir: Option<PathBuf>,

    /// Log full LLM system prompts, history, and tool payloads on `model-context`.
    #[arg(long, global = true)]
    debug_context: bool,

    /// Skip the singleton check (for debugging only).
    #[arg(long, hide = true)]
    no_singleton: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start the daemon in the foreground (default when no subcommand is given).
    Start,
    /// Stop the running daemon gracefully after flushing telemetry.
    Stop,
    /// Restart the running daemon in-place (`exec()`), preserving child shells.
    Restart,
    /// List detached PTY sessions kept alive in the background.
    #[command(visible_alias = "sessions")]
    ListSessions,
    /// Show daemon version, power state, Dream Cycle state, and active session counts.
    Status,
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter_module("zbus", log::LevelFilter::Warn)
        .filter_module("tracing", log::LevelFilter::Warn)
        .filter_module("sqlx", log::LevelFilter::Warn)
        .filter_module("rig", log::LevelFilter::Warn)
        .filter_module("rmcp", log::LevelFilter::Warn)
        .init();

    let cli = Cli::parse();
    boxxy_sys_utils::configure_dev_options(boxxy_sys_utils::CliDevOverrides {
        dev: cli.dev,
        instance: cli.instance.clone(),
        ephemeral: cli.ephemeral,
        sync_settings: cli.sync_settings,
        config_dir: cli.config_dir.clone(),
        debug_context: cli.debug_context,
    });

    // Handle subcommands that talk to an already-running daemon over D-Bus
    if let Some(cmd) = &cli.command {
        match cmd {
            Commands::Stop => {
                return run_async(async {
                    info!(
                        "Requesting daemon stop ({})...",
                        boxxy_sys_utils::agent_bus_name()
                    );
                    cli_ops::request_stop().await
                });
            }
            Commands::Restart => {
                return run_async(async {
                    info!(
                        "Requesting daemon restart ({})...",
                        boxxy_sys_utils::agent_bus_name()
                    );
                    cli_ops::request_restart().await
                });
            }
            Commands::ListSessions => {
                return run_async(async {
                    let sessions = cli_ops::query_detached_sessions().await?;
                    cli_ops::print_sessions_human(&sessions);
                    Ok(())
                });
            }
            Commands::Status => {
                return run_async(async {
                    let report = cli_ops::query_status().await?;
                    cli_ops::print_status_human(&report);
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

    // Prepare isolated dev or ephemeral directory (if active) before hydrating Settings.
    boxxy_sys_utils::prepare_dev_environment();

    // Hydrate the settings cache from disk BEFORE starting the runtime —
    // the claw engine reads model selection from `Settings::load()`, which
    // is backed by a process-global cache.
    boxxy_preferences::Settings::init();

    // Start the async runtime ONLY in the final daemon process (after any fork).
    run_async(async move {
        let resolved_config_dir = boxxy_sys_utils::get_config_dir();
        info!(
            "Boxxy Agent starting (v{}, bus={})...",
            env!("CARGO_PKG_VERSION"),
            boxxy_sys_utils::agent_bus_name()
        );
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

/// Helper to run a future to completion using a dedicated Tokio runtime.
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
                    use std::os::unix::io::{AsRawFd, IntoRawFd};
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
                        if fd <= libc::STDERR_FILENO {
                            let _ = dev_null.into_raw_fd();
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
