//! Host filesystem operations, path blacklist policy, and shell output spillover.

pub(super) const EXEC_SHELL_MAX_OUTPUT_BYTES: usize = 50 * 1024;
const EXEC_SHELL_MAX_LINES: usize = 2_000;
const READ_FILE_MAX_RAW_BYTES: u64 = 5 * 1024 * 1024;
const READ_FILE_MAX_SLICE_BYTES: usize = 64 * 1024;

const BUILTIN_BLACKLIST: &[&str] = &[
    "/etc/shadow",
    "/etc/gshadow",
    "/etc/sudoers",
    "/root/.ssh",
    ".ssh/id_rsa",
    ".ssh/id_ed25519",
    ".ssh/id_ed25519_sk",
    ".ssh/id_ecdsa",
    ".ssh/id_ecdsa_sk",
    ".ssh/id_dsa",
    ".ssh/authorized_keys",
    ".gnupg/private-keys-v1.d",
    ".aws/credentials",
];

pub(super) async fn read_file(
    path: String,
    start_line: u32,
    end_line: u32,
) -> anyhow::Result<String> {
    ensure_path_allowed(&path)?;

    let metadata = match tokio::fs::metadata(&path).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let suggestions = suggest_similar_files(&path).await;
            if suggestions.is_empty() {
                return Err(anyhow::anyhow!("File not found: {path}"));
            }
            return Err(anyhow::anyhow!(
                "File not found: {path}\n\nDid you mean one of these?\n{}",
                suggestions.join("\n")
            ));
        }
        Err(e) => return Err(e.into()),
    };
    if metadata.is_dir() {
        return Err(anyhow::anyhow!(
            "Path is a directory, not a file: {path} (use `list_directory` instead)"
        ));
    }
    if metadata.len() > READ_FILE_MAX_RAW_BYTES {
        return Err(anyhow::anyhow!(
            "File is too large ({} bytes > 5 MB limit) to read into memory",
            metadata.len()
        ));
    }

    let bytes = tokio::fs::read(&path).await?;
    let probe_len = bytes.len().min(8192);
    if bytes[..probe_len].contains(&0) {
        return Err(anyhow::anyhow!(
            "File appears to be binary and cannot be read as text: {path}"
        ));
    }

    let content = String::from_utf8(bytes)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());

    if start_line == 0 && end_line == 0 {
        Ok(content)
    } else {
        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len() as u32;

        if total_lines == 0 {
            return Ok(format!("[Read file {path}, 0 lines]\n"));
        }

        let start = if start_line > 0 {
            start_line.saturating_sub(1)
        } else {
            0
        };
        let end = if end_line > 0 {
            end_line.min(total_lines)
        } else {
            total_lines
        };

        if start >= total_lines {
            return Ok(format!(
                "[WARNING: Start line {} exceeds total lines {}]",
                start + 1,
                total_lines
            ));
        }

        let selected_lines = &lines[start as usize..end as usize];
        let mut result = selected_lines.join("\n");

        if result.len() > READ_FILE_MAX_SLICE_BYTES {
            let cut = result.floor_char_boundary(READ_FILE_MAX_SLICE_BYTES);
            result.truncate(cut);
            result.push_str(
                "\n\n[NOTE: Output capped at 64 KB. Specify a narrower start_line/end_line range to read more.]",
            );
        }

        if end < total_lines {
            result.push_str(&format!(
                "\n[Output truncated. Continue reading with start_line: {}]",
                end + 1
            ));
        }

        result = format!(
            "[Showing lines {}-{} of {}]\n{}",
            start + 1,
            end,
            total_lines,
            result
        );

        Ok(result)
    }
}

pub(super) async fn write_file(path: String, content: String) -> anyhow::Result<()> {
    ensure_path_allowed(&path)?;

    if let Some(parent) = std::path::Path::new(&path).parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent).await?;
    }

    tokio::fs::write(&path, content).await?;
    Ok(())
}

