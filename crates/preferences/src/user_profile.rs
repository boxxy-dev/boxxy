use crate::config::Settings;
use gtk4::gdk;
use gtk4::glib;
use libadwaita as adw;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

pub const DEFAULT_USER_COLOR: &str = "#0461be";

const USER_AVATAR_STAT_TTL: Duration = Duration::from_secs(2);

struct CachedUserAvatar {
    path: PathBuf,
    last_checked: Instant,
    mtime: Option<SystemTime>,
    texture: Option<gdk::Texture>,
}

thread_local! {
    static USER_AVATAR_CACHE: RefCell<Option<CachedUserAvatar>> = const { RefCell::new(None) };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveUserProfile {
    pub display_name: String,
    pub color: String,
    pub avatar_path: PathBuf,
}

#[must_use]
pub fn current() -> EffectiveUserProfile {
    effective(&Settings::load())
}

#[must_use]
pub fn effective(settings: &Settings) -> EffectiveUserProfile {
    let trimmed = settings.user_profile.display_name.trim();
    let display_name = if trimmed.is_empty() {
        fallback_display_name()
    } else {
        trimmed.to_string()
    };

    EffectiveUserProfile {
        display_name,
        color: settings
            .user_profile
            .color
            .as_deref()
            .filter(|color| is_safe_hex_color(color))
            .unwrap_or(DEFAULT_USER_COLOR)
            .to_string(),
        avatar_path: boxxy_assets::user::get_user_avatar_path(),
    }
}

#[must_use]
pub fn fallback_display_name() -> String {
    let real_name = glib::real_name();
    let real_trimmed = real_name.to_string_lossy();
    let real_trimmed = real_trimmed.trim();
    if real_trimmed.is_empty() {
        glib::user_name().to_string_lossy().into_owned()
    } else {
        real_trimmed.to_string()
    }
}

/// Clears the thread-local user avatar texture cache so newly uploaded or removed
/// `AVATAR.png` files take effect immediately without waiting for the 2s stat TTL.
pub fn invalidate_avatar_cache() {
    USER_AVATAR_CACHE.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

fn load_cached_user_avatar_texture(path: &Path) -> Option<gdk::Texture> {
    let now = Instant::now();
    USER_AVATAR_CACHE.with(|cell| {
        {
            let borrowed = cell.borrow();
            if let Some(entry) = borrowed.as_ref()
                && entry.path == path
                && now.duration_since(entry.last_checked) < USER_AVATAR_STAT_TTL
            {
                return entry.texture.clone();
            }
        }

        let mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());

        let mut slot = cell.borrow_mut();
        if let Some(entry) = slot.as_mut()
            && entry.path == path
            && entry.mtime == mtime
        {
            entry.last_checked = now;
            return entry.texture.clone();
        }

        let texture = if mtime.is_some() {
            gdk::Texture::from_filename(path).ok()
        } else {
            None
        };
        *slot = Some(CachedUserAvatar {
            path: path.to_path_buf(),
            last_checked: now,
            mtime,
            texture: texture.clone(),
        });
        texture
    })
}

pub fn configure_avatar(avatar: &adw::Avatar, profile: &EffectiveUserProfile) {
    avatar.set_text(Some(&profile.display_name));
    let texture = load_cached_user_avatar_texture(&profile.avatar_path);
    avatar.set_custom_image(texture.as_ref());
}

const fn is_safe_hex_color(color: &str) -> bool {
    let bytes = color.as_bytes();
    if bytes.len() != 7 || bytes[0] != b'#' {
        return false;
    }
    let mut i = 1;
    while i < 7 {
        if !bytes[i].is_ascii_hexdigit() {
            return false;
        }
        i += 1;
    }
    true
}
