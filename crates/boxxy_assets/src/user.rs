use std::fs;
use std::path::PathBuf;

/// Gets the `~/.config/boxxy-terminal/user/` directory, creating it if it doesn't exist.
pub fn get_user_dir() -> Option<PathBuf> {
    let config_dir = boxxy_sys_utils::get_config_dir();
    let user_dir = config_dir.join("user");
    if !user_dir.exists() {
        let _ = fs::create_dir_all(&user_dir);
    }
    Some(user_dir)
}

/// Gets the path to `~/.config/boxxy-terminal/user/AVATAR.png`
pub fn get_user_avatar_path() -> Option<PathBuf> {
    get_user_dir().map(|dir| dir.join("AVATAR.png"))
}
