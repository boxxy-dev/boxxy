#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]

use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub history_json: Option<String>,
    pub pending_tasks_json: Option<String>,
    pub agent_name: Option<String>,
    pub character_id: String,
    pub character_display_name: String,
    pub last_cwd: Option<String>,
    pub title: Option<String>,
    pub model_id: Option<String>,
    pub pinned: bool,
    pub total_tokens: i64,
    #[sqlx(default)]
    pub context_tokens: i64,
    #[sqlx(default)]
    pub total_cost_usd: f64,
    pub last_dream_at: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    #[sqlx(default)]
    pub message_count: i64,
}

/// Typed payload for [`crate::store::Store::upsert_session`], avoiding error-prone
/// positional `&str` parameter lists when persisting per-turn session state.
#[derive(Debug, Clone, Copy)]
pub struct SessionUpsert<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub character_id: &'a str,
    pub character_display_name: &'a str,
    pub title: &'a str,
    pub history_json: &'a str,
    pub pending_tasks_json: &'a str,
    pub agent_name: &'a str,
    pub last_cwd: &'a str,
    pub model_id: &'a str,
    pub pinned: bool,
    pub total_tokens: i64,
    pub context_tokens: i64,
    pub total_cost_usd: f64,
}

/// Represents an episodic interaction or summary linked to a session.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Interaction {
    pub id: i64,
    pub session_id: String,
    pub project_path: Option<String>,
    pub content: String,
    pub metadata: Option<String>,
    pub embedding: Option<Vec<u8>>,
    pub processing_state: Option<String>,
    pub created_at: Option<String>,
    pub last_accessed_at: Option<String>,
}

/// Represents a global or project-scoped long-term fact or preference.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Memory {
    pub id: i64,
    pub key: String,
    pub project_path: String,
    pub content: String,
    pub category: Option<String>,
    pub verified: Option<bool>,
    pub pinned: Option<bool>,
    pub observation_count: i64,
    pub confidence_score: f64,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub last_accessed_at: Option<String>,
    pub access_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct SkillRecord {
    pub name: String,
    pub description: String,
    pub triggers: String,
    pub content: String,
    pub pinned: bool,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct MsgBarHistory {
    pub id: i64,
    pub text: String,
    pub attachments_json: String,
    pub created_at: Option<String>,
}

/// Durable local OpenTelemetry event queued in `telemetry_journal` for background export.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct TelemetryJournalEntry {
    pub id: i64,
    pub event_id: String,
    pub schema_version: i64,
    pub install_id: String,
    pub session_id: String,
    pub day_bucket: i64,
    pub recorded_at_ms: i64,
    pub metric_name: String,
    pub value: f64,
    pub attributes_json: String,
}

/// Typed payload for [`crate::store::Store::record_telemetry_events`].
#[derive(Debug, Clone, Copy)]
pub struct TelemetryJournalInsert<'a> {
    pub event_id: &'a str,
    pub schema_version: i64,
    pub install_id: &'a str,
    pub session_id: &'a str,
    pub recorded_at_ms: i64,
    pub metric_name: &'a str,
    pub value: f64,
    pub attributes_json: &'a str,
}

/// Typed acknowledgement for [`crate::store::Store::ack_telemetry_events`].
///
/// If new increments coalesced into row `id` while an HTTP export was in flight,
/// `exported_value` is subtracted atomically and `replacement_event_id` is assigned to
/// the residual delta so neither increments nor idempotency are lost.
#[derive(Debug, Clone, Copy)]
pub struct TelemetryJournalAck<'a> {
    pub id: i64,
    pub exported_value: f64,
    pub replacement_event_id: &'a str,
}

/// Summary of rows pruned during a background memory hygiene pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HygieneStats {
    pub deleted_interactions: u64,
    pub deleted_stale_candidates: u64,
    pub deleted_quarantine_noise: u64,
}

impl Interaction {
    #[must_use]
    pub fn parse_embedding(blob: Option<&[u8]>) -> Option<Vec<f32>> {
        blob.map(Self::parse_embedding_slice)
    }

    #[must_use]
    pub fn parse_embedding_slice(bytes: &[u8]) -> Vec<f32> {
        let (chunks, remainder) = bytes.as_chunks::<4>();
        if !remainder.is_empty() {
            log::warn!(
                "Embedding blob length ({}) is not a multiple of 4 bytes; ignoring {} trailing byte(s)",
                bytes.len(),
                remainder.len()
            );
        }
        chunks
            .iter()
            .map(|chunk| f32::from_le_bytes(*chunk))
            .collect()
    }

    #[must_use]
    pub fn serialize_embedding(f32s: &[f32]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(f32s.len().saturating_mul(4));
        for &f in f32s {
            bytes.extend_from_slice(&f.to_le_bytes());
        }
        bytes
    }
}
