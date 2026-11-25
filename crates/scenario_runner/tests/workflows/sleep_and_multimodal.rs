//! E2E Workflow 11:
//! - Sleep Mode lifecycle flush (`flushed_watermark`), 0-token silence on terminal errors while
//!   sleeping, and `[WHILE_YOU_SLEPT]` catch-up pairing on Hard Wake.
//! - Cursor-ordered interleaved multimodal `[Text, Image, Text, Image]` wire payloads and
//!   post-update `--- UPDATE CHANGELOG CONTEXT ---` ("Read What's New") zero-tool execution.

use base64::Engine as _;
use boxxy_claw_protocol::{AgentStatus, ClawEngineEvent, ClawMessage, ImageAttachmentMeta};
use boxxy_db::store::Store;
use image::{ImageFormat, Rgb, RgbImage};
use scenario_runner::{MockStep, TestWorkspace};
use serial_test::serial;
use std::io::Cursor;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

fn encode_synthetic_image(format: ImageFormat, rgb: [u8; 3]) -> Vec<u8> {
    let img = RgbImage::from_pixel(4, 4, Rgb(rgb));
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, format)
        .expect("encode synthetic image");
    buf.into_inner()
}

#[tokio::test]
#[serial(boxxy_e2e)]
async fn sleep_mode_flushes_history_ignores_errors_and_summarizes_wake_delta() {
    let mut ws = TestWorkspace::new().await.expect("workspace");
    let mut pane = ws.spawn_pane("pane-sleep-1", 0).await.expect("spawn pane");
    pane.wait_ready(TIMEOUT).await.expect("ready");

    // 1. Complete Turn 1 so `SessionState.history` has unflushed messages above `flushed_watermark`.
    ws.llm.push_aux_text("Initial Workspace Audit");
    ws.llm
        .push_step(MockStep::text("Audited workspace Cargo manifest."));
    pane.send_query("Audit the workspace manifest.")
        .await
        .expect("send turn 1");
    pane.wait_for_diagnosis(TIMEOUT).await.expect("turn 1 done");

    // Wait for Turn-1 background title generation to consume its aux step first.
    let store = Store::new(ws.db.pool());
    let title_deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        if let Ok(Some(s)) = store.get_session(pane.session_id()).await
            && s.title.as_deref() == Some("Initial Workspace Audit")
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < title_deadline,
            "timed out waiting for Turn 1 session title"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // 2. Enter `Sleep`: must flush unflushed history to `mock-memory-model` at `flushed_watermark`!
    ws.llm.push_aux_text(
        "{\"facts\":[{\"key\":\"manifest_edition\",\"content\":\"Rust 2024 workspace\"}],\"summary\":\"Audited workspace manifest.\"}",
    );
    pane.send(ClawMessage::SleepToggle(true))
        .await
        .expect("enter sleep");
    pane.wait_for_status(&AgentStatus::Sleep, TIMEOUT)
        .await
        .expect("AgentStatus::Sleep");

    let ws_scope = ws.path().to_string_lossy().into_owned();
    let flush_deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        if let Ok(Some(mem)) = store
            .get_memory_by_key("manifest_edition", Some(&ws_scope))
            .await
        {
            assert_eq!(mem.content, "Rust 2024 workspace");
            break;
        }
        assert!(
            tokio::time::Instant::now() < flush_deadline,
            "entering Sleep must flush unflushed history via flush_history"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // 3. Failing commands while `Sleep` is active must emit ZERO `LazyErrorIndicator`s and spend ZERO tokens!
    let main_reqs_before_sleep_cmds = ws.llm.main_requests().len();
    let ev_cursor_before = pane.advance_cursor_to_end();

    pane.notify_command_finished(127, "unknown_cmd --check\nbash: unknown_cmd: not found\n")
        .await
        .expect("slept command 1");
    pane.notify_command_finished(0, "cargo test --workspace\ntest result: ok\n")
        .await
        .expect("slept command 2");

    // 4. Hard Wake (`SleepToggle(false)`) summarizes slept-through commands and returns to `Waiting`.
    pane.send(ClawMessage::SleepToggle(false))
        .await
        .expect("wake from sleep");
    pane.wait_for_status(&AgentStatus::Waiting, TIMEOUT)
        .await
        .expect("returned to Waiting after wake");

    assert_eq!(
        ws.llm.main_requests().len(),
        main_reqs_before_sleep_cmds,
        "commands during Sleep and <= 3-command wake summary must spend 0 main LLM calls"
    );
    let sleep_events = pane.events_since(ev_cursor_before);
    assert!(
        !sleep_events
            .iter()
            .any(|e| matches!(e, ClawEngineEvent::LazyErrorIndicator { visible: true, .. })),
        "Sleep mode must suppress LazyErrorIndicator on failing commands"
    );

    // 5. Post-wake Turn 2 receives `[WHILE_YOU_SLEPT]` with the exact slept-through commands
    //    AND clean alternation (no false `[Previous task was interrupted by the user]`!).
    ws.llm
        .push_step(MockStep::text("Welcome back — I see `unknown_cmd` failed and `cargo test` passed."));
    pane.send_query("What happened while you were sleeping?")
        .await
        .expect("send post-wake query");
    pane.wait_for_diagnosis(TIMEOUT)
        .await
        .expect("post-wake diagnosis");

    let post_wake_conv = ws
        .llm
        .main_requests()
        .last()
        .expect("post-wake req")
        .conversation_text();
    assert!(
        post_wake_conv.contains("[WHILE_YOU_SLEPT]")
            && post_wake_conv.contains("$ unknown_cmd --check (exit: 127)")
            && post_wake_conv.contains("$ cargo test --workspace (exit: 0)"),
        "post-wake turn must include [WHILE_YOU_SLEPT] command delta:\n{post_wake_conv}"
    );
    assert!(
        !post_wake_conv.contains("[Previous task was interrupted by the user]"),
        "[WHILE_YOU_SLEPT] must pair cleanly with WHILE_YOU_SLEPT_ACK without false interruption markers"
    );

    pane.shutdown().await;
    ws.teardown().await;
}

#[tokio::test]
#[serial(boxxy_e2e)]
async fn interleaved_multimodal_attachments_and_whats_new_zero_tool_workflow() {
    let mut ws = TestWorkspace::new().await.expect("workspace");

    // 1. Dynamically generate synthetic 4x4 PNG (inline attachment) + WebP (`@assets/diagram.webp`).
    let png_bytes = encode_synthetic_image(ImageFormat::Png, [40, 120, 220]);
    let webp_bytes = encode_synthetic_image(ImageFormat::WebP, [220, 90, 50]);
    let png_b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);

    let diagram_path = ws.file_path("assets/diagram.webp");
    tokio::fs::create_dir_all(diagram_path.parent().unwrap())
        .await
        .expect("mkdir assets");
    tokio::fs::write(&diagram_path, &webp_bytes)
        .await
        .expect("write synthetic diagram.webp");

    let mut pane = ws.spawn_pane("pane-mm-1", 0).await.expect("spawn pane");
    pane.wait_ready(TIMEOUT).await.expect("ready");

    ws.llm.push_steps([
        MockStep::text("Compared inline PNG chart with WebP diagram and spec snippet."),
        MockStep::text("Follow-up turn confirmed without re-sending heavy image blobs."),
    ]);

    let multimodal_prompt = "Compare [Image 1 · chart.png] with @assets/diagram.webp please.\n\n--- ATTACHMENTS ---\n\n**[File 1 · spec.toml]**\n```toml\nmode = \"strict\"\n```";
    pane.send_query_with_images(
        multimodal_prompt,
        vec![png_b64],
        vec![ImageAttachmentMeta {
            token: "[Image 1 · chart.png]".to_string(),
            label: "chart.png".to_string(),
        }],
    )
    .await
    .expect("send multimodal turn 1");
    pane.wait_for_diagnosis(TIMEOUT)
        .await
        .expect("multimodal turn 1 done");

    // Verify OpenAI wire `content` array preserves cursor-ordered `[text, image_url(png), text, image_url(webp), text(attachments + context)]`!
    let mm_reqs = ws.llm.main_requests();
    let user_msg = mm_reqs[0]
        .messages
        .iter()
        .rfind(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
        .expect("outgoing user message");
    let parts = user_msg
        .get("content")
        .and_then(|c| c.as_array())
        .expect("multimodal user content must serialize as an array of parts");
    let part_kinds: Vec<String> = parts
        .iter()
        .map(|p| {
            let ty = p.get("type").and_then(|t| t.as_str()).unwrap_or("unknown");
            if ty == "image_url" {
                let url = p
                    .get("image_url")
                    .and_then(|u| u.get("url"))
                    .and_then(|u| u.as_str())
                    .unwrap_or_default();
                let mime = url.split(';').next().unwrap_or(url);
                format!("image_url({mime})")
            } else {
                ty.to_string()
            }
        })
        .collect();
    assert_eq!(
        part_kinds,
        vec![
            "text".to_string(),
            "image_url(data:image/png)".to_string(),
            "text".to_string(),
            "image_url(data:image/webp)".to_string(),
            "text".to_string(),
        ],
        "unexpected multimodal content part ordering: {part_kinds:?}"
    );

    // Turn 2: history must retain `[Image 1 · chart.png]` and `@assets/diagram.webp` text anchors
    // without re-sending `image_url` base64 payloads!
    pane.send_query("Summarize the difference in one line.")
        .await
        .expect("send turn 2");
    pane.wait_for_diagnosis(TIMEOUT).await.expect("turn 2 done");

    let t2_req = &ws.llm.main_requests()[1];
    assert!(
        !serde_json::to_string(&t2_req.messages)
            .unwrap()
            .contains("image_url"),
        "settled history on Turn 2 must not re-send multi-MB base64 image_url payloads"
    );
    assert!(
        t2_req.conversation_text().contains("[Image 1 · chart.png]")
            && t2_req.conversation_text().contains("@assets/diagram.webp"),
        "settled history on Turn 2 must preserve inline image filename anchors"
    );

    // 2. Post-update `Read What's New` (`--- UPDATE CHANGELOG CONTEXT ---`):
    //    zero tools, zero `@file` preload, zero skills, full character persona + DB persistence!
    ws.write_file("src/module.rs", "pub const SHOULD_NOT_PRELOAD: u8 = 1;\n")
        .await
        .expect("write module.rs");
    ws.write_skill(
        "changelog-noise-skill",
        "---\nname: Changelog Noise Skill\ndescription: Synthetic skill\ntriggers:\n  - refactor\n---\nNOISE_SKILL_BODY\n",
    )
    .await
    .expect("write skill");

    let mut whats_new_pane = ws
        .spawn_pane("pane-whats-new", 1)
        .await
        .expect("spawn whats-new pane");
    whats_new_pane.wait_ready(TIMEOUT).await.expect("ready");

    ws.llm.push_aux_text("Boxxy Nightly Update Summary");
    ws.llm
        .push_step(MockStep::text("Key highlights: faster PTY ring buffer and cleaner diffs."));

    let changelog_prompt = "Summarize what's new in this update!\n\n--- UPDATE CHANGELOG CONTEXT ---\nCommits:\n- refactor(vte): optimize @src/module.rs parser\n";
    whats_new_pane
        .send_query(changelog_prompt)
        .await
        .expect("send whats_new prompt");
    let (wn_reply, _) = whats_new_pane
        .wait_for_diagnosis(TIMEOUT)
        .await
        .expect("whats_new diagnosis");
    assert!(wn_reply.contains("PTY ring buffer"));

    let wn_reqs = ws.llm.main_requests();
    let wn_req = wn_reqs.last().expect("whats_new req");
    assert!(
        wn_req.tools.is_empty(),
        "Read What's New turn must send zero tool schemas"
    );
    assert!(
        wn_req.system_prompt_text().contains("Full-Metal Kernel Alchemist"),
        "Read What's New turn must preserve selected character role & personality"
    );
    let wn_conv = wn_req.conversation_text();
    assert!(
        !wn_conv.contains("SHOULD_NOT_PRELOAD") && !wn_conv.contains("NOISE_SKILL_BODY"),
        "Read What's New turn must skip @file preloading and skill injection on changelog text"
    );

    pane.shutdown().await;
    whats_new_pane.shutdown().await;
    ws.teardown().await;
}
