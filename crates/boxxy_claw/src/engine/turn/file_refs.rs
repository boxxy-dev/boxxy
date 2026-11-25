use base64::Engine as _;
use std::collections::HashSet;
use std::path::Path;

pub const REFERENCED_FILES_HEADER: &str = "--- REFERENCED FILES (PRE-LOADED VIA @MENTION) ---";
pub const REFERENCED_FILES_FOOTER: &str = "--- END REFERENCED FILES ---";

const DEFAULT_LINE_LIMIT: usize = 500;
const MAX_LINE_LIMIT: usize = 1000;
const MAX_SLICE_BYTES: usize = 64 * 1024;
const MAX_TOTAL_REF_BYTES: usize = 128 * 1024;
const MAX_LINE_CHARS: usize = 2000;
const BINARY_CHECK_BYTES: usize = 8192;
const MAX_DIR_ENTRIES: usize = 250;
const MAX_TEXT_FILE_BYTES: u64 = 5 * 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_STALE_CHECK_FILES: usize = 16;

#[derive(Debug, Default)]
pub struct ResolvedFileRefs {
    /// Formatted `--- REFERENCED FILES (PRE-LOADED VIA @MENTION) ---` markdown block
    /// to inject into the turn context.
    pub context_block: String,
    /// `(canonical_or_resolved_path, content_hash)` pairs to register in `SessionState.read_file_hashes`
    /// so the model can immediately call `replace_in_file` on Turn 1 without a redundant `file_read`.
    pub read_hashes: Vec<(String, u64)>,
    /// Base64-encoded image attachments resolved from `@path/to/image.png` tokens.
    pub image_attachments: Vec<String>,
    /// Inline `@mention` token and filename metadata paired 1:1 with `image_attachments`.
    pub image_attachment_meta: Vec<boxxy_claw_protocol::ImageAttachmentMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMentionToken {
    pub raw_token: String,
    pub path_str: String,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
}

/// Extracts `@<path>[#|:start[-end]]` tokens from a user prompt (outside fenced code blocks).
#[must_use]
pub fn extract_file_mention_tokens(text: &str) -> Vec<FileMentionToken> {
    let mut results = Vec::new();
    let mut seen = HashSet::new();
    let mut in_fence = false;

    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }

        for word in line.split_whitespace() {
            let Some(rest) = word.strip_prefix('@') else {
                continue;
            };
            let trimmed =
                rest.trim_end_matches([',', ';', '!', '?', ')', ']', '}', '"', '\'']);
            // Strip a trailing period unless the token is `..` or ends with `/.`
            let trimmed = if trimmed.ends_with('.') && trimmed != ".." && !trimmed.ends_with("/.") {
                trimmed.trim_end_matches('.')
            } else {
                trimmed
            };

            if trimmed.is_empty() {
                continue;
            }

            let (path_part, start_line, end_line) = split_line_range(trimmed);
            if path_part.is_empty() {
                continue;
            }

            let key = format!("{path_part}:{start_line:?}:{end_line:?}");
            if seen.insert(key) {
                results.push(FileMentionToken {
                    raw_token: format!("@{trimmed}"),
                    path_str: path_part.to_string(),
                    start_line,
                    end_line,
                });
            }
        }
    }

    results
}

fn split_line_range(token: &str) -> (&str, Option<usize>, Option<usize>) {
    for sep in ['#', ':'] {
        if let Some(idx) = token.rfind(sep) {
            if idx == 0 {
                continue;
            }
            let tail = &token[idx + 1..];
            if tail.is_empty() {
                continue;
            }
            let (start_str, end_opt_str) = match tail.split_once('-') {
                Some((s, e)) => (s, Some(e)),
                None => (tail, None),
            };
            if !start_str.is_empty()
                && start_str.chars().all(|c| c.is_ascii_digit())
                && end_opt_str.is_none_or(|e| !e.is_empty() && e.chars().all(|c| c.is_ascii_digit()))
                && let Ok(start) = start_str.parse::<usize>()
                && start >= 1
            {
                let end = end_opt_str
                    .and_then(|e| e.parse::<usize>().ok())
                    .filter(|&e| e >= start);
                return (&token[..idx], Some(start), end);
            }
        }
    }
    (token, None, None)
}

