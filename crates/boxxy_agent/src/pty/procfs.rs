//! Linux `/proc` process inspection, foreground command unwrapping, and descendant traversal.

use zbus::fdo;

/// Parses a NUL-delimited `/proc/<pid>/cmdline` buffer to extract the effective
/// foreground executable basename, unwrapping privilege/env wrappers (`sudo`, `doas`,
/// `env`, `nice`, `nohup`, `time`) and interpreter-wrapped CLI binaries (`python3 /usr/bin/ranger`,
/// `node /usr/local/bin/claude`).
pub(crate) fn resolve_foreground_command_from_cmdline(cmdline: &[u8]) -> Option<String> {
    let args: Vec<&str> = cmdline
        .split(|&b| b == 0)
        .filter_map(|s| std::str::from_utf8(s).ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    resolve_foreground_command_from_args(&args)
}

fn resolve_foreground_command_from_args(args: &[&str]) -> Option<String> {
    let first = *args.first()?;
    // Handle monolithic argv[0] strings if a process overwrote its argv[0] with spaces
    let first_tok = first.split_whitespace().next().unwrap_or(first);
    let base = first_tok
        .rsplit('/')
        .next()
        .unwrap_or(first_tok)
        .trim_start_matches('-');
    if base.is_empty() {
        return None;
    }

    let lower = base.to_ascii_lowercase();

    // Unwrap privilege / scheduling wrappers: `sudo vim`, `env FOO=1 nvim`, `nice -n 10 btop`
    if matches!(
        lower.as_str(),
        "sudo" | "doas" | "env" | "nice" | "ionice" | "nohup" | "time" | "rlwrap"
    ) && args.len() > 1
    {
        let mut idx = 1;
        while idx < args.len() {
            let arg = args[idx];
            if arg.starts_with('-') {
                if matches!(arg, "-u" | "-g" | "-C" | "-D" | "-R" | "-T" | "-n" | "-c") {
                    idx += 2;
                } else {
                    idx += 1;
                }
                continue;
            }
            if arg.contains('=') {
                idx += 1;
                continue;
            }
            return resolve_foreground_command_from_args(&args[idx..]);
        }
    }

    // Unwrap interpreter-launched CLI tools (e.g. `python3 /usr/bin/ranger`, `node /usr/bin/claude`)
    let is_interpreter = lower == "python"
        || lower.starts_with("python3")
        || matches!(
            lower.as_str(),
            "node" | "nodejs" | "bun" | "deno" | "ruby" | "perl"
        );
    if is_interpreter && args.len() > 1 {
        for &arg in &args[1..] {
            if arg.starts_with('-') {
                continue;
            }
            let script_base = arg.rsplit('/').next().unwrap_or(arg);
            // If it's an installed CLI entrypoint (no source extension, or located in a bin dir)
            let has_source_ext = script_base.ends_with(".py")
                || script_base.ends_with(".js")
                || script_base.ends_with(".mjs")
                || script_base.ends_with(".cjs")
                || script_base.ends_with(".ts")
                || script_base.ends_with(".rb")
                || script_base.ends_with(".pl");
            if !script_base.is_empty()
                && (!has_source_ext || arg.contains("/bin/") || arg.contains("/.bin/"))
            {
                let clean = script_base
                    .strip_suffix(".py")
                    .or_else(|| script_base.strip_suffix(".js"))
                    .unwrap_or(script_base);
                if !clean.is_empty() {
                    return Some(clean.to_string());
                }
            }
            break;
        }
    }

    Some(base.to_string())
}

/// Reads `/proc/<pid>/stat` into a stack buffer (zero heap allocations) and returns
/// `Some(tpgid)` when the terminal foreground process group differs from `pid`'s own
/// process group (`tpgid != 0 && tpgid != pgrp`).
pub(crate) fn read_foreground_tpgid(pid: u32) -> Option<u32> {
    use std::io::Read;

    let mut file = std::fs::File::open(format!("/proc/{pid}/stat")).ok()?;
    let mut buf = [0u8; 512];
    let n = file.read(&mut buf).ok()?;
    let stat_bytes = &buf[..n];

    let rp_pos = stat_bytes.iter().rposition(|&b| b == b')')?;
    if rp_pos == 0 || rp_pos + 2 >= stat_bytes.len() {
        return None;
    }

    let rest = std::str::from_utf8(&stat_bytes[rp_pos + 2..]).ok()?;
    let mut fields = rest.split_ascii_whitespace();
    // Fields after ')': [0]=state, [1]=ppid, [2]=pgrp, [3]=session, [4]=tty_nr, [5]=tpgid
    let pgrp: u32 = fields.nth(2)?.parse().ok()?;
    let tpgid: u32 = fields.nth(2)?.parse().ok()?;
    (tpgid != 0 && tpgid != pgrp).then_some(tpgid)
}

/// Returns `true` when `pid` itself is a direct command (`boxxy-terminal -e nvim`,
/// `exec btop`, or `sh -c ...`) rather than an interactive login/subshell prompt.
pub(crate) fn is_non_shell_root_process(pid: u32) -> bool {
    if let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) {
        let args: Vec<&[u8]> = cmdline
            .split(|&b| b == 0)
            .filter(|s| !s.is_empty())
            .collect();
        if args.iter().skip(1).any(|&arg| arg == b"-c") {
            return true;
        }
    }
    if let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) {
        let name = comm.trim();
        let is_shell = matches!(
            name,
            "sh" | "bash"
                | "zsh"
                | "fish"
                | "dash"
                | "nu"
                | "ksh"
                | "tcsh"
                | "csh"
                | "elvish"
                | "xonsh"
        );
        if !name.is_empty() && !is_shell {
            return true;
        }
    }
    false
}

