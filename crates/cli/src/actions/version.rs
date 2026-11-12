use crate::actions::ActionContext;
use crate::output::{OutputFormat, print_json};
use anyhow::Result;
use clap::Args;
use serde::Serialize;
use std::process::ExitCode;

#[derive(Args, Debug, Clone, Default)]
#[command(about = "Print Boxxy Terminal version and build information")]
pub struct VersionArgs {
    /// Emit version metadata as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Serialize)]
struct VersionReport {
    name: &'static str,
    version: &'static str,
    git_hash: &'static str,
    full_version: &'static str,
    channel: &'static str,
    arch: &'static str,
    os: &'static str,
    self_update_available: bool,
}

#[must_use]
pub const fn full_version_str() -> &'static str {
    concat!(env!("CARGO_PKG_VERSION"), "+", env!("GIT_HASH"))
}

pub fn run(ctx: &ActionContext, args: &VersionArgs) -> Result<ExitCode> {
    ctx.apply_dev_overrides();

    let channel = if boxxy_sys_utils::is_flatpak() {
        "flatpak"
    } else if cfg!(feature = "disable-self-update") {
        "system-package"
    } else {
        "native"
    };

    let report = VersionReport {
        name: "boxxy-terminal",
        version: env!("CARGO_PKG_VERSION"),
        git_hash: env!("GIT_HASH"),
        full_version: full_version_str(),
        channel,
        arch: std::env::consts::ARCH,
        os: std::env::consts::OS,
        self_update_available: boxxy_sys_utils::can_self_update(),
    };

    match ctx.format(args.json) {
        OutputFormat::Json => print_json(&report)?,
        OutputFormat::Human => {
            println!(
                "{} {} ({}, {}/{})",
                ctx.style.bold("Boxxy Terminal"),
                report.full_version,
                report.channel,
                report.os,
                report.arch
            );
        }
    }

    Ok(ExitCode::SUCCESS)
}
