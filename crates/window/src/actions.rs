use crate::state::AppInput;
use async_channel::Sender;
use gtk4::gio;
use gtk4::prelude::*;

pub fn setup_actions(window: &libadwaita::ApplicationWindow, sender: Sender<AppInput>) {
    let action_group = gio::SimpleActionGroup::new();

    macro_rules! add_action {
        ($name:expr, $input:expr) => {
            let action = gio::SimpleAction::new($name, None);
            let s = sender.clone();
            action.connect_activate(move |_, _| {
                let _ = s.send_blocking($input.clone());
            });
            action_group.add_action(&action);
        };
    }

    add_action!("command-palette", AppInput::CommandPalette);
    add_action!("model-selection", AppInput::ModelSelection);
    add_action!("themes", AppInput::ShowThemesSidebar);
    add_action!("ai-chat", AppInput::ShowAiChat);
    add_action!("claw", AppInput::ShowClawSidebar);
    add_action!("new-window", AppInput::NewWindow);
    add_action!("new-tab", AppInput::NewTab);
    add_action!("detached-sessions", AppInput::ShowDetachedSessions);
    add_action!("close-tab", AppInput::CloseActiveTab);
    add_action!("toggle-sidebar", AppInput::ToggleSidebar);
    add_action!("preferences", AppInput::OpenPreferences);
    add_action!("bookmarks", AppInput::OpenBookmarks);
    add_action!("shortcuts", AppInput::OpenShortcuts);
    add_action!("about", AppInput::OpenAbout);
    add_action!("open-in-files", AppInput::OpenInFiles);
    add_action!("zoom-in", AppInput::ZoomIn);
    add_action!("zoom-out", AppInput::ZoomOut);
    add_action!("reset-zoom", AppInput::ResetZoom);
    add_action!("copy", AppInput::Copy);
    add_action!("paste", AppInput::Paste);

    let execute_bm = gtk4::gio::SimpleAction::new(
        "execute-bookmark",
        Some(gtk4::glib::VariantTy::new("(sss)").unwrap()),
    );
    let s = sender.clone();
    execute_bm.connect_activate(move |_, param| {
        if let Some((name, filename, script)) =
            param.and_then(|p| p.get::<(String, String, String)>())
        {
            let _ = s.send_blocking(AppInput::ExecuteBookmark(name, filename, script));
        }
    });
    action_group.add_action(&execute_bm);
    add_action!("split-vertical", AppInput::SplitVertical);
    add_action!("split-horizontal", AppInput::SplitHorizontal);
    add_action!("close-split", AppInput::CloseSplit);
    add_action!("toggle-maximize", AppInput::ToggleMaximize);
    add_action!("focus-left", AppInput::FocusLeft);
    add_action!("focus-right", AppInput::FocusRight);
    add_action!("focus-up", AppInput::FocusUp);
    add_action!("focus-down", AppInput::FocusDown);
    add_action!("swap-left", AppInput::SwapLeft);
    add_action!("swap-right", AppInput::SwapRight);
    add_action!("swap-up", AppInput::SwapUp);
    add_action!("swap-down", AppInput::SwapDown);

    window.insert_action_group("win", Some(&action_group));
}
