//! E2E Workflow 10: Tiered Skill Injection (`SkillRegistry` Turn-1 FTS5 sync, `activate_skill`,
//! `disabled_skills` filtering) and Multi-Tier Memory Recall (`PINNED`, `BOOTSTRAP`,
//! `[PROVISIONAL]` candidates, and `memory_store` persistence).

use boxxy_claw_protocol::TurnTraceItem;
use boxxy_db::store::Store;
use boxxy_preferences::Settings;
use scenario_runner::{MockStep, TestWorkspace};
use serde_json::json;
use serial_test::serial;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
#[serial(boxxy_e2e)]
async fn skills_semantic_injection_activation_and_memory_recall_workflow() {
    let mut ws = TestWorkspace::new().await.expect("workspace");

    Settings::update(|s| {
        if !s.disabled_skills.iter().any(|d| d == "disabled-helper") {
            s.disabled_skills.push("disabled-helper".to_string());
        }
    });

    ws.write_skill(
        "container-inspector",
        "---\nname: Container Inspector\ndescription: Inspect OCI container layers and image manifests\ntriggers:\n  - podman\n  - oci\n---\nOCI_LAYER_INSPECTION_PROTOCOL: run skopeo inspect before unpacking layers.\n",
    )
    .await
    .expect("write container-inspector skill");

    ws.write_skill(
        "archive-packager",
        "---\nname: Archive Packager\ndescription: Create deterministic tarball archives\ntriggers:\n  - tarball\n---\nREPRODUCIBLE_TARBALL_FLAGS: pass --sort=name and --mtime=@0.\n",
    )
    .await
    .expect("write archive-packager skill");

    ws.write_skill(
        "disabled-helper",
        "---\nname: Disabled Helper\ndescription: Disabled workflow helper\ntriggers:\n  - podman\n---\nDISABLED_SKILL_SECRET_BODY_NEVER_LOADED\n",
    )
    .await
    .expect("write disabled-helper skill");

    // Seed workspace-scoped pinned, bootstrap, and provisional candidate memories in SQLite.
    let ws_scope = ws.path().to_string_lossy().into_owned();
    let store = Store::new(ws.db.pool());
    store
        .add_memory(
            "pinned_lint_rule",
            Some(&ws_scope),
            "Always run clippy with --all-targets",
            Some("preference"),
            true,
            true,
        )
        .await
        .expect("seed pinned memory");
    store
        .add_memory(
            "package_manager",
            Some(&ws_scope),
            "pacman",
            Some("system"),
            true,
            false,
        )
        .await
        .expect("seed bootstrap memory");
    store
        .add_memory(
            "container_registry_mirror",
            Some(&ws_scope),
            "Mirror OCI builds through registry.example.com",
            Some("dreamed"),
            true,
            false,
        )
        .await
        .expect("seed dreamed verified fact");
    store
        .upsert_dream_candidate(
            "podman_network_mode",
            Some(&ws_scope),
            "Podman containers use pasta rootless networking",
            0.5,
        )
        .await
        .expect("seed provisional candidate");

    let mut pane = ws.spawn_pane("pane-skills-1", 0).await.expect("spawn pane");
    pane.wait_ready(TIMEOUT).await.expect("ready");

    // Turn 1: FTS5 + trigger matches `Container Inspector` (full body), lists `Archive Packager`
    // compactly in Toolbox, skips `disabled-helper`, recalls all 3 memory tiers, activates
    // `Archive Packager` on demand, and stores a new durable preference via `memory_store`.
    ws.llm.push_steps([
        MockStep::tool_call("activate_skill", &json!({ "name": "Archive Packager" })),
        MockStep::tool_call(
            "memory_store",
            &json!({
                "key": "container_runtime",
                "content": "Prefer rootless podman for local container builds",
                "project_path": ws_scope
            }),
        ),
        MockStep::text("Inspected OCI layers, activated Archive Packager, and saved runtime preference."),
    ]);

    pane.send_query("Inspect podman container layers and package the result.")
        .await
        .expect("send turn 1");
    let (reply_1, trace_1) = pane.wait_for_diagnosis(TIMEOUT).await.expect("turn 1 done");
    assert!(reply_1.contains("Archive Packager"));
    assert!(
        trace_1.iter().any(
            |t| matches!(t, TurnTraceItem::Skill { name } if name == "Container Inspector")
        ),
        "triggered skill must be recorded in TurnTraceItem::Skill, got: {trace_1:?}"
    );

    let reqs_t1 = ws.llm.main_requests();
    assert_eq!(reqs_t1.len(), 3);
    let first_conv = reqs_t1[0].conversation_text();
    assert!(
        first_conv.contains("--- ACTIVE SKILLS ---")
            && first_conv.contains("OCI_LAYER_INSPECTION_PROTOCOL"),
        "top-1 matching skill must be injected in full on Turn 1:\n{first_conv}"
    );
    assert!(
        first_conv.contains("--- AVAILABLE SKILLS (TOOLBOX - Compact) ---")
            && first_conv.contains("Archive Packager")
            && !first_conv.contains("REPRODUCIBLE_TARBALL_FLAGS"),
        "non-active enabled skills must be listed compactly without their full body on step 1"
    );
    assert!(
        !first_conv.contains("DISABLED_SKILL_SECRET_BODY_NEVER_LOADED")
            && !first_conv.contains("Disabled Helper"),
        "skills listed in Settings.disabled_skills must never be loaded or advertised"
    );
    assert!(
        first_conv.contains("--- PINNED FACTS (CRITICAL) ---")
            && first_conv.contains("pinned_lint_rule: Always run clippy with --all-targets"),
        "pinned memories must be unconditionally injected"
    );
    assert!(
        first_conv.contains("--- SYSTEM ENVIRONMENT ---")
            && first_conv.contains("package_manager: pacman"),
        "bootstrap system facts must be unconditionally injected"
    );
    assert!(
        first_conv.contains("--- PROVISIONAL CANDIDATES (treat with skepticism) ---")
            && first_conv.contains("[PROVISIONAL] podman_network_mode"),
        "unverified candidate facts matching FTS5 must surface under [PROVISIONAL]"
    );

    // Step 2 (after `activate_skill`) must contain the dynamically loaded `Archive Packager` body!
    assert!(
        reqs_t1[1]
            .conversation_text()
            .contains("REPRODUCIBLE_TARBALL_FLAGS"),
        "activate_skill must return full skill instructions to the model"
    );

    // Turn 2: Querying about container runtime & registry recalls both the newly `memory_store`d
    // `preference` fact (under SYSTEM ENVIRONMENT) and the `dreamed` verified fact (via FTS5)!
    ws.llm
        .push_step(MockStep::text("Using rootless podman and registry.example.com."));
    pane.send_query("Which container runtime and registry should we use for builds?")
        .await
        .expect("send turn 2");
    pane.wait_for_diagnosis(TIMEOUT).await.expect("turn 2 done");

    let t2_conv = ws
        .llm
        .main_requests()
        .last()
        .expect("turn 2 req")
        .conversation_text();
    assert!(
        t2_conv.contains("container_runtime: Prefer rootless podman for local container builds"),
        "memory_store preference from Turn 1 must be injected on Turn 2:\n{t2_conv}"
    );
    assert!(
        t2_conv.contains("--- RELEVANT PREFERENCES & FACTS ---")
            && t2_conv.contains("container_registry_mirror: Mirror OCI builds through registry.example.com"),
        "dreamed verified memory must be recalled via FTS5 under RELEVANT PREFERENCES & FACTS:\n{t2_conv}"
    );

    pane.shutdown().await;
    ws.teardown().await;
}
