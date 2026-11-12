use crate::actions::ActionContext;
use crate::output::{OutputFormat, print_json};
use anyhow::Result;
use boxxy_agent::cli_ops;
use clap::{Args, Subcommand};
use serde::Serialize;
use std::process::ExitCode;

#[derive(Args, Debug, Clone)]
#[command(about = "Inspect or control the host-side boxxy-agent D-Bus daemon")]
pub struct AgentArgs {
    #[command(subcommand)]
    pub command: Option<AgentCommand>,

    /// Emit output as JSON.
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCommand {
    /// Show daemon version, power state, Dream Cycle status, and active session counts (default).
    Status,
    /// List detached PTY shells kept alive in the background by boxxy-agent.
    #[command(visible_alias = "list-sessions")]
    Sessions,
    /// Gracefully stop the running daemon after flushing telemetry.
    Stop,
    /// Reload the running daemon binary in-place (`exec()`), preserving child shells.
    Restart,
}

#[derive(Serialize)]
struct AckResponse {
    bus_name: String,
    action: &'static str,
    ok: bool,
}

pub fn run(ctx: &ActionContext, args: &AgentArgs) -> Result<ExitCode> {
    ctx.apply_dev_overrides();
    let sub = args.command.unwrap_or(AgentCommand::Status);
    let fmt = ctx.format(args.json);

    match sub {
        AgentCommand::Status => {
            let report = ctx.run_async(cli_ops::query_status())?;
            match fmt {
                OutputFormat::Json => print_json(&report)?,
                OutputFormat::Human => cli_ops::print_status_human(&report),
            }
        }
        AgentCommand::Sessions => {
            let sessions = ctx.run_async(cli_ops::query_detached_sessions())?;
            match fmt {
                OutputFormat::Json => print_json(&sessions)?,
                OutputFormat::Human => cli_ops::print_sessions_human(&sessions),
            }
        }
        AgentCommand::Stop => {
            ctx.run_async(cli_ops::request_stop())?;
            let bus_name = boxxy_sys_utils::agent_bus_name().to_string();
            match fmt {
                OutputFormat::Json => print_json(&AckResponse {
                    bus_name,
                    action: "stop",
                    ok: true,
                })?,
                OutputFormat::Human => {
                    println!(
                        "{} Requested graceful stop for {bus_name}.",
                        ctx.style.green("✓")
                    );
                }
            }
        }
        AgentCommand::Restart => {
            ctx.run_async(cli_ops::request_restart())?;
            let bus_name = boxxy_sys_utils::agent_bus_name().to_string();
            match fmt {
                OutputFormat::Json => print_json(&AckResponse {
                    bus_name,
                    action: "restart",
                    ok: true,
                })?,
                OutputFormat::Human => {
                    println!(
                        "{} Requested in-place reload for {bus_name}.",
                        ctx.style.green("✓")
                    );
                }
            }
        }
    }

    Ok(ExitCode::SUCCESS)
}
