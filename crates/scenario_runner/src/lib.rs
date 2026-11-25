//! `scenario-runner`: Deterministic, GTK-free End-to-End workflow test harness for Boxxy Terminal.
//!
//! Drives real `ClawSession` actors, `ClawSubsystem` host operations, `boxxy-db` SQLite/FTS5
//! persistence, `PtyRegistry` + `/bin/bash` `OSC 7`/`OSC 133` shell integration, and `boxxy-cli`
//! `+actions` against an isolated ephemeral config directory and an in-process scriptable
//! `MockLlmServer`.

pub mod config_env;
pub mod harness;
pub mod mock_llm;
pub mod pane_driver;
pub mod pty_driver;

pub use harness::TestWorkspace;
pub use mock_llm::{MockLlmServer, MockStep, MockToolCall, RecordedChatRequest, TurnGate};
pub use pane_driver::{CompanionPane, CompanionTable, PaneDriver};
pub use pty_driver::PtyShellSession;
