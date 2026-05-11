use crate::config::Settings;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::path::PathBuf;

pub const DEFAULT_USER_COLOR: &str = "#0461be";

#[derive(Debug, Clone)]
pub struct EffectiveUserProfile {
    pub display_name: String,
    pub color: String,
    pub avatar_path: Option<PathBuf>,
}

pub fn current() -> EffectiveUserProfile {
    effective(&Settings::load())
}

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

pub fn fallback_display_name() -> String {
    let real_name = glib::real_name().to_string_lossy().trim().to_string();
    if real_name.is_empty() {
        glib::user_name().to_string_lossy().to_string()
    } else {
        real_name
    }
}

pub fn configure_avatar(avatar: &adw::Avatar, profile: &EffectiveUserProfile) {
    avatar.set_text(Some(&profile.display_name));
    avatar.set_custom_image(None::<&gdk::Texture>);

    if let Some(path) = &profile.avatar_path
        && path.exists()
        && let Ok(texture) = gdk::Texture::from_filename(path)
    {
        avatar.set_custom_image(Some(&texture));
    }
}

pub fn set_default_color_button(button: &gtk::ColorButton, settings: &Settings) {
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
