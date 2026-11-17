//! Session-resumption history coalescer (`coalesce_restored_history`).

use super::tool_summary::{is_file_edit_tool, tool_result_to_trace_item};
use super::{TraceCategory, TurnTraceItem};
use crate::PersistentClawRow;

fn is_different_speaker(
    pending_name: Option<&str>,
    pending_cid: Option<&str>,
    row_name: Option<&str>,
    row_cid: Option<&str>,
) -> bool {
    match (pending_cid, row_cid) {
        (Some(a), Some(b)) => a != b,
        (None, None) => pending_name != row_name,
        _ => match (pending_name, row_name) {
            (Some(a), Some(b)) => a != b,
            _ => false,
        },
    }
}

struct CoalesceState {
    out: Vec<PersistentClawRow>,
    pending_trace: Vec<TurnTraceItem>,
    pending_meta: Option<(String, Option<String>, Option<String>)>,
}

impl CoalesceState {
    fn with_capacity(cap: usize) -> Self {
        Self {
            out: Vec::with_capacity(cap),
            pending_trace: Vec::new(),
            pending_meta: None,
        }
    }

    fn flush_orphan(&mut self) {
        if self.pending_trace.is_empty() {
            self.pending_meta = None;
            return;
        }
        let (pane_id, agent_name, character_id) = self
            .pending_meta
            .take()
            .unwrap_or_else(|| (String::new(), None, None));
        self.out.push(PersistentClawRow::Diagnosis {
            pane_id,
            agent_name,
            character_id,
            content: String::new(),
            trace: std::mem::take(&mut self.pending_trace),
            usage: None,
        });
    }

    fn flush_if_speaker_changed(&mut self, agent_name: Option<&str>, character_id: Option<&str>) {
        if let Some((_, ref p_name, ref p_cid)) = self.pending_meta
            && is_different_speaker(
                p_name.as_deref(),
                p_cid.as_deref(),
                agent_name,
                character_id,
            )
        {
            self.flush_orphan();
        }
    }

    fn push_trace_item(
        &mut self,
        pane_id: String,
        agent_name: Option<String>,
        character_id: Option<String>,
        item: TurnTraceItem,
    ) {
        self.flush_if_speaker_changed(agent_name.as_deref(), character_id.as_deref());
        if self.pending_meta.is_none() {
            self.pending_meta = Some((pane_id, agent_name, character_id));
        }
        self.pending_trace.push(item);
    }

    fn take_merged_trace(
        &mut self,
        agent_name: Option<&str>,
        character_id: Option<&str>,
        persisted_turn_trace: Vec<TurnTraceItem>,
    ) -> Vec<TurnTraceItem> {
        self.flush_if_speaker_changed(agent_name, character_id);
        self.pending_meta = None;
        merge_turn_traces(&mut self.pending_trace, persisted_turn_trace)
    }

    fn ingest_tool_call(
        &mut self,
        pane_id: String,
        agent_name: Option<String>,
        character_id: Option<String>,
        tool_name: &str,
        result: &str,
    ) {
        let Some(item) = tool_result_to_trace_item(tool_name, result) else {
            return;
        };
        self.flush_if_speaker_changed(agent_name.as_deref(), character_id.as_deref());
        // If a proposed file write was persisted before user approval and then
        // rejected/failed, drop the unapproved `Edit` row so it isn't shown as applied.
        if is_file_edit_tool(tool_name)
            && matches!(&item, TurnTraceItem::Tool { failed: true, .. })
            && matches!(
                self.pending_trace.last(),
                Some(TurnTraceItem::Edit {
                    auto_approved: false,
                    ..
                })
            )
        {
            self.pending_trace.pop();
        }
        self.push_trace_item(pane_id, agent_name, character_id, item);
    }

