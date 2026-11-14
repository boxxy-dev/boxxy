use crate::config::Settings;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use libadwaita as adw;
use std::cell::RefCell;
use std::path::PathBuf;
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

#[derive(Debug, Clone)]
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
    let display_name = if settings.user_profile.display_name.trim().is_empty() {
        fallback_display_name()
    } else {
        settings.user_profile.display_name.trim().to_string()
    };

    EffectiveUserProfile {
        display_name,
        color: settings
            .user_profile
            .color
            .clone()
            .filter(|color| is_safe_hex_color(color))
            .unwrap_or_else(|| DEFAULT_USER_COLOR.to_string()),
        avatar_path: boxxy_assets::user::get_user_avatar_path(),
    }
}

#[must_use]
pub fn fallback_display_name() -> String {
    let real_name = glib::real_name().to_string_lossy().trim().to_string();
    if real_name.is_empty() {
        glib::user_name().to_string_lossy().to_string()
    } else {
        real_name
    }
}

fn load_cached_user_avatar_texture(path: &PathBuf) -> Option<gdk::Texture> {
    let now = Instant::now();
    USER_AVATAR_CACHE.with(|cell| {
        {
            let borrowed = cell.borrow();
            if let Some(entry) = borrowed.as_ref()
                && &entry.path == path
                && now.duration_since(entry.last_checked) < USER_AVATAR_STAT_TTL
            {
                return entry.texture.clone();
            }
        }

        let mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());

        let mut slot = cell.borrow_mut();
        if let Some(entry) = slot.as_mut()
            && &entry.path == path
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
            path: path.clone(),
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

pub fn set_default_color_button(button: &gtk::ColorDialogButton, settings: &Settings) {
    let color = settings
        .user_profile
        .color
        .as_deref()
        .filter(|color| is_safe_hex_color(color))
        .unwrap_or(DEFAULT_USER_COLOR);

    if let Ok(rgba) = gdk::RGBA::parse(color) {
        button.set_rgba(&rgba);
    }
}

fn is_safe_hex_color(color: &str) -> bool {
    color.len() == 7 && color.starts_with('#') && color[1..].chars().all(|c| c.is_ascii_hexdigit())
}
