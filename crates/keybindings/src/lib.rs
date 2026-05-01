use gtk4::prelude::*;

#[derive(Clone, Debug)]
pub struct Keybinding {
    pub trigger: &'static str,
    pub action_name: &'static str,
}

pub const NEW_WINDOW: Keybinding = Keybinding {
    trigger: "<Ctrl><Shift>n",
    action_name: "win.new-window",
};
pub const NEW_TAB: Keybinding = Keybinding {
    trigger: "<Ctrl><Shift>t",
    action_name: "win.new-tab",
};
pub const CLOSE_TAB: Keybinding = Keybinding {
    trigger: "<Ctrl><Shift>q",
    action_name: "win.close-tab",
};
pub const TOGGLE_SIDEBAR: Keybinding = Keybinding {
    trigger: "<Ctrl><Shift>j",
    action_name: "win.toggle-sidebar",
};
pub const PREFERENCES: Keybinding = Keybinding {
    trigger: "<Ctrl>comma",
    action_name: "win.preferences",
};
pub const ZOOM_IN: Keybinding = Keybinding {
    trigger: "<Ctrl>plus",
    action_name: "win.zoom-in",
};
pub const ZOOM_OUT: Keybinding = Keybinding {
    trigger: "<Ctrl>minus",
    action_name: "win.zoom-out",
};
pub const RESET_ZOOM: Keybinding = Keybinding {
    trigger: "<Ctrl>0",
    action_name: "win.reset-zoom",
};
pub const SEARCH: Keybinding = Keybinding {
    trigger: "<Ctrl><Shift>f",
    action_name: "win.search",
};
pub const COMMAND_PALETTE: Keybinding = Keybinding {
    trigger: "<Ctrl><Shift>p",
    action_name: "win.command-palette",
};
pub const COPY: Keybinding = Keybinding {
    trigger: "<Ctrl><Shift>c",
    action_name: "win.copy",
};
pub const PASTE: Keybinding = Keybinding {
    trigger: "<Ctrl><Shift>v",
    action_name: "win.paste",
};
pub const MESSAGE_BAR: Keybinding = Keybinding {
    trigger: "<Ctrl>slash",
    action_name: "win.message-bar", // Note: handled manually in pane/mod.rs
};

pub const SPLIT_DOWN: Keybinding = Keybinding {
    trigger: "<Ctrl><Shift>e",
    action_name: "win.split-horizontal",
};
pub const SPLIT_RIGHT: Keybinding = Keybinding {
    trigger: "<Ctrl><Shift>o",
    action_name: "win.split-vertical",
};
pub const CLOSE_PANE: Keybinding = Keybinding {
    trigger: "<Ctrl><Shift>w",
    action_name: "win.close-split",
};

pub const FOCUS_LEFT: Keybinding = Keybinding {
    trigger: "<Ctrl><Alt>Left",
    action_name: "win.focus-left",
};
pub const FOCUS_RIGHT: Keybinding = Keybinding {
    trigger: "<Ctrl><Alt>Right",
    action_name: "win.focus-right",
};
pub const FOCUS_UP: Keybinding = Keybinding {
    trigger: "<Ctrl><Alt>Up",
    action_name: "win.focus-up",
};
pub const FOCUS_DOWN: Keybinding = Keybinding {
    trigger: "<Ctrl><Alt>Down",
    action_name: "win.focus-down",
};

pub const SWAP_LEFT: Keybinding = Keybinding {
    trigger: "<Ctrl><Shift>Left",
    action_name: "win.swap-left",
};
pub const SWAP_RIGHT: Keybinding = Keybinding {
    trigger: "<Ctrl><Shift>Right",
    action_name: "win.swap-right",
};
pub const SWAP_UP: Keybinding = Keybinding {
    trigger: "<Ctrl><Shift>Up",
    action_name: "win.swap-up",
};
pub const SWAP_DOWN: Keybinding = Keybinding {
    trigger: "<Ctrl><Shift>Down",
    action_name: "win.swap-down",
};