fn is_supported_image(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|ext| {
        ext.eq_ignore_ascii_case("png")
            || ext.eq_ignore_ascii_case("jpg")
            || ext.eq_ignore_ascii_case("jpeg")
            || ext.eq_ignore_ascii_case("webp")
            || ext.eq_ignore_ascii_case("gif")
    })
}

/// Resolves all `@<file>` / `@<dir>` / `@<image>` references in `prompt` relative to `cwd`
/// on a blocking worker thread.
pub async fn resolve_prompt_file_refs(prompt: &str, cwd: &str) -> ResolvedFileRefs {
    let tokens = extract_file_mention_tokens(prompt);
    if tokens.is_empty() {
        return ResolvedFileRefs::default();
    }

    let cwd_owned = cwd.to_string();
    tokio::task::spawn_blocking(move || resolve_tokens_sync(&tokens, &cwd_owned))
        .await
        .unwrap_or_default()
}

fn resolve_tokens_sync(tokens: &[FileMentionToken], cwd: &str) -> ResolvedFileRefs {
    let mut out = ResolvedFileRefs::default();
    let blacklist = boxxy_core_toolbox::blacklist::load_blacklist();
    let mut sections = Vec::new();
    let mut total_bytes_used = 0usize;

    for token in tokens {
        let resolved_path = boxxy_core_toolbox::utils::resolve_path(cwd, &token.path_str);
        let path = Path::new(&resolved_path);
        if boxxy_core_toolbox::blacklist::is_path_blacklisted(path, &blacklist) {
            continue;
        }

        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };

        if meta.is_dir() {
            if let Some(dir_section) = format_directory_ref(&token.raw_token, &resolved_path, path)
            {
                total_bytes_used = total_bytes_used.saturating_add(dir_section.len());
                sections.push(dir_section);
            }
        } else if meta.is_file() {
            if is_supported_image(&resolved_path) && meta.len() <= MAX_IMAGE_BYTES {
                if let Ok(bytes) = std::fs::read(path) {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                    let label = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(&token.path_str)
                        .to_string();
                    out.image_attachments.push(b64);
                    out.image_attachment_meta
                        .push(boxxy_claw_protocol::ImageAttachmentMeta {
                            token: token.raw_token.clone(),
                            label,
                        });
                    sections.push(format!(
                        "### `{}` (`{}`)\n[Attached image ({} bytes) into multimodal vision input]",
                        token.raw_token,
                        resolved_path,
                        meta.len()
                    ));
                }
                continue;
            }

            if meta.len() > MAX_TEXT_FILE_BYTES {
                sections.push(format!(
                    "### `{}` (`{}`)\n[File is too large ({} bytes > 5 MB limit) to preload; use `grep_search` or `file_read` if applicable]",
                    token.raw_token,
                    resolved_path,
                    meta.len()
                ));
                continue;
            }

            if total_bytes_used >= MAX_TOTAL_REF_BYTES {
                break;
            }

            if let Some((file_section, content_hash)) =
                format_file_ref(token, &resolved_path, path)
            {
                total_bytes_used = total_bytes_used.saturating_add(file_section.len());
                out.read_hashes.push((resolved_path, content_hash));
                sections.push(file_section);
            }
        }
    }

    if !sections.is_empty() {
        out.context_block = format!(
            "\n{REFERENCED_FILES_HEADER}\n\
            The user attached the following workspace file(s)/folder(s) with `@`. Their contents are already loaded below and registered for `replace_in_file` — do NOT call `file_read` on these line ranges again unless you need lines outside the shown window:\n\n\
            {}\n\
            {REFERENCED_FILES_FOOTER}\n",
            sections.join("\n\n")
        );
    }

    out
}

