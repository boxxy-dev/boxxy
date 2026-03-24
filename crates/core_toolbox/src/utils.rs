use std::path::Path;

/// Resolves a path against a base directory.
/// - If `path` is absolute, returns it as is.
/// - If `path` starts with `~/`, resolves it against the user's home directory.
/// - Otherwise, joins `path` with `base`.
pub fn resolve_path(base: &str, path: &str) -> String {
    let p = Path::new(path);

    if p.is_absolute() {
        return path.to_string();
    }

    if path.starts_with("~/") {
        if let Some(home) = directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()) {
            return home.join(&path[2..]).to_string_lossy().to_string();
        }
    } else if path == "~"
        && let Some(home) = directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()) {
            return home.to_string_lossy().to_string();
        }

    if base.is_empty() {
        path.to_string()
    } else {
        Path::new(base).join(path).to_string_lossy().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_path_absolute() {
        assert_eq!(resolve_path("/home/user", "/etc/passwd"), "/etc/passwd");
    }

    #[test]
    fn test_resolve_path_relative() {
        assert_eq!(
            resolve_path("/home/user", "file.txt"),
            "/home/user/file.txt"
        );
    }

    #[test]
    fn test_resolve_path_empty_base() {
        assert_eq!(resolve_path("", "/etc/passwd"), "/etc/passwd");
        assert_eq!(resolve_path("", "file.txt"), "file.txt");
    }

    #[test]
    fn test_resolve_path_home() {
        let home = directories::UserDirs::new()
            .unwrap()
            .home_dir()
            .to_path_buf();
        let expected = home.join("test.txt").to_string_lossy().to_string();
        assert_eq!(resolve_path("/tmp", "~/test.txt"), expected);

        let expected_home = home.to_string_lossy().to_string();
        assert_eq!(resolve_path("/tmp", "~"), expected_home);
    }
}
