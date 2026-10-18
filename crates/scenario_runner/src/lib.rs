pub mod runner;
pub mod schema;

use anyhow::Result;
use std::path::Path;

pub async fn run_scenario_from_file(path: impl AsRef<Path>) -> Result<()> {
    let content = tokio::fs::read_to_string(path).await?;
    #[allow(deprecated)]
    let scenario: schema::Scenario = serde_yml::from_str(&content)?;

    let mut runner = runner::ScenarioRunner::new(scenario).await?;
    runner.run().await
}
