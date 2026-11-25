//! E2E Workflow 7: Real Linux PTY + `/bin/bash` `OSC 7` / `OSC 133` shell integration,
//! `PtyRegistry` detach / 4 MB ring-buffer capture / reattach lifecycle, and headless
//! `boxxy-cli` `+doctor` & `+env` verification against an isolated `--config-dir`.

use boxxy_agent::pty::registry::DetachOutcome;
use boxxy_cli::{ParseOutcome, parse_and_dispatch};
use scenario_runner::TestWorkspace;
use serial_test::serial;
use std::process::ExitCode;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
#[serial(boxxy_e2e)]
async fn real_pty_shell_integration_detach_ring_buffer_and_reattach_workflow() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    let mut pty = ws
        .spawn_pty_bash("pane-pty-e2e", ":1.pty_owner")
        .await
        .expect("spawn PTY bash");

    // 1. Wait for interactive prompt and verify Boxxy's zero-config Bash integration
    //    emits `OSC 133;A` semantic prompt markers and `OSC 7` CWD reporting!
    pty.send_line("printf 'READY_MARKER_%d\\n' 1")
        .expect("send initial command");
    let initial_out = pty
        .wait_for_output("READY_MARKER_1", TIMEOUT)
        .await
        .expect("receive initial PTY output");
    assert!(
        initial_out.contains("\x1b]133;"),
        "expected OSC 133 semantic shell integration markers in PTY stream, got:\n{initial_out:?}"
    );
    assert!(
        initial_out.contains("\x1b]7;file://"),
        "expected OSC 7 CWD reporting marker in PTY stream"
    );

    // 2. Enable persistence, wait until a child job is demonstrably running (`shell_has_active_process`),
    //    and then simulate UI disconnect so `PtyRegistry` detaches rather than reaping an idle shell!
    pty.set_persistence(true).await;
    pty.send_line(
        "bash -c 'printf \"CHILD_%s\\n\" RUNNING_NOW; sleep 0.25; printf \"DETACHED_RING_%s\\n\" 9001'",
    )
    .expect("start active child job");
    pty.wait_for_output("CHILD_RUNNING_NOW", TIMEOUT)
        .await
        .expect("child process actively running before disconnect");

    let outcomes = pty.simulate_ui_disconnect().await;
    assert_eq!(outcomes, vec![(pty.pid(), DetachOutcome::Detached)]);

    let detached_list = ws.agent_state.pty_registry.list_detached().await;
    assert!(
        detached_list.iter().any(|(pid, _, _)| *pid == pty.pid()),
        "detached session must be listed in PtyRegistry"
    );

    // Allow the background command inside the detached shell to emit into the 4 MB ring buffer.
    tokio::time::sleep(Duration::from_millis(350)).await;

    // 3. Reattach and verify the detached output was preserved in `RingBuffer` AND live I/O resumes!
    let replayed = pty.reattach().await.expect("reattach to detached PTY");
    assert!(
        replayed.contains("DETACHED_RING_9001"),
        "ring buffer must replay output produced while UI was disconnected; got:\n{replayed:?}"
    );

    pty.send_line("printf 'LIVE_AFTER_REATTACH_%d\\n' 2")
        .expect("send post-reattach command");
    pty.wait_for_output("LIVE_AFTER_REATTACH_2", TIMEOUT)
        .await
        .expect("live I/O works after reattach");

    drop(pty);
    ws.teardown().await;
}

#[tokio::test]
#[serial(boxxy_e2e)]
async fn cli_doctor_and_env_actions_succeed_against_isolated_config_dir() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    let cfg_dir = ws.config_root().to_string_lossy().into_owned();
    let cwd = ws.path().to_path_buf();

    // `parse_and_dispatch` creates its own lightweight single-threaded Tokio runtime internally
    // inside `ActionContext::run_async`, so invoke it on `spawn_blocking` just like process startup.
    let cfg_for_doctor = cfg_dir.clone();
    let cwd_for_doctor = cwd.clone();
    let doctor_outcome = tokio::task::spawn_blocking(move || {
        parse_and_dispatch(
            [
                "boxxy-terminal",
                "+doctor",
                "--json",
                "--config-dir",
                &cfg_for_doctor,
            ],
            Some(&cwd_for_doctor),
        )
    })
    .await
    .expect("join +doctor");
    assert!(
        matches!(doctor_outcome, ParseOutcome::Exit(code) if code == ExitCode::SUCCESS || code == ExitCode::from(1)),
        "+doctor must exit cleanly without panicking or touching GTK"
    );

    let env_outcome = tokio::task::spawn_blocking(move || {
        parse_and_dispatch(
            [
                "boxxy-terminal",
                "+env",
                "--config-dir",
                &cfg_dir,
            ],
            Some(&cwd),
        )
    })
    .await
    .expect("join +env");
    assert!(
        matches!(env_outcome, ParseOutcome::Exit(code) if code == ExitCode::SUCCESS),
        "+env must exit with ExitCode::SUCCESS"
    );

    ws.teardown().await;
}
