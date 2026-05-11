use std::fs;
use std::path::PathBuf;
use directories::ProjectDirs;

/// Gets the `~/.config/boxxy-terminal/user/` directory, creating it if it doesn't exist.
pub fn get_user_dir() -> Option<PathBuf> {
    if let Some(dirs) = ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
        let user_dir = dirs.config_dir().join("user");
        if !user_dir.exists() {
            let _ = fs::create_dir_all(&user_dir);
        }
        Some(user_dir)
    } else {
        None
    }
}

/// Gets the path to `~/.config/boxxy-terminal/user/AVATAR.png`
pub fn get_user_avatar_path() -> Option<PathBuf> {
    get_user_dir().map(|dir| dir.join("AVATAR.png"))
}
