//! Shared character display formatting, deduplicated CSS color classes, and GTK texture caching.
//!
//! Centralizes character name resolution across virtualized chat rows, the sidebar hero,
//! and the Past Conversations list, while caching decoded `gdk::Texture` avatars on the
//! main GTK thread (with a 2s metadata stat TTL) and deduplicating CSS color classes into
//! a single shared `gtk::CssProvider` so scrolling never triggers display-wide CSS restyles.

use boxxy_claw_protocol::characters::CharacterInfo;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

const TEXTURE_STAT_TTL: Duration = Duration::from_secs(2);

struct CachedTextureEntry {
    last_checked: Instant,
    mtime: Option<SystemTime>,
    texture: gdk::Texture,
}

type AvatarTextureCache = HashMap<PathBuf, CachedTextureEntry>;

struct DynamicColorRegistry {
    provider: Option<gtk::CssProvider>,
    stylesheet: String,
    ring_classes: HashMap<String, String>,
    char_btn_classes: HashMap<String, String>,
    next_id: usize,
}

impl DynamicColorRegistry {
    fn new() -> Self {
        Self {
            provider: None,
            stylesheet: String::with_capacity(512),
            ring_classes: HashMap::new(),
            char_btn_classes: HashMap::new(),
            next_id: 0,
        }
    }

    fn ensure_provider(&mut self) -> Option<&gtk::CssProvider> {
        if self.provider.is_none()
            && let Some(display) = gdk::Display::default()
        {
            let p = gtk::CssProvider::new();
            gtk::style_context_add_provider_for_display(
                &display,
                &p,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
            self.provider = Some(p);
        }
        self.provider.as_ref()
    }

    fn reload_stylesheet(&mut self) {
        let css = self.stylesheet.clone();
        if let Some(p) = self.ensure_provider() {
            p.load_from_string(&css);
        }
    }

    fn get_or_register_ring_class(&mut self, raw_color: &str) -> String {
        let normalized = if is_safe_hex_color(raw_color) {
            raw_color.to_ascii_lowercase()
        } else {
            "@accent_color".to_string()
        };
        if let Some(existing) = self.ring_classes.get(&normalized) {
            return existing.clone();
        }
        let class_name = format!("claw-ring-color-{}", self.next_id);
        self.next_id += 1;
        let _ = writeln!(self.stylesheet, ".{class_name} {{ color: {normalized}; }}");
        self.reload_stylesheet();
        self.ring_classes.insert(normalized, class_name.clone());
        class_name
    }

    fn get_or_register_char_btn_class(&mut self, raw_color: &str) -> Option<String> {
        if !is_safe_hex_color(raw_color) {
            return None;
        }
        let normalized = raw_color.to_ascii_lowercase();
        if let Some(existing) = self.char_btn_classes.get(&normalized) {
            return Some(existing.clone());
        }
        let class_name = format!("char-btn-color-{}", self.next_id);
        self.next_id += 1;
        let _ = writeln!(
            self.stylesheet,
            ".{class_name} label {{ background-image: radial-gradient(circle, alpha({normalized}, 0.85) 62%, transparent 70%); }}"
        );
        self.reload_stylesheet();
        self.char_btn_classes.insert(normalized, class_name.clone());
        Some(class_name)
    }
}

thread_local! {
    static AVATAR_TEXTURES: RefCell<AvatarTextureCache> = RefCell::new(HashMap::new());
    static INCOGNITO_TEXTURE: OnceCell<Option<gdk::Texture>> = const { OnceCell::new() };
    static COLOR_CSS_REGISTRY: RefCell<DynamicColorRegistry> =
        RefCell::new(DynamicColorRegistry::new());
}

/// Returns a deduplicated CSS class setting `color: <hex>` for `.claw-avatar-frame`.
/// Subsequent lookups for the same color return in 0 ns without touching `GtkCssProvider`.
#[must_use]
pub(crate) fn avatar_ring_color_class(raw_color: &str) -> String {
    COLOR_CSS_REGISTRY.with(|reg| reg.borrow_mut().get_or_register_ring_class(raw_color))
}

/// Returns a deduplicated CSS class styling a character selector pill's radial glow for `raw_color`.
#[must_use]
pub fn character_btn_color_class(raw_color: &str) -> Option<String> {
    COLOR_CSS_REGISTRY.with(|reg| reg.borrow_mut().get_or_register_char_btn_class(raw_color))
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
/// timestamp changes (throttling `stat()` syscalls to once per 2s during fast scrolling).
#[must_use]
pub(crate) fn load_cached_texture(path: &Path) -> Option<gdk::Texture> {
    let now = Instant::now();

    AVATAR_TEXTURES.with(|cache_cell| {
        {
            let cache = cache_cell.borrow();
            if let Some(entry) = cache.get(path)
                && now.duration_since(entry.last_checked) < TEXTURE_STAT_TTL
            {
                return Some(entry.texture.clone());
            }
        }

        let mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
        let mut cache = cache_cell.borrow_mut();

        if let Some(entry) = cache.get_mut(path)
            && entry.mtime == mtime
        {
            entry.last_checked = now;
            return Some(entry.texture.clone());
        }

        let texture = gdk::Texture::from_filename(path).ok()?;
        if cache.len() >= 128 {
            cache.clear();
        }
        cache.insert(
            path.to_path_buf(),
            CachedTextureEntry {
                last_checked: now,
                mtime,
                texture: texture.clone(),
            },
        );
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
pub fn load_character_avatar_texture(info: &CharacterInfo) -> Option<gdk::Texture> {
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

    #[test]
    fn color_class_registry_deduplicates_case_insensitively() {
        let c1 = avatar_ring_color_class("#7C5CFF");
        let c2 = avatar_ring_color_class("#7c5cff");
        assert_eq!(c1, c2);

        let fallback1 = avatar_ring_color_class("invalid");
        let fallback2 = avatar_ring_color_class("");
        assert_eq!(fallback1, fallback2);
        assert_ne!(c1, fallback1);

        assert_eq!(
            character_btn_color_class("#FF66AA"),
            character_btn_color_class("#ff66aa")
        );
        assert!(character_btn_color_class("not-a-color").is_none());
    }
}
