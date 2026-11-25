pub const SNAPSHOT_EMPTY_MARKER: &str = "[TERMINAL_SNAPSHOT: EMPTY]";
pub const SNAPSHOT_NO_CHANGE_MARKER: &str = "[TERMINAL_SNAPSHOT: NO_CHANGE]";

const CLEAR_SCREEN_ESCAPES: &[&str] = &["\x1b[2J", "\x1b[3J", "\x1b[H", "\x1bc"];

#[must_use]
pub(crate) fn snapshot_hash(snapshot: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    snapshot.hash(&mut hasher);
    hasher.finish()
}

/// Strips raw terminal clear-screen/reset control escapes while preserving all structured
/// `OSC 133` semantic blocks (`[PROMPT]`, `[COMMAND]`, `[OUTPUT]`) and markdown code fences.
#[must_use]
fn sanitize_terminal_snapshot(raw: &str) -> String {
    let mut cleaned = if CLEAR_SCREEN_ESCAPES.iter().any(|seq| raw.contains(seq)) {
        let mut s = raw.to_string();
        for seq in CLEAR_SCREEN_ESCAPES {
            s = s.replace(seq, "");
        }
        s
    } else {
        raw.to_string()
    };

    let trimmed = cleaned.trim_matches(['\r', '\n']);
    if trimmed.trim().is_empty() {
        return SNAPSHOT_EMPTY_MARKER.to_string();
    }
    if trimmed.len() != cleaned.len() {
        cleaned = trimmed.to_string();
    }
    cleaned
}

#[must_use]
fn max_contiguous_backticks(s: &str) -> usize {
    let mut max_run = 0usize;
    let mut cur_run = 0usize;
    for b in s.bytes() {
        if b == b'`' {
            cur_run += 1;
            if cur_run > max_run {
                max_run = cur_run;
            }
        } else {
            cur_run = 0;
        }
    }
    max_run
}

/// Formats `Terminal Snapshot:` using a `CommonMark` fence strictly longer than any backtick run
/// inside `snapshot_text`.
///
/// Uses `3` backticks for plain output and `4+` backticks when `OSC 133`
/// ```` ```text ```` / ```` ```bash ```` blocks or markdown fences appear inside the snapshot.
#[must_use]
pub fn format_terminal_snapshot_block(snapshot_text: &str) -> String {
    let trimmed = snapshot_text.trim_matches(['\r', '\n']);
    let fence_len = max_contiguous_backticks(trimmed).saturating_add(1).max(3);
    let fence = "`".repeat(fence_len);
    format!("Terminal Snapshot:\n{fence}\n{trimmed}\n{fence}")
}

pub fn process_snapshot(snapshot: &str, last_snapshot_hash: &mut Option<u64>) -> String {
    let current_hash = snapshot_hash(snapshot);

    if snapshot.trim().is_empty() {
        *last_snapshot_hash = Some(current_hash);
        return SNAPSHOT_EMPTY_MARKER.to_string();
    }

    if let Some(prev_hash) = last_snapshot_hash
        && *prev_hash == current_hash
    {
        return SNAPSHOT_NO_CHANGE_MARKER.to_string();
    }
    *last_snapshot_hash = Some(current_hash);

    sanitize_terminal_snapshot(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_osc133_semantic_blocks_and_nests_outer_snapshot_fence() {
        let osc133 = "\n[PROMPT]\n```text\nbox$ \n```\n\n[COMMAND]\n```bash\ncargo check\n```\n\n[OUTPUT]\n```text\nerror[E0425]: cannot find value `port`\n```\n";
        let mut last_hash = None;
        let processed = process_snapshot(osc133, &mut last_hash);

        assert!(processed.contains("[PROMPT]\n```text\nbox$ "));
        assert!(processed.contains("[COMMAND]\n```bash\ncargo check"));
        assert!(processed.contains("error[E0425]: cannot find value `port`"));

        let block = format_terminal_snapshot_block(&processed);
        assert!(
            block.starts_with("Terminal Snapshot:\n````\n[PROMPT]"),
            "OSC 133 snapshot with 3-backtick blocks must wrap in a 4-backtick outer fence:\n{block}"
        );
        assert!(block.ends_with("\n````"));

        // Second call with identical buffer deduplicates to NO_CHANGE
        assert_eq!(
            process_snapshot(osc133, &mut last_hash),
            SNAPSHOT_NO_CHANGE_MARKER
        );
    }

    #[test]
    fn plain_snapshots_keep_standard_three_backtick_fence_and_strip_clear_escapes() {
        let mut last_hash = None;
        let processed = process_snapshot("\x1b[2J$ cargo test\nok\n", &mut last_hash);
        assert_eq!(processed, "$ cargo test\nok");
        assert_eq!(
            format_terminal_snapshot_block(&processed),
            "Terminal Snapshot:\n```\n$ cargo test\nok\n```"
        );
    }
}