/// Reads the foreground process name from `/proc` for `pid`, resolving wrappers
/// (`sudo`, `doas`, `env`), interpreter-launched CLI tools via `/proc/<tpgid>/cmdline`,
/// and direct non-shell root processes (`boxxy-terminal -e <cmd>` or `exec <cmd>`).
pub(crate) fn get_foreground_process(pid: u32) -> fdo::Result<String> {
    let Some(tpgid) =
        read_foreground_tpgid(pid).or_else(|| is_non_shell_root_process(pid).then_some(pid))
    else {
        return Ok(String::new());
    };

    if let Ok(cmdline_bytes) = std::fs::read(format!("/proc/{tpgid}/cmdline"))
        && let Some(resolved) = resolve_foreground_command_from_cmdline(&cmdline_bytes)
    {
        return Ok(resolved);
    }

    let tpgid_stat_path = format!("/proc/{tpgid}/stat");
    if let Ok(tpgid_stat) = std::fs::read_to_string(&tpgid_stat_path)
        && let (Some(lp), Some(rp)) = (tpgid_stat.find('('), tpgid_stat.rfind(')'))
        && rp > lp
    {
        return Ok(tpgid_stat[lp + 1..rp].to_string());
    }

    Ok(String::new())
}

/// Inspects the foreground process group of `shell_pid` and checks whether
/// any process in that foreground group is blocked on a TTY read (`wchan` in
/// `n_tty_read` / `tty_read`). Never flags the shell prompt itself.
pub(crate) fn inspect_foreground_state(shell_pid: u32) -> (String, bool) {
    let Some(tpgid) = read_foreground_tpgid(shell_pid)
        .or_else(|| is_non_shell_root_process(shell_pid).then_some(shell_pid))
    else {
        return (String::new(), false);
    };

    let mut fg_name = String::new();
    let tpgid_stat_path = format!("/proc/{tpgid}/stat");
    if let Ok(tpgid_stat) = std::fs::read_to_string(&tpgid_stat_path)
        && let (Some(lp), Some(rp)) = (tpgid_stat.find('('), tpgid_stat.rfind(')'))
        && rp > lp
    {
        fg_name = tpgid_stat[lp + 1..rp].to_string();
    }

    if fg_name.is_empty() {
        return (String::new(), false);
    }

    let mut pids_to_check = vec![tpgid];
    if let Some(children) = read_direct_children_procfs(tpgid) {
        pids_to_check.extend(children);
    }

    let waiting_on_tty = pids_to_check.into_iter().any(is_pid_wchan_tty_read);
    (fg_name, waiting_on_tty)
}

/// Returns `true` if `shell_pid` is currently running a foreground command
/// (`tpgid != pgrp`), has at least one descendant child process, or was
/// launched as a non-interactive command (e.g. `sh -c ...` or `exec <cmd>`).
/// Returns `true` if `/proc/<shell_pid>` does not exist (synthetic PIDs in unit tests).
pub(crate) fn shell_has_active_process(shell_pid: u32) -> bool {
    let proc_path = format!("/proc/{shell_pid}");
    if !std::path::Path::new(&proc_path).exists() {
        return true;
    }

    if is_non_shell_root_process(shell_pid) {
        return true;
    }

    let has_fg = get_foreground_process(shell_pid)
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    if has_fg {
        return true;
    }
    !collect_descendant_processes(shell_pid).is_empty()
}

pub(crate) fn collect_active_pty_processes(root_pid: u32) -> Vec<(u32, String)> {
    let mut list = Vec::new();
    if is_non_shell_root_process(root_pid)
        && let Some(name) = read_process_display_name(root_pid)
    {
        list.push((root_pid, name));
    }
    list.extend(collect_descendant_processes(root_pid));
    list
}

fn is_pid_wchan_tty_read(pid: u32) -> bool {
    let wchan_path = format!("/proc/{pid}/wchan");
    if let Ok(wchan) = std::fs::read_to_string(&wchan_path) {
        let trimmed = wchan.trim();
        if matches!(trimmed, "n_tty_read" | "tty_read") {
            return true;
        }
    }
    false
}

fn read_process_display_name(pid: u32) -> Option<String> {
    let cmdline_path = format!("/proc/{pid}/cmdline");
    if let Ok(cmdline) = std::fs::read(&cmdline_path) {
        let cmd = String::from_utf8_lossy(&cmdline)
            .replace('\0', " ")
            .trim()
            .to_string();
        if !cmd.is_empty() {
            return Some(cmd);
        }
    }

    let stat_path = format!("/proc/{pid}/stat");
    let stat_content = std::fs::read_to_string(&stat_path).ok()?;
    let lp = stat_content.find('(')?;
    let rp = stat_content.rfind(')')?;
    (rp > lp).then(|| stat_content[lp + 1..rp].to_string())
}

