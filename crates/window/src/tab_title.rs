use crate::application_detector::{
    TerminalAppState, detect_application_state, is_prompt_helper_executable, is_shell_executable,
    is_ssh_executable, normalize_executable_name,
};
use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

/// Cached local hostname for distinguishing local vs remote `user@host:` OSC titles.
static LOCAL_HOSTNAME: LazyLock<Option<String>> = LazyLock::new(|| {
    if let Ok(h) = std::env::var("HOSTNAME") {
        let trimmed = h.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    if let Ok(h) = std::fs::read_to_string("/etc/hostname") {
        let trimmed = h.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
});

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TabTitleInput {
    pub custom_title: Option<String>,
    pub cwd: Option<String>,
    pub raw_title: Option<String>,
    pub foreground_process: Option<String>,
    pub alternate_screen_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTabTitle {
    pub title: String,
    pub tooltip: String,
    pub has_running_app: bool,
    pub app_state: TerminalAppState,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedOscTitle {
    pub remote_host: Option<String>,
    pub command: Option<String>,
    pub path: Option<String>,
}

/// Resolves tab titles and hover tooltips for all tabs in a window using the
/// host environment's `$HOME` and hostname.
pub fn resolve_tab_titles_for_host(
    tabs: &[TabTitleInput],
    smart_naming: bool,
) -> Vec<ResolvedTabTitle> {
    let home = std::env::var("HOME").ok();
    let hostname = LOCAL_HOSTNAME.as_deref();
    resolve_tab_titles(tabs, smart_naming, home.as_deref(), hostname)
}

/// Pure, deterministic tab title and tooltip resolver.
///
/// Priorities when `smart_naming` is `true`:
/// 1. User-specified `custom_title` (when set via tab context menu).
/// 2. Remote host session (`host · boxxy-terminal`).
/// 3. Foreground application or command (`vim`), when available.
/// 4. Current directory's basename (`boxxy-terminal`, `Downloads`, `src`).
/// 5. If multiple tabs have identical titles, disambiguate with parent/directory (`src · boxxy-terminal`).
/// 6. Full path is preserved in `tooltip` for hover inspection.
pub fn resolve_tab_titles(
    tabs: &[TabTitleInput],
    smart_naming: bool,
    home_dir: Option<&str>,
    local_hostname: Option<&str>,
) -> Vec<ResolvedTabTitle> {
    if !smart_naming {
        return tabs
            .iter()
            .map(|tab| resolve_classic_tab_title(tab, home_dir, local_hostname))
            .collect();
    }

    #[derive(Clone)]
    struct Candidate {
        is_custom: bool,
        base_title: String,
        disambiguator: Option<String>,
        tooltip: String,
        app_state: TerminalAppState,
    }

    let mut candidates = Vec::with_capacity(tabs.len());

    for tab in tabs {
        let normalized_cwd = tab
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|p| expand_tilde(p, home_dir));

        let fg_clean = tab
            .foreground_process
            .as_deref()
            .and_then(normalize_process_name)
            .filter(|p| !is_shell_name(p) && !is_prompt_helper(p));

        let is_fg_ssh = fg_clean.as_deref().is_some_and(is_ssh_process);

        let parsed_osc = tab
            .raw_title
            .as_deref()
            .map(|raw| parse_osc_title(raw, local_hostname, is_fg_ssh))
            .unwrap_or_default();

        let effective_fg_for_detect = fg_clean.as_deref().or(parsed_osc.command.as_deref());
        let app_state = detect_application_state(
            effective_fg_for_detect,
            parsed_osc.remote_host.as_deref(),
            tab.alternate_screen_active,
        );

        if let Some(custom) = tab
            .custom_title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let tooltip = normalized_cwd
                .clone()
                .or_else(|| {
                    tab.raw_title
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| custom.to_string());

            candidates.push(Candidate {
                is_custom: true,
                base_title: custom.to_string(),
                disambiguator: None,
                tooltip,
                app_state,
            });
            continue;
        }

        // 1. Remote host check
        if let Some(ref host) = parsed_osc.remote_host {
            // For remote sessions, prefer the remote path emitted in the OSC title,
            // falling back to the local working directory if the remote shell hasn't emitted one.
            let remote_path_raw = parsed_osc.path.as_deref().or(tab.cwd.as_deref());
            let dir_basename = remote_path_raw
                .map(|p| expand_tilde(p, home_dir))
                .and_then(|p| extract_dir_parts(&p).map(|(base, _)| base));

            let base_title = if let Some(ref dir) = dir_basename {
                format!("{host} · {dir}")
            } else {
                host.clone()
            };

            let tooltip = if let Some(p) = remote_path_raw.map(str::trim).filter(|s| !s.is_empty())
            {
                format!("{host}:{p}")
            } else {
                host.clone()
            };

            candidates.push(Candidate {
                is_custom: false,
                base_title,
                disambiguator: None,
                tooltip,
                app_state,
            });
            continue;
        }

        // Resolve local directory path (prefer OSC 7 / /proc `cwd` over potentially shortened OSC 0 path)
        let effective_path = normalized_cwd.or_else(|| {
            parsed_osc
                .path
                .as_deref()
                .map(|p| expand_tilde(p, home_dir))
        });

        let dir_parts = effective_path.as_deref().and_then(extract_dir_parts);

        // 2. Foreground application or command
        let effective_cmd = fg_clean.or(parsed_osc.command);
        if let Some(cmd) = effective_cmd {
            let dir_base = dir_parts.as_ref().map(|(base, _)| base.clone());
            let tooltip = effective_path
                .clone()
                .or_else(|| {
                    tab.raw_title
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| cmd.clone());

            candidates.push(Candidate {
                is_custom: false,
                base_title: cmd,
                disambiguator: dir_base,
                tooltip,
                app_state,
            });
            continue;
        }

        // 3. Current directory's basename (with parent available for disambiguation)
        if let Some((basename, parent)) = dir_parts {
            let tooltip = effective_path.unwrap_or_else(|| basename.clone());
            candidates.push(Candidate {
                is_custom: false,
                base_title: basename,
                disambiguator: parent,
                tooltip,
                app_state,
            });
            continue;
        }

        // 4. Fallback
        candidates.push(Candidate {
            is_custom: false,
            base_title: "Terminal".to_string(),
            disambiguator: None,
            tooltip: "Terminal".to_string(),
            app_state,
        });
    }

    // Count occurrences of each auto-generated `base_title` to disambiguate identical titles
    let mut counts: HashMap<String, usize> = HashMap::with_capacity(candidates.len());
    for c in &candidates {
        if !c.is_custom && c.base_title != "Terminal" {
            *counts.entry(c.base_title.clone()).or_insert(0) += 1;
        }
    }

    candidates
        .into_iter()
        .map(|c| {
            let needs_disambiguation =
                !c.is_custom && counts.get(c.base_title.as_str()).copied().unwrap_or(0) > 1;
            let title = if needs_disambiguation {
                if let Some(ref parent) = c.disambiguator
                    && !parent.is_empty()
                    && *parent != c.base_title
                {
                    format!("{} · {}", c.base_title, parent)
                } else {
                    c.base_title
                }
            } else {
                c.base_title
            };

            ResolvedTabTitle {
                title,
                tooltip: c.tooltip,
                has_running_app: c.app_state.application_category.is_active(),
                app_state: c.app_state,
            }
        })
        .collect()
}

fn resolve_classic_tab_title(
    tab: &TabTitleInput,
    home_dir: Option<&str>,
    local_hostname: Option<&str>,
) -> ResolvedTabTitle {
    let full_path = tab
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|p| expand_tilde(p, home_dir));

    let fg_clean = tab
        .foreground_process
        .as_deref()
        .and_then(normalize_process_name)
        .filter(|p| !is_shell_name(p) && !is_prompt_helper(p));
    let is_fg_ssh = fg_clean.as_deref().is_some_and(is_ssh_process);
    let parsed_osc = tab
        .raw_title
        .as_deref()
        .map(|raw| parse_osc_title(raw, local_hostname, is_fg_ssh))
        .unwrap_or_default();
    let effective_fg_for_detect = fg_clean.as_deref().or(parsed_osc.command.as_deref());
    let app_state = detect_application_state(
        effective_fg_for_detect,
        parsed_osc.remote_host.as_deref(),
        tab.alternate_screen_active,
    );
    let has_running_app = app_state.application_category.is_active();

    if let Some(custom) = tab
        .custom_title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return ResolvedTabTitle {
            title: custom.to_string(),
            tooltip: full_path.unwrap_or_else(|| custom.to_string()),
            has_running_app,
            app_state,
        };
    }

    let raw = tab
        .raw_title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "Terminal");

    let title = raw
        .map(str::to_string)
        .or_else(|| full_path.clone())
        .unwrap_or_else(|| "Terminal".to_string());

    let tooltip = full_path.unwrap_or_else(|| title.clone());

    ResolvedTabTitle {
        title,
        tooltip,
        has_running_app,
        app_state,
    }
}

/// Parses an OSC 0 / OSC 2 window title into optional remote host, foreground command, and path.
pub fn parse_osc_title(
    raw: &str,
    local_hostname: Option<&str>,
    force_remote_host: bool,
) -> ParsedOscTitle {
    let mut s = raw.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("terminal") || is_shell_name(s) {
        return ParsedOscTitle::default();
    }

    // Strip trailing shell suffixes such as " - fish", " — zsh", " - bash"
    s = strip_trailing_shell_suffix(s);
    // Strip leading shell prefixes such as "fish: ", "zsh - ", "fish /path"
    s = strip_leading_shell_prefix(s);

    if s.is_empty() || is_shell_name(s) {
        return ParsedOscTitle::default();
    }

    let mut remote_host: Option<String> = None;
    let mut explicit_ssh = force_remote_host;

    if let Some(rest) = s.strip_prefix("[ssh]").or_else(|| s.strip_prefix("[SSH]")) {
        explicit_ssh = true;
        s = rest.trim();
    }

    // Detect `user@host: ...` or `host: ~/...` / `host:/...`
    if let Some((host, rest)) = split_user_host_prefix(s) {
        if explicit_ssh || !is_local_host(host, local_hostname) {
            remote_host = Some(host.to_string());
        }
        s = rest.trim();
    }

    if s.is_empty() || is_shell_name(s) {
        return ParsedOscTitle {
            remote_host,
            command: None,
            path: None,
        };
    }

    // Pure path: e.g. "~/Projects/boxxy-dev/boxxy-terminal" or "/home/user/Downloads"
    if looks_like_path(s) {
        return ParsedOscTitle {
            remote_host,
            command: None,
            path: Some(s.to_string()),
        };
    }

    // Vim / Neovim window title: e.g. "main.rs (~/Projects/boxxy-terminal/src) - NVIM"
    if let Some((cmd, path_opt)) = parse_editor_title(s) {
        return ParsedOscTitle {
            remote_host,
            command: Some(cmd),
            path: path_opt,
        };
    }

    // "<cmd> - <path>" or "<path> - <cmd>"
    for sep in [" - ", " — ", " – ", ": "] {
        if let Some((left, right)) = s.split_once(sep) {
            let left = left.trim();
            let right = right.trim();
            if looks_like_path(right) {
                let (ssh_host, cmd) = parse_command_or_ssh(left);
                if remote_host.is_none() {
                    remote_host = ssh_host;
                }
                return ParsedOscTitle {
                    remote_host,
                    command: cmd,
                    path: Some(right.to_string()),
                };
            } else if looks_like_path(left) {
                let (ssh_host, cmd) = parse_command_or_ssh(right);
                if remote_host.is_none() {
                    remote_host = ssh_host;
                }
                return ParsedOscTitle {
                    remote_host,
                    command: cmd,
                    path: Some(left.to_string()),
                };
            }
        }
    }

    // Fish-style "<cmd> <path>" where the trailing segment is "~/..." or "/..." or "~"
    if let Some((cmd_part, path_part)) = split_trailing_path(s) {
        let (ssh_host, cmd) = parse_command_or_ssh(cmd_part);
        if remote_host.is_none() {
            remote_host = ssh_host;
        }
        return ParsedOscTitle {
            remote_host,
            command: cmd,
            path: Some(path_part.to_string()),
        };
    }

    // Bare command or "ssh user@host"
    let (ssh_host, cmd) = parse_command_or_ssh(s);
    if remote_host.is_none() {
        remote_host = ssh_host;
    }

    ParsedOscTitle {
        remote_host,
        command: cmd,
        path: None,
    }
}

fn strip_trailing_shell_suffix(s: &str) -> &str {
    for sep in [" - ", " — ", " – ", " | "] {
        if let Some((head, tail)) = s.rsplit_once(sep)
            && is_shell_name(tail.trim())
        {
            return head.trim();
        }
    }
    s
}

fn strip_leading_shell_prefix(s: &str) -> &str {
    for sep in [": ", " - ", " — ", " – "] {
        if let Some((head, tail)) = s.split_once(sep)
            && is_shell_name(head.trim())
        {
            return tail.trim();
        }
    }
    if let Some((first, rest)) = s.split_once(char::is_whitespace)
        && is_shell_name(first)
        && looks_like_path(rest.trim())
    {
        return rest.trim();
    }
    s
}

fn split_user_host_prefix(s: &str) -> Option<(&str, &str)> {
    let (left, right) = s.split_once(':')?;
    let left = left.trim();
    let right = right.trim();
    if left.is_empty() || left.contains(char::is_whitespace) || left.contains('/') {
        return None;
    }

    if let Some((user, host)) = left.rsplit_once('@') {
        if !user.is_empty() && is_valid_hostname(host) {
            return Some((host, right));
        }
        return None;
    }

    // Without `user@`, only treat `host: ...` as a host prefix if `right` is a path
    // and `left` is not a shell name.
    if is_valid_hostname(left) && !is_shell_name(left) && looks_like_path(right) {
        return Some((left, right));
    }

    None
}

fn parse_editor_title(s: &str) -> Option<(String, Option<String>)> {
    let (head, editor) = if let Some(h) = s.strip_suffix(" - NVIM") {
        (h.trim(), "nvim")
    } else if let Some(h) = s.strip_suffix(" - VIM") {
        (h.trim(), "vim")
    } else {
        return None;
    };

    let mut extracted_path = None;
    if let Some(open) = head.rfind('(')
        && let Some(close) = head[open + 1..].find(')')
    {
        let inside = head[open + 1..open + 1 + close].trim();
        if looks_like_path(inside) {
            extracted_path = Some(inside.to_string());
        }
    }

    Some((editor.to_string(), extracted_path))
}

fn split_trailing_path(s: &str) -> Option<(&str, &str)> {
    for marker in [" ~/", " /"] {
        if let Some(idx) = s.find(marker) {
            let cmd_part = s[..idx].trim();
            let path_part = s[idx + 1..].trim();
            if !cmd_part.is_empty() && looks_like_path(path_part) {
                return Some((cmd_part, path_part));
            }
        }
    }
    if let Some(cmd_part) = s.strip_suffix(" ~") {
        let cmd_part = cmd_part.trim();
        if !cmd_part.is_empty() {
            return Some((cmd_part, "~"));
        }
    }
    None
}

fn parse_command_or_ssh(s: &str) -> (Option<String>, Option<String>) {
    let tokens: Vec<&str> = s.split_whitespace().collect();
    if tokens.is_empty() {
        return (None, None);
    }

    let first_norm = match normalize_process_name(tokens[0]) {
        Some(n) => n,
        None => return (None, None),
    };

    // Skip privilege / environment wrappers like `sudo vim`
    if matches!(first_norm.as_str(), "sudo" | "doas" | "env") && tokens.len() > 1 {
        for &tok in &tokens[1..] {
            if !tok.starts_with('-') && !tok.contains('=') {
                return parse_command_or_ssh(tok);
            }
        }
    }

    if is_ssh_process(&first_norm) {
        if let Some(host) = extract_ssh_target_host(&tokens[1..]) {
            return (Some(host), None);
        }
        return (None, Some(first_norm));
    }

    if is_shell_name(&first_norm) || is_prompt_helper(&first_norm) {
        return (None, None);
    }

    (None, Some(first_norm))
}

fn extract_ssh_target_host(args: &[&str]) -> Option<String> {
    let mut skip_next = false;
    for &arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg.starts_with('-') {
            // Common ssh flags that take a separate parameter
            if matches!(
                arg,
                "-p" | "-i"
                    | "-l"
                    | "-F"
                    | "-o"
                    | "-J"
                    | "-L"
                    | "-R"
                    | "-D"
                    | "-W"
                    | "-b"
                    | "-c"
                    | "-E"
                    | "-m"
                    | "-O"
                    | "-Q"
                    | "-S"
                    | "-w"
            ) {
                skip_next = true;
            }
            continue;
        }
        let host = arg.rsplit_once('@').map(|(_, h)| h).unwrap_or(arg);
        let host = host.split(':').next().unwrap_or(host);
        if is_valid_hostname(host) {
            return Some(host.to_string());
        }
    }
    None
}

