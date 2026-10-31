use boxxy_core_widgets::autocomplete::CompletionItem;
use frizbee::{Config as FrizbeeConfig, Matcher};
use gtk4 as gtk;
use gtk4::prelude::*;
use ignore::WalkBuilder;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const INDEX_TTL: Duration = Duration::from_secs(5);
const MAX_INDEXED_ENTRIES: usize = 25_000;
const MAX_HOME_INDEXED_ENTRIES: usize = 8_000;
const MAX_COMPLETION_RESULTS: usize = 30;
const MAX_FRECENCY_ENTRIES: usize = 1_000;

const IGNORED_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".flatpak-builder",
    ".direnv",
    "__pycache__",
    ".venv",
    "venv",
    ".next",
    "dist",
    "build",
    ".cache",
];

#[derive(Clone, Debug)]
pub struct WorkspaceEntry {
    /// Relative path from the indexed root (directories always end with `/`).
    pub rel_path: String,
    pub is_dir: bool,
    pub size_bytes: u64,
    pub mtime_secs: u64,
    pub depth: usize,
}

#[derive(Clone)]
struct CachedWorkspaceIndex {
    updated_at: Instant,
    entries: Vec<WorkspaceEntry>,
}

static WORKSPACE_INDEX_CACHE: std::sync::LazyLock<RwLock<HashMap<String, CachedWorkspaceIndex>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

static REFRESHING_DIRS: std::sync::LazyLock<RwLock<HashSet<String>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashSet::new()));

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FrecencyEntry {
    path: String,
    frequency: u32,
    last_open_ms: u64,
}

static FRECENCY_MAP: std::sync::LazyLock<RwLock<HashMap<String, FrecencyEntry>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));
static FRECENCY_LOADED: AtomicBool = AtomicBool::new(false);

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn frecency_file_path() -> PathBuf {
    boxxy_sys_utils::get_config_dir()
        .join("cache")
        .join("file_frecency.jsonl")
}

fn ensure_frecency_loaded_async() {
    if FRECENCY_LOADED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    boxxy_sys_utils::runtime().spawn(async {
        let path = frecency_file_path();
        let Ok(content) = tokio::fs::read_to_string(&path).await else {
            return;
        };
        let parsed = parse_frecency_jsonl(&content);
        let mut guard = FRECENCY_MAP.write();
        for entry in parsed {
            guard.insert(entry.path.clone(), entry);
        }
    });
}

fn parse_frecency_jsonl(text: &str) -> Vec<FrecencyEntry> {
    let mut map: HashMap<String, FrecencyEntry> = HashMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<FrecencyEntry>(trimmed) {
            map.insert(entry.path.clone(), entry);
        }
    }
    let mut vec: Vec<FrecencyEntry> = map.into_values().collect();
    vec.sort_by(|a, b| b.last_open_ms.cmp(&a.last_open_ms));
    vec.truncate(MAX_FRECENCY_ENTRIES);
    vec
}

fn calculate_frecency(entry: &FrecencyEntry, now_ms: u64) -> f64 {
    let elapsed_days = (now_ms.saturating_sub(entry.last_open_ms) as f64) / 86_400_000.0;
    (entry.frequency as f64) / (1.0 + elapsed_days)
}

fn get_frecency_score(cwd: &str, rel_path: &str, now_ms: u64) -> f64 {
    let trimmed = rel_path.trim_end_matches('/');
    let abs = Path::new(cwd).join(trimmed).to_string_lossy().into_owned();
    let guard = FRECENCY_MAP.read();
    guard
        .get(&abs)
        .or_else(|| guard.get(trimmed))
        .map(|e| calculate_frecency(e, now_ms))
        .unwrap_or(0.0)
}