pub(super) async fn list_directory(path: String) -> anyhow::Result<Vec<(String, bool, u64)>> {
    ensure_path_allowed(&path)?;

    let mut entries = Vec::new();
    let mut dir = tokio::fs::read_dir(&path).await?;

    while let Some(entry) = dir.next_entry().await? {
        let metadata = entry.metadata().await.ok();
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        entries.push((name, is_dir, size));
    }

    // Deterministic sort: directories first, then case-insensitive alphabetical
    entries.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });

    Ok(entries)
}

pub(super) async fn delete_file(path: String) -> anyhow::Result<()> {
    ensure_path_allowed(&path)?;

    tokio::fs::remove_file(&path).await?;
    Ok(())
}

async fn suggest_similar_files(missing_path: &str) -> Vec<String> {
    let p = std::path::Path::new(missing_path);
    let Some(parent) = p.parent() else {
        return Vec::new();
    };
    let Some(base) = p
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .filter(|s| !s.is_empty())
    else {
        return Vec::new();
    };

    let Ok(mut dir) = tokio::fs::read_dir(parent).await else {
        return Vec::new();
    };

    let mut suggestions = Vec::new();
    while let Ok(Some(entry)) = dir.next_entry().await {
        let candidate_os = entry.file_name();
        let candidate_name = candidate_os.to_string_lossy();
        let candidate_lower = candidate_name.to_lowercase();
        if candidate_lower.contains(&base) || base.contains(&candidate_lower) {
            suggestions.push(parent.join(&candidate_os).to_string_lossy().into_owned());
        }
    }
    suggestions.sort();
    suggestions.truncate(3);
    suggestions
}

fn resolve_spillover_dir() -> std::path::PathBuf {
    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        runtime_dir.join("boxxy-terminal").join("tool-output")
    } else {
        boxxy_sys_utils::get_config_dir()
            .join("cache")
            .join("tool-output")
    }
}

fn spill_truncated_output_to_disk(full_output: &str) -> Option<String> {
    let dir = resolve_spillover_dir();
    spill_truncated_output_in_dir(&dir, full_output)
}

fn spill_truncated_output_in_dir(dir: &std::path::Path, full_output: &str) -> Option<String> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(dir).ok()?;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));

    // Opportunistically sweep spillover logs older than 24 hours so the
    // tool-output directory never accumulates stale files over time.
    if let Ok(entries) = std::fs::read_dir(dir) {
        let cutoff =
            std::time::SystemTime::now().checked_sub(std::time::Duration::from_secs(24 * 3600));
        for entry in entries.flatten() {
            if let (Some(cutoff), Ok(meta)) = (cutoff, entry.metadata())
                && let Ok(modified) = meta.modified()
                && modified < cutoff
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    let file_path = dir.join(format!("shell-{}.log", uuid::Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&file_path)
        .ok()?;
    file.write_all(full_output.as_bytes()).ok()?;
    Some(file_path.to_string_lossy().into_owned())
}

pub(super) fn truncate_head_tail(s: &str, max_bytes: usize) -> String {
    let line_count = s.lines().count();
    if s.len() <= max_bytes && line_count <= EXEC_SHELL_MAX_LINES {
        return s.to_string();
    }

    let effective_budget = if s.len() > max_bytes {
        max_bytes
    } else {
        // Triggered by line count (> 2,000 lines); cap to ~80% of byte budget
        (s.len() * 3 / 4).min(max_bytes).max(1024)
    };

    let head_budget = effective_budget / 4;
    let tail_budget = effective_budget.saturating_sub(head_budget);
    let head_end = s.floor_char_boundary(head_budget.min(s.len()));
    let tail_start = s
        .ceil_char_boundary(s.len().saturating_sub(tail_budget))
        .max(head_end);
    let omitted = tail_start.saturating_sub(head_end);

    let spillover_hint = match spill_truncated_output_to_disk(s) {
        Some(path) => format!(
            "\nFull untruncated output saved to: {path}\nUse `grep_search` or `file_read` (with `start_line`/`end_line`) on `{path}` to inspect specific sections."
        ),
        None => String::new(),
    };

    format!(
        "{}\n\n...[truncated {omitted} bytes of middle output]{spillover_hint}...\n\n{}",
        &s[..head_end],
        &s[tail_start..]
    )
}

fn normalize_lexical_path(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut normalized = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            c => normalized.push(c.as_os_str()),
        }
    }
    normalized
}

