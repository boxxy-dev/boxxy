//! Path resolution and zero-allocation string comparison utilities.

use std::cmp::Ordering;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

static HOME_DIR: LazyLock<Option<PathBuf>> =
    LazyLock::new(|| directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()));

fn expand_tilde(raw: &str) -> Option<PathBuf> {
    if raw == "~" {
        return HOME_DIR.clone();
    }
    if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = HOME_DIR.as_ref()
    {
        return Some(home.join(rest));
    }
    None
}

/// Strips redundant `.` (`CurDir`) components and trailing slashes without collapsing `..`
/// (`ParentDir`), so symlink-aware security guards (`vibe_guard`, `blacklist`, `fs_ops`)
/// continue to observe `..` traversals while `./src/lib.rs` and `src/lib.rs` resolve identically.
fn clean_cur_dir_components(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    let mut saw_any = false;
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            other => {
                out.push(other.as_os_str());
                saw_any = true;
            }
        }
    }
    if saw_any {
        out
    } else if path.as_os_str() == "." {
        PathBuf::from(".")
    } else {
        PathBuf::new()
    }
}

/// Resolves `path` against `base`:
/// - If `path` is absolute, returns it with redundant `.` segments removed.
/// - If `path` is `~` or starts with `~/`, resolves it against the cached user home directory.
/// - Otherwise joins `path` onto `base` (expanding `~` on `base` if needed).
#[must_use]
pub fn resolve_path(base: &str, path: &str) -> String {
    let p = Path::new(path);
    if p.is_absolute() {
        return clean_cur_dir_components(p).to_string_lossy().into_owned();
    }

    if let Some(expanded) = expand_tilde(path) {
        return clean_cur_dir_components(&expanded)
            .to_string_lossy()
            .into_owned();
    }

    if base.is_empty() {
        if path.is_empty() {
            return String::new();
        }
        return clean_cur_dir_components(p).to_string_lossy().into_owned();
    }

    let base_buf = expand_tilde(base).unwrap_or_else(|| PathBuf::from(base));
    let joined = if path.is_empty() || path == "." {
        base_buf
    } else {
        base_buf.join(p)
    };
    clean_cur_dir_components(&joined)
        .to_string_lossy()
        .into_owned()
}

/// Resolves an optional directory argument against `base`, falling back to `"."`
/// when both `base` and `dir` are empty.
#[must_use]
pub fn resolve_dir_path(base: &str, dir: Option<&str>) -> String {
    let raw = dir.map_or("", str::trim);
    let resolved = resolve_path(base, raw);
    if resolved.is_empty() {
        ".".to_string()
    } else {
        resolved
    }
}

/// Case-insensitive substring check (`needle_lower` must already be lowercased).
/// Uses a zero-allocation byte scan when `haystack` and `needle_lower` are ASCII.
#[must_use]
pub(crate) fn contains_case_insensitive(haystack: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if haystack.is_ascii() && needle_lower.is_ascii() {
        let h_bytes = haystack.as_bytes();
        let n_bytes = needle_lower.as_bytes();
        if n_bytes.len() > h_bytes.len() {
            return false;
        }
        return h_bytes
            .windows(n_bytes.len())
            .any(|win| win.eq_ignore_ascii_case(n_bytes));
    }
    haystack.to_lowercase().contains(needle_lower)
}

/// Deterministic case-insensitive string comparison with an exact byte-order tie-breaker,
/// avoiding heap allocations when both inputs are ASCII.
#[must_use]
pub(crate) fn cmp_case_insensitive(a: &str, b: &str) -> Ordering {
    let primary = if a.is_ascii() && b.is_ascii() {
        a.bytes()
            .map(|c| c.to_ascii_lowercase())
            .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
    } else {
        a.chars()
            .flat_map(char::to_lowercase)
            .cmp(b.chars().flat_map(char::to_lowercase))
    };
    primary.then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_path_absolute() {
        assert_eq!(resolve_path("/workspace/app", "/etc/hosts"), "/etc/hosts");
        assert_eq!(
            resolve_path("/workspace/app", "/etc/./hosts"),
            "/etc/hosts"
        );
    }

    #[test]
    fn test_resolve_path_relative_and_curdir_cleanup() {
        assert_eq!(
            resolve_path("/workspace/app", "file.txt"),
            "/workspace/app/file.txt"
        );
        assert_eq!(
            resolve_path("/workspace/app", "./src/./lib.rs"),
            "/workspace/app/src/lib.rs"
        );
        // Must preserve `..` components for downstream symlink guards
        assert_eq!(
            resolve_path("/workspace/app", "../other/file.txt"),
            "/workspace/app/../other/file.txt"
        );
        // Empty or `.` relative path resolves cleanly to base without trailing slash
        assert_eq!(resolve_path("/workspace/app", ""), "/workspace/app");
        assert_eq!(resolve_path("/workspace/app", "."), "/workspace/app");
    }

    #[test]
    fn test_resolve_path_empty_base_and_dir_fallback() {
        assert_eq!(resolve_path("", "/etc/hosts"), "/etc/hosts");
        assert_eq!(resolve_path("", "file.txt"), "file.txt");
        assert_eq!(resolve_path("", "./file.txt"), "file.txt");
        assert_eq!(resolve_dir_path("", None), ".");
        assert_eq!(resolve_dir_path("", Some("  ")), ".");
        assert_eq!(resolve_dir_path("/workspace/app", None), "/workspace/app");
    }

    #[test]
    fn test_resolve_path_home() {
        let Some(home) = HOME_DIR.as_ref() else {
            return;
        };
        let expected = home.join("test.txt").to_string_lossy().into_owned();
        assert_eq!(resolve_path("/workspace", "~/test.txt"), expected);

        let expected_home = home.to_string_lossy().into_owned();
        assert_eq!(resolve_path("/workspace", "~"), expected_home);
    }

    #[test]
    fn test_case_insensitive_helpers() {
        assert!(contains_case_insensitive("cargo-build", "cargo"));
        assert!(contains_case_insensitive("CARGO-TEST", "cargo"));
        assert!(!contains_case_insensitive("firefox", "cargo"));

        assert_eq!(cmp_case_insensitive("alpha", "Beta"), Ordering::Less);
        assert_eq!(cmp_case_insensitive("Makefile", "makefile"), Ordering::Less);
    }
}