/// Records a file selection in the OpenCode-style frecency store so frequently
/// and recently attached files float to the top of `@` completions.
pub fn record_file_frecency(cwd: &str, raw_replacement: &str) {
    ensure_frecency_loaded_async();
    let stripped = raw_replacement.trim().trim_start_matches('@');
    let (base_path, _) = parse_file_line_range(stripped);
    let clean_rel = base_path.trim_end_matches('/');
    if clean_rel.is_empty() {
        return;
    }

    let abs_path = if Path::new(clean_rel).is_absolute() {
        clean_rel.to_string()
    } else if let Some(rest) = clean_rel.strip_prefix("~/")
        && let Some(home) = home::home_dir()
    {
        home.join(rest).to_string_lossy().into_owned()
    } else if !cwd.is_empty() {
        Path::new(cwd).join(clean_rel).to_string_lossy().into_owned()
    } else {
        clean_rel.to_string()
    };

    let now_ms = now_epoch_ms();
    let (entry, snapshot_for_compact) = {
        let mut guard = FRECENCY_MAP.write();
        let next_freq = guard
            .get(&abs_path)
            .map(|e| e.frequency.saturating_add(1))
            .unwrap_or(1);
        let new_entry = FrecencyEntry {
            path: abs_path.clone(),
            frequency: next_freq,
            last_open_ms: now_ms,
        };
        guard.insert(abs_path, new_entry.clone());

        if guard.len() > MAX_FRECENCY_ENTRIES {
            let mut all: Vec<FrecencyEntry> = guard.values().cloned().collect();
            all.sort_by(|a, b| b.last_open_ms.cmp(&a.last_open_ms));
            all.truncate(MAX_FRECENCY_ENTRIES);
            *guard = all.iter().cloned().map(|e| (e.path.clone(), e)).collect();
            (new_entry, Some(all))
        } else {
            (new_entry, None)
        }
    };

    boxxy_sys_utils::runtime().spawn(async move {
        let path = frecency_file_path();
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        if let Some(compacted) = snapshot_for_compact {
            let mut out = String::new();
            for item in compacted {
                if let Ok(line) = serde_json::to_string(&item) {
                    out.push_str(&line);
                    out.push('\n');
                }
            }
            let _ = tokio::fs::write(&path, out).await;
        } else if let Ok(line) = serde_json::to_string(&entry) {
            use tokio::io::AsyncWriteExt;
            if let Ok(mut file) = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await
            {
                let _ = file.write_all(format!("{line}\n").as_bytes()).await;
            }
        }
    });
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedLineRange {
    /// Raw suffix as typed by the user, e.g. `"#20-45"` or `":10-30"` or `"#20-"`.
    pub raw_suffix: String,
    pub start_line: usize,
    pub end_line: Option<usize>,
}

/// Splits an `@` file query into `(base_path_query, Option<ParsedLineRange>)`.
/// Supports both OpenCode-style `#start-end` (`src/state.rs#20-45`) and
/// editor-style `:start-end` (`src/state.rs:20-45`).
pub fn parse_file_line_range(input: &str) -> (&str, Option<ParsedLineRange>) {
    for sep in ['#', ':'] {
        if let Some(idx) = input.rfind(sep) {
            if idx == 0 {
                continue;
            }
            let tail = &input[idx + 1..];
            if tail.is_empty() {
                // Allow trailing `#` or `:` while the user is about to type line numbers
                return (
                    &input[..idx],
                    Some(ParsedLineRange {
                        raw_suffix: input[idx..].to_string(),
                        start_line: 1,
                        end_line: None,
                    }),
                );
            }
            let (start_str, end_opt_str) = match tail.split_once('-') {
                Some((s, e)) => (s, Some(e)),
                None => (tail, None),
            };
            if !start_str.is_empty()
                && start_str.chars().all(|c| c.is_ascii_digit())
                && end_opt_str.is_none_or(|e| e.chars().all(|c| c.is_ascii_digit()))
                && let Ok(start_line) = start_str.parse::<usize>()
                && start_line >= 1
            {
                let end_line = end_opt_str
                    .filter(|e| !e.is_empty())
                    .and_then(|e| e.parse::<usize>().ok())
                    .filter(|&end| end >= start_line);
                return (
                    &input[..idx],
                    Some(ParsedLineRange {
                        raw_suffix: input[idx..].to_string(),
                        start_line,
                        end_line,
                    }),
                );
            }
        }
    }
    (input, None)
}

fn load_blacklist() -> Vec<String> {
    let mut blacklisted = vec![
        "/etc/shadow".to_string(),
        "/etc/gshadow".to_string(),
        "/root/.ssh".to_string(),
        ".ssh/id_rsa".to_string(),
        ".ssh/id_ed25519".to_string(),
    ];

    let blacklist_path = boxxy_sys_utils::get_config_dir()
        .join("boxxyclaw")
        .join("BLACKLIST.md");
    if let Ok(content) = std::fs::read_to_string(&blacklist_path) {
        let custom: Vec<String> = content
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(ToString::to_string)
            .collect();
        if !custom.is_empty() {
            blacklisted = custom;
        }
    }
    blacklisted
}

fn is_blacklisted(path_str: &str, blacklist: &[String]) -> bool {
    blacklist.iter().any(|b| path_str.contains(b))
}

fn is_home_or_root(dir: &Path) -> bool {
    if dir == Path::new("/") {
        return true;
    }
    if let Some(home) = home::home_dir()
        && dir == home
    {
        return true;
    }
    false
}

/// Synchronously scans `root_dir` (called on `tokio::task::spawn_blocking`) using
/// `ignore::WalkBuilder` and returns all workspace files and ancestor directories.
pub fn scan_workspace_entries(root_dir: &Path) -> Vec<WorkspaceEntry> {
    if !root_dir.is_dir() {
        return Vec::new();
    }

    let blacklist = load_blacklist();
    let shallow_mode = is_home_or_root(root_dir);
    let max_entries = if shallow_mode {
        MAX_HOME_INDEXED_ENTRIES
    } else {
        MAX_INDEXED_ENTRIES
    };

    let mut builder = WalkBuilder::new(root_dir);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .follow_links(false);

    if shallow_mode {
        builder.max_depth(Some(3));
    } else {
        builder.max_depth(Some(12));
    }

    let root_owned = root_dir.to_path_buf();
    builder.filter_entry(move |entry| {
        if entry.path() == root_owned {
            return true;
        }
        let name = entry.file_name().to_str().unwrap_or("");
        if entry.file_type().is_some_and(|ft| ft.is_dir()) {
            if IGNORED_DIRS.contains(&name) {
                return false;
            }
            if shallow_mode && name.starts_with('.') {
                return false;
            }
        }
        true
    });

    let mut files = Vec::new();
    let mut seen_dirs: HashSet<String> = HashSet::new();
    let mut dirs = Vec::new();

    for result in builder.build() {
        let Ok(entry) = result else {
            continue;
        };
        let path = entry.path();
        if path == root_dir {
            continue;
        }
        let path_lossy = path.to_string_lossy();
        if is_blacklisted(&path_lossy, &blacklist) {
            continue;
        }
        let Ok(rel) = path.strip_prefix(root_dir) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if rel_str.is_empty() {
            continue;
        }

        let Some(ft) = entry.file_type() else {
            continue;
        };

        // Extract ancestor directories (OpenCode ripgrepLayer pattern)
        let parts: Vec<&str> = rel_str.split('/').collect();
        let depth = parts.len();
        for depth_idx in 0..depth.saturating_sub(1) {
            let dir_rel = format!("{}/", parts[..=depth_idx].join("/"));
            if seen_dirs.insert(dir_rel.clone()) {
                dirs.push(WorkspaceEntry {
                    rel_path: dir_rel,
                    is_dir: true,
                    size_bytes: 0,
                    mtime_secs: 0,
                    depth: depth_idx + 1,
                });
            }
        }

        if ft.is_dir() {
            let dir_rel = format!("{rel_str}/");
            if seen_dirs.insert(dir_rel.clone()) {
                dirs.push(WorkspaceEntry {
                    rel_path: dir_rel,
                    is_dir: true,
                    size_bytes: 0,
                    mtime_secs: 0,
                    depth,
                });
            }
        } else if ft.is_file() {
            let (size_bytes, mtime_secs) = entry
                .metadata()
                .ok()
                .map(|m| {
                    let mtime = m
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    (m.len(), mtime)
                })
                .unwrap_or((0, 0));

            files.push(WorkspaceEntry {
                rel_path: rel_str,
                is_dir: false,
                size_bytes,
                mtime_secs,
                depth,
            });
        }

        if files.len() + dirs.len() >= max_entries {
            break;
        }
    }

    files.extend(dirs);
    files
}

/// Pre-warms or refreshes the background workspace file index for `cwd` without blocking the GTK thread.
pub fn refresh_workspace_index_async(
    cwd: String,
    entry_weak: Option<gtk::glib::WeakRef<gtk::TextView>>,
) {
    ensure_frecency_loaded_async();
    if cwd.trim().is_empty() {
        return;
    }

    {
        let mut refreshing = REFRESHING_DIRS.write();
        if !refreshing.insert(cwd.clone()) {
            return;
        }
    }

    let cwd_for_scan = cwd.clone();
    let scan_fut = async move {
        let entries = tokio::task::spawn_blocking({
            let cwd_path = PathBuf::from(&cwd_for_scan);
            move || scan_workspace_entries(&cwd_path)
        })
        .await
        .unwrap_or_default();

        WORKSPACE_INDEX_CACHE.write().insert(
            cwd_for_scan.clone(),
            CachedWorkspaceIndex {
                updated_at: Instant::now(),
                entries,
            },
        );
        REFRESHING_DIRS.write().remove(&cwd_for_scan);
    };

    if let Some(entry_weak) = entry_weak {
        gtk::glib::spawn_future_local(async move {
            let handle = boxxy_sys_utils::runtime().spawn(scan_fut);
            let _ = handle.await;
            if let Some(entry) = entry_weak.upgrade() {
                let buf = entry.buffer();
                let text = buf.text(&buf.start_iter(), &buf.end_iter(), false);
                if text.contains('@') {
                    buf.emit_by_name::<()>("changed", &[]);
                }
            }
        });
    } else {
        boxxy_sys_utils::runtime().spawn(scan_fut);
    }
}

/// Ranks a slice of `WorkspaceEntry` items against `raw_query` using `frizbee` (`0.13.0`)
/// SIMD Smith-Waterman fuzzy matching, OpenCode-style frecency boosting, and line-range preservation.
pub fn rank_workspace_entries(
    entries: &[WorkspaceEntry],
    cwd: &str,
    raw_query: &str,
    limit: usize,
) -> Vec<CompletionItem> {
    if entries.is_empty() {
        return Vec::new();
    }

    let (base_query_raw, line_range_opt) = parse_file_line_range(raw_query);
    let base_query = base_query_raw.trim_start_matches("./");
    let now_ms = now_epoch_ms();

    // If the user already typed a `#start-end` or `:start-end` suffix, only match files (not directories).
    let filtered_entries: Vec<&WorkspaceEntry> = entries
        .iter()
        .filter(|e| !(line_range_opt.is_some() && e.is_dir))
        .collect();

    if filtered_entries.is_empty() {
        return Vec::new();
    }

    if base_query.is_empty() {
        // Empty `@` query: sort by frecency DESC, shallow files & recent mtime DESC, then path length ASC
        let mut ranked: Vec<(&WorkspaceEntry, f64)> = filtered_entries
            .into_iter()
            .map(|e| {
                let frec = get_frecency_score(cwd, &e.rel_path, now_ms);
                (e, frec)
            })
            .collect();

        ranked.sort_by(|(a, frec_a), (b, frec_b)| {
            frec_b
                .partial_cmp(frec_a)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.is_dir.cmp(&b.is_dir)) // Files first on empty `@`, just like OpenCode Screenshot 1
                .then_with(|| a.depth.cmp(&b.depth))
                .then_with(|| b.mtime_secs.cmp(&a.mtime_secs))
                .then_with(|| a.rel_path.len().cmp(&b.rel_path.len()))
                .then_with(|| a.rel_path.cmp(&b.rel_path))
        });

        return ranked
            .into_iter()
            .take(limit)
            .map(|(entry, _)| build_completion_item(entry, line_range_opt.as_ref()))
            .collect();
    }

    // If query ends with `/` (e.g. `@crates/window/`), prioritize direct children of that directory
    let dir_prefix_filter = if base_query.ends_with('/') {
        Some(base_query)
    } else {
        None
    };

    if let Some(prefix) = dir_prefix_filter {
        let mut direct_children: Vec<&WorkspaceEntry> = filtered_entries
            .iter()
            .copied()
            .filter(|e| {
                if let Some(rest) = e.rel_path.strip_prefix(prefix) {
                    let trimmed = rest.trim_end_matches('/');
                    !trimmed.is_empty() && !trimmed.contains('/')
                } else {
                    false
                }
            })
            .collect();

        if !direct_children.is_empty() {
            direct_children.sort_by(|a, b| {
                b.is_dir
                    .cmp(&a.is_dir) // Folders first when drilling into a directory
                    .then_with(|| a.rel_path.cmp(&b.rel_path))
            });
            return direct_children
                .into_iter()
                .take(limit)
                .map(|entry| build_completion_item(entry, line_range_opt.as_ref()))
                .collect();
        }
    }

    let haystacks: Vec<&str> = filtered_entries
        .iter()
        .map(|e| e.rel_path.as_str())
        .collect();

    let config_exact = FrizbeeConfig::default().max_typos(Some(0));
    let mut matches = Matcher::new(base_query, &config_exact).match_list(&haystacks);

    // If 0-typo match returned nothing and needle is >= 4 chars, allow 1 typo via Frizbee's SIMD Smith-Waterman
    if matches.is_empty() && base_query.len() >= 4 {
        let config_typo = FrizbeeConfig::default().max_typos(Some(1));
        matches = Matcher::new(base_query, &config_typo).match_list(&haystacks);
    }

    let query_lower = base_query.to_lowercase();
    let mut scored: Vec<(&WorkspaceEntry, f64)> = matches
        .into_iter()
        .filter_map(|m| {
            let entry = *filtered_entries.get(m.index as usize)?;
            let mut score = f64::from(m.score);

            let file_name = entry
                .rel_path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(&entry.rel_path)
                .to_lowercase();

            if file_name == query_lower {
                score += 200.0;
            } else if file_name.starts_with(&query_lower) {
                score += 90.0;
            } else if file_name.contains(&query_lower) {
                score += 45.0;
            }

            if entry.rel_path.to_lowercase().starts_with(&query_lower) {
                score += 60.0;
            }

            let frec = get_frecency_score(cwd, &entry.rel_path, now_ms);
            score *= 1.0 + frec * 0.35;

            Some((entry, score))
        })
        .collect();

    scored.sort_by(|(a, score_a), (b, score_b)| {
        score_b
            .partial_cmp(score_a)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.rel_path.len().cmp(&b.rel_path.len()))
            .then_with(|| a.rel_path.cmp(&b.rel_path))
    });

    scored
        .into_iter()
        .take(limit)
        .map(|(entry, _)| build_completion_item(entry, line_range_opt.as_ref()))
        .collect()
}

