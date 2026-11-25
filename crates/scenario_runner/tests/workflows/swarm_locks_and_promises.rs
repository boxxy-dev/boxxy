//! E2E Workflow 12: Collaborative Swarm Orchestration — `GLOBAL RADAR` peer discovery,
//! `LockTable` contention (`acquire_lock` / `release_lock`), cross-pane `delegate_task`,
//! and `EventBus` `subscribe_to_pane(event_type="output_match")` autonomous wake-up.

use boxxy_claw_protocol::AgentStatus;
use scenario_runner::{MockStep, TestWorkspace};
use serde_json::json;
use serial_test::serial;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
#[serial(boxxy_e2e)]
async fn swarm_lock_contention_delegation_and_output_subscription_workflow() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    ws.write_file("src/shared.rs", "pub fn queue_depth() -> usize { 64 }\n")
        .await
        .expect("write shared.rs");

    // Spawn Leader (`Niko Una`, `pane-1`) and Peer Worker (`Levi Kujo`, `pane-2`).
    let mut leader = ws
        .spawn_pane("pane-swarm-leader", 0)
        .await
        .expect("spawn leader");
    leader.wait_ready(TIMEOUT).await.expect("leader ready");

    let mut worker = ws
        .spawn_pane("pane-swarm-worker", 1)
        .await
        .expect("spawn worker");
    worker.wait_ready(TIMEOUT).await.expect("worker ready");

    // Strictly deterministic step sequence across Leader & Worker:
    // 1. Leader acquires `src/shared.rs` lock.
    // 2. Leader calls `delegate_task` -> blocks waiting on Worker (`pane-2`).
    // 3. Worker wakes, attempts `acquire_lock("src/shared.rs")` -> receives `LOCKED by agent` contention error!
    // 4. Worker returns delegated summary to Leader.
    // 5. Leader unblocks, releases lock on `src/shared.rs`, subscribes to `output_match` (`BENCH_DONE_[0-9]+`)
    //    on `pane-2`, and finishes its turn -> suspending autonomously in `AgentStatus::Sleep` (0 tokens).
    // 6. Updating `pane-2`'s terminal snapshot with `BENCH_DONE_42` fires the EventBus trigger and wakes Leader!
    ws.llm.push_steps([
        MockStep::tool_call("acquire_lock", &json!({ "resource": "src/shared.rs" })),
        MockStep::tool_call(
            "delegate_task",
            &json!({
                "agent_name": "pane-2",
                "prompt": "Check lock status on src/shared.rs and audit queue_depth."
            }),
        ),
        // Consumed by Worker (`pane-2`) while Leader awaits `delegate_task`:
        MockStep::tool_call("acquire_lock", &json!({ "resource": "src/shared.rs" })),
        MockStep::text("Worker report: `src/shared.rs` is locked by leader; read-only audit passed."),
        // Consumed by Leader once `delegate_task` resolves:
        MockStep::tool_call("release_lock", &json!({ "resource": "src/shared.rs" })),
        MockStep::tool_call(
            "subscribe_to_pane",
            &json!({
                "agent_name": "pane-2",
                "event_type": "output_match",
                "regex": "BENCH_DONE_[0-9]+"
            }),
        ),
        MockStep::text("Lock released; subscribed to `pane-2` benchmark completion and suspending."),
        // Consumed by Leader when woken automatically by `EventFilter::OutputMatch`:
        MockStep::text("Benchmark completed on `pane-2` (`BENCH_DONE_42`)!"),
    ]);

    leader
        .send_query("Coordinate `src/shared.rs` refactor with `pane-2` and watch for its benchmark.")
        .await
        .expect("send leader query");

    // Worker receives incoming delegated bubble from Leader first, then emits its own reply!
    let (incoming_bubble, _) = worker
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("worker received incoming delegated prompt bubble");
    assert!(incoming_bubble.contains("Check lock status on src/shared.rs"));

    let (worker_diag, _) = worker
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("worker finished delegated task");
    assert!(worker_diag.contains("locked by leader"));

    let (leader_suspend_reply, _) = leader
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("leader completed initial turn and subscribed");
    assert!(leader_suspend_reply.contains("suspending"));

    // Verify `GLOBAL RADAR` discovered `pane-2` AND Worker observed real `LOCKED by agent` contention!
    let reqs_before_wake = ws.llm.main_requests();
    assert!(
        reqs_before_wake[0].conversation_text().contains("GLOBAL RADAR")
            && reqs_before_wake[0].conversation_text().contains("pane-2"),
        "Leader's Turn 1 context must include peer `pane-2` in GLOBAL RADAR"
    );
    let ws_dir_str = ws.path().to_string_lossy().into_owned();
    assert!(
        reqs_before_wake[2]
            .conversation_text()
            .contains(&format!("CWD: `{ws_dir_str}`")),
        "newly spawned peer Worker pane must retain workspace CWD across Initialize + delegate_task:\n{}",
        reqs_before_wake[2].conversation_text()
    );
    assert!(
        reqs_before_wake[3]
            .conversation_text()
            .contains("LOCKED by agent"),
        "Worker's acquire_lock on held resource must report LOCKED contention:\n{}",
        reqs_before_wake[3].conversation_text()
    );
    assert!(
        reqs_before_wake[4]
            .conversation_text()
            .contains("Worker report: `src/shared.rs` is locked by leader"),
        "Leader must receive Worker's delegated response in delegate_task ToolResult"
    );

    // Non-matching output on `pane-2` must spend 0 tokens while Leader sleeps!
    let req_count_while_asleep = ws.llm.main_requests().len();
    worker
        .set_terminal_snapshot("$ cargo bench\ncompiling bench harness...\n")
        .await;
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(
        ws.llm.main_requests().len(),
        req_count_while_asleep,
        "non-matching terminal output must not wake subscribed sleeping agent"
    );

    // Matching output (`BENCH_DONE_42`) on `pane-2` wakes Leader via EventBus `OutputMatch`!
    worker
        .set_terminal_snapshot("$ cargo bench\nBENCH_DONE_42\n")
        .await;

    let (wake_reply, _) = leader
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("leader woken automatically on OutputMatch");
    assert!(wake_reply.contains("BENCH_DONE_42"));
    leader
        .wait_for_status(&AgentStatus::Waiting, TIMEOUT)
        .await
        .expect("leader returned to Waiting");

    leader.shutdown().await;
    worker.shutdown().await;
    ws.teardown().await;
}
