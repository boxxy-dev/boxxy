//! Shared character display formatting, color sanitization, and GTK texture caching.
//!
//! Centralizes character name resolution across virtualized chat rows, the sidebar hero,
//! and the Past Conversations list, while caching decoded `gdk::Texture` avatars on the
//! main GTK thread keyed by file modification time so scrolling never re-reads PNGs from disk.

use boxxy_claw_protocol::characters::CharacterInfo;
use gtk4::gdk;
use gtk4::glib;
use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

type AvatarTextureCache = HashMap<PathBuf, (Option<SystemTime>, gdk::Texture)>;

thread_local! {
    static AVATAR_TEXTURES: RefCell<AvatarTextureCache> = RefCell::new(HashMap::new());
    static INCOGNITO_TEXTURE: OnceCell<Option<gdk::Texture>> = const { OnceCell::new() };
}

/// Converts a hyphenated or space-separated character identifier (e.g. `"niko-una"`)
/// into title case (`"Niko Una"`).
#[must_use]
pub(crate) fn prettify_character_name(raw: &str) -> String {
    raw.split(['-', ' '])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Resolves the canonical human-readable display name for a `CharacterInfo`.
#[must_use]
pub(crate) fn resolve_character_display_name(info: &CharacterInfo) -> String {
    if !info.config.display_name.trim().is_empty() {
        prettify_character_name(&info.config.display_name)
    } else {
        prettify_character_name(&info.config.name)
    }
}

/// Validates a `#RGB`, `#RGBA`, `#RRGGBB`, or `#RRGGBBAA` hex color string so malformed
/// user edits in `CHARACTER.toml` never break Pango markup or GTK CSS parsing.
#[must_use]
pub(crate) fn is_safe_hex_color(raw: &str) -> bool {
    let Some(hex) = raw.strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// Loads a `gdk::Texture` from disk on cache miss or when the file's modification
/// timestamp changes, returning the cached texture on subsequent binds/renders.
#[must_use]
pub(crate) fn load_cached_texture(path: &Path) -> Option<gdk::Texture> {
    let mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());

    AVATAR_TEXTURES.with(|cache_cell| {
        if let Some((cached_mtime, texture)) = cache_cell.borrow().get(path)
            && *cached_mtime == mtime
        {
            return Some(texture.clone());
        }

        let texture = gdk::Texture::from_filename(path).ok()?;
        let mut cache = cache_cell.borrow_mut();
        // Bound cache growth if many transient attachment thumbnails are viewed across sessions.
        if cache.len() >= 128 {
            cache.clear();
        }
        cache.insert(path.to_path_buf(), (mtime, texture.clone()));
        Some(texture)
    })
}

/// Loads and caches the static bundled `Inco Gnito` avatar texture once per UI thread.
#[must_use]
pub(crate) fn load_incognito_texture() -> Option<gdk::Texture> {
    INCOGNITO_TEXTURE.with(|cell| {
        cell.get_or_init(|| {
            let bytes = glib::Bytes::from_static(
                boxxy_claw_protocol::character_loader::incognito_avatar_bytes(),
            );
            gdk::Texture::from_bytes(&bytes).ok()
        })
        .clone()
    })
}

/// Loads a character's `AVATAR.png` texture (if present) using the mtime-aware cache.
#[must_use]
pub(crate) fn load_character_avatar_texture(info: &CharacterInfo) -> Option<gdk::Texture> {
    if !info.has_avatar {
        return None;
    }
    let dir = boxxy_claw_protocol::character_loader::get_characters_dir().ok()?;
    let avatar_path = dir.join(&info.config.name).join("AVATAR.png");
    load_cached_texture(&avatar_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prettify_character_name_formats_slugs_and_spaces() {
        assert_eq!(prettify_character_name("niko-una"), "Niko Una");
        assert_eq!(prettify_character_name("  levi  kujo "), "Levi Kujo");
        assert_eq!(prettify_character_name("kuro"), "Kuro");
        assert_eq!(prettify_character_name(""), "");
    }

    #[test]
    fn safe_hex_color_rejects_malformed_inputs() {
        assert!(is_safe_hex_color("#7C5CFF"));
        assert!(is_safe_hex_color("#fff"));
        assert!(is_safe_hex_color("#12345678"));
        assert!(!is_safe_hex_color("7C5CFF"));
        assert!(!is_safe_hex_color("#12345"));
        assert!(!is_safe_hex_color("#GGGGGG"));
        assert!(!is_safe_hex_color("red; background: black"));
    }
}
