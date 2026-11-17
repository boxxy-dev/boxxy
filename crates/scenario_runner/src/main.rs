use anyhow::{Context, Result};
use std::env;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter_module("zbus", log::LevelFilter::Warn)
        .filter_module("rig", log::LevelFilter::Warn)
        .filter_module("rig_core", log::LevelFilter::Warn)
        .filter_module("rig_core::driver", log::LevelFilter::Error)
        .init();

    // Initialize settings (loads API keys and model config)
    boxxy_preferences::Settings::init();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        println!("Usage: scenario-runner <scenario.yml>");
        return Ok(());
    }

    let scenario_path = PathBuf::from(&args[1]);
    scenario_runner::run_scenario_from_file(&scenario_path)
        .await
        .with_context(|| format!("Failed to run scenario: {:?}", scenario_path))?;

    Ok(())
}
