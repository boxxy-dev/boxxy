use std::path::PathBuf;

/// Resolves the `<config_dir>/user/` directory without performing disk I/O.
///
/// Callers writing files inside this directory should create it first
/// (for example via `tokio::fs::create_dir_all`).
#[must_use]
pub fn get_user_dir() -> PathBuf {
    boxxy_sys_utils::get_config_dir().join("user")
}

/// Resolves the `<config_dir>/user/AVATAR.png` path without performing disk I/O.
#[must_use]
pub fn get_user_avatar_path() -> PathBuf {
    get_user_dir().join("AVATAR.png")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_user_paths_under_config_dir() {
        let user_dir = get_user_dir();
        let avatar_path = get_user_avatar_path();
        assert!(user_dir.ends_with("user"));
        assert_eq!(avatar_path, user_dir.join("AVATAR.png"));
    }
}
