#[derive(Debug, Clone)]
pub struct TerminalInit {
    pub id: String,
    pub working_dir: Option<String>,
    pub spawn_intent: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TerminalEvent {
    pub id: String,
    pub kind: TerminalEventKind,
}

#[derive(Debug, Clone)]
pub enum TerminalProposal {
    None,
    Command(String),
    Bookmark(String, String, Vec<String>), // filename, script, placeholders
    FileWrite(String, String),             // The path and content of the file
}

#[derive(Debug, Clone)]
pub enum TerminalEventKind {
    TitleChanged(String),
    DirectoryChanged(String),
    Exited(i32),
    BellRung,
    Osc133A,
    Osc133B,
    Osc133C,
    Osc133D(String, Option<i32>),
    ClawEvent(String, boxxy_claw::engine::ClawEngineEvent),
    FocusClawSidebar,
    PaneFocused(String),
}

#[derive(Debug, Clone)]
pub struct PaneInit {
    pub id: String,
    pub working_dir: Option<String>,
    pub spawn_intent: Option<String>,
}

#[derive(Debug, Clone)]
pub enum PaneOutput {
    Focused(String),
    Exited(String, i32),
    TitleChanged(String, String),
    DirectoryChanged(String, String),
    BellRung(String),
    Osc133A(String),
    Osc133B(String),
    Osc133C(String),
    Osc133D(String, Option<i32>),
    ClawEvent(String, boxxy_claw::engine::ClawEngineEvent),
    FocusClawSidebar(String),
}
