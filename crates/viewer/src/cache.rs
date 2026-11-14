use crate::parser::blocks::ContentBlock;
use crate::parser::markdown::{normalize_fence_language, parse_markdown, write_escaped_pango};
use boxxy_assets::image::{
    ThumbnailOutput, ThumbnailSpec, decode_rgba_from_png, encode_rgba_to_png,
};
use gtk4 as gtk;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::UNIX_EPOCH;

const THUMB_MEMORY_CACHE_CAPACITY: usize = 96;
const AST_CACHE_CAPACITY: usize = 512;
const HIGHLIGHT_CACHE_CAPACITY: usize = 512;
const DISK_CACHE_MAGIC: &[u8; 4] = b"BXT1";

/// Compact bounded LRU cache backed by `VecDeque`, supporting zero-allocation borrowed lookups.
pub(crate) struct SimpleLru<K, V> {
    capacity: usize,
    entries: VecDeque<(K, V)>,
}

impl<K: Eq, V: Clone> SimpleLru<K, V> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: VecDeque::with_capacity(capacity),
        }
    }

    pub(crate) fn get(&mut self, key: &K) -> Option<V> {
        self.get_by(|k| k == key)
    }

    pub(crate) fn get_by(&mut self, mut pred: impl FnMut(&K) -> bool) -> Option<V> {
        let pos = self.entries.iter().position(|(k, _)| pred(k))?;
        if pos == 0 {
            return Some(self.entries[0].1.clone());
        }
        let (k, v) = self.entries.remove(pos)?;
        let out = v.clone();
        self.entries.push_front((k, v));
        Some(out)
    }

    pub(crate) fn insert(&mut self, key: K, val: V) {
        if let Some(pos) = self.entries.iter().position(|(k, _)| k == &key) {
            self.entries.remove(pos);
        } else if self.entries.len() >= self.capacity {
            self.entries.pop_back();
        }
        self.entries.push_front((key, val));
    }
}

// ---------------------------------------------------------------------------
// Tier-1: Main-Thread GTK MemoryTexture LRU Cache
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThumbnailCacheKey {
    pub url: String,
    pub spec: ThumbnailSpec,
}

#[derive(Clone)]
pub struct CachedThumbnail {
    pub texture: gtk::gdk::MemoryTexture,
    pub disp_w: i32,
    pub disp_h: i32,
    pub original_width: u32,
    pub original_height: u32,
}

thread_local! {
    static TIER1_TEXTURE_CACHE: RefCell<SimpleLru<ThumbnailCacheKey, CachedThumbnail>> =
        RefCell::new(SimpleLru::new(THUMB_MEMORY_CACHE_CAPACITY));
}

/// Synchronously queries the Tier-1 in-memory texture cache on the GTK main thread.
/// Hits in 0 ms during `GtkListView` row recycling (`connect_bind`).
#[must_use]
pub fn get_cached_thumbnail(key: &ThumbnailCacheKey) -> Option<CachedThumbnail> {
    TIER1_TEXTURE_CACHE.with(|cache| cache.borrow_mut().get(key))
}

/// Inserts a ready-to-render `MemoryTexture` into the Tier-1 main-thread LRU cache.
pub fn insert_cached_thumbnail(key: ThumbnailCacheKey, entry: CachedThumbnail) {
    TIER1_TEXTURE_CACHE.with(|cache| cache.borrow_mut().insert(key, entry));
}

// ---------------------------------------------------------------------------
// Tier-2: Persistent Disk Thumbnail Cache (<config_dir>/cache/thumbnails/)
// ---------------------------------------------------------------------------

fn disk_cache_dir() -> PathBuf {
    boxxy_sys_utils::get_config_dir()
        .join("cache")
        .join("thumbnails")
}

/// Computes the deterministic SHA-256 disk cache filename for a local or remote image.
#[must_use]
pub fn compute_disk_cache_key(
    url_or_path: &str,
    mtime_secs: u64,
    file_len: u64,
    spec: ThumbnailSpec,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url_or_path.as_bytes());
    hasher.update(mtime_secs.to_le_bytes());
    hasher.update(file_len.to_le_bytes());
    hasher.update(spec.max_width.to_le_bytes());
    hasher.update(spec.max_height.to_le_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(68);
    for b in digest {
        let _ = write!(&mut hex, "{b:02x}");
    }
    hex.push_str(".bxt");
    hex
}

