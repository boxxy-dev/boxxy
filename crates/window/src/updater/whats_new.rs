use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use self_update::http::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use serde::{Deserialize, Serialize};

use super::{GITHUB_API_BASE, REPO_NAME, REPO_OWNER, USER_AGENT_VALUE, Updater};

const WHATS_NEW_MARKER_FILE: &str = ".whats_new_pending";
const WHATS_NEW_LAST_FILE: &str = "whats_new_last.json";
const WHATS_NEW_CACHE_FILE: &str = "whats_new_cache.json";

/// Maximum commit range depth / list size for the What's New dialog.
pub const MAX_WHATS_NEW_COMMITS: usize = 100;

/// Marker persisted across the update restart so the newly launched binary knows
/// which commit it upgraded from and can present the "What's New" dialog once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhatsNewPending {
    pub from_commit: String,
    pub to_commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitEntry {
    pub short_sha: String,
    pub message: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContributorInfo {
    pub login: String,
    pub avatar_url: Option<String>,
    #[serde(skip)]
    pub avatar_bytes: Option<Vec<u8>>,
}

/// Lightweight summary of a GitHub comparison (`{from_commit}...{to_commit}`).
/// Code patches (`files[].patch`) are intentionally ignored during deserialization
/// so zero source code diffs are held in memory or sent to the LLM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangelogSummary {
    pub from_commit: String,
    pub to_commit: String,
    pub total_commits: usize,
    pub files_changed: usize,
    pub additions: usize,
    pub deletions: usize,
    pub commits: Vec<CommitEntry>,
    #[serde(default)]
    pub contributors: Vec<ContributorInfo>,
}

impl ChangelogSummary {
    /// Formats a concise one-line fun stat summary for UI display.
    pub fn stats_badge_line(&self) -> String {
        let commit_word = if self.total_commits == 1 {
            "commit"
        } else {
            "commits"
        };
        let file_word = if self.files_changed == 1 {
            "file"
        } else {
            "files"
        };
        format!(
            "{} {} · {} {} changed · +{} lines added, -{} lines removed",
            self.total_commits,
            commit_word,
            self.files_changed,
            file_word,
            self.additions,
            self.deletions
        )
    }
}

impl CommitEntry {
    /// Returns the first line of the commit message (the commit subject/title).
    #[must_use]
    pub fn title(&self) -> &str {
        self.message
            .lines()
            .next()
            .unwrap_or(&self.message)
            .trim()
    }

    /// Formats a concise secondary metadata string (e.g. `e158d50 · 3h ago`) for the commit row caption.
    #[must_use]
    pub fn meta_caption(&self) -> String {
        if let Some(ref raw_date) = self.date
            && let Some(formatted) = format_commit_date(raw_date)
        {
            format!("{}  ·  {}", self.short_sha, formatted)
        } else {
            self.short_sha.clone()
        }
    }
}

fn format_commit_date(raw: &str) -> Option<String> {
    let dt = chrono::DateTime::parse_from_rfc3339(raw.trim()).ok()?;
    let now = chrono::Utc::now();
    let diff = now.signed_duration_since(dt.with_timezone(&chrono::Utc));
    let secs = diff.num_seconds();
    if secs < 0 {
        return Some(dt.format("%b %-d").to_string());
    }
    if secs < 60 {
        Some("just now".to_string())
    } else if secs < 3600 {
        Some(format!("{}m ago", secs / 60))
    } else if secs < 86_400 {
        Some(format!("{}h ago", secs / 3600))
    } else if secs < 86_400 * 14 {
        let days = secs / 86_400;
        if days == 1 {
            Some("yesterday".to_string())
        } else {
            Some(format!("{days}d ago"))
        }
    } else {
        Some(dt.format("%b %-d").to_string())
    }
}

#[derive(Debug, Deserialize)]
struct GithubComparePayload {
    #[serde(default)]
    total_commits: usize,
    #[serde(default)]
    commits: Vec<GithubCompareCommitItem>,
    #[serde(default)]
    files: Vec<GithubCompareFileStat>,
}

#[derive(Debug, Deserialize)]
struct GithubCompareCommitItem {
    sha: String,
    commit: GithubCompareCommitInner,
    #[serde(default)]
    author: Option<GithubCompareUser>,
}