    fn ingest(&mut self, row: PersistentClawRow) {
        match row {
            PersistentClawRow::ToolCall {
                pane_id,
                agent_name,
                character_id,
                tool_name,
                result,
                ..
            } => {
                self.ingest_tool_call(pane_id, agent_name, character_id, &tool_name, &result);
            }
            PersistentClawRow::ProcessList {
                pane_id,
                agent_name,
                character_id,
                ..
            } => {
                let item = TurnTraceItem::Tool {
                    tool_name: "list_processes".to_string(),
                    category: TraceCategory::Other,
                    label: "Inspected running processes".to_string(),
                    preview: None,
                    failed: false,
                };
                self.push_trace_item(pane_id, agent_name, character_id, item);
            }
            PersistentClawRow::FileDiff {
                pane_id,
                agent_name,
                character_id,
                diff,
                auto_approved,
                ..
            } => {
                let item = TurnTraceItem::Edit {
                    diff,
                    auto_approved,
                };
                self.push_trace_item(pane_id, agent_name, character_id, item);
            }
            PersistentClawRow::Diagnosis {
                pane_id,
                agent_name,
                character_id,
                content,
                trace,
                usage,
            } => {
                let merged_trace =
                    self.take_merged_trace(agent_name.as_deref(), character_id.as_deref(), trace);
                self.out.push(PersistentClawRow::Diagnosis {
                    pane_id,
                    agent_name,
                    character_id,
                    content,
                    trace: merged_trace,
                    usage,
                });
            }
            PersistentClawRow::Suggested {
                pane_id,
                agent_name,
                character_id,
                diagnosis,
                command,
                trace,
                usage,
            } => {
                let merged_trace =
                    self.take_merged_trace(agent_name.as_deref(), character_id.as_deref(), trace);
                self.out.push(PersistentClawRow::Suggested {
                    pane_id,
                    agent_name,
                    character_id,
                    diagnosis,
                    command,
                    trace: merged_trace,
                    usage,
                });
            }
            other @ (PersistentClawRow::User { .. }
            | PersistentClawRow::Command { .. }
            | PersistentClawRow::SystemMessage { .. }) => {
                self.flush_orphan();
                self.out.push(other);
            }
        }
    }
}

/// Coalesces persisted `PersistentClawRow` history on session resumption.
///
/// Sequential `ToolCall`, `ProcessList`, and `FileDiff` events belonging to a turn
/// fold cleanly into the turn's `Diagnosis` or `Suggested` row `trace`.
#[must_use]
pub fn coalesce_restored_history(rows: Vec<PersistentClawRow>) -> Vec<PersistentClawRow> {
    let mut state = CoalesceState::with_capacity(rows.len());
    for row in rows {
        state.ingest(row);
    }
    state.flush_orphan();
    state.out
}

