//! Self-update restart coordination between an exiting parent `boxxy-terminal`
//! and its newly spawned `--gapplication-replace` replacement process.

/// Waits for an exiting parent `boxxy-terminal` process to release the D-Bus bus name
/// when `--gapplication-replace` was passed (e.g. from `Updater::apply_update_and_restart()`).
///
/// Normal launches (`replace_existing == false`) return immediately without touching `/proc`.
pub(crate) fn wait_for_restarting_parent(replace_existing: bool) {
    if !replace_existing {
        return;
    }

    #[cfg(unix)]
    wait_for_restarting_parent_unix();
}

#[cfg(unix)]
fn wait_for_restarting_parent_unix() {
    use std::time::{Duration, Instant};

    let initial_ppid = std::os::unix::process::parent_id();
    if initial_ppid <= 1 {
        // Parent already exited and reparented to init; allow D-Bus socket teardown to settle.
        std::thread::sleep(Duration::from_millis(100));
        return;
    }

    let comm_path = format!("/proc/{initial_ppid}/comm");
    let parent_comm =
        std::fs::read_to_string(comm_path).map_or_else(|_| String::new(), |s| s.trim().to_string());

    if parent_comm == "boxxy-terminal" {
        log::info!(
            "Detected restarting parent boxxy-terminal (PID {initial_ppid}); waiting for it to exit..."
        );
        let start = Instant::now();
        while std::os::unix::process::parent_id() == initial_ppid
            && is_proc_alive(initial_ppid)
            && start.elapsed() < Duration::from_secs(5)
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        // Allow the session D-Bus daemon to process the parent's closed socket
        // and release `dev.boxxy.BoxxyTerminal` before GApplication registers.
        std::thread::sleep(Duration::from_millis(150));
    } else {
        // External `--gapplication-replace` or parent already reparented to `systemd --user`.
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(unix)]
fn is_proc_alive(pid: u32) -> bool {
    let stat_path = format!("/proc/{pid}/stat");
    std::fs::read_to_string(stat_path)
        .ok()
        .and_then(|raw| parse_proc_stat_state(&raw))
        .is_some_and(|state| !matches!(state, 'Z' | 'X' | 'x'))
}

#[cfg(any(unix, test))]
fn parse_proc_stat_state(stat_line: &str) -> Option<char> {
    // `/proc/<pid>/stat` formats `comm` inside parentheses that may themselves contain
    // spaces or nested `)` characters; the process state is the first token after the last `)`.
    let after_comm = stat_line.rsplit_once(')')?.1;
    after_comm.trim_start().chars().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_replacing_launch_is_noop() {
        // Must return immediately without sleeping.
        let start = std::time::Instant::now();
        wait_for_restarting_parent(false);
        assert!(start.elapsed() < std::time::Duration::from_millis(50));
    }

    #[test]
    fn parses_proc_stat_state_including_tricky_comm_names() {
        assert_eq!(
            parse_proc_stat_state("4242 (boxxy-terminal) S 1 4242 4242 0 -1"),
            Some('S')
        );
        assert_eq!(
            parse_proc_stat_state("4242 (worker (2) test) Z 1 4242 4242 0 -1"),
            Some('Z')
        );
        assert_eq!(parse_proc_stat_state("malformed line"), None);
        assert_eq!(parse_proc_stat_state("4242 (empty_tail)"), None);
    }
}