fn build_completion_item(
    entry: &WorkspaceEntry,
    line_range: Option<&ParsedLineRange>,
) -> CompletionItem {
    let suffix = if entry.is_dir {
        ""
    } else {
        line_range.map_or("", |lr| lr.raw_suffix.as_str())
    };

    let display_name = format!("{}{}", entry.rel_path, suffix);
    let replacement_text = format!("@{}{}", entry.rel_path, suffix);

    let (icon_name, secondary_text) = if entry.is_dir {
        ("folder-symbolic".to_string(), "Folder".to_string())
    } else {
        let lower = entry.rel_path.to_lowercase();
        let is_img = lower.ends_with(".png")
            || lower.ends_with(".jpg")
            || lower.ends_with(".jpeg")
            || lower.ends_with(".webp")
            || lower.ends_with(".svg")
            || lower.ends_with(".gif");
        let icon = if is_img {
            "image-x-generic-symbolic"
        } else {
            "text-x-generic-symbolic"
        };
        let size_str = format_file_size(entry.size_bytes);
        let sec = if let Some(lr) = line_range {
            match lr.end_line {
                Some(end) => format!("L{}-{}", lr.start_line, end),
                None => format!("L{}", lr.start_line),
            }
        } else if is_img {
            format!("Image · {size_str}")
        } else {
            size_str
        };
        (icon.to_string(), sec)
    };

    CompletionItem {
        display_name,
        replacement_text,
        icon_name: Some(icon_name),
        icon_path: None,
        secondary_text: Some(secondary_text),
        badge_text: None,
        badge_color: None,
        disabled: false,
    }
}

