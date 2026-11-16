use crate::state::{AppInput, AppWindowInner};
use async_channel::Sender;
use gtk4::gio;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

pub fn setup_actions(
    window: &libadwaita::ApplicationWindow,
    inner: &Rc<RefCell<AppWindowInner>>,
    sender: Sender<AppInput>,
) {
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

    macro_rules! add_active_tab_action {
        ($name:expr, $method:ident) => {
            let action = gio::SimpleAction::new($name, None);
            let inner_weak = Rc::downgrade(inner);
            action.connect_activate(move |_, _| {
                if let Some(inner_rc) = inner_weak.upgrade()
                    && let Some(tab) = inner_rc.borrow().active_tab()
                {
                    tab.controller.$method();
                }
            });
            action_group.add_action(&action);
        };
    }

    add_action!("command-palette", AppInput::CommandPalette);
    add_action!("model-selection", AppInput::ModelSelection);
    add_action!("themes", AppInput::ShowThemesSidebar);
    add_action!("terminal-colors", AppInput::ShowThemesSidebar);
    add_action!("app-themes", AppInput::OpenThemesPreferences);
    add_action!("claw", AppInput::ShowClawSidebar);
    add_action!("toggle-claw-code-blocks", AppInput::ToggleClawCodeBlocks);
    add_action!("new-window", AppInput::NewWindow);
    add_action!("new-tab", AppInput::NewTab);

    let new_tab_launch =
        gio::SimpleAction::new("new-tab-launch", Some(gtk4::glib::VariantTy::STRING));
    let s_tab_launch = sender.clone();
    new_tab_launch.connect_activate(move |_, param| {
        if let Some(raw_json) = param.and_then(|p| p.get::<String>())
            && let Ok(spec) = serde_json::from_str::<crate::init::TabLaunchSpec>(&raw_json)
        {
            let _ = s_tab_launch.send_blocking(AppInput::NewTabLaunch(spec));
        }
    });
    action_group.add_action(&new_tab_launch);

    add_action!("detached-sessions", AppInput::ShowDetachedSessions);
    add_action!("close-tab", AppInput::CloseActiveTab);
    add_action!("toggle-sidebar", AppInput::ToggleSidebar);
    add_action!("preferences", AppInput::OpenPreferences);
    add_action!("open-apis", AppInput::OpenApisPreferences);
    add_action!("open-mcp", AppInput::OpenMcpPreferences);

    let open_prefs_page_name = boxxy_command_palette::OPEN_PREFERENCES_PAGE_ACTION
        .strip_prefix("win.")
        .unwrap_or("open-preferences-page");
    let open_prefs_page =
        gio::SimpleAction::new(open_prefs_page_name, Some(gtk4::glib::VariantTy::STRING));
    let s_prefs_page = sender.clone();
    open_prefs_page.connect_activate(move |_, param| {
        if let Some(page) = param.and_then(|p| p.get::<String>()) {
            let _ = s_prefs_page.send_blocking(AppInput::OpenPreferencesPage(page));
        }
    });
    action_group.add_action(&open_prefs_page);
    add_action!("bookmarks", AppInput::OpenBookmarks);
    add_action!("shortcuts", AppInput::OpenShortcuts);
    add_action!("whats-new", AppInput::OpenWhatsNew);
    add_action!("about", AppInput::OpenAbout);
    add_action!("open-in-files", AppInput::OpenInFiles);
    add_action!("zoom-in", AppInput::ZoomIn);
    add_action!("zoom-out", AppInput::ZoomOut);
    add_action!("reset-zoom", AppInput::ResetZoom);

    // Execute copy/paste and direct terminal operations synchronously during GTK
    // action activation so GDK Wayland retains the active input event serial when
    // claiming the clipboard.
    add_active_tab_action!("copy", copy);
    add_active_tab_action!("paste", paste);
    add_active_tab_action!("select-all", select_all);
    add_active_tab_action!("search", toggle_search);
    add_active_tab_action!("copy-last-output", copy_last_command_output);
    add_active_tab_action!("open-scrollback", open_scrollback_in_editor);
    add_active_tab_action!("clear-scrollback", clear_scrollback);
    add_active_tab_action!("reset-terminal", reset_terminal);

    let execute_bm = gio::SimpleAction::new(
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

#[cfg(test)]
mod tests {
    #[test]
    fn command_palette_preferences_categories_match_preferences_component() {
        let palette_cats: Vec<(&str, &str)> = boxxy_command_palette::PREFERENCES_CATEGORIES
            .iter()
            .map(|c| (c.page_name, c.title))
            .collect();
        assert_eq!(
            palette_cats.as_slice(),
            boxxy_preferences::PREFERENCE_CATEGORIES.as_slice(),
            "Command Palette Preferences categories drifted from PreferencesComponent"
        );
        assert!(
            boxxy_command_palette::OPEN_PREFERENCES_PAGE_ACTION.starts_with("win."),
            "Expected win.* action for opening Preferences pages"
        );
    }
}
