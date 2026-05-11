//! UI-shaped, host-agnostic description of an agent proposal.
//!
//! The widget consumes this enum; concrete hosts (the terminal, or any
//! future surface) convert their own proposal shapes to this one via
//! `From` at the boundary.

#[derive(Debug, Clone)]
pub enum Proposal {
    None,
    Command(String),
    Bookmark {
        filename: String,
        script: String,
        placeholders: Vec<String>,
    },
    FileWrite {
        path: String,
        content: String,
    },
    FileDelete {
        path: String,
    },
    KillProcess {
        pid: u32,
        name: String,
    },
    BackgroundCommand {
        command: String,
        explanation: String,
    },
    GetClipboard,
    SetClipboard(String),
}