/// Attempts to load a pre-scaled thumbnail from the Tier-2 disk cache.
/// Binary format: `[b"BXT1" (4B), orig_w: u32 LE (4B), orig_h: u32 LE (4B), png_bytes...]`.
pub async fn load_disk_cached_thumbnail(cache_filename: &str) -> Option<ThumbnailOutput> {
    let path = disk_cache_dir().join(cache_filename);
    let bytes = tokio::fs::read(&path).await.ok()?;
    tokio::task::spawn_blocking(move || decode_disk_cache_blob(&bytes))
        .await
        .ok()
        .flatten()
}

/// Writes a generated thumbnail to the Tier-2 disk cache atomically.
pub async fn save_disk_cached_thumbnail(cache_filename: &str, thumb: &ThumbnailOutput) {
    let dir = disk_cache_dir();
    if tokio::fs::create_dir_all(&dir).await.is_err() {
        return;
    }

    let rgba = thumb.rgba_bytes.clone();
    let tw = thumb.thumb_width;
    let th = thumb.thumb_height;
    let orig_w = thumb.original_width;
    let orig_h = thumb.original_height;

    let Ok(Some(blob)) =
        tokio::task::spawn_blocking(move || encode_disk_cache_blob(&rgba, tw, th, orig_w, orig_h))
            .await
    else {
        return;
    };

    let final_path = dir.join(cache_filename);
    let tmp_path = dir.join(format!("{cache_filename}.tmp"));
    if tokio::fs::write(&tmp_path, &blob).await.is_ok() {
        let _ = tokio::fs::rename(&tmp_path, &final_path).await;
    }
}

fn encode_disk_cache_blob(
    rgba: &[u8],
    thumb_w: u32,
    thumb_h: u32,
    orig_w: u32,
    orig_h: u32,
) -> Option<Vec<u8>> {
    let png = encode_rgba_to_png(rgba, thumb_w, thumb_h).ok()?;
    let mut out = Vec::with_capacity(12 + png.len());
    out.extend_from_slice(DISK_CACHE_MAGIC);
    out.extend_from_slice(&orig_w.to_le_bytes());
    out.extend_from_slice(&orig_h.to_le_bytes());
    out.extend_from_slice(&png);
    Some(out)
}

fn decode_disk_cache_blob(blob: &[u8]) -> Option<ThumbnailOutput> {
    if blob.len() <= 12 || &blob[0..4] != DISK_CACHE_MAGIC {
        return None;
    }
    let orig_w = u32::from_le_bytes(blob[4..8].try_into().ok()?);
    let orig_h = u32::from_le_bytes(blob[8..12].try_into().ok()?);
    let (thumb_w, thumb_h, rgba_bytes) = decode_rgba_from_png(&blob[12..]).ok()?;
    Some(ThumbnailOutput {
        rgba_bytes,
        thumb_width: thumb_w,
        thumb_height: thumb_h,
        original_width: orig_w,
        original_height: orig_h,
    })
}

/// Extracts `(mtime_secs, len)` from `std::fs::Metadata` for cache invalidation when local files change.
#[must_use]
pub fn metadata_signature(meta: &std::fs::Metadata) -> (u64, u64) {
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    (mtime, meta.len())
}

// ---------------------------------------------------------------------------
// Shared Syntect Syntax & Theme Sets
// ---------------------------------------------------------------------------

static SYNTAX_SET: LazyLock<syntect::parsing::SyntaxSet> =
    LazyLock::new(syntect::parsing::SyntaxSet::load_defaults_newlines);
static THEME_SET: LazyLock<syntect::highlighting::ThemeSet> =
    LazyLock::new(syntect::highlighting::ThemeSet::load_defaults);

/// Returns a reference to the process-wide default `syntect::parsing::SyntaxSet`.
#[must_use]
pub fn syntax_set() -> &'static syntect::parsing::SyntaxSet {
    &SYNTAX_SET
}

