//! Deduplicated badge CSS registry and TTL-bounded avatar texture cache for autocomplete rows.

use gtk4 as gtk;
use gtk4::gdk;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

const TEXTURE_STAT_TTL: Duration = Duration::from_secs(2);
const MAX_AVATAR_CACHE_ENTRIES: usize = 64;
const MAX_BADGE_CLASSES: usize = 256;
const FALLBACK_BADGE_CLASS: &str = "autocomplete-badge-fallback";

struct CachedAvatarEntry {
    last_checked: Instant,
    mtime: Option<SystemTime>,
    texture: Option<gdk::Texture>,
}

struct BadgeCssRegistry {
    provider: Option<gtk::CssProvider>,
    stylesheet: String,
    classes: HashMap<String, String>,
    next_id: usize,
}

impl BadgeCssRegistry {
    fn new() -> Self {
        let mut stylesheet = String::with_capacity(768);
        let _ = writeln!(
            stylesheet,
            "popover.autocomplete-popover > contents .{FALLBACK_BADGE_CLASS}, .{FALLBACK_BADGE_CLASS} {{ background-color: @accent_bg_color; color: @accent_fg_color; border-radius: 12px; padding: 2px 8px; }}\npopover.autocomplete-popover > contents .{FALLBACK_BADGE_CLASS} label, .{FALLBACK_BADGE_CLASS} label {{ color: @accent_fg_color; font-weight: bold; font-size: 0.9em; }}"
        );
        Self {
            provider: None,
            stylesheet,
            classes: HashMap::new(),
            next_id: 0,
        }
    }

    fn ensure_provider(&mut self) -> Option<&gtk::CssProvider> {
        if self.provider.is_none()
            && let Some(display) = gdk::Display::default()
        {
            let p = gtk::CssProvider::new();
            p.load_from_string(&self.stylesheet);
            // Register above `STYLE_PROVIDER_PRIORITY_USER` (`boxxy_skeleton.css` = 800)
            // so `popover > contents label { color: @popover_fg_color; font-weight: normal; }`
            // in `menus-popovers.css` cannot clobber per-badge contrast ink or bold weight.
            gtk::style_context_add_provider_for_display(
                &display,
                &p,
                gtk::STYLE_PROVIDER_PRIORITY_USER + 50,
            );
            self.provider = Some(p);
        }
        self.provider.as_ref()
    }

    fn get_or_register(&mut self, raw_color: &str) -> String {
        let Some((css_bg, rgb)) = sanitize_badge_color(raw_color) else {
            let _ = self.ensure_provider();
            return FALLBACK_BADGE_CLASS.to_string();
        };

        if let Some(existing) = self.classes.get(&css_bg) {
            return existing.clone();
        }

        if self.classes.len() >= MAX_BADGE_CLASSES {
            let _ = self.ensure_provider();
            return FALLBACK_BADGE_CLASS.to_string();
        }

        let fg = readable_badge_fg(rgb);
        let class_name = format!("autocomplete-badge-c{}", self.next_id);
        self.next_id += 1;
        let _ = writeln!(
            self.stylesheet,
            "popover.autocomplete-popover > contents .{class_name}, .{class_name} {{ background-color: {css_bg}; color: {fg}; border-radius: 12px; padding: 2px 8px; }}\npopover.autocomplete-popover > contents .{class_name} label, .{class_name} label {{ color: {fg}; font-weight: bold; font-size: 0.9em; }}"
        );

        let css = self.stylesheet.clone();
        if let Some(p) = self.ensure_provider() {
            p.load_from_string(&css);
        } else {
            // Headless unit test without a display: keep map consistent anyway
        }

        self.classes.insert(css_bg, class_name.clone());
        class_name
    }
}

thread_local! {
    static AVATAR_TEXTURE_CACHE: RefCell<HashMap<PathBuf, CachedAvatarEntry>> =
        RefCell::new(HashMap::new());
    static BADGE_CSS_REGISTRY: RefCell<BadgeCssRegistry> =
        RefCell::new(BadgeCssRegistry::new());
}

/// Returns a deduplicated CSS class name for `raw_color` backed by a single display-level
/// `GtkCssProvider`, avoiding per-row style-cascade churn and CSS injection.
#[must_use]
pub(super) fn badge_color_css_class(raw_color: &str) -> String {
    BADGE_CSS_REGISTRY.with(|reg| reg.borrow_mut().get_or_register(raw_color))
}

