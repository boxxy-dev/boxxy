use crate::pty::registry::PtyRegistry;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct AgentState {
    pub tracked_pids: Arc<RwLock<HashSet<u32>>>,
    pub api_keys: Arc<RwLock<HashMap<String, String>>>,
    pub ollama_url: Arc<RwLock<String>>,
    /// Daemon-wide PTY session table. Carries viewer count, persistence
    /// flag, and (when detached) the reader task + ring buffer. See
    /// `crate::pty::registry` for the full lifecycle. Already
    /// cheap-cloneable via an internal `Arc`; don't double-wrap.
    pub pty_registry: PtyRegistry,
    /// PIDs requested for restoration by an external desktop companion while
    /// no Boxxy UI window was open. Drained by the first window that connects.
    pub pending_restores: Arc<RwLock<Vec<u32>>>,
}

impl Default for AgentState {
    fn default() -> Self {
        Self {
            tracked_pids: Arc::new(RwLock::new(HashSet::new())),
            api_keys: Arc::new(RwLock::new(HashMap::new())),
            ollama_url: Arc::new(RwLock::new(String::new())),
            pty_registry: PtyRegistry::new(),
            pending_restores: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl AgentState {
    pub fn new() -> Self {
        Self::default()
    }
}
