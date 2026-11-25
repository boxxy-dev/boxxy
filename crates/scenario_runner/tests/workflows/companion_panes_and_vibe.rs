//! E2E Workflows 5 & 6:
//! - `1 Home Pane + Companion Terminal` (`spawn_agent` -> `pane-2` -> `terminal_exec` -> `read_pane_buffer` -> `send_keystrokes_to_pane` -> `close_agent`)
//! - Vibe Mode ("Scoped YOLO") guardrails (trusted workspace auto-approve vs out-of-workspace manual approval fallback vs `BLACKLIST.md` hard block)

use boxxy_claw_protocol::{ClawEngineEvent, ClawMessage};
use scenario_runner::{MockStep, TestWorkspace};
use serde_json::json;
use serial_test::serial;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
#[serial(boxxy_e2e)]
async fn home_pane_controls_companion_terminal_lifecycle_end_to_end() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    let mut home = ws.spawn_pane("pane-home", 0).await.expect("spawn home");
    home.wait_ready(TIMEOUT).await.expect("ready");
    home.set_auto_run_companion_commands(true);

    // Home pane sits at alias `pane-1`; spawning a vertical split mints `pane-2` synchronously.
    ws.llm.push_steps([
        MockStep::tool_call("spawn_agent", &json!({ "location": "vertical" })),
        MockStep::tool_call(
            "terminal_exec",
            &json!({
                "command": "printf 'COMPANION_BUILD_SUCCESS\\n'",
                "explanation": "Run build inside companion pane",
                "target_pane": "pane-2"
            }),
        ),
        MockStep::tool_call("read_pane_buffer", &json!({ "agent_name": "pane-2" })),
        MockStep::tool_call(
            "send_keystrokes_to_pane",
            &json!({
                "agent_name": "pane-2",
                "keys": "q\n"
            }),
        ),
        MockStep::tool_call("close_agent", &json!({ "agent_name": "pane-2" })),
        MockStep::text("Companion `pane-2` ran the build, reported success, and was closed."),
    ]);

    home.send_query("Open a vertical companion split, run the build there, inspect its output, send `q`, and close it.")
        .await
        .expect("send companion workflow query");

    let (summary, _) = home
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("companion turn complete");
    assert!(summary.contains("pane-2"));

    // Verify `read_pane_buffer` actually received `COMPANION_BUILD_SUCCESS` in the LLM transcript!
    let main_reqs = ws.llm.main_requests();
    assert!(
        main_reqs
            .last()
            .expect("final request")
            .flattened_message_text()
            .contains("COMPANION_BUILD_SUCCESS"),
        "agent must observe companion pane output via read_pane_buffer"
    );

    // Verify companion pane recorded the injected keystrokes and was marked closed.
    let comp = ws
        .companions
        .find_by_id_or_alias("pane-2")
        .expect("companion pane-2 recorded");
    assert_eq!(comp.keystrokes, vec!["q\n".to_string()]);
    assert!(comp.closed, "companion pane-2 must be closed by close_agent");

    home.shutdown().await;
    ws.teardown().await;
}

#[tokio::test]
#[serial(boxxy_e2e)]
async fn vibe_mode_auto_approves_trusted_paths_and_blocks_outside_or_blacklisted_paths() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    ws.write_file("src/safe.rs", "pub const ENABLED: bool = false;\n")
        .await
        .expect("write safe.rs");
    ws.write_file("secrets/token.txt", "TOP_SECRET_SYNTHETIC_VALUE\n")
        .await
        .expect("write blacklisted target");
    ws.set_blacklist_markdown("secrets\n")
        .await
        .expect("write BLACKLIST.md");
    ws.trust_workspace_for_vibe();

    let outside_dir = tempfile::Builder::new()
        .prefix("boxxy_e2e_outside_")
        .tempdir()
        .expect("outside dir");
    let outside_file = outside_dir.path().join("outside.txt");

    let mut pane = ws.spawn_pane("pane-vibe-1", 0).await.expect("spawn pane");
    pane.wait_ready(TIMEOUT).await.expect("ready");

    // Enable Vibe Mode on this pane.
    pane.send(ClawMessage::ToggleVibe(true))
        .await
        .expect("enable vibe");
    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::VibeStatusChanged(true) => Some(()),
        _ => None,
    })
    .await
    .expect("VibeStatusChanged(true)");

    // 1. In-workspace `replace_in_file` auto-approves WITHOUT needing `FileWriteReply`!
    // 2. Blacklisted `secrets/token.txt` read/write is hard-blocked by `BLACKLIST.md`!
    // 3. Out-of-workspace `file_write` falls back to `auto_approve: false` and blocks for manual approval!
    ws.llm.push_steps([
        MockStep::tool_call(
            "replace_in_file",
            &json!({
                "path": "src/safe.rs",
                "old_string": "false",
                "new_string": "true"
            }),
        ),
        MockStep::tool_call("file_read", &json!({ "path": "secrets/token.txt" })),
        MockStep::tool_call(
            "file_write",
            &json!({
                "path": outside_file.to_string_lossy(),
                "content": "untrusted write\n"
            }),
        ),
        MockStep::text("UNEXPECTED_IF_REJECTED"),
    ]);

    pane.send_query("Flip `src/safe.rs` to true, check `secrets/token.txt`, and write outside.")
        .await
        .expect("send vibe turn");

    // First, observe `auto_approve: true` on `src/safe.rs` followed by its `ToolResult` write completion.
    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::ProposeFileWrite {
            path,
            auto_approve: true,
            ..
        } if path.ends_with("src/safe.rs") => Some(()),
        _ => None,
    })
    .await
    .expect("in-workspace edit auto-approved in Vibe mode");

    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::ToolResult { tool_name, .. } if tool_name == "replace_in_file" => Some(()),
        _ => None,
    })
    .await
    .expect("replace_in_file finished writing to disk");

    assert_eq!(
        ws.read_file("src/safe.rs").await.unwrap(),
        "pub const ENABLED: bool = true;\n"
    );

    // Next, out-of-workspace `outside.txt` MUST halt with `auto_approve: false`!
    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::ProposeFileWrite {
            path,
            auto_approve: false,
            ..
        } if path.ends_with("outside.txt") => Some(()),
        _ => None,
    })
    .await
    .expect("out-of-workspace write must NOT auto-approve even in Vibe mode");

    assert!(
        !outside_file.exists(),
        "outside.txt must not be written before explicit user approval"
    );

    // Verify `secrets/token.txt` was blocked and never leaked into the LLM request messages!
    let reqs = ws.llm.main_requests();
    assert!(
        reqs.iter()
            .all(|r| !r.flattened_message_text().contains("TOP_SECRET_SYNTHETIC_VALUE")),
        "BLACKLIST.md must prevent secrets/token.txt contents from ever reaching the model"
    );

    // Reject the out-of-workspace write cleanly.
    pane.reply_file_write(false)
        .await
        .expect("reject outside write");
    pane.wait_for_next(TIMEOUT, |ev| match ev {
        ClawEngineEvent::ProposalResolved {
            approved: false, ..
        } => Some(()),
        _ => None,
    })
    .await
    .expect("outside write rejected");
    assert!(!outside_file.exists());

    pane.shutdown().await;
    ws.teardown().await;
}