/// Loads a cached `gdk::Texture` for `path`, throttling `stat()` syscalls (including negative
/// misses for deleted or missing files) to once per 2 seconds and bounding total entries.
#[must_use]
pub(super) fn load_cached_avatar_texture(path_str: &str) -> Option<gdk::Texture> {
    let path = Path::new(path_str);
    let now = Instant::now();

    AVATAR_TEXTURE_CACHE.with(|cache_cell| {
        {
            let cache = cache_cell.borrow();
            if let Some(entry) = cache.get(path)
                && now.duration_since(entry.last_checked) < TEXTURE_STAT_TTL
            {
                return entry.texture.clone();
            }
        }

        let mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
        let mut cache = cache_cell.borrow_mut();

        if let Some(entry) = cache.get_mut(path)
            && entry.mtime == mtime
        {
            entry.last_checked = now;
            return entry.texture.clone();
        }

        let texture = mtime.and_then(|_| crate::texture::decode_texture_from_file(path));
        if cache.len() >= MAX_AVATAR_CACHE_ENTRIES
            && let Some(oldest_key) = cache
                .iter()
                .min_by_key(|(_, v)| v.last_checked)
                .map(|(k, _)| k.clone())
        {
            cache.remove(&oldest_key);
        }

        cache.insert(
            path.to_path_buf(),
            CachedAvatarEntry {
                last_checked: now,
                mtime,
                texture: texture.clone(),
            },
        );
        texture
    })
}

/// Parses and canonicalizes `#RGB`, `#RGBA`, `#RRGGBB`, `#RRGGBBAA`, or `rgb(r, g, b)` into
/// a safe CSS color token and an approximate `[r, g, b]` triple for contrast selection.
#[must_use]
fn sanitize_badge_color(raw: &str) -> Option<(String, [u8; 3])> {
    let trimmed = raw.trim();
    if let Some(hex) = trimmed.strip_prefix('#') {
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let rgb = match hex.len() {
            3 | 4 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
                [r, g, b]
            }
            6 | 8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                [r, g, b]
            }
            _ => return None,
        };
        return Some((format!("#{}", hex.to_ascii_lowercase()), rgb));
    }

    let inner = trimmed
        .strip_prefix("rgb(")
        .or_else(|| trimmed.strip_prefix("RGB("))?
        .strip_suffix(')')?;
    let mut parts = inner.split(',');
    let r = parts.next()?.trim().parse::<u8>().ok()?;
    let g = parts.next()?.trim().parse::<u8>().ok()?;
    let b = parts.next()?.trim().parse::<u8>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((format!("#{r:02x}{g:02x}{b:02x}"), [r, g, b]))
}

#[must_use]
fn srgb_channel_to_linear(channel: u8) -> f64 {
    let c = f64::from(channel) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Picks crisp dark ink on bright/medium-bright badges and white ink on dark badges
/// using WCAG relative luminance crossover (~0.28).
#[must_use]
fn readable_badge_fg([r, g, b]: [u8; 3]) -> &'static str {
    let lum = 0.2126_f64.mul_add(
        srgb_channel_to_linear(r),
        0.7152_f64.mul_add(
            srgb_channel_to_linear(g),
            0.0722 * srgb_channel_to_linear(b),
        ),
    );
    if lum > 0.28 { "#111318" } else { "#ffffff" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_badge_color_accepts_hex_and_rgb_and_rejects_injection() {
        assert_eq!(
            sanitize_badge_color(" #7C5CFF "),
            Some(("#7c5cff".to_string(), [0x7C, 0x5C, 0xFF]))
        );
        assert_eq!(
            sanitize_badge_color("#f0a"),
            Some(("#f0a".to_string(), [0xFF, 0x00, 0xAA]))
        );
        assert_eq!(
            sanitize_badge_color("rgb(50, 120, 199)"),
            Some(("#3278c7".to_string(), [50, 120, 199]))
        );

        // Malicious or malformed inputs must be rejected
        assert!(sanitize_badge_color("red; } * { opacity: 0;").is_none());
        assert!(sanitize_badge_color("#12345").is_none());
        assert!(sanitize_badge_color("rgb(300, 0, 0)").is_none());
        assert!(sanitize_badge_color("rgb(1, 2, 3, 4)").is_none());
        assert!(sanitize_badge_color("").is_none());
    }

    #[test]
    fn test_readable_badge_fg_switches_for_light_pastels_and_yellow() {
        // Bright yellow (`#f8e45c`) and bright green (`#2ec27e`) get dark ink
        assert_eq!(readable_badge_fg([0xF8, 0xE4, 0x5C]), "#111318");
        assert_eq!(readable_badge_fg([0x2E, 0xC2, 0x7E]), "#111318");
        // Deep purple (`#8035d4`) and dark navy get white ink
        assert_eq!(readable_badge_fg([0x80, 0x35, 0xD4]), "#ffffff");
        assert_eq!(readable_badge_fg([45, 55, 90]), "#ffffff");
    }

    #[test]
    fn test_badge_stylesheet_targets_child_label_explicitly() {
        let mut reg = BadgeCssRegistry::new();
        let cls = reg.get_or_register("#f8e45c");
        assert!(reg.stylesheet.contains(&format!(
            ".{cls} label {{ color: #111318; font-weight: bold;"
        )));
    }
}
