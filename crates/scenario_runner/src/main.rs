//! CLI entry point for running a quick headless Boxxy E2E smoke check from the terminal.

use anyhow::{Result, ensure};
use boxxy_claw_protocol::ClawEngineEvent;
use scenario_runner::{MockStep, TestWorkspace};
use serde_json::json;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    scenario_runner::config_env::init_test_logger();

    println!("Running Boxxy headless E2E smoke check (isolated config + MockLlmServer + PTY)...");

    let mut ws = TestWorkspace::new().await?;
    ws.write_file("src/parser.rs", "pub fn version() -> u32 {\n    1\n}\n")
        .await?;

    let mut pane = ws.spawn_pane("smoke-pane-1", 0).await?;
    pane.wait_ready(Duration::from_secs(5)).await?;

    ws.llm.push_steps([
        MockStep::tool_call(
            "replace_in_file",
            &json!({
                "path": "src/parser.rs",
                "old_string": "    1",
                "new_string": "    2"
            }),
        ),
        MockStep::text("Updated `version()` to return 2."),
    ]);

    pane.send_query("Please bump @src/parser.rs version from 1 to 2.")
        .await?;

    let proposed_path = pane
        .wait_for_next(Duration::from_secs(5), |ev| match ev {
            ClawEngineEvent::ProposeFileWrite {
                path,
                auto_approve: false,
                ..
            } => Some(path.clone()),
            _ => None,
        })
        .await?;
    ensure!(
        proposed_path.ends_with("src/parser.rs"),
        "unexpected proposed path: {proposed_path}"
    );

    pane.reply_file_write(true).await?;
    let (reply, _) = pane.wait_for_diagnosis(Duration::from_secs(5)).await?;
    ensure!(reply.contains("return 2"), "unexpected diagnosis: {reply}");
    ensure!(
        ws.read_file("src/parser.rs").await?.contains("    2"),
        "expected approved edit on disk"
    );

    // Verify `/undo` restores original file without spending LLM tokens.
    pane.send_query("/undo").await?;
    let undo_msg = pane
        .wait_for_next(Duration::from_secs(5), |ev| match ev {
            ClawEngineEvent::SystemMessage { text } => Some(text.clone()),
            _ => None,
        })
        .await?;
    ensure!(
        ws.read_file("src/parser.rs").await?.contains("    1"),
        "expected /undo to restore original contents (got message: {undo_msg})"
    );
    let persisted = boxxy_db::store::Store::new(ws.db.pool())
        .get_session(pane.session_id())
        .await?
        .and_then(|s| s.history_json)
        .unwrap_or_default();
    ensure!(
        persisted.contains("ran /undo to revert file changes"),
        "expected /undo exchange to persist in sessions.history_json"
    );

    // Verify real Linux PTY + Bash shell integration round-trip.
    let pty = ws.spawn_pty_bash("smoke-pty-1", ":1.smoke").await?;
    pty.send_line("printf 'BOXXY_PTY_OK_%s\\n' 42")?;
    pty.wait_for_output("BOXXY_PTY_OK_42", Duration::from_secs(5))
        .await?;

    pane.shutdown().await;
    ws.teardown().await;

    println!("OK: Headless E2E smoke check passed.");
    println!("Run the full multi-workflow suite with: cargo test -p scenario-runner");
    Ok(())
}