fn format_file_ref(
    token: &FileMentionToken,
    resolved_path: &str,
    path: &Path,
) -> Option<(String, u64)> {
    let bytes = std::fs::read(path).ok()?;
    let check_len = bytes.len().min(BINARY_CHECK_BYTES);
    if bytes[..check_len].contains(&0) {
        return Some((
            format!(
                "### `{}` (`{}`)\n[Binary file ({} bytes) — skipped text preview]",
                token.raw_token,
                resolved_path,
                bytes.len()
            ),
            0,
        ));
    }

    let content = String::from_utf8_lossy(&bytes);
    let content_hash = boxxy_core_toolbox::hash_file_content(&content);
    let all_lines: Vec<&str> = content.lines().collect();
    let total_lines = all_lines.len();

    if total_lines == 0 {
        return Some((
            format!(
                "### `{}` (`{}`)\n[Empty file: 0 lines]",
                token.raw_token, resolved_path
            ),
            content_hash,
        ));
    }

    let start_line = token.start_line.unwrap_or(1).max(1);
    if start_line > total_lines {
        return Some((
            format!(
                "### `{}` (`{}`)\n[Requested start_line {} exceeds file length of {} lines]",
                token.raw_token, resolved_path, start_line, total_lines
            ),
            content_hash,
        ));
    }

    let requested_limit = match (token.start_line, token.end_line) {
        (Some(s), Some(e)) if e >= s => (e - s + 1).min(MAX_LINE_LIMIT),
        _ => DEFAULT_LINE_LIMIT,
    };

    let start_idx = start_line - 1;
    let max_end_idx = (start_idx + requested_limit).min(total_lines);

    let mut body = String::new();
    let mut actual_end_idx = start_idx;
    let mut hit_byte_cap = false;

    for (idx, raw_line) in all_lines[start_idx..max_end_idx].iter().enumerate() {
        let line_num = start_line + idx;
        let formatted_line = if raw_line.chars().count() > MAX_LINE_CHARS {
            let truncated: String = raw_line.chars().take(MAX_LINE_CHARS).collect();
            format!("{line_num}: {truncated}... [line truncated]\n")
        } else {
            format!("{line_num}: {raw_line}\n")
        };

        if body.len() + formatted_line.len() > MAX_SLICE_BYTES && idx > 0 {
            hit_byte_cap = true;
            break;
        }
        body.push_str(&formatted_line);
        actual_end_idx = start_idx + idx + 1;
    }

    let mut section = format!(
        "### `{}` (`{}`)\n[Showing lines {}-{} of {}]\n```text\n{}```",
        token.raw_token,
        resolved_path,
        start_line,
        actual_end_idx,
        total_lines,
        body
    );

    if actual_end_idx < total_lines {
        let next_line = actual_end_idx + 1;
        let reason = if hit_byte_cap { "64 KB slice cap" } else { "line window" };
        section.push_str(&format!(
            "\n[Truncated at {reason}. Call `file_read` with `start_line: {next_line}` if you need the rest of the file.]"
        ));
    }

    Some((section, content_hash))
}

fn format_directory_ref(raw_token: &str, resolved_path: &str, path: &Path) -> Option<String> {
    let read_dir = std::fs::read_dir(path).ok()?;
    let mut dirs = Vec::new();
    let mut files = Vec::new();

    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Ok(ft) = entry.file_type() {
            if ft.is_dir() {
                dirs.push(format!("{name}/"));
            } else {
                let size = entry.metadata().map_or(0, |m| m.len());
                files.push(format!("{name} ({size} bytes)"));
            }
        } else {
            files.push(name);
        }
    }

    dirs.sort();
    files.sort();

    let total = dirs.len() + files.len();
    let entries: Vec<String> = dirs
        .into_iter()
        .chain(files)
        .take(MAX_DIR_ENTRIES)
        .collect();

    Some(format!(
        "### `{}` (`{}/`, {} entries)\n```text\n{}\n```",
        raw_token,
        resolved_path.trim_end_matches('/'),
        total,
        entries.join("\n")
    ))
}

