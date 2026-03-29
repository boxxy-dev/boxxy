#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::single_option_map)]
use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub history_json: Option<String>,
    pub pending_tasks_json: Option<String>,
    pub agent_name: Option<String>,
    pub last_cwd: Option<String>,
    pub title: Option<String>,
    pub model_id: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    #[sqlx(default)]
    pub message_count: i64,
}

/// Represents an episodic interaction or summary linked to a session
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Interaction {
    pub id: i64,
    pub session_id: String,
    pub project_path: Option<String>,
    pub content: String,
    pub metadata: Option<String>,
    pub embedding: Option<Vec<u8>>,
    pub created_at: Option<String>,
    pub last_accessed_at: Option<String>,
}

/// Represents a global, long-term fact or preference
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Memory {
    pub id: i64,
    pub key: String,
    pub project_path: String,
    pub content: String,
    pub category: Option<String>,
    pub verified: Option<bool>,
    pub pinned: Option<bool>,
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
    pub attachments: String, // Stored as a JSON string
    pub created_at: Option<String>,
}

// Helper methods for embeddings (used in interactions)
impl Interaction {
    #[must_use]
    pub fn parse_embedding(blob: Option<Vec<u8>>) -> Option<Vec<f32>> {
        blob.map(|bytes| {
            let mut f32s = Vec::with_capacity(bytes.len() / 4);
            for chunk in bytes.chunks_exact(4) {
                f32s.push(f32::from_le_bytes(chunk.try_into().unwrap()));
            }
            f32s
        })
    }

    #[must_use]
    pub fn serialize_embedding(f32s: &[f32]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(f32s.len() * 4);
        for &f in f32s {
            bytes.extend_from_slice(&f.to_le_bytes());
        }
        bytes
    }
}