fn merge_turn_traces(
    pending_db_items: &mut Vec<TurnTraceItem>,
    persisted_turn_trace: Vec<TurnTraceItem>,
) -> Vec<TurnTraceItem> {
    if persisted_turn_trace.is_empty() {
        return std::mem::take(pending_db_items);
    }
    // If `persisted_turn_trace` already captured the full ordered turn trace (thoughts + tools + edits),
    // prefer it so exact interleaving of thoughts and tools is preserved without duplicates.
    let has_non_thought = persisted_turn_trace.iter().any(|i| {
        !matches!(
            i,
            TurnTraceItem::Thought { .. } | TurnTraceItem::Skill { .. }
        )
    });
    if has_non_thought {
        pending_db_items.clear();
        return persisted_turn_trace;
    }

    // Otherwise prepend thoughts/skills before DB tool/diff rows.
    let mut combined = persisted_turn_trace;
    combined.append(pending_db_items);
    combined
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ActivitySummary, FileDiff};

    #[test]
    fn coalesce_restored_history_folds_legacy_tools_and_diffs_into_diagnosis() {
        let diff = FileDiff::compute("src/module.rs", Some("fn a() {}\n"), "fn b() {}\n");
        let rows = vec![
            PersistentClawRow::User {
                pane_id: "pane-1".to_string(),
                content: "Refactor module".to_string(),
                images: Vec::new(),
            },
            PersistentClawRow::ToolCall {
                pane_id: "pane-1".to_string(),
                agent_name: Some("Niko".to_string()),
                character_id: Some("char-1".to_string()),
                tool_name: "file_read".to_string(),
                result: "\"[Showing lines 1-10 of 10 in src/module.rs]\"".to_string(),
                usage: None,
            },
            PersistentClawRow::FileDiff {
                pane_id: "pane-1".to_string(),
                agent_name: Some("Niko".to_string()),
                character_id: Some("char-1".to_string()),
                diff,
                auto_approved: false,
                usage: None,
            },
            PersistentClawRow::Diagnosis {
                pane_id: "pane-1".to_string(),
                agent_name: Some("Niko".to_string()),
                character_id: Some("char-1".to_string()),
                content: "Updated `src/module.rs`.".to_string(),
                trace: Vec::new(),
                usage: None,
            },
        ];

        let coalesced = coalesce_restored_history(rows);
        assert_eq!(coalesced.len(), 2);
        match &coalesced[1] {
            PersistentClawRow::Diagnosis { content, trace, .. } => {
                assert_eq!(content, "Updated `src/module.rs`.");
                let summary = ActivitySummary::summarize(trace);
                assert_eq!(summary.reads, 1);
                assert_eq!(summary.edits, 1);
            }
            other => panic!("expected coalesced Diagnosis row, got {other:?}"),
        }
    }

    #[test]
    fn coalesce_drops_unapproved_file_diff_when_proposal_was_rejected() {
        let diff = FileDiff::compute("src/module.rs", Some("fn a() {}\n"), "fn b() {}\n");
        let rows = vec![
            PersistentClawRow::FileDiff {
                pane_id: "pane-1".to_string(),
                agent_name: Some("Niko".to_string()),
                character_id: Some("char-1".to_string()),
                diff,
                auto_approved: false,
                usage: None,
            },
            PersistentClawRow::ToolCall {
                pane_id: "pane-1".to_string(),
                agent_name: Some("Niko".to_string()),
                character_id: Some("char-1".to_string()),
                tool_name: "file_write".to_string(),
                result: "\"[USER_EXPLICIT_REJECT]\"".to_string(),
                usage: None,
            },
            PersistentClawRow::Diagnosis {
                pane_id: "pane-1".to_string(),
                agent_name: Some("Niko".to_string()),
                character_id: Some("char-1".to_string()),
                content: "Stopped as requested.".to_string(),
                trace: vec![TurnTraceItem::Thought {
                    summary: None,
                    content: "Proposing file write".to_string(),
                    duration_ms: None,
                }],
                usage: None,
            },
        ];

        let coalesced = coalesce_restored_history(rows);
        assert_eq!(coalesced.len(), 1);
        let trace = coalesced[0].trace();
        assert!(
            !trace
                .iter()
                .any(|i| matches!(i, TurnTraceItem::Edit { .. }))
        );
        let summary = ActivitySummary::summarize(trace);
        assert_eq!(summary.thoughts, 1);
        assert_eq!(summary.edits, 1);
    }

    #[test]
    fn coalesce_restored_history_isolates_traces_across_interrupted_speaker_handoff() {
        let rows = vec![
            PersistentClawRow::ToolCall {
                pane_id: "pane-1".to_string(),
                agent_name: Some("Agent Alpha".to_string()),
                character_id: Some("char-alpha".to_string()),
                tool_name: "file_read".to_string(),
                result: "\"[Showing lines 1-5 of 5 in src/parser.rs]\"".to_string(),
                usage: None,
            },
            PersistentClawRow::ToolCall {
                pane_id: "pane-1".to_string(),
                agent_name: Some("Agent Beta".to_string()),
                character_id: Some("char-beta".to_string()),
                tool_name: "sys_shell_exec".to_string(),
                result: r#"{"command":"cargo check","stdout":"ok","stderr":"","exit_code":0}"#
                    .to_string(),
                usage: None,
            },
            PersistentClawRow::Diagnosis {
                pane_id: "pane-1".to_string(),
                agent_name: Some("Agent Beta".to_string()),
                character_id: Some("char-beta".to_string()),
                content: "Check passed.".to_string(),
                trace: Vec::new(),
                usage: None,
            },
        ];

        let coalesced = coalesce_restored_history(rows);
        assert_eq!(coalesced.len(), 2);
        match &coalesced[0] {
            PersistentClawRow::Diagnosis {
                character_id,
                content,
                trace,
                ..
            } => {
                assert_eq!(character_id.as_deref(), Some("char-alpha"));
                assert_eq!(content, "");
                let summary = ActivitySummary::summarize(trace);
                assert_eq!(summary.reads, 1);
                assert_eq!(summary.commands, 0);
            }
            other => panic!("expected flushed orphan Diagnosis for Alpha, got {other:?}"),
        }
        match &coalesced[1] {
            PersistentClawRow::Diagnosis {
                character_id,
                content,
                trace,
                ..
            } => {
                assert_eq!(character_id.as_deref(), Some("char-beta"));
                assert_eq!(content, "Check passed.");
                let summary = ActivitySummary::summarize(trace);
                assert_eq!(summary.commands, 1);
                assert_eq!(summary.reads, 0);
            }
            other => panic!("expected Diagnosis with Beta's command trace, got {other:?}"),
        }
    }
}