/// Checks files previously read or edited in this session (`tracked_hashes`) against disk,
/// skipping any path already freshly pre-loaded on the current turn (`freshly_loaded`).
///
/// Returns `(notice_block, deleted_paths_to_prune)` so the model never relies on stale
/// `file_read` history after the user edits or deletes a file externally.
pub async fn check_externally_modified_files<S: std::hash::BuildHasher>(
    env: &dyn boxxy_claw_protocol::ClawEnvironment,
    tracked_hashes: &std::collections::HashMap<String, u64, S>,
    freshly_loaded: &[(String, u64)],
) -> (String, Vec<String>) {
    if tracked_hashes.is_empty() {
        return (String::new(), Vec::new());
    }

    let mut candidates: Vec<(String, u64)> = tracked_hashes
        .iter()
        .filter(|(path, _)| !freshly_loaded.iter().any(|(fresh, _)| fresh == *path))
        .map(|(path, &hash)| (path.clone(), hash))
        .collect();
    if candidates.is_empty() {
        return (String::new(), Vec::new());
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    candidates.truncate(MAX_STALE_CHECK_FILES);

    let checks = futures_util::future::join_all(candidates.into_iter().map(
        |(path, known_hash)| async move {
            match env.read_file(path.clone(), 0, 0).await {
                Ok(disk_content) => {
                    let disk_hash = boxxy_core_toolbox::hash_file_content(&disk_content);
                    (path, disk_hash != known_hash, false)
                }
                Err(e) => {
                    let msg = e.to_string();
                    let is_missing =
                        msg.contains("File not found") || msg.contains("No such file");
                    (path, false, is_missing)
                }
            }
        },
    ))
    .await;

    let mut modified_lines = Vec::new();
    let mut deleted_paths = Vec::new();
    for (path, is_modified, is_deleted) in checks {
        if is_modified {
            modified_lines.push(format!(
                "- `{path}` (modified on disk — your earlier `file_read` output in history is STALE)"
            ));
        } else if is_deleted {
            modified_lines.push(format!("- `{path}` (deleted or moved on disk)"));
            deleted_paths.push(path);
        }
    }

    if modified_lines.is_empty() {
        return (String::new(), deleted_paths);
    }

    let notice = format!(
        "\n--- EXTERNALLY MODIFIED FILES NOTICE ---\n\
        The following file(s) you read or edited earlier in this session were changed on disk outside this chat (e.g. edited by the user in an editor or by a command):\n\
        {}\n\
        CRITICAL: Do NOT trust or quote earlier `file_read` outputs in conversation history for any file listed above. Call `file_read` on the file FIRST to inspect the latest disk contents before answering questions about it or calling `replace_in_file` / `file_write`.\n\
        ----------------------------------------\n",
        modified_lines.join("\n")
    );

    (notice, deleted_paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_file_mention_tokens_and_ranges() {
        let prompt = "Check @src/module.rs#10-20 and @src/parser.rs:5-15, plus @Cargo.toml.";
        let tokens = extract_file_mention_tokens(prompt);
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0].path_str, "src/module.rs");
        assert_eq!(tokens[0].start_line, Some(10));
        assert_eq!(tokens[0].end_line, Some(20));

        assert_eq!(tokens[1].path_str, "src/parser.rs");
        assert_eq!(tokens[1].start_line, Some(5));
        assert_eq!(tokens[1].end_line, Some(15));

        assert_eq!(tokens[2].path_str, "Cargo.toml");
        assert_eq!(tokens[2].start_line, None);
    }

    #[tokio::test]
    async fn test_resolve_prompt_file_refs_preloads_and_hashes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let file_path = root.join("lib.rs");
        std::fs::write(&file_path, "line one\nline two\nline three\nline four\n").unwrap();

        let resolved = resolve_prompt_file_refs(
            "Please refactor @lib.rs#2-3 right now.",
            &root.to_string_lossy(),
        )
        .await;

        assert!(resolved.context_block.contains("Showing lines 2-3 of 4"));
        assert!(resolved.context_block.contains("2: line two"));
        assert!(resolved.context_block.contains("3: line three"));
        assert!(!resolved.context_block.contains("1: line one"));
        assert_eq!(resolved.read_hashes.len(), 1);
        assert_eq!(resolved.read_hashes[0].0, file_path.to_string_lossy());
    }

    #[tokio::test]
    async fn shared_blacklist_blocks_auth_tokens_codex_and_symlinked_keys_in_at_mentions() {
        let merged =
            boxxy_core_toolbox::blacklist::parse_blacklist_with_defaults(Some("# comment\nsecrets.toml\n"));
        assert!(merged.iter().any(|r| r == "/etc/shadow"));
        assert!(merged.iter().any(|r| r == ".ssh/id_ed25519"));
        assert!(merged.iter().any(|r| r == ".codex/auth.json"));
        assert!(merged.iter().any(|r| r == "secrets.toml"));

        let tmp = tempfile::tempdir().unwrap();
        let ssh_dir = tmp.path().join(".ssh");
        std::fs::create_dir_all(&ssh_dir).unwrap();
        let secret_key = ssh_dir.join("id_ed25519");
        std::fs::write(&secret_key, "PRIVATE KEY DATA").unwrap();

        let codex_dir = tmp.path().join(".codex");
        std::fs::create_dir_all(&codex_dir).unwrap();
        std::fs::write(codex_dir.join("auth.json"), "{\"access_token\":\"secret\"}").unwrap();

        let workspace = tmp.path().join("project");
        std::fs::create_dir_all(&workspace).unwrap();
        let link = workspace.join("innocent_note.txt");
        std::os::unix::fs::symlink(&secret_key, &link).unwrap();

        assert!(boxxy_core_toolbox::blacklist::is_path_blacklisted(
            &link, &merged
        ));

        // Verify `resolve_prompt_file_refs` refuses to preload symlinked keys or `.codex/auth.json`
        let resolved = resolve_prompt_file_refs(
            "Inspect @innocent_note.txt and @../.codex/auth.json",
            &workspace.to_string_lossy(),
        )
        .await;
        assert_eq!(resolved.context_block, "");
        assert_eq!(resolved.read_hashes, Vec::<(String, u64)>::new());
    }

    #[tokio::test]
    async fn check_externally_modified_files_emits_notice_only_when_disk_diverges() {
        struct SyntheticDiskEnv {
            files: std::collections::HashMap<String, String>,
        }

        #[async_trait::async_trait]
        impl boxxy_claw_protocol::ClawEnvironment for SyntheticDiskEnv {
            async fn exec_shell(&self, _: String) -> anyhow::Result<(i32, String, String)> {
                Ok((0, String::new(), String::new()))
            }
            async fn spawn_detached(&self, _: String, _: String) -> anyhow::Result<u32> {
                Ok(1)
            }
            async fn read_file(
                &self,
                path: String,
                _start: u32,
                _end: u32,
            ) -> anyhow::Result<String> {
                self.files
                    .get(&path)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("File not found: {path}"))
            }
            async fn write_file(&self, _: String, _: String) -> anyhow::Result<()> {
                Ok(())
            }
            async fn list_directory(
                &self,
                _: String,
            ) -> anyhow::Result<Vec<(String, bool, u64)>> {
                Ok(Vec::new())
            }
            async fn delete_file(&self, _: String) -> anyhow::Result<()> {
                Ok(())
            }
            async fn kill_process(&self, _: u32, _: i32) -> anyhow::Result<()> {
                Ok(())
            }
            async fn get_clipboard(&self) -> anyhow::Result<String> {
                Ok(String::new())
            }
            async fn set_clipboard(&self, _: String) -> anyhow::Result<()> {
                Ok(())
            }
        }

        let mut disk_files = std::collections::HashMap::new();
        disk_files.insert("/workspace/src/unchanged.rs".to_string(), "fn a() {}\n".to_string());
        disk_files.insert(
            "/workspace/src/edited.rs".to_string(),
            "fn b() { /* externally modified */ }\n".to_string(),
        );
        disk_files.insert(
            "/workspace/src/reattached.rs".to_string(),
            "fn c() { /* fresh @mention */ }\n".to_string(),
        );
        let env = SyntheticDiskEnv { files: disk_files };

        let mut tracked = std::collections::HashMap::new();
        tracked.insert(
            "/workspace/src/unchanged.rs".to_string(),
            boxxy_core_toolbox::hash_file_content("fn a() {}\n"),
        );

        // 1. All tracked files match disk -> zero-token empty notice
        let (empty_notice, pruned_none) =
            check_externally_modified_files(&env, &tracked, &[]).await;
        assert_eq!(empty_notice, "");
        assert_eq!(pruned_none, Vec::<String>::new());

        // 2. Track modified, deleted, and freshly @-attached files
        tracked.insert(
            "/workspace/src/edited.rs".to_string(),
            boxxy_core_toolbox::hash_file_content("fn b() {}\n"),
        );
        tracked.insert(
            "/workspace/src/removed.rs".to_string(),
            boxxy_core_toolbox::hash_file_content("fn gone() {}\n"),
        );
        tracked.insert(
            "/workspace/src/reattached.rs".to_string(),
            boxxy_core_toolbox::hash_file_content("fn c() {}\n"),
        );

        let freshly_loaded = vec![("/workspace/src/reattached.rs".to_string(), 123_u64)];
        let (notice, deleted) =
            check_externally_modified_files(&env, &tracked, &freshly_loaded).await;

        assert!(notice.contains("EXTERNALLY MODIFIED FILES NOTICE"));
        assert!(notice.contains("/workspace/src/edited.rs"));
        assert!(notice.contains("/workspace/src/removed.rs"));
        assert!(!notice.contains("/workspace/src/unchanged.rs"));
        assert!(!notice.contains("/workspace/src/reattached.rs"));
        assert_eq!(deleted, vec!["/workspace/src/removed.rs".to_string()]);
    }
}