fn format_file_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if (bytes as f64) < MB {
        format!("{:.1} KB", (bytes as f64) / KB)
    } else {
        format!("{:.1} MB", (bytes as f64) / MB)
    }
}

/// Queries the cached workspace file index for `cwd` (or triggers an async background scan if
/// not yet cached / stale). Also handles explicit `../`, `~/`, and `/` relative path traversal.
pub fn get_file_completions(
    cwd: &str,
    query: &str,
    entry_weak: Option<gtk::glib::WeakRef<gtk::TextView>>,
) -> Vec<CompletionItem> {
    let effective_cwd = if cwd.trim().is_empty() {
        std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".".to_string())
    } else {
        cwd.to_string()
    };

    let (base_query, _) = parse_file_line_range(query);

    // Handle explicit outside-CWD traversal (`../`, `~/`, `/`)
    if base_query.starts_with("../")
        || base_query == ".."
        || base_query.starts_with("~/")
        || base_query == "~"
        || base_query.starts_with('/')
    {
        return complete_explicit_traversal(&effective_cwd, query, entry_weak);
    }

    let (needs_refresh, entries) = {
        let cache = WORKSPACE_INDEX_CACHE.read();
        match cache.get(&effective_cwd) {
            Some(cached) => (
                cached.updated_at.elapsed() >= INDEX_TTL,
                cached.entries.clone(),
            ),
            None => (true, Vec::new()),
        }
    };

    if needs_refresh {
        refresh_workspace_index_async(effective_cwd.clone(), entry_weak);
    }

    rank_workspace_entries(&entries, &effective_cwd, query, MAX_COMPLETION_RESULTS)
}

