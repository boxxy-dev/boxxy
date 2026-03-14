use std::path::Path;

fn main() {
    let resources_dir = Path::new("../../resources");

    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<gresources>
  <gresource prefix="/play/mii/Boxxy">
    <file compressed="true">style.css</file>
    <file compressed="true" alias="icons/play.mii.Boxxy.png">icons/play.mii.Boxxy.png</file>
    <file compressed="true" alias="icons/paper-plane-symbolic.svg">icons/paper-plane-symbolic.svg</file>
    <file compressed="true" alias="icons/edit-clear-all-symbolic.svg">icons/edit-clear-all-symbolic.svg</file>
    <file compressed="true" alias="icons/split-close-symbolic.svg">icons/split-close-symbolic.svg</file>
    <file compressed="true" alias="icons/split-horizontal-symbolic.svg">icons/split-horizontal-symbolic.svg</file>
    <file compressed="true" alias="icons/split-maximize-symbolic.svg">icons/split-maximize-symbolic.svg</file>
    <file compressed="true" alias="icons/split-unmaximize-symbolic.svg">icons/split-unmaximize-symbolic.svg</file>
    <file compressed="true" alias="icons/split-vertical-symbolic.svg">icons/split-vertical-symbolic.svg</file>
    <file compressed="true" alias="icons/appearance-symbolic.svg">icons/appearance-symbolic.svg</file>
    <file compressed="true" alias="icons/ai-slop-symbolic.svg">icons/ai-slop-symbolic.svg</file>
    <file compressed="true" alias="icons/visual-bell-symbolic.svg">icons/visual-bell-symbolic.svg</file>
    <file compressed="true" alias="icons/brain-symbolic.svg">icons/brain-symbolic.svg</file>
    <file compressed="true" alias="icons/chat-symbolic.svg">icons/chat-symbolic.svg</file>
    <file compressed="true" alias="icons/chat-none-symbolic.svg">icons/chat-none-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxyclaw.svg">icons/boxxyclaw.svg</file>
    <file compressed="true" alias="icons/running-symbolic.svg">icons/running-symbolic.svg</file>
    <file compressed="true" alias="icons/walking2-symbolic.svg">icons/walking2-symbolic.svg</file>
    <file compressed="true" alias="icons/bug-symbolic.svg">icons/bug-symbolic.svg</file>
    <file compressed="true" alias="prompts/ai_chat.md">prompts/ai_chat.md</file>
    <file compressed="true" alias="prompts/claw.md">prompts/claw.md</file>
    <file compressed="true" alias="prompts/boxxy_app_generator.md">prompts/boxxy_app_generator.md</file>
    <file compressed="true" alias="prompts/memory_expansion.md">prompts/memory_expansion.md</file>
    <file compressed="true" alias="prompts/memory_flush.md">prompts/memory_flush.md</file>
    <file compressed="true" alias="prompts/memory_summarizer.md">prompts/memory_summarizer.md</file>
    <file compressed="true" alias="ui/preferences.ui">ui/preferences.ui</file>
  </gresource>
</gresources>
"#.to_string();

    // Only write if changed — avoids spurious rebuild loops.
    let xml_path = resources_dir.join("resources.gresource.xml");
    let existing = std::fs::read_to_string(&xml_path).unwrap_or_default();
    if existing != xml {
        std::fs::write(&xml_path, &xml).expect("failed to write resources.gresource.xml");
    }

    // Rerun if any palette file is added/removed/modified.
    println!("cargo:rerun-if-changed=../../resources/style.css");
    println!("cargo:rerun-if-changed=../../resources/icons/play.mii.Boxxy.png");
    println!("cargo:rerun-if-changed=../../resources/icons/paper-plane-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/edit-clear-all-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/split-close-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/split-horizontal-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/split-maximize-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/split-unmaximize-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/split-vertical-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/appearance-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/ai-slop-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/visual-bell-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/brain-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/chat-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/chat-none-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxyclaw.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/running-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/walking2-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/bug-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/prompts/ai_chat.md");
    println!("cargo:rerun-if-changed=../../resources/prompts/claw.md");
    println!("cargo:rerun-if-changed=../../resources/prompts/boxxy_app_generator.md");
    println!("cargo:rerun-if-changed=../../resources/prompts/memory_expansion.md");
    println!("cargo:rerun-if-changed=../../resources/prompts/memory_flush.md");
    println!("cargo:rerun-if-changed=../../resources/prompts/memory_summarizer.md");
    println!("cargo:rerun-if-changed=../../resources/ui/preferences.ui");

    glib_build_tools::compile_resources(
        &["../../resources"],
        "../../resources/resources.gresource.xml",
        "compiled.gresource",
    );
}
