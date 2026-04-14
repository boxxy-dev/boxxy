use std::path::Path;

fn main() {
    let resources_dir = Path::new("../../resources");

    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<gresources>
  <gresource prefix="/dev/boxxy/BoxxyTerminal">
    <file compressed="true">style.css</file>
    <file compressed="true" alias="icons/dev.boxxy.BoxxyTerminal.png">icons/dev.boxxy.BoxxyTerminal.png</file>
    <file compressed="true" alias="icons/boxxy-paper-plane-symbolic.svg">icons/boxxy-paper-plane-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-edit-clear-all-symbolic.svg">icons/boxxy-edit-clear-all-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-split-close-symbolic.svg">icons/boxxy-split-close-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-split-horizontal-symbolic.svg">icons/boxxy-split-horizontal-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-split-maximize-symbolic.svg">icons/boxxy-split-maximize-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-split-unmaximize-symbolic.svg">icons/boxxy-split-unmaximize-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-split-vertical-symbolic.svg">icons/boxxy-split-vertical-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-appearance-symbolic.svg">icons/boxxy-appearance-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-ai-slop-symbolic.svg">icons/boxxy-ai-slop-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-visual-bell-symbolic.svg">icons/boxxy-visual-bell-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-timer-symbolic.svg">icons/boxxy-timer-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-chat-symbolic.svg">icons/boxxy-chat-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-comic-bubble-symbolic.svg">icons/boxxy-comic-bubble-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxyclaw.svg">icons/boxxyclaw.svg</file>
    <file compressed="true" alias="icons/boxxy-boxxyclaw-symbolic.svg">icons/boxxy-boxxyclaw-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-running-symbolic.svg">icons/boxxy-running-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-walking2-symbolic.svg">icons/boxxy-walking2-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-sleep-symbolic.svg">icons/boxxy-sleep-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-bug-symbolic.svg">icons/boxxy-bug-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-user-bookmarks-symbolic.svg">icons/boxxy-user-bookmarks-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-external-link-symbolic.svg">icons/boxxy-external-link-symbolic.svg</file>
    <file compressed="true" alias="icons/console.svg">icons/console.svg</file>
    <file compressed="true" alias="icons/python.svg">icons/python.svg</file>
    <file compressed="true" alias="icons/boxxy-dock-left-symbolic.svg">icons/boxxy-dock-left-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-bookmark-filled-symbolic.svg">icons/boxxy-bookmark-filled-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-edit-find-symbolic.svg">icons/boxxy-edit-find-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-globe-symbolic.svg">icons/boxxy-globe-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-up-symbolic.svg">icons/boxxy-up-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-down-symbolic.svg">icons/boxxy-down-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-settings-symbolic.svg">icons/boxxy-settings-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-cross-small-symbolic.svg">icons/boxxy-cross-small-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-right-symbolic.svg">icons/boxxy-right-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-plus-symbolic.svg">icons/boxxy-plus-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-minus-symbolic.svg">icons/boxxy-minus-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-entry-clear-symbolic.svg">icons/boxxy-entry-clear-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-build-circle-symbolic.svg">icons/boxxy-build-circle-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-dialog-warning-symbolic.svg">icons/boxxy-dialog-warning-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-document-edit-symbolic.svg">icons/boxxy-document-edit-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-edit-clear-symbolic.svg">icons/boxxy-edit-clear-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-edit-copy-symbolic.svg">icons/boxxy-edit-copy-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-folder-open-symbolic.svg">icons/boxxy-folder-open-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-media-playback-start-symbolic.svg">icons/boxxy-media-playback-start-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-media-playback-stop-symbolic.svg">icons/boxxy-media-playback-stop-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-object-select-symbolic.svg">icons/boxxy-object-select-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-up-arrow-in-a-star-symbolic.svg">icons/boxxy-up-arrow-in-a-star-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-user-trash-symbolic.svg">icons/boxxy-user-trash-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-window-close-symbolic.svg">icons/boxxy-window-close-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-open-menu-symbolic.svg">icons/boxxy-open-menu-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-brain-symbolic.svg">icons/boxxy-brain-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-window-new-symbolic.svg">icons/boxxy-window-new-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-chat-none-symbolic.svg">icons/boxxy-chat-none-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-view-pin-symbolic.svg">icons/boxxy-view-pin-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-lock-symbolic.svg">icons/boxxy-lock-symbolic.svg</file>
    <file compressed="true" alias="icons/boxxy-spinner.gpa">icons/boxxy-spinner.gpa</file>
    <file compressed="true" alias="prompts/ai_chat.md">prompts/ai_chat.md</file>
    <file compressed="true" alias="prompts/claw.md">prompts/claw.md</file>
    <file compressed="true" alias="prompts/bookmark_generator.md">prompts/bookmark_generator.md</file>
    <file compressed="true" alias="prompts/memory_expansion.md">prompts/memory_expansion.md</file>
    <file compressed="true" alias="prompts/memory_flush.md">prompts/memory_flush.md</file>
    <file compressed="true" alias="prompts/memory_summarizer.md">prompts/memory_summarizer.md</file>
    <file compressed="true" alias="prompts/privacy_policy.md">prompts/privacy_policy.md</file>
    <file compressed="true" alias="ui/preferences.ui">ui/preferences.ui</file>
    <file compressed="true" alias="ui/widgets/notification_pill.ui">ui/widgets/notification_pill.ui</file>
    <file compressed="true" alias="ui/widgets/notification_details.ui">ui/widgets/notification_details.ui</file>
    <file compressed="true" alias="sounds/task.wav">sounds/task.wav</file>
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
    println!("cargo:rerun-if-changed=../../resources/icons/dev.boxxy.BoxxyTerminal.png");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-paper-plane-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-edit-clear-all-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-split-close-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-split-horizontal-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-split-maximize-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-split-unmaximize-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-split-vertical-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-appearance-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-ai-slop-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-visual-bell-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-timer-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-brain-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-chat-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-chat-none-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-comic-bubble-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-view-pin-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-lock-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-spinner.gpa");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxyclaw.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-running-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-walking2-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-sleep-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-bug-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-user-bookmarks-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-external-link-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/console.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/python.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-dock-left-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-bookmark-filled-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-edit-find-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-globe-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-up-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-down-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-settings-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-cross-small-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-right-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-plus-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-minus-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-entry-clear-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-build-circle-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-dialog-warning-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-document-edit-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-edit-clear-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-edit-copy-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-folder-open-symbolic.svg");
    println!(
        "cargo:rerun-if-changed=../../resources/icons/boxxy-media-playback-start-symbolic.svg"
    );
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-media-playback-stop-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-object-select-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-up-arrow-in-a-star-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-user-trash-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-window-close-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/icons/boxxy-open-menu-symbolic.svg");
    println!("cargo:rerun-if-changed=../../resources/prompts/ai_chat.md");
    println!("cargo:rerun-if-changed=../../resources/prompts/claw.md");
    println!("cargo:rerun-if-changed=../../resources/prompts/bookmark_generator.md");
    println!("cargo:rerun-if-changed=../../resources/prompts/memory_expansion.md");
    println!("cargo:rerun-if-changed=../../resources/prompts/memory_flush.md");
    println!("cargo:rerun-if-changed=../../resources/prompts/memory_summarizer.md");
    println!("cargo:rerun-if-changed=../../resources/prompts/privacy_policy.md");
    println!("cargo:rerun-if-changed=../../resources/ui/preferences.ui");
    println!("cargo:rerun-if-changed=../../resources/ui/widgets/notification_pill.ui");
    println!("cargo:rerun-if-changed=../../resources/ui/widgets/notification_details.ui");
    println!("cargo:rerun-if-changed=../../resources/sounds/task.wav");

    glib_build_tools::compile_resources(
        &["../../resources"],
        "../../resources/resources.gresource.xml",
        "compiled.gresource",
    );
}