fn resolve_deepest_canonical(path: &std::path::Path) -> Option<std::path::PathBuf> {
    if let Ok(c) = std::fs::canonicalize(path) {
        return Some(c);
    }
    use std::path::Component;
    let mut acc = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                acc.pop();
                if !acc.as_os_str().is_empty()
                    && let Ok(canon) = std::fs::canonicalize(&acc)
                {
                    acc = canon;
                }
            }
            c => {
                acc.push(c.as_os_str());
                if let Ok(canon) = std::fs::canonicalize(&acc) {
                    acc = canon;
                }
            }
        }
    }
    (!acc.as_os_str().is_empty()).then_some(acc)
}

fn path_matches_blacklist_entry(candidate_path: &str, rule: &str) -> bool {
    let rule = rule.trim().trim_end_matches('/');
    if rule.is_empty() {
        return false;
    }
    let cand = candidate_path.trim_end_matches('/');
    if rule.starts_with('/') {
        // Root-anchored prefix rule: matches `/etc/shadow` or `/root/.ssh/...`
        cand == rule
            || cand
                .strip_prefix(rule)
                .is_some_and(|rest| rest.starts_with('/'))
    } else {
        // Segment-boundary rule: e.g. `.ssh/id_ed25519` or `.env` matches
        // `.env`, `/proj/.env`, or `/proj/.env/keys`, without falsely blocking
        // `src/environment.rs`.
        if cand == rule {
            return true;
        }
        if cand
            .strip_prefix(rule)
            .is_some_and(|rest| rest.starts_with('/'))
        {
            return true;
        }
        if let Some(prefix) = cand.strip_suffix(rule)
            && prefix.ends_with('/')
        {
            return true;
        }
        let mid_needle = format!("/{rule}/");
        cand.contains(&mid_needle)
    }
}

fn ensure_path_allowed_with_rules(path: &str, blacklisted: &[String]) -> anyhow::Result<()> {
    let p = std::path::Path::new(path);
    let lexical = normalize_lexical_path(p).to_string_lossy().into_owned();
    let anchored = if p.is_relative() {
        std::env::current_dir()
            .map(|cwd| cwd.join(p))
            .unwrap_or_else(|_| p.to_path_buf())
    } else {
        p.to_path_buf()
    };
    let canonical = resolve_deepest_canonical(&anchored).map(|cp| cp.to_string_lossy().into_owned());

    for black_path in blacklisted {
        if path_matches_blacklist_entry(path, black_path)
            || path_matches_blacklist_entry(&lexical, black_path)
            || canonical
                .as_deref()
                .is_some_and(|c| path_matches_blacklist_entry(c, black_path))
        {
            return Err(anyhow::anyhow!("Path is blacklisted for security reasons"));
        }
    }
    Ok(())
}

fn ensure_path_allowed(path: &str) -> anyhow::Result<()> {
    let blacklisted = load_blacklist();
    ensure_path_allowed_with_rules(path, &blacklisted)
}

fn parse_blacklist_with_defaults(custom_content: Option<&str>) -> Vec<String> {
    let mut blacklisted: Vec<String> = BUILTIN_BLACKLIST.iter().map(|s| (*s).to_string()).collect();
    if let Some(content) = custom_content {
        for line in content.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty()
                && !trimmed.starts_with('#')
                && !blacklisted.iter().any(|existing| existing == trimmed)
            {
                blacklisted.push(trimmed.to_string());
            }
        }
    }
    blacklisted
}