fn complete_explicit_traversal(
    cwd: &str,
    raw_query: &str,
    entry_weak: Option<gtk::glib::WeakRef<gtk::TextView>>,
) -> Vec<CompletionItem> {
    let (base_query, line_range_opt) = parse_file_line_range(raw_query);

    let (target_root, display_prefix, sub_query) = if base_query == "~" {
        let Some(home) = home::home_dir() else {
            return Vec::new();
        };
        (home, "~/".to_string(), "")
    } else if let Some(rest) = base_query.strip_prefix("~/") {
        let Some(home) = home::home_dir() else {
            return Vec::new();
        };
        (home, "~/".to_string(), rest)
    } else if base_query == ".." {
        let parent = Path::new(cwd)
            .parent()
            .unwrap_or_else(|| Path::new(cwd))
            .to_path_buf();
        (parent, "../".to_string(), "")
    } else if base_query.starts_with("../") {
        let mut up_count = 0usize;
        let mut rem = base_query;
        while let Some(r) = rem.strip_prefix("../") {
            up_count += 1;
            rem = r;
        }
        let mut anc = Path::new(cwd);
        for _ in 0..up_count {
            if let Some(p) = anc.parent() {
                anc = p;
            }
        }
        (anc.to_path_buf(), "../".repeat(up_count), rem)
    } else {
        // Starts with `/`
        let slash_idx = base_query.rfind('/').unwrap_or(0);
        let dir_part = if slash_idx == 0 {
            "/"
        } else {
            &base_query[..=slash_idx]
        };
        let rem = &base_query[slash_idx + 1..];
        (PathBuf::from(dir_part), dir_part.to_string(), rem)
    };

    let root_key = target_root.to_string_lossy().into_owned();
    let (needs_refresh, entries) = {
        let cache = WORKSPACE_INDEX_CACHE.read();
        match cache.get(&root_key) {
            Some(cached) => (
                cached.updated_at.elapsed() >= INDEX_TTL,
                cached.entries.clone(),
            ),
            None => (true, Vec::new()),
        }
    };

    if needs_refresh {
        refresh_workspace_index_async(root_key.clone(), entry_weak);
    }

    let sub_with_range = match line_range_opt {
        Some(ref lr) => format!("{sub_query}{}", lr.raw_suffix),
        None => sub_query.to_string(),
    };

    let mut items =
        rank_workspace_entries(&entries, &root_key, &sub_with_range, MAX_COMPLETION_RESULTS);
    for item in &mut items {
        let stripped = item.replacement_text.trim_start_matches('@');
        let prefixed = format!(
            "{}{}",
            display_prefix,
            stripped.trim_start_matches('/')
        );
        item.display_name = prefixed.clone();
        item.replacement_text = format!("@{prefixed}");
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_file_line_range_hash_and_colon() {
        let (base, range) = parse_file_line_range("crates/window/src/state.rs#20-45");
        assert_eq!(base, "crates/window/src/state.rs");
        assert_eq!(
            range,
            Some(ParsedLineRange {
                raw_suffix: "#20-45".to_string(),
                start_line: 20,
                end_line: Some(45),
            })
        );

        let (base2, range2) = parse_file_line_range("src/module.rs:12-30");
        assert_eq!(base2, "src/module.rs");
        assert_eq!(
            range2,
            Some(ParsedLineRange {
                raw_suffix: ":12-30".to_string(),
                start_line: 12,
                end_line: Some(30),
            })
        );

        let (base3, range3) = parse_file_line_range("src/parser.rs#15");
        assert_eq!(base3, "src/parser.rs");
        assert_eq!(
            range3,
            Some(ParsedLineRange {
                raw_suffix: "#15".to_string(),
                start_line: 15,
                end_line: None,
            })
        );

        let (base4, range4) = parse_file_line_range("src/parser.rs");
        assert_eq!(base4, "src/parser.rs");
        assert!(range4.is_none());
    }

    #[test]
    fn test_scan_and_rank_workspace_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        std::fs::create_dir_all(root.join("src/widgets")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("src/widgets/overlay.rs"), "pub struct Overlay;\n").unwrap();
        std::fs::write(root.join("target/debug/artifact"), "bin").unwrap();

        let entries = scan_workspace_entries(root);
        // Should include Cargo.toml, src/main.rs, src/widgets/overlay.rs, src/, src/widgets/, and exclude target/
        assert!(entries.iter().any(|e| e.rel_path == "Cargo.toml" && !e.is_dir));
        assert!(entries.iter().any(|e| e.rel_path == "src/widgets/overlay.rs" && !e.is_dir));
        assert!(entries.iter().any(|e| e.rel_path == "src/" && e.is_dir));
        assert!(entries.iter().any(|e| e.rel_path == "src/widgets/" && e.is_dir));
        assert!(!entries.iter().any(|e| e.rel_path.starts_with("target")));

        let root_str = root.to_string_lossy();

        // Fuzzy query with line range preserved
        let results = rank_workspace_entries(&entries, &root_str, "overlay#10-25", 10);
        assert!(!results.is_empty());
        assert_eq!(
            results[0].replacement_text,
            "@src/widgets/overlay.rs#10-25"
        );
        assert_eq!(results[0].secondary_text.as_deref(), Some("L10-25"));

        // Directory drilling query (`src/`) lists direct children (`src/widgets/` and `src/main.rs`)
        let dir_results = rank_workspace_entries(&entries, &root_str, "src/", 10);
        assert_eq!(dir_results.len(), 2);
        assert_eq!(dir_results[0].replacement_text, "@src/widgets/");
        assert_eq!(dir_results[1].replacement_text, "@src/main.rs");
    }
}
