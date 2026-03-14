use boxxy_keybindings as kb;
use libadwaita as adw;
use libadwaita::prelude::*;

#[derive(Debug)]
pub struct ShortcutsComponent {
    dialog: adw::ShortcutsDialog,
}

impl ShortcutsComponent {
    pub fn new() -> Self {
        let dialog = adw::ShortcutsDialog::builder()
            .title("Shortcuts")
            .build();

        let general_section = adw::ShortcutsSection::new(Some("General"));
        general_section.add(create_item("New Window", kb::NEW_WINDOW.trigger));
        general_section.add(create_item("New Tab", kb::NEW_TAB.trigger));
        general_section.add(create_item("Close Tab", kb::CLOSE_TAB.trigger));
        general_section.add(create_item("Command Palette", kb::COMMAND_PALETTE.trigger));
        general_section.add(create_item("Toggle Sidebar", kb::TOGGLE_SIDEBAR.trigger));
        general_section.add(create_item("Focus Claw", kb::CLAW_TOGGLE_FOCUS.trigger));
        general_section.add(create_item("Preferences", kb::PREFERENCES.trigger));

        let terminal_section = adw::ShortcutsSection::new(Some("Terminal"));
        terminal_section.add(create_item("Copy", kb::COPY.trigger));
        terminal_section.add(create_item("Paste", kb::PASTE.trigger));
        terminal_section.add(create_item("Search", kb::SEARCH.trigger));
        terminal_section.add(create_item("Zoom In", kb::ZOOM_IN.trigger));
        terminal_section.add(create_item("Zoom Out", kb::ZOOM_OUT.trigger));

        let split_section = adw::ShortcutsSection::new(Some("Split Panes"));
        split_section.add(create_item("Split Down", kb::SPLIT_DOWN.trigger));
        split_section.add(create_item("Split Right", kb::SPLIT_RIGHT.trigger));
        split_section.add(create_item("Close Pane", kb::CLOSE_PANE.trigger));
        split_section.add(create_item("Focus Up", kb::FOCUS_UP.trigger));
        split_section.add(create_item("Focus Down", kb::FOCUS_DOWN.trigger));
        split_section.add(create_item("Focus Left", kb::FOCUS_LEFT.trigger));
        split_section.add(create_item("Focus Right", kb::FOCUS_RIGHT.trigger));
        split_section.add(create_item("Swap Up", kb::SWAP_UP.trigger));
        split_section.add(create_item("Swap Down", kb::SWAP_DOWN.trigger));
        split_section.add(create_item("Swap Left", kb::SWAP_LEFT.trigger));
        split_section.add(create_item("Swap Right", kb::SWAP_RIGHT.trigger));

        dialog.add(general_section);
        dialog.add(terminal_section);
        dialog.add(split_section);

        Self { dialog }
    }

    pub fn show(&self, parent: &gtk4::Window) {
        self.dialog.present(Some(parent));
    }

    pub fn widget(&self) -> &adw::ShortcutsDialog {
        &self.dialog
    }

    pub fn hide(&self) {
        self.dialog.close();
    }
}

impl Default for ShortcutsComponent {
    fn default() -> Self {
        Self::new()
    }
}

fn create_item(title: &str, accel: &str) -> adw::ShortcutsItem {
    adw::ShortcutsItem::new(title, accel)
}
