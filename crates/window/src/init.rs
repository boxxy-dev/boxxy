use libadwaita;
use std::cell::RefCell;
use std::collections::HashMap;
use boxxy_terminal::TerminalComponent;

thread_local! {
    pub static ORPHAN_TABS: RefCell<HashMap<String, TerminalController>> = RefCell::new(HashMap::new());
}

#[derive(Clone)]
pub struct TerminalController {
    pub controller: TerminalComponent,
    pub id: String,
    pub cwd: Option<String>,
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