fn load_blacklist() -> Vec<String> {
    let config_dir = boxxy_sys_utils::get_config_dir();
    let blacklist_path = config_dir.join("boxxyclaw").join("BLACKLIST.md");
    let custom = std::fs::read_to_string(&blacklist_path).ok();
    parse_blacklist_with_defaults(custom.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_head_tail_preserves_both_ends_and_utf8_boundaries() {
        let input = format!("START_MARKER_{}_END_MARKER", "x".repeat(1000));
        let truncated = truncate_head_tail(&input, 200);
        assert!(truncated.starts_with("START_MARKER_"));
        assert!(truncated.ends_with("_END_MARKER"));
        assert!(truncated.contains("...[truncated "));
        assert!(truncated.contains("Full untruncated output saved to:"));
    }

    #[test]
    fn ensure_path_allowed_blocks_blacklisted_and_dotdot_traversal() {
        assert!(ensure_path_allowed("/etc/shadow").is_err());
        assert!(ensure_path_allowed("/tmp/../etc/shadow").is_err());
        assert!(ensure_path_allowed("/home/someone/.ssh/id_ed25519").is_err());

        let tmp = tempfile::tempdir().unwrap();
        let safe_file = tmp.path().join("notes.txt");
        assert!(ensure_path_allowed(&safe_file.to_string_lossy()).is_ok());

        // Symlink ancestor pointing to /etc with multiple non-existent nested segments
        let link_path = tmp.path().join("etc_link");
        std::os::unix::fs::symlink("/etc", &link_path).unwrap();
        let nested_via_symlink = link_path.join("nonexistent_sub").join("..").join("shadow");
        assert!(
            ensure_path_allowed(&nested_via_symlink.to_string_lossy()).is_err(),
            "symlink ancestor with non-existent intermediate segments must still resolve and block /etc/shadow"
        );
    }

    #[test]
    fn custom_blacklist_extends_defaults_and_matches_segment_boundaries() {
        let rules = parse_blacklist_with_defaults(Some("# comment\n.env\nconfig/secrets.toml\n"));
        // Built-in defaults must remain active even when custom entries exist
        assert!(ensure_path_allowed_with_rules("/etc/shadow", &rules).is_err());
        assert!(ensure_path_allowed_with_rules(".ssh/id_rsa", &rules).is_err());
        // Custom segment rules block exact path segments without substring false positives
        assert!(ensure_path_allowed_with_rules(".env", &rules).is_err());
        assert!(ensure_path_allowed_with_rules("/workspace/app/.env", &rules).is_err());
        assert!(
            ensure_path_allowed_with_rules("/workspace/app/config/secrets.toml", &rules).is_err()
        );
        assert!(ensure_path_allowed_with_rules("src/environment.rs", &rules).is_ok());
        assert!(
            ensure_path_allowed_with_rules("/workspace/app/src/dotenv_parser.rs", &rules).is_ok()
        );
    }

    #[test]
    fn spillover_creates_owner_only_dir_and_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let out_dir = tmp.path().join("tool-output");
        let saved = spill_truncated_output_in_dir(&out_dir, "synthetic output payload")
            .expect("spillover should write log file");

        let dir_mode = std::fs::metadata(&out_dir).unwrap().permissions().mode() & 0o777;
        let file_mode = std::fs::metadata(&saved).unwrap().permissions().mode() & 0o600;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(
            std::fs::metadata(&saved).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(file_mode, 0o600);
    }

    #[tokio::test]
    async fn read_file_includes_header_and_suggests_similar_on_not_found_and_write_creates_parents()
    {
        let tmp = tempfile::tempdir().unwrap();
        let nested_file = tmp.path().join("deep/nested/config_loader.rs");

        // 1. write_file auto-creates missing parent directories
        write_file(
            nested_file.to_string_lossy().into_owned(),
            "line1\nline2\nline3\n".to_string(),
        )
        .await
        .expect("write_file should auto-create parent dirs");

        // 2. read_file always includes [Showing lines X-Y of Z] header
        let read_all = read_file(nested_file.to_string_lossy().into_owned(), 1, 500)
            .await
            .unwrap();
        assert!(read_all.starts_with("[Showing lines 1-3 of 3]\nline1\nline2\nline3"));

        // 3. Truncated read includes continuation hint
        let read_page = read_file(nested_file.to_string_lossy().into_owned(), 1, 2)
            .await
            .unwrap();
        assert!(read_page.contains("[Showing lines 1-2 of 3]"));
        assert!(read_page.contains("[Output truncated. Continue reading with start_line: 3]"));

        // 4. Missing file suggests similar sibling filename
        let missing = tmp.path().join("deep/nested/config_loader");
        let err = read_file(missing.to_string_lossy().into_owned(), 1, 500)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Did you mean one of these?"));
        assert!(err.contains("config_loader.rs"));
    }
}
