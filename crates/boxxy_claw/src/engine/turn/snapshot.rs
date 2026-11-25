use crate::engine::dispatcher::extract_command_and_clean;

pub const SNAPSHOT_EMPTY_MARKER: &str = "[TERMINAL_SNAPSHOT: EMPTY]";
pub const SNAPSHOT_NO_CHANGE_MARKER: &str = "[TERMINAL_SNAPSHOT: NO_CHANGE]";

#[must_use]
pub(crate) fn snapshot_hash(snapshot: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    snapshot.hash(&mut hasher);
    hasher.finish()
}

pub fn process_snapshot(snapshot: &str, last_snapshot_hash: &mut Option<u64>) -> String {
    // 1. Snapshot Management: Only include if changed to save tokens
    let current_hash = snapshot_hash(snapshot);

    if snapshot.trim().is_empty() {
        *last_snapshot_hash = Some(current_hash);
        return SNAPSHOT_EMPTY_MARKER.to_string();
    }

    let final_snapshot = if let Some(prev_hash) = last_snapshot_hash
        && *prev_hash == current_hash
    {
        SNAPSHOT_NO_CHANGE_MARKER.to_string()
    } else {
        snapshot.to_string()
    };
    *last_snapshot_hash = Some(current_hash);

    // 2. Security/Noise: Strip passwords and clear-screen sequences
    let extracted = extract_command_and_clean(&final_snapshot);
    extracted.1
}
