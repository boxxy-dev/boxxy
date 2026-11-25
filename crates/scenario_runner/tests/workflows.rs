//! Unified headless E2E workflow integration binary (`tests/workflows.rs`).
//!
//! Consolidating domain workflow modules into a single integration test binary avoids linking
//! 6 separate multi-hundred-MB debug executables while keeping each workflow cleanly isolated.

#[path = "workflows/companion_panes_and_vibe.rs"]
mod companion_panes_and_vibe;
#[path = "workflows/context_hygiene_and_compaction.rs"]
mod context_hygiene_and_compaction;
#[path = "workflows/edit_and_undo.rs"]
mod edit_and_undo;
#[path = "workflows/error_diagnosis.rs"]
mod error_diagnosis;
#[path = "workflows/incognito_and_roundtable.rs"]
mod incognito_and_roundtable;
#[path = "workflows/persistence_and_resume.rs"]
mod persistence_and_resume;
#[path = "workflows/pty_and_cli.rs"]
mod pty_and_cli;
#[path = "workflows/queue_btw_and_abort.rs"]
mod queue_btw_and_abort;