fn looks_like_path(s: &str) -> bool {
    let s = s.trim();
    s == "~" || s.starts_with("~/") || s.starts_with('/')
}

fn expand_tilde(path: &str, home_dir: Option<&str>) -> String {
    let trimmed = path.trim();
    if let Some(home) = home_dir.map(|h| h.trim_end_matches('/'))
        && !home.is_empty()
    {
        if trimmed == "~" {
            return home.to_string();
        }
        if let Some(rest) = trimmed.strip_prefix("~/") {
            return format!("{home}/{rest}");
        }
    }
    trimmed.to_string()
}

/// Returns `(basename, parent_basename)` for a directory path.
fn extract_dir_parts(path: &str) -> Option<(String, Option<String>)> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed == "~" {
        return Some(("~".to_string(), None));
    }
    if trimmed == "/" {
        return Some(("/".to_string(), None));
    }

    let p = Path::new(trimmed);
    let basename = p.file_name()?.to_string_lossy().into_owned();
    if basename.is_empty() {
        return Some(("/".to_string(), None));
    }

    let parent = p
        .parent()
        .and_then(|parent_path| parent_path.file_name())
        .map(|os| os.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty() && s != "~");

    Some((basename, parent))
}

fn normalize_process_name(raw: &str) -> Option<String> {
    normalize_executable_name(raw)
}

