//! Shared, GTK-free DTO and wire-protocol layer across the Boxxy UI ↔ `boxxy-agent` D-Bus boundary.

pub mod character_loader;
pub mod characters;
pub mod diff;
pub mod environment;
pub mod messages;
pub mod rows;
pub mod skill_loader;
pub mod slash;
pub mod status;
mod str_utils;
pub mod tasks;
pub mod trace;
pub mod usage;

pub use diff::{
    DiffCell, DiffHunk, DiffKind, DiffLineKind, DiffPairRow, DiffSpan, DiffStats, FileDiff,
    MAX_DIFF_LINE_CHARS, MAX_DIFF_RENDER_ROWS,
};
pub use environment::{ClawEnvironment, ProcessSample};
pub use messages::{ClawEngineEvent, ClawMessage};
pub use rows::PersistentClawRow;
pub use slash::{SlashCommand, SlashSubmitEffect, classify_submit_effect};
pub use status::{
    AgentStatus, ClawEvent, ContextQuality, SpawnLocation, TransitionRequest, TriggerSource,
};
pub use tasks::{
    IntervalPhase, IntervalSchedule, PendingTaskInfo, ScheduledTask, TaskStatus, TaskType,
};
pub use trace::{
    ActivitySummary, TraceCategory, TurnTraceItem, busy_tool_label, coalesce_restored_history,
    extract_reasoning_summary, live_reasoning_title, tool_result_to_trace_item,
};
pub use usage::{SessionUsage, UsageWrapper, format_number_with_commas, format_usd};
