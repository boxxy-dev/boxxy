//! Modular `+action` execution layer.
//!
//! Each `+action` lives in its own focused module and receives an [`ActionContext`].
//! Future headless AI workflows (such as `actions/claw.rs` for `+claw`) plug directly
//! into [`ActionContext`] to connect to `boxxy-agent` over session D-Bus without
//! initializing GTK or opening windows.

pub mod agent;
pub mod completions;
pub mod doctor;
pub mod env_cmd;
pub mod version;

use crate::output::{OutputFormat, TermStyle};
use anyhow::{Context, Result};
use boxxy_sys_utils::CliDevOverrides;
use std::process::ExitCode;

/// Shared execution context passed to every `+action`.
pub struct ActionContext {
    pub bin_name: String,
    pub dev: CliDevOverrides,
    pub style: TermStyle,
}

impl ActionContext {
    #[must_use]
    pub fn new(bin_name: String, dev: CliDevOverrides) -> Self {
        Self {
            bin_name,
            dev,
            style: TermStyle::stdout(),
        }
    }

    /// Applies `--dev`, `--instance`, `--ephemeral`, `--config-dir`, and `--debug-context`
    /// to `boxxy-sys-utils` so path and D-Bus bus-name resolution match GUI launches.
    pub fn apply_dev_overrides(&self) {
        boxxy_sys_utils::configure_dev_options(self.dev.clone());
    }

    /// Runs a short-lived async future (e.g. D-Bus RPC to `boxxy-agent`) on a lightweight
    /// single-threaded Tokio runtime without spawning multi-thread worker pools.
    pub fn run_async<F, T>(&self, fut: F) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>>,
    {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("Failed to initialize CLI async runtime")?;
        rt.block_on(fut)
    }

    #[must_use]
    pub const fn format(&self, json: bool) -> OutputFormat {
        OutputFormat::from_json_flag(json)
    }
}

/// Converts an `anyhow::Result<ExitCode>` from an action into a clean process exit code,
/// printing human-readable errors to `stderr`.
pub fn finish_action(res: Result<ExitCode>, style: TermStyle) -> ExitCode {
    match res {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{} {err:#}", style.red("error:"));
            ExitCode::FAILURE
        }
    }
}