#[derive(Debug, Deserialize)]
struct GithubCompareCommitInner {
    message: String,
    #[serde(default)]
    author: Option<GithubCompareGitAuthor>,
}

#[derive(Debug, Deserialize)]
struct GithubCompareUser {
    login: String,
    #[serde(default)]
    avatar_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubCompareGitAuthor {
    name: String,
    #[serde(default)]
    date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubCompareFileStat {
    #[serde(default)]
    additions: usize,
    #[serde(default)]
    deletions: usize,
    // `patch` is intentionally omitted so serde skips unified code diffs completely.
}

pub fn shorten_sha(sha: &str) -> String {
    let trimmed = sha.trim();
    if trimmed.len() > 7 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        trimmed[..7].to_string()
    } else {
        trimmed.to_string()
    }
}

/// Clamps `~N` commit-depth expressions (such as `HEAD~1300` or `a1b2c3d~500`) to
/// `MAX_WHATS_NEW_COMMITS` (100) so out-of-range ancestry queries do not 404 on GitHub.
/// Returns `(normalized_ref, was_clamped)`.
pub fn sanitize_commit_ref(raw: &str) -> (String, bool) {
    let trimmed = raw.trim();
    if let Some((base, depth_str)) = trimmed.rsplit_once('~')
        && !base.is_empty()
        && !depth_str.is_empty()
        && depth_str.chars().all(|c| c.is_ascii_digit())
    {
        let base_short = shorten_sha(base);
        let depth = depth_str.parse::<usize>().unwrap_or(MAX_WHATS_NEW_COMMITS + 1);
        if depth > MAX_WHATS_NEW_COMMITS {
            return (format!("{base_short}~{MAX_WHATS_NEW_COMMITS}"), true);
        }
        return (format!("{base_short}~{depth}"), false);
    }
    (shorten_sha(trimmed), false)
}

pub fn parse_simulate_update_range(raw: &str) -> Option<WhatsNewPending> {
    let trimmed = raw.trim();
    let (from, to) = trimmed
        .split_once("...")
        .or_else(|| trimmed.split_once(".."))?;
    let (from, _) = sanitize_commit_ref(from);
    let (to, _) = sanitize_commit_ref(to);
    if from.is_empty() || to.is_empty() {
        return None;
    }
    Some(WhatsNewPending {
        from_commit: from,
        to_commit: to,
    })
}

pub(crate) fn write_whats_new_pending_in(
    app_dir: &Path,
    from_commit: &str,
    to_commit: &str,
) -> Result<()> {
    let from_short = shorten_sha(from_commit);
    let to_short = shorten_sha(to_commit);
    if from_short.is_empty() || from_short == "unknown" {
        return Ok(());
    }
    let pending = WhatsNewPending {
        from_commit: from_short,
        to_commit: to_short,
    };
    let json = serde_json::to_string(&pending)?;
    fs::write(app_dir.join(WHATS_NEW_MARKER_FILE), &json)?;
    let _ = fs::write(app_dir.join(WHATS_NEW_LAST_FILE), &json);
    Ok(())
}

pub(crate) fn take_whats_new_pending_in(
    app_dir: &Path,
    running_commit: &str,
) -> Option<WhatsNewPending> {
    let path = app_dir.join(WHATS_NEW_MARKER_FILE);
    let content = fs::read_to_string(&path).ok()?;
    // Always remove the marker on read so the dialog is presented only once.
    let _ = fs::remove_file(&path);

    let mut pending: WhatsNewPending = serde_json::from_str(content.trim()).ok()?;
    pending.from_commit = shorten_sha(&pending.from_commit);
    pending.to_commit = shorten_sha(&pending.to_commit);

    let running_short = shorten_sha(running_commit);
    if (pending.to_commit.is_empty() || pending.to_commit == "nightly")
        && !running_short.is_empty()
        && running_short != "unknown"
    {
        pending.to_commit = running_short;
    }

    if pending.from_commit.is_empty()
        || pending.to_commit.is_empty()
        || pending.from_commit == pending.to_commit
    {
        return None;
    }

    if let Ok(json) = serde_json::to_string(&pending) {
        let _ = fs::write(app_dir.join(WHATS_NEW_LAST_FILE), json);
    }

    Some(pending)
}

/// Checks if a post-update "What's New" dialog should be shown on this launch.
/// Supports `BOXXY_SIMULATE_UPDATE="commit_a..commit_b"` for instant local testing.
pub fn take_whats_new_pending() -> Option<WhatsNewPending> {
    if let Ok(sim) = std::env::var("BOXXY_SIMULATE_UPDATE")
        && let Some(pending) = parse_simulate_update_range(&sim)
    {
        log::info!(
            "Simulating post-update What's New dialog via BOXXY_SIMULATE_UPDATE ({} -> {})",
            pending.from_commit,
            pending.to_commit
        );
        save_last_whats_new(&pending);
        return Some(pending);
    }

    let app_dir = Updater::get_app_dir().ok()?;
    let pending = take_whats_new_pending_in(&app_dir, super::CURRENT_COMMIT)?;
    save_last_whats_new(&pending);
    Some(pending)
}

/// Persists the most recently viewed or installed commit range so the Command Palette's
/// "What's New" action can reopen it at any time.
pub fn save_last_whats_new(pending: &WhatsNewPending) {
    let Ok(json) = serde_json::to_string(pending) else {
        return;
    };
    let config_dir = boxxy_sys_utils::get_config_dir();
    let _ = fs::create_dir_all(&config_dir);
    let _ = fs::write(config_dir.join(WHATS_NEW_LAST_FILE), &json);
    if let Ok(app_dir) = Updater::get_app_dir()
        && app_dir.exists()
    {
        let _ = fs::write(app_dir.join(WHATS_NEW_LAST_FILE), &json);
    }
}

/// Loads the last update commit range for the "What's New" Command Palette action,
/// falling back to `BOXXY_SIMULATE_UPDATE` or the latest 5 commits ending at `CURRENT_COMMIT`.
pub fn load_last_whats_new() -> WhatsNewPending {
    if let Ok(sim) = std::env::var("BOXXY_SIMULATE_UPDATE")
        && let Some(pending) = parse_simulate_update_range(&sim)
    {
        return pending;
    }

    let candidates = [
        Some(boxxy_sys_utils::get_config_dir().join(WHATS_NEW_LAST_FILE)),
        Updater::get_app_dir()
            .ok()
            .map(|d| d.join(WHATS_NEW_LAST_FILE)),
    ];

    for path in candidates.into_iter().flatten() {
        if let Ok(raw) = fs::read_to_string(&path)
            && let Ok(pending) = serde_json::from_str::<WhatsNewPending>(raw.trim())
            && !pending.from_commit.is_empty()
            && !pending.to_commit.is_empty()
        {
            return pending;
        }
    }

    let running = shorten_sha(super::CURRENT_COMMIT);
    if !running.is_empty() && running != "unknown" {
        WhatsNewPending {
            from_commit: format!("{running}~5"),
            to_commit: running,
        }
    } else {
        WhatsNewPending {
            from_commit: "HEAD~5".to_string(),
            to_commit: "HEAD".to_string(),
        }
    }
}

/// Persists the most recently fetched `ChangelogSummary` so reopening the "What's New"
/// dialog for the same commit range loads in 0ms without waiting on GitHub.
pub fn save_cached_changelog_summary(summary: &ChangelogSummary) {
    let Ok(json) = serde_json::to_string(summary) else {
        return;
    };
    let config_dir = boxxy_sys_utils::get_config_dir();
    let _ = fs::create_dir_all(&config_dir);
    let _ = fs::write(config_dir.join(WHATS_NEW_CACHE_FILE), &json);
    if let Ok(app_dir) = Updater::get_app_dir()
        && app_dir.exists()
    {
        let _ = fs::write(app_dir.join(WHATS_NEW_CACHE_FILE), &json);
    }
}

/// Loads a cached `ChangelogSummary` if it matches the requested `pending` commit range.
pub fn load_cached_changelog_summary(pending: &WhatsNewPending) -> Option<ChangelogSummary> {
    let from = shorten_sha(&pending.from_commit);
    let to = shorten_sha(&pending.to_commit);

    let candidates = [
        Some(boxxy_sys_utils::get_config_dir().join(WHATS_NEW_CACHE_FILE)),
        Updater::get_app_dir()
            .ok()
            .map(|d| d.join(WHATS_NEW_CACHE_FILE)),
    ];

    for path in candidates.into_iter().flatten() {
        if let Ok(raw) = fs::read_to_string(&path)
            && let Ok(summary) = serde_json::from_str::<ChangelogSummary>(raw.trim())
            && summary.from_commit == from
            && summary.to_commit == to
            && summary.total_commits > 0
            && summary
                .commits
                .first()
                .is_some_and(|c| c.author.is_some() || c.date.is_some())
        {
            return Some(summary);
        }
    }

    None
}

/// Fetches contributor avatars concurrently (up to 5) in the background so the main
/// changelog summary can be rendered immediately without waiting on image downloads.
pub async fn fetch_contributor_avatars(
    contributors: &[ContributorInfo],
) -> Vec<(String, Vec<u8>)> {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    else {
        return Vec::new();
    };

    let mut set = tokio::task::JoinSet::new();
    for contributor in contributors.iter().take(5) {
        let Some(ref raw_url) = contributor.avatar_url else {
            continue;
        };
        let sized_url = if raw_url.contains('?') {
            format!("{raw_url}&s=64")
        } else {
            format!("{raw_url}?s=64")
        };
        let login = contributor.login.clone();
        let client_c = client.clone();
        set.spawn(async move {
            if let Ok(img_resp) = client_c
                .get(&sized_url)
                .header(USER_AGENT, USER_AGENT_VALUE)
                .send()
                .await
                && img_resp.status().is_success()
                && let Ok(bytes) = img_resp.bytes().await
                && bytes.len() <= 256 * 1024
            {
                Some((login, bytes.to_vec()))
            } else {
                None
            }
        });
    }

    let mut out = Vec::new();
    while let Some(res) = set.join_next().await {
        if let Ok(Some(pair)) = res {
            out.push(pair);
        }
    }
    out
}

/// Fetches the commit messages and line-change counts between `from_commit` and `to_commit`
/// from GitHub's Compare API without loading code diffs.
pub async fn fetch_changelog_summary(
    from_commit: &str,
    to_commit: &str,
) -> Result<ChangelogSummary> {
    let from = from_commit.trim();
    let to = to_commit.trim();
    let url = format!(
        "{}/repos/{}/{}/compare/{}...{}",
        GITHUB_API_BASE.trim_end_matches('/'),
        REPO_OWNER,
        REPO_NAME,
        from,
        to
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .build()?;

    let token = Updater::github_token_from_env();
    let mut req = client
        .get(&url)
        .header(USER_AGENT, USER_AGENT_VALUE)
        .header(ACCEPT, "application/vnd.github+json");

    if let Some(ref token) = token {
        req = req.header(AUTHORIZATION, format!("Bearer {token}"));
    }

    let mut resp = req.send().await?;
    if (resp.status() == reqwest::StatusCode::UNAUTHORIZED
        || resp.status() == reqwest::StatusCode::FORBIDDEN)
        && token.is_some()
    {
        resp = client
            .get(&url)
            .header(USER_AGENT, USER_AGENT_VALUE)
            .header(ACCEPT, "application/vnd.github+json")
            .send()
            .await?;
    }

    let status = resp.status();
    if !status.is_success() {
        let err = self_update::Error::http_status_error_with_headers(
            status.as_u16(),
            &url,
            resp.headers(),
        );
        if let self_update::Error::RateLimited { .. } = &err {
            log::warn!(
                "GitHub API rate limit reached while fetching changelog summary (retry delay: {:?})",
                err.rate_limit_delay()
            );
        }
        return Err(err).with_context(|| {
            format!("GitHub compare API returned HTTP {status} for {from}...{to}")
        });
    }

    let body = resp.text().await?;
    let summary = parse_compare_payload(from, to, &body)?;
    save_cached_changelog_summary(&summary);

    Ok(summary)
}

pub(crate) fn parse_compare_payload(
    from_commit: &str,
    to_commit: &str,
    json_str: &str,
) -> Result<ChangelogSummary> {
    let payload: GithubComparePayload = serde_json::from_str(json_str)
        .context("Failed to deserialize GitHub compare payload")?;

    let additions: usize = payload.files.iter().map(|f| f.additions).sum();
    let deletions: usize = payload.files.iter().map(|f| f.deletions).sum();
    let files_changed = payload.files.len();

    let mut contributors = Vec::new();
    let mut seen_contributors = std::collections::HashSet::new();

    for item in payload.commits.iter().rev() {
        let candidate = item
            .author
            .as_ref()
            .and_then(|u| {
                let login = u.login.trim();
                (!login.is_empty()).then(|| (login.to_string(), u.avatar_url.clone()))
            })
            .or_else(|| {
                item.commit.author.as_ref().and_then(|a| {
                    let name = a.name.trim();
                    (!name.is_empty()).then(|| (name.to_string(), None))
                })
            });

        if let Some((login, avatar_url)) = candidate
            && seen_contributors.insert(login.to_lowercase())
        {
            contributors.push(ContributorInfo {
                login,
                avatar_url,
                avatar_bytes: None,
            });
        }
    }

    let mut commits: Vec<CommitEntry> = payload
        .commits
        .into_iter()
        .map(|item| {
            let author = item
                .author
                .as_ref()
                .map(|u| u.login.trim().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    item.commit
                        .author
                        .as_ref()
                        .map(|a| a.name.trim().to_string())
                        .filter(|s| !s.is_empty())
                });
            let date = item
                .commit
                .author
                .as_ref()
                .and_then(|a| a.date.as_ref())
                .map(|d| d.trim().to_string())
                .filter(|s| !s.is_empty());

            CommitEntry {
                short_sha: shorten_sha(&item.sha),
                message: item.commit.message.trim().to_string(),
                author,
                date,
            }
        })
        .collect();

    let total_commits = if payload.total_commits > 0 {
        payload.total_commits
    } else {
        commits.len()
    };

    if commits.len() > MAX_WHATS_NEW_COMMITS {
        let drop_count = commits.len() - MAX_WHATS_NEW_COMMITS;
        commits.drain(0..drop_count);
    }

    Ok(ChangelogSummary {
        from_commit: shorten_sha(from_commit),
        to_commit: shorten_sha(to_commit),
        total_commits,
        files_changed,
        additions,
        deletions,
        commits,
        contributors,
    })
}

/// Builds the prompt sent to the Claw overlay's default character when the user clicks
/// "Read What's New". Everything after `\n\n--- UPDATE CHANGELOG CONTEXT ---` is stripped
/// from the visible user bubble so the chat history stays clean while the model gets
/// the full commit log and diff stats without calling any tools.
pub fn build_claw_whats_new_prompt(
    pending: &WhatsNewPending,
    summary: Option<&ChangelogSummary>,
) -> String {
    let user_visible_header = format!(
        "Read me what's new in Boxxy Terminal from commit `{}` to `{}`!",
        pending.from_commit, pending.to_commit
    );

    let mut ctx = String::from("\n\n--- UPDATE CHANGELOG CONTEXT ---\n");
    ctx.push_str(
        "Boxxy Terminal was just updated! Summarize what changed for the user in a clear, \
         friendly, and exciting way based ONLY on the commit messages and line statistics below.\n\
         CRITICAL RULES:\n\
         - Do NOT call any tools (`http_fetch`, `web_search`, `file_read`, or shell commands) and do NOT read any source code.\n\
         - Highlight the practical benefits and improvements for the user.\n\
         - Include a fun fact at the end about the update statistics (how many lines of code were added/removed across how many files and commits).\n\n",
    );

    if let Some(s) = summary
        && !s.commits.is_empty()
    {
        let total_lines_touched = s.additions + s.deletions;
        ctx.push_str(&format!(
            "### Update Statistics (`{}` -> `{}`)\n\
             - Commits: {}\n\
             - Files changed: {}\n\
             - Lines added: +{}\n\
             - Lines deleted: -{}\n\
             - Total lines touched: {}\n\n\
             ### Commit Messages\n",
            s.from_commit,
            s.to_commit,
            s.total_commits,
            s.files_changed,
            s.additions,
            s.deletions,
            total_lines_touched
        ));

        for entry in s.commits.iter().take(50) {
            let formatted_msg = entry.message.replace('\n', "\n  ");
            ctx.push_str(&format!("- `{}`: {}\n", entry.short_sha, formatted_msg));
        }
    } else {
        ctx.push_str(&format!(
            "Compare URL: https://github.com/{}/{}/compare/{}...{}\n\
             (Commit metadata could not be pre-fetched; let the user know Boxxy updated from `{}` to `{}` and share the compare link above.)",
            REPO_OWNER,
            REPO_NAME,
            pending.from_commit,
            pending.to_commit,
            pending.from_commit,
            pending.to_commit
        ));
    }

    format!("{user_visible_header}{ctx}")
}

/// Builds a self-contained prompt that users without a configured Claw model can copy
/// and paste directly into any web AI tool (ChatGPT, Claude, Gemini, etc.).
pub fn build_clipboard_whats_new_prompt(
    pending: &WhatsNewPending,
    summary: Option<&ChangelogSummary>,
) -> String {
    let compare_url = format!(
        "https://github.com/{}/{}/compare/{}...{}",
        REPO_OWNER, REPO_NAME, pending.from_commit, pending.to_commit
    );

    let mut out = format!(
        "Read me what's new in Boxxy Terminal from commit {} to {} and give me a user-friendly overview of the changes, plus a fun fact about how many lines of code changed!\n\nGitHub Compare: {}",
        pending.from_commit, pending.to_commit, compare_url
    );

    if let Some(s) = summary
        && !s.commits.is_empty()
    {
        out.push_str(&format!(
            "\n\nStats: {} commits, {} files changed, +{} lines added, -{} lines removed ({} total lines touched)\n\nCommits:\n",
            s.total_commits,
            s.files_changed,
            s.additions,
            s.deletions,
            s.additions + s.deletions
        ));
        for entry in s.commits.iter().take(50) {
            let first_line = entry.message.lines().next().unwrap_or(&entry.message);
            out.push_str(&format!("- [{}] {}\n", entry.short_sha, first_line));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_compare_payload_extracts_commits_and_sums_lines_ignoring_patches() {
        let sample_json = r#"{
            "total_commits": 2,
            "commits": [
                {
                    "sha": "b111111222222333333444444555555666666777",
                    "commit": {
                        "message": "feat(claw): add side-channel /btw query dialog\n\nAllows quick questions while thinking."
                    },
                    "author": {
                        "login": "alice",
                        "avatar_url": "https://example.com/alice.png"
                    }
                },
                {
                    "sha": "c222222333333444444555555666666777777888",
                    "commit": {
                        "message": "fix(vte): improve scrollback rendering speed"
                    },
                    "author": {
                        "login": "alice",
                        "avatar_url": "https://example.com/alice.png"
                    }
                }
            ],
            "files": [
                {
                    "filename": "crates/window/src/ui.rs",
                    "additions": 120,
                    "deletions": 15,
                    "changes": 135,
                    "patch": "@@ -1,15 +1,120 @@\n+lots of rust code that must be ignored"
                },
                {
                    "filename": "crates/boxxy_vte/src/lib.rs",
                    "additions": 45,
                    "deletions": 30,
                    "changes": 75,
                    "patch": "@@ -10,30 +10,45 @@\n-old code\n+new code"
                }
            ]
        }"#;

        let summary = parse_compare_payload("a000000", "c222222", sample_json).unwrap();
        assert_eq!(summary.from_commit, "a000000");
        assert_eq!(summary.to_commit, "c222222");
        assert_eq!(summary.total_commits, 2);
        assert_eq!(summary.files_changed, 2);
        assert_eq!(summary.additions, 165);
        assert_eq!(summary.deletions, 45);
        assert_eq!(summary.commits.len(), 2);
        assert_eq!(summary.commits[0].short_sha, "b111111");
        assert!(summary.commits[0]
            .message
            .starts_with("feat(claw): add side-channel /btw query dialog"));
        assert_eq!(summary.commits[1].short_sha, "c222222");
        assert_eq!(summary.contributors.len(), 1);
        assert_eq!(summary.contributors[0].login, "alice");
        assert_eq!(
            summary.contributors[0].avatar_url.as_deref(),
            Some("https://example.com/alice.png")
        );
        assert_eq!(
            summary.stats_badge_line(),
            "2 commits · 2 files changed · +165 lines added, -45 lines removed"
        );
    }

    #[test]
    fn whats_new_pending_marker_roundtrip_and_single_consumption() {
        let tmp = tempfile::tempdir().unwrap();
        write_whats_new_pending_in(tmp.path(), "a1b2c3d4e5", "nightly").unwrap();

        // First read consumes the file and substitutes the running binary's commit hash for "nightly"
        let pending = take_whats_new_pending_in(tmp.path(), "f9e8d7c6b5").unwrap();
        assert_eq!(
            pending,
            WhatsNewPending {
                from_commit: "a1b2c3d".to_string(),
                to_commit: "f9e8d7c".to_string(),
            }
        );

        // Second read returns None because the marker was consumed
        assert!(take_whats_new_pending_in(tmp.path(), "f9e8d7c6b5").is_none());
    }

    #[test]
    fn whats_new_pending_skips_identical_commits() {
        let tmp = tempfile::tempdir().unwrap();
        write_whats_new_pending_in(tmp.path(), "a1b2c3d", "a1b2c3d").unwrap();
        assert!(take_whats_new_pending_in(tmp.path(), "a1b2c3d").is_none());
    }

    #[test]
    fn parse_simulate_update_range_accepts_double_and_triple_dots() {
        let p1 = parse_simulate_update_range("1111111222..3333333444").unwrap();
        assert_eq!(p1.from_commit, "1111111");
        assert_eq!(p1.to_commit, "3333333");

        let p2 = parse_simulate_update_range("abcdef1...fedcba9").unwrap();
        assert_eq!(p2.from_commit, "abcdef1");
        assert_eq!(p2.to_commit, "fedcba9");

        let (clamped, was_clamped) = sanitize_commit_ref("HEAD~1300");
        assert_eq!(clamped, "HEAD~100");
        assert!(was_clamped);

        let (unclamped, was_clamped_ok) = sanitize_commit_ref("HEAD~25");
        assert_eq!(unclamped, "HEAD~25");
        assert!(!was_clamped_ok);
    }

    #[test]
    fn prompt_builders_include_commits_and_line_stats_without_code() {
        let pending = WhatsNewPending {
            from_commit: "a1b2c3d".to_string(),
            to_commit: "e4f5a6b".to_string(),
        };
        let summary = ChangelogSummary {
            from_commit: "a1b2c3d".to_string(),
            to_commit: "e4f5a6b".to_string(),
            total_commits: 1,
            files_changed: 3,
            additions: 88,
            deletions: 12,
            commits: vec![CommitEntry {
                short_sha: "e4f5a6b".to_string(),
                message: "feat: add what's new dialog on update".to_string(),
                author: Some("alice".to_string()),
                date: None,
            }],
            contributors: vec![ContributorInfo {
                login: "alice".to_string(),
                avatar_url: None,
                avatar_bytes: None,
            }],
        };

        let claw_prompt = build_claw_whats_new_prompt(&pending, Some(&summary));
        assert!(claw_prompt.starts_with(
            "Read me what's new in Boxxy Terminal from commit `a1b2c3d` to `e4f5a6b`!"
        ));
        assert!(claw_prompt.contains("--- UPDATE CHANGELOG CONTEXT ---"));
        assert!(claw_prompt.contains("+88"));
        assert!(claw_prompt.contains("-12"));
        assert!(claw_prompt.contains("feat: add what's new dialog on update"));

        let clip_prompt = build_clipboard_whats_new_prompt(&pending, Some(&summary));
        assert!(clip_prompt.contains("from commit a1b2c3d to e4f5a6b"));
        assert!(clip_prompt.contains("https://github.com/boxxy-dev/boxxy/compare/a1b2c3d...e4f5a6b"));
        assert!(clip_prompt.contains("+88 lines added, -12 lines removed"));
    }
}