fn read_direct_children_procfs(pid: u32) -> Option<Vec<u32>> {
    let primary_path = format!("/proc/{pid}/task/{pid}/children");
    let mut children = Vec::new();

    let task_dir = format!("/proc/{pid}/task");
    if let Ok(entries) = std::fs::read_dir(&task_dir) {
        let mut read_any = false;
        for entry in entries.flatten() {
            let children_path = entry.path().join("children");
            if let Ok(content) = std::fs::read_to_string(&children_path) {
                read_any = true;
                for token in content.split_whitespace() {
                    if let Ok(child_pid) = token.parse::<u32>() {
                        children.push(child_pid);
                    }
                }
            }
        }
        if read_any {
            return Some(children);
        }
    }

    if let Ok(content) = std::fs::read_to_string(&primary_path) {
        for token in content.split_whitespace() {
            if let Ok(child_pid) = token.parse::<u32>() {
                children.push(child_pid);
            }
        }
        return Some(children);
    }

    None
}

/// Collects all descendant processes `(pid, display_name)` of `root_pid`.
///
/// Uses the direct `/proc/<pid>/task/<tid>/children` kernel interface ($O(k)$
/// file reads where $k$ is the number of descendant processes), falling back
/// to a `/proc` parent-PID scan only if `children` is unavailable.
pub(crate) fn collect_descendant_processes(root_pid: u32) -> Vec<(u32, String)> {
    // Fast path: traverse `/proc/<pid>/task/*/children` directly.
    if let Some(initial_children) = read_direct_children_procfs(root_pid) {
        let mut descendants = Vec::new();
        let mut to_visit = initial_children;
        let mut visited = std::collections::HashSet::new();
        visited.insert(root_pid);

        while let Some(child_pid) = to_visit.pop() {
            if !visited.insert(child_pid) {
                continue;
            }
            if let Some(name) = read_process_display_name(child_pid) {
                descendants.push((child_pid, name));
            }
            if let Some(grandchildren) = read_direct_children_procfs(child_pid) {
                to_visit.extend(grandchildren);
            }
        }
        return descendants;
    }

    // If the root process doesn't exist at all, return empty immediately.
    if !std::path::Path::new(&format!("/proc/{root_pid}")).exists() {
        return Vec::new();
    }

    // Fallback for kernels compiled without CONFIG_PROC_CHILDREN: scan `/proc/*/stat`
    // for `(pid, ppid)`, and only read `cmdline` for matched descendants.
    let mut proc_edges = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            if let Ok(file_name) = entry.file_name().into_string()
                && let Ok(p) = file_name.parse::<u32>()
            {
                let stat_path = format!("/proc/{p}/stat");
                if let Ok(stat_content) = std::fs::read_to_string(&stat_path)
                    && let Some(rp) = stat_content.rfind(')')
                    && rp + 2 < stat_content.len()
                {
                    let rest = &stat_content[rp + 2..];
                    let mut parts = rest.split_whitespace();
                    let _state = parts.next();
                    if let Some(ppid_str) = parts.next()
                        && let Ok(ppid) = ppid_str.parse::<u32>()
                    {
                        proc_edges.push((p, ppid));
                    }
                }
            }
        }
    }

    let mut descendants = Vec::new();
    let mut to_visit = vec![root_pid];
    let mut visited = std::collections::HashSet::new();

    while let Some(current) = to_visit.pop() {
        if !visited.insert(current) {
            continue;
        }
        for &(p, ppid) in &proc_edges {
            if ppid == current {
                if let Some(name) = read_process_display_name(p) {
                    descendants.push((p, name));
                }
                to_visit.push(p);
            }
        }
    }

    descendants
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_descendant_processes_finds_spawned_child() {
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .expect("spawn sleep");
        let child_pid = child.id();
        let my_pid = std::process::id();

        let descendants = collect_descendant_processes(my_pid);
        assert!(
            descendants
                .iter()
                .any(|(pid, name)| *pid == child_pid && name.contains("sleep")),
            "expected child pid {child_pid} in {descendants:?}"
        );

        let _ = child.kill();
        let _ = child.wait();

        let after_exit = collect_descendant_processes(my_pid);
        assert!(!after_exit.iter().any(|(pid, _)| *pid == child_pid));
    }

    #[test]
    fn direct_non_shell_root_process_reports_foreground_and_active_entry() {
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .expect("spawn sleep");
        let child_pid = child.id();

        assert!(is_non_shell_root_process(child_pid));
        let fg = get_foreground_process(child_pid).expect("fg process");
        assert_eq!(fg, "sleep");

        let active = collect_active_pty_processes(child_pid);
        assert!(
            active
                .iter()
                .any(|(pid, name)| *pid == child_pid && name.contains("sleep")),
            "expected direct -e root process {child_pid} in {active:?}"
        );

        let _ = child.kill();
        let _ = child.wait();
    }
}
