//! Unified headless E2E workflow integration binary (`tests/workflows.rs`).
//!
//! Consolidating domain workflow modules into a single integration test binary avoids linking
//! separate multi-hundred-MB debug executables while keeping each workflow cleanly isolated.

#[path = "workflows/companion_panes_and_vibe.rs"]
mod companion_panes_and_vibe;
#[path = "workflows/context_hygiene_and_compaction.rs"]
mod context_hygiene_and_compaction;
#[path = "workflows/edit_and_undo.rs"]
mod edit_and_undo;
#[path = "workflows/error_diagnosis.rs"]
mod error_diagnosis;
#[path = "workflows/hotswap_dreaming_and_handoff.rs"]
mod hotswap_dreaming_and_handoff;
#[path = "workflows/incognito_and_roundtable.rs"]
mod incognito_and_roundtable;
#[path = "workflows/interactive_handoff_and_drama.rs"]
mod interactive_handoff_and_drama;
#[path = "workflows/osc133_and_workspace_scoping.rs"]
mod osc133_and_workspace_scoping;
#[path = "workflows/persistence_and_resume.rs"]
mod persistence_and_resume;
#[path = "workflows/pty_and_cli.rs"]
mod pty_and_cli;
#[path = "workflows/queue_btw_and_abort.rs"]
mod queue_btw_and_abort;
#[path = "workflows/rebind_headless_and_collab_tools.rs"]
mod rebind_headless_and_collab_tools;
#[path = "workflows/skills_and_memories.rs"]
mod skills_and_memories;
#[path = "workflows/sleep_and_multimodal.rs"]
mod sleep_and_multimodal;
#[path = "workflows/swarm_locks_and_promises.rs"]
mod swarm_locks_and_promises;
