use crate::engine::dispatcher::extract_command_and_clean;

pub fn process_snapshot(snapshot: &str, last_snapshot_hash: &mut Option<u64>) -> String {
    // 1. Snapshot Management: Only include if changed to save tokens
    let mut final_snapshot = snapshot.to_string();
    let current_hash = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        snapshot.hash(&mut hasher);
        hasher.finish()
    };

    if let Some(prev_hash) = last_snapshot_hash {
        if *prev_hash == current_hash {
            final_snapshot = "[TERMINAL_SNAPSHOT: NO_CHANGE]".to_string();
        }
    }
    *last_snapshot_hash = Some(current_hash);

    // 2. Security/Noise: Strip passwords and clear-screen sequences
    let extracted = extract_command_and_clean(&final_snapshot);
    extracted.1
}