/// Returns a reference to the process-wide default `syntect::highlighting::ThemeSet`.
#[must_use]
pub fn theme_set() -> &'static syntect::highlighting::ThemeSet {
    &THEME_SET
}

/// Returns true if `lang` is a shell/terminal snippet language (matching OpenCode's `shellLanguages`).
#[must_use]
pub fn is_shell_language(lang: &str) -> bool {
    let norm = normalize_fence_language(lang);
    matches!(
        norm.as_str(),
        "bash" | "sh" | "shell" | "zsh" | "fish" | "console" | "terminal"
    )
}

/// Resolves a syntax definition by fence info-string / language token / extension, with
/// fallback aliases for modern languages and config formats missing from `syntect`'s default pack.
#[must_use]
pub fn resolve_syntax<'a>(
    ss: &'a syntect::parsing::SyntaxSet,
    lang: &str,
    first_line_sample: Option<&str>,
) -> &'a syntect::parsing::SyntaxReference {
    let norm = normalize_fence_language(lang);
    if !norm.is_empty() && norm != "text" && norm != "plain" && norm != "plaintext" {
        if let Some(syn) = ss
            .find_syntax_by_token(&norm)
            .or_else(|| ss.find_syntax_by_extension(&norm))
        {
            return syn;
        }

        let alias_ext = match norm.as_str() {
            "ts" | "typescript" | "tsx" | "jsx" | "mts" | "cts" | "mjs" | "cjs" => Some("js"),
            "jsonc" | "jsonl" | "json5" => Some("json"),
            "toml" | "ini" | "conf" | "cfg" | "env" | "dotenv" | "zsh" | "fish" | "nu"
            | "nushell" | "just" | "justfile" | "dockerfile" | "containerfile" => Some("sh"),
            "zig" | "wgsl" | "odin" => Some("rs"),
            "glsl" | "hlsl" | "proto" | "protobuf" => Some("cpp"),
            "kt" | "kts" | "kotlin" => Some("java"),
            "nix" | "hcl" | "tf" | "terraform" | "gleam" => Some("rb"),
            "svelte" | "vue" | "astro" => Some("html"),
            "yml" | "gql" | "graphql" => Some("yaml"),
            _ => None,
        };

        if let Some(ext) = alias_ext
            && let Some(syn) = ss
                .find_syntax_by_extension(ext)
                .or_else(|| ss.find_syntax_by_token(ext))
        {
            return syn;
        }
    }

    first_line_sample
        .and_then(|s| ss.find_syntax_by_first_line(s))
        .unwrap_or_else(|| ss.find_syntax_plain_text())
}

/// Returns whether the active theme's `@card_bg_color` surface is dark.
///
/// Code blocks (`.claw-drawer frame.view`) and diff cards (`.viewer-diff-card`) render on
/// `@card_bg_color`, which in hybrid themes (e.g. dark arcade night `window_bg` with bright
/// brick-gold `card_bg`) may have the opposite luminance of `window_bg` / `StyleManager::is_dark()`.
#[must_use]
pub fn is_card_surface_dark() -> bool {
    if let Some(display) = gtk::gdk::Display::default() {
        use gtk::glib::prelude::ObjectExt;
        if let Some(ptr) = unsafe { display.data::<bool>("boxxy-card-is-dark") } {
            return unsafe { *ptr.as_ref() };
        }
    }
    libadwaita::StyleManager::default().is_dark()
}

/// Resolves the luminance-clamped `(r, g, b)` foreground color for a `syntect` token,
/// returning `None` when the token matches the theme's default foreground so it inherits `@card_fg_color`.
#[must_use]
pub fn resolve_syntect_fg(
    style: syntect::highlighting::Style,
    default_fg: Option<syntect::highlighting::Color>,
    is_dark: bool,
) -> Option<(u8, u8, u8)> {
    let c = style.foreground;
    let is_default_fg = default_fg.is_some_and(|fg| {
        fg.r.abs_diff(c.r) <= 4 && fg.g.abs_diff(c.g) <= 4 && fg.b.abs_diff(c.b) <= 4
    });
    if is_default_fg {
        None
    } else {
        Some(adjust_syntax_fg(c.r, c.g, c.b, is_dark))
    }
}