fn is_shell_name(name: &str) -> bool {
    is_shell_executable(name)
}

fn is_prompt_helper(name: &str) -> bool {
    is_prompt_helper_executable(name)
}

fn is_ssh_process(name: &str) -> bool {
    is_ssh_executable(name)
}

fn is_valid_hostname(host: &str) -> bool {
    !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

fn is_local_host(host: &str, local_hostname: Option<&str>) -> bool {
    let h = host.trim();
    if h.eq_ignore_ascii_case("localhost")
        || h.eq_ignore_ascii_case("localhost.localdomain")
        || h == "127.0.0.1"
        || h == "::1"
    {
        return true;
    }
    let h_short = h.split('.').next().unwrap_or(h);
    if let Some(local) = local_hostname.map(str::trim).filter(|s| !s.is_empty()) {
        let local_short = local.split('.').next().unwrap_or(local);
        if h.eq_ignore_ascii_case(local) || h_short.eq_ignore_ascii_case(local_short) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application_detector::ApplicationCategory;

    const HOME: Option<&str> = Some("/home/user");
    const LOCAL_HOST: Option<&str> = Some("workstation");

    #[test]
    fn test_working_in_directory_shows_basename_and_full_path_tooltip() {
        let tabs = vec![
            TabTitleInput {
                custom_title: None,
                cwd: Some("/home/user/Projects/boxxy-dev/boxxy-terminal".to_string()),
                raw_title: Some("~/Projects/boxxy-dev/boxxy-terminal - fish".to_string()),
                foreground_process: None,
                ..Default::default()
            },
            TabTitleInput {
                custom_title: None,
                cwd: Some("/home/user/Downloads".to_string()),
                raw_title: Some("~/Downloads - fish".to_string()),
                foreground_process: None,
                ..Default::default()
            },
            TabTitleInput {
                custom_title: None,
                cwd: Some("/home/user/Projects/boxxy-dev/boxxy-terminal/src".to_string()),
                raw_title: None,
                foreground_process: None,
                ..Default::default()
            },
        ];

        let resolved = resolve_tab_titles(&tabs, true, HOME, LOCAL_HOST);
        assert_eq!(resolved[0].title, "boxxy-terminal");
        assert!(!resolved[0].has_running_app);
        assert_eq!(
            resolved[0].app_state.application_category,
            ApplicationCategory::Idle
        );
        assert_eq!(
            resolved[0].tooltip,
            "/home/user/Projects/boxxy-dev/boxxy-terminal"
        );
        assert_eq!(resolved[1].title, "Downloads");
        assert!(!resolved[1].has_running_app);
        assert_eq!(resolved[1].tooltip, "/home/user/Downloads");
        assert_eq!(resolved[2].title, "src");
        assert!(!resolved[2].has_running_app);
        assert_eq!(
            resolved[2].tooltip,
            "/home/user/Projects/boxxy-dev/boxxy-terminal/src"
        );
    }

    #[test]
    fn test_two_tabs_in_src_disambiguate_with_parent_directory() {
        let tabs = vec![
            TabTitleInput {
                custom_title: None,
                cwd: Some("/home/user/Projects/boxxy-dev/boxxy-terminal/src".to_string()),
                raw_title: Some("~/P/b/b/src - fish".to_string()),
                foreground_process: None,
                ..Default::default()
            },
            TabTitleInput {
                custom_title: None,
                cwd: Some("/home/user/Projects/other-project/src".to_string()),
                raw_title: Some("~/P/o/src - fish".to_string()),
                foreground_process: None,
                ..Default::default()
            },
            TabTitleInput {
                custom_title: None,
                cwd: Some("/home/user/Downloads".to_string()),
                raw_title: None,
                foreground_process: None,
                ..Default::default()
            },
        ];

        let resolved = resolve_tab_titles(&tabs, true, HOME, LOCAL_HOST);
        assert_eq!(resolved[0].title, "src · boxxy-terminal");
        assert_eq!(
            resolved[0].tooltip,
            "/home/user/Projects/boxxy-dev/boxxy-terminal/src"
        );
        assert_eq!(resolved[1].title, "src · other-project");
        assert_eq!(resolved[1].tooltip, "/home/user/Projects/other-project/src");
        assert_eq!(resolved[2].title, "Downloads");
    }

    #[test]
    fn test_running_vim_shows_foreground_command() {
        let tabs = vec![TabTitleInput {
            custom_title: None,
            cwd: Some("/home/user/Projects/boxxy-dev/boxxy-terminal".to_string()),
            raw_title: Some("vim ~/Projects/boxxy-dev/boxxy-terminal".to_string()),
            foreground_process: Some("vim".to_string()),
            alternate_screen_active: true,
        }];

        let resolved = resolve_tab_titles(&tabs, true, HOME, LOCAL_HOST);
        assert_eq!(resolved[0].title, "vim");
        assert!(resolved[0].has_running_app);
        assert_eq!(
            resolved[0].app_state.application_category,
            ApplicationCategory::Editor
        );
        assert_eq!(
            resolved[0].tooltip,
            "/home/user/Projects/boxxy-dev/boxxy-terminal"
        );
    }

    #[test]
    fn test_connected_to_remote_host_shows_host_and_directory() {
        let tabs = vec![
            // Remote shell emitting user@host: path
            TabTitleInput {
                custom_title: None,
                cwd: Some("/home/user".to_string()),
                raw_title: Some("deploy@host: ~/Projects/boxxy-terminal".to_string()),
                foreground_process: Some("ssh".to_string()),
                ..Default::default()
            },
            // Local fish title while ssh is running in boxxy-terminal
            TabTitleInput {
                custom_title: None,
                cwd: Some("/home/user/Projects/boxxy-dev/boxxy-terminal".to_string()),
                raw_title: Some("ssh user@host ~/Projects/boxxy-dev/boxxy-terminal".to_string()),
                foreground_process: Some("ssh".to_string()),
                ..Default::default()
            },
        ];

        let resolved = resolve_tab_titles(&tabs, true, HOME, LOCAL_HOST);
        assert_eq!(resolved[0].title, "host · boxxy-terminal");
        assert_eq!(
            resolved[0].app_state.application_category,
            ApplicationCategory::Remote
        );
        assert_eq!(resolved[0].tooltip, "host:~/Projects/boxxy-terminal");
        assert_eq!(resolved[1].title, "host · boxxy-terminal");
    }

    #[test]
    fn test_local_bash_user_at_host_is_not_treated_as_remote() {
        let tabs = vec![TabTitleInput {
            custom_title: None,
            cwd: Some("/home/user/Projects/boxxy-dev/boxxy-terminal".to_string()),
            raw_title: Some("user@workstation: ~/Projects/boxxy-dev/boxxy-terminal".to_string()),
            foreground_process: None,
            ..Default::default()
        }];

        let resolved = resolve_tab_titles(&tabs, true, HOME, LOCAL_HOST);
        assert_eq!(resolved[0].title, "boxxy-terminal");
    }

    #[test]
    fn test_custom_title_overrides_smart_naming() {
        let tabs = vec![TabTitleInput {
            custom_title: Some("Release Build".to_string()),
            cwd: Some("/home/user/Projects/boxxy-dev/boxxy-terminal".to_string()),
            raw_title: Some("cargo build".to_string()),
            foreground_process: Some("cargo".to_string()),
            ..Default::default()
        }];

        let resolved = resolve_tab_titles(&tabs, true, HOME, LOCAL_HOST);
        assert_eq!(resolved[0].title, "Release Build");
        assert_eq!(
            resolved[0].app_state.application_category,
            ApplicationCategory::RunningCommand
        );
        assert_eq!(
            resolved[0].tooltip,
            "/home/user/Projects/boxxy-dev/boxxy-terminal"
        );
    }

    #[test]
    fn test_smart_naming_disabled_uses_raw_title() {
        let tabs = vec![TabTitleInput {
            custom_title: None,
            cwd: Some("/home/user/Projects/boxxy-dev/boxxy-terminal".to_string()),
            raw_title: Some("~/Projects/boxxy-dev/boxxy-terminal - fish".to_string()),
            foreground_process: None,
            ..Default::default()
        }];

        let resolved = resolve_tab_titles(&tabs, false, HOME, LOCAL_HOST);
        assert_eq!(
            resolved[0].title,
            "~/Projects/boxxy-dev/boxxy-terminal - fish"
        );
        assert_eq!(
            resolved[0].tooltip,
            "/home/user/Projects/boxxy-dev/boxxy-terminal"
        );
    }
}
