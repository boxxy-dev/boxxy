use boxxy_terminal::TerminalComponent;
use libadwaita;
use std::cell::RefCell;
use std::collections::HashMap;

use crate::state::TabColor;

thread_local! {
    pub static ORPHAN_TABS: RefCell<HashMap<String, TerminalController>> = RefCell::new(HashMap::new());
}

#[derive(Clone)]
pub struct TerminalController {
    pub controller: TerminalComponent,
    pub id: String,
    pub cwd: Option<String>,
    pub tab_color: TabColor,
    pub custom_title: Option<String>,
}

impl TerminalController {
    pub fn cancel_task_by_id(&self, pane_id: &str, task_id: uuid::Uuid) -> bool {
        self.controller.cancel_task_by_id(pane_id, task_id)
    }

    pub fn active_pane_id(&self) -> String {
        self.controller.active_pane_id()
    }

    pub fn widget(&self) -> &gtk4::Widget {
        use gtk4::prelude::Cast;
        self.controller.widget().upcast_ref()
    }

    pub fn grab_focus(&self) {
        self.controller.grab_focus();
    }

    pub fn set_claw_active(&self, active: bool) {
        self.controller.set_claw_active(active);
    }

    pub fn update_diagnosis_mode_for_pane(
        &self,
        pane_id: &str,
        mode: &boxxy_preferences::config::ClawAutoDiagnosisMode,
    ) -> bool {
        self.controller.update_diagnosis_mode_for_pane(pane_id, mode)
    }

    pub fn is_claw_active(&self) -> bool {
        self.controller.is_claw_active()
    }

    pub fn is_proactive(&self) -> bool {
        self.controller.is_proactive()
    }

    pub fn claw_history_widget(&self) -> gtk4::ListBox {
        self.controller.claw_history_widget()
    }

    pub fn get_total_tokens(&self) -> u64 {
        self.controller.get_total_tokens()
    }

    pub fn split_vertical(&self, intent: Option<String>) {
        self.controller.split_vertical(intent);
    }

    pub fn split_horizontal(&self, intent: Option<String>) {
        self.controller.split_horizontal(intent);
    }

    pub fn close_pane_by_id(&self, pane_id: &str) -> bool {
        self.controller.close_pane_by_id(pane_id)
    }

    pub fn inject_keystrokes_by_id(&self, pane_id: &str, keys: &str) -> bool {
        self.controller.inject_keystrokes_by_id(pane_id, keys)
    }
}

pub struct AppInit {
    pub incoming_tab_view: Option<libadwaita::TabView>,
    pub working_dir: Option<String>,
}

impl AppInit {
    pub fn new() -> Self {
        Self {
            incoming_tab_view: None,
            working_dir: None,
        }
    }
}

impl Default for AppInit {
    fn default() -> Self {
        Self::new()
    }
}