/// Appends a single `syntect` token formatted as Pango markup directly into `out` without
/// intermediate `String` allocations.
pub fn write_syntect_span(
    out: &mut String,
    style: syntect::highlighting::Style,
    default_fg: Option<syntect::highlighting::Color>,
    token: &str,
    is_dark: bool,
) {
    if let Some((r, g, b)) = resolve_syntect_fg(style, default_fg, is_dark) {
        let _ = write!(out, "<span foreground=\"#{r:02x}{g:02x}{b:02x}\">");
        write_escaped_pango(out, token);
        out.push_str("</span>");
    } else {
        write_escaped_pango(out, token);
    }
}

/// Formats a single `syntect` token into Pango markup.
#[must_use]
pub fn format_syntect_span(
    style: syntect::highlighting::Style,
    default_fg: Option<syntect::highlighting::Color>,
    token: &str,
    is_dark: bool,
) -> String {
    let mut out = String::with_capacity(token.len() + 34);
    write_syntect_span(&mut out, style, default_fg, token, is_dark);
    out
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn adjust_syntax_fg(r: u8, g: u8, b: u8, is_dark: bool) -> (u8, u8, u8) {
    let rf = f32::from(r) / 255.0;
    let gf = f32::from(g) / 255.0;
    let bf = f32::from(b) / 255.0;
    let lum = 0.299 * rf + 0.587 * gf + 0.114 * bf;

    if is_dark {
        let min_lum = 0.72_f32;
        if lum < min_lum {
            let t = ((min_lum - lum) / (1.0 - lum).max(0.01)).clamp(0.0, 1.0);
            let nr = ((rf + (1.0 - rf) * t) * 255.0).round() as u8;
            let ng = ((gf + (1.0 - gf) * t) * 255.0).round() as u8;
            let nb = ((bf + (1.0 - bf) * t) * 255.0).round() as u8;
            return (nr, ng, nb);
        }
    } else {
        let max_lum = 0.28_f32;
        if lum > max_lum {
            let target_lum = 0.05_f32;
            let t = ((lum - max_lum) / (lum - target_lum).max(0.01)).clamp(0.0, 1.0);
            let nr = ((rf * (1.0 - t) + 0.05 * t) * 255.0).round() as u8;
            let ng = ((gf * (1.0 - t) + 0.06 * t) * 255.0).round() as u8;
            let nb = ((bf * (1.0 - t) + 0.08 * t) * 255.0).round() as u8;
            return (nr, ng, nb);
        }
    }

    (r, g, b)
}

// ---------------------------------------------------------------------------
// Collision-Safe Parsed Markdown AST Cache & Syntax Highlight Cache
// ---------------------------------------------------------------------------

#[derive(PartialEq, Eq)]
struct AstCacheKey {
    hash: u64,
    raw: Box<str>,
}

#[derive(PartialEq, Eq)]
struct HighlightCacheKey {
    hash: u64,
    is_dark: bool,
    lang: Box<str>,
    code: Box<str>,
}

static AST_CACHE: LazyLock<Mutex<SimpleLru<AstCacheKey, Arc<[ContentBlock]>>>> =
    LazyLock::new(|| Mutex::new(SimpleLru::new(AST_CACHE_CAPACITY)));

static HIGHLIGHT_CACHE: LazyLock<Mutex<SimpleLru<HighlightCacheKey, Arc<str>>>> =
    LazyLock::new(|| Mutex::new(SimpleLru::new(HIGHLIGHT_CACHE_CAPACITY)));

fn hash_str(s: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

fn hash_highlight(lang: &str, code: &str, is_dark: bool) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    lang.hash(&mut hasher);
    code.hash(&mut hasher);
    is_dark.hash(&mut hasher);
    hasher.finish()
}

/// Returns cached parsed `ContentBlock`s for `input`, or parses and caches them on miss.
/// Lookup compares the 64-bit hash first and verifies exact `&str` equality on match (0 allocations on hit).
#[must_use]
pub fn get_or_parse_markdown(input: &str) -> Arc<[ContentBlock]> {
    let h = hash_str(input);
    if let Some(cached) = AST_CACHE
        .lock()
        .get_by(|k| k.hash == h && &*k.raw == input)
    {
        return cached;
    }
    let parsed: Arc<[ContentBlock]> = Arc::from(parse_markdown(input));
    AST_CACHE.lock().insert(
        AstCacheKey {
            hash: h,
            raw: input.into(),
        },
        parsed.clone(),
    );
    parsed
}

/// Synchronously looks up cached `syntect` Pango markup for a `(lang, code, is_dark)` tuple.
#[must_use]
pub fn get_cached_highlight(lang: &str, code: &str, is_dark: bool) -> Option<Arc<str>> {
    let h = hash_highlight(lang, code, is_dark);
    HIGHLIGHT_CACHE.lock().get_by(|k| {
        k.hash == h && k.is_dark == is_dark && &*k.lang == lang && &*k.code == code
    })
}

/// Stores `syntect` Pango markup in the highlight LRU cache.
pub fn insert_cached_highlight(lang: &str, code: &str, is_dark: bool, markup: impl Into<Arc<str>>) {
    let h = hash_highlight(lang, code, is_dark);
    HIGHLIGHT_CACHE.lock().insert(
        HighlightCacheKey {
            hash: h,
            is_dark,
            lang: lang.into(),
            code: code.into(),
        },
        markup.into(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disk_cache_blob_roundtrip() {
        let rgba = vec![128u8; 8 * 6 * 4];
        let blob = encode_disk_cache_blob(&rgba, 8, 6, 3840, 2160).unwrap();
        let decoded = decode_disk_cache_blob(&blob).unwrap();
        assert_eq!(decoded.thumb_width, 8);
        assert_eq!(decoded.thumb_height, 6);
        assert_eq!(decoded.original_width, 3840);
        assert_eq!(decoded.original_height, 2160);
        assert_eq!(decoded.rgba_bytes, rgba);
    }

    #[test]
    fn test_ast_and_highlight_cache() {
        let blocks1 = get_or_parse_markdown("# Hello\n\nWorld");
        let blocks2 = get_or_parse_markdown("# Hello\n\nWorld");
        assert!(Arc::ptr_eq(&blocks1, &blocks2));

        assert!(get_cached_highlight("rust", "fn main() {}", true).is_none());
        insert_cached_highlight("rust", "fn main() {}", true, "<tt>fn main()</tt>");
        assert_eq!(
            get_cached_highlight("rust", "fn main() {}", true).as_deref(),
            Some("<tt>fn main()</tt>")
        );
    }

    #[test]
    fn test_simple_lru_verifies_key_equality_on_hash_collision() {
        let mut lru: SimpleLru<AstCacheKey, usize> = SimpleLru::new(4);
        lru.insert(
            AstCacheKey {
                hash: 42,
                raw: "alpha".into(),
            },
            1,
        );
        lru.insert(
            AstCacheKey {
                hash: 42,
                raw: "bravo".into(),
            },
            2,
        );
        assert_eq!(lru.get_by(|k| k.hash == 42 && &*k.raw == "alpha"), Some(1));
        assert_eq!(lru.get_by(|k| k.hash == 42 && &*k.raw == "bravo"), Some(2));
        assert_eq!(lru.get_by(|k| k.hash == 42 && &*k.raw == "charlie"), None);
    }

    #[test]
    fn test_resolve_syntax_handles_info_strings_and_modern_aliases() {
        let ss = syntax_set();
        let plain = ss.find_syntax_plain_text().name.as_str();

        assert_ne!(resolve_syntax(ss, "Rust,no_run", None).name, plain);
        assert_ne!(resolve_syntax(ss, "tsx title=\"App.tsx\"", None).name, plain);
        assert_ne!(resolve_syntax(ss, "jsonc", None).name, plain);
        assert_ne!(resolve_syntax(ss, "dockerfile", None).name, plain);
        assert_ne!(resolve_syntax(ss, "zig", None).name, plain);
        assert!(is_shell_language("zsh"));
        assert!(is_shell_language("bash title=\"run.sh\""));
        assert!(!is_shell_language("rust"));
    }
}