pub struct ShortcutCategory {
    pub name: &'static str,
    pub items: Vec<(&'static str, Keybinding)>,
}

pub fn get_shortcuts_by_category() -> Vec<ShortcutCategory> {
    vec![
        ShortcutCategory {
            name: "General",
            items: vec![
                ("New Window", NEW_WINDOW),
                ("New Tab", NEW_TAB),
                ("Close Tab", CLOSE_TAB),
                ("Command Palette", COMMAND_PALETTE),
                ("Toggle Sidebar", TOGGLE_SIDEBAR),
                ("Ask Boxxy-Claw", MESSAGE_BAR),
                ("Preferences", PREFERENCES),
            ],
        },
        ShortcutCategory {
            name: "Terminal",
            items: vec![
                ("Copy", COPY),
                ("Paste", PASTE),
                ("Search", SEARCH),
                ("Zoom In", ZOOM_IN),
                ("Zoom Out", ZOOM_OUT),
            ],
        },
        ShortcutCategory {
            name: "Split Panes",
            items: vec![
                ("Split Down", SPLIT_DOWN),
                ("Split Right", SPLIT_RIGHT),
                ("Close Pane", CLOSE_PANE),
                ("Focus Up", FOCUS_UP),
                ("Focus Down", FOCUS_DOWN),
                ("Focus Left", FOCUS_LEFT),
                ("Focus Right", FOCUS_RIGHT),
                ("Swap Up", SWAP_UP),
                ("Swap Down", SWAP_DOWN),
                ("Swap Left", SWAP_LEFT),
                ("Swap Right", SWAP_RIGHT),
            ],
        },
    ]
}

pub fn bind_shortcuts(app: &libadwaita::Application) {
    app.set_accels_for_action(NEW_WINDOW.action_name, &[NEW_WINDOW.trigger]);
    app.set_accels_for_action(NEW_TAB.action_name, &[NEW_TAB.trigger]);
    app.set_accels_for_action(CLOSE_TAB.action_name, &[CLOSE_TAB.trigger]);
    app.set_accels_for_action(TOGGLE_SIDEBAR.action_name, &[TOGGLE_SIDEBAR.trigger]);
    app.set_accels_for_action(PREFERENCES.action_name, &[PREFERENCES.trigger]);
    app.set_accels_for_action(
        ZOOM_IN.action_name,
        &[
            ZOOM_IN.trigger,
            "<Ctrl>equal",
            "<Ctrl>KP_Add",
            "<Ctrl><Shift>plus",
            "<Ctrl><Shift>equal",
        ],
    );
    app.set_accels_for_action(
        ZOOM_OUT.action_name,
        &[
            ZOOM_OUT.trigger,
            "<Ctrl>KP_Subtract",
            "<Ctrl>underscore",
            "<Ctrl><Shift>minus",
        ],
    );
    app.set_accels_for_action(RESET_ZOOM.action_name, &[RESET_ZOOM.trigger, "<Ctrl>KP_0"]);
    app.set_accels_for_action(SEARCH.action_name, &[SEARCH.trigger]);
    app.set_accels_for_action(COMMAND_PALETTE.action_name, &[COMMAND_PALETTE.trigger]);
    app.set_accels_for_action(COPY.action_name, &[COPY.trigger]);
    app.set_accels_for_action(PASTE.action_name, &[PASTE.trigger]);
    app.set_accels_for_action(SPLIT_DOWN.action_name, &[SPLIT_DOWN.trigger]);
    app.set_accels_for_action(SPLIT_RIGHT.action_name, &[SPLIT_RIGHT.trigger]);
    app.set_accels_for_action(CLOSE_PANE.action_name, &[CLOSE_PANE.trigger]);
    app.set_accels_for_action(FOCUS_LEFT.action_name, &[FOCUS_LEFT.trigger]);
    app.set_accels_for_action(FOCUS_RIGHT.action_name, &[FOCUS_RIGHT.trigger]);
    app.set_accels_for_action(FOCUS_UP.action_name, &[FOCUS_UP.trigger]);
    app.set_accels_for_action(FOCUS_DOWN.action_name, &[FOCUS_DOWN.trigger]);
    app.set_accels_for_action(SWAP_LEFT.action_name, &[SWAP_LEFT.trigger]);
    app.set_accels_for_action(SWAP_RIGHT.action_name, &[SWAP_RIGHT.trigger]);
    app.set_accels_for_action(SWAP_UP.action_name, &[SWAP_UP.trigger]);
    app.set_accels_for_action(SWAP_DOWN.action_name, &[SWAP_DOWN.trigger]);
}
