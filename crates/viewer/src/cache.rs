use crate::parser::blocks::ContentBlock;
use crate::parser::markdown::parse_markdown;
use boxxy_assets::image::{
    ThumbnailOutput, ThumbnailSpec, decode_rgba_from_png, encode_rgba_to_png,
};
use gtk4 as gtk;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::UNIX_EPOCH;

const THUMB_MEMORY_CACHE_CAPACITY: usize = 96;
const AST_CACHE_CAPACITY: usize = 256;
const HIGHLIGHT_CACHE_CAPACITY: usize = 128;
const DISK_CACHE_MAGIC: &[u8; 4] = b"BXT1";

/// Compact bounded LRU cache backed by `VecDeque`.
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
        let pos = self.entries.iter().position(|(k, _)| k == key)?;
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
        use std::fmt::Write;
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

// ---------------------------------------------------------------------------
// Parsed Markdown AST Cache & Syntax Highlight Cache
// ---------------------------------------------------------------------------

type AstCacheKey = (usize, u64);
type HighlightCacheKey = (usize, usize, u64, bool);

static AST_CACHE: LazyLock<Mutex<SimpleLru<AstCacheKey, Arc<[ContentBlock]>>>> =
    LazyLock::new(|| Mutex::new(SimpleLru::new(AST_CACHE_CAPACITY)));

static HIGHLIGHT_CACHE: LazyLock<Mutex<SimpleLru<HighlightCacheKey, String>>> =
    LazyLock::new(|| Mutex::new(SimpleLru::new(HIGHLIGHT_CACHE_CAPACITY)));

fn hash_str(s: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

fn highlight_cache_key(lang: &str, code: &str, is_dark: bool) -> HighlightCacheKey {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    lang.hash(&mut hasher);
    code.hash(&mut hasher);
    (lang.len(), code.len(), hasher.finish(), is_dark)
}

/// Returns cached parsed `ContentBlock`s for `input`, or parses and caches them on miss.
#[must_use]
pub fn get_or_parse_markdown(input: &str) -> Arc<[ContentBlock]> {
    let key = (input.len(), hash_str(input));
    if let Some(cached) = AST_CACHE.lock().get(&key) {
        return cached;
    }
    let parsed: Arc<[ContentBlock]> = Arc::from(parse_markdown(input));
    AST_CACHE.lock().insert(key, parsed.clone());
    parsed
}

/// Synchronously looks up cached `syntect` Pango markup for a `(lang, code, is_dark)` tuple.
#[must_use]
pub fn get_cached_highlight(lang: &str, code: &str, is_dark: bool) -> Option<String> {
    let key = highlight_cache_key(lang, code, is_dark);
    HIGHLIGHT_CACHE.lock().get(&key)
}

/// Stores `syntect` Pango markup in the highlight LRU cache.
pub fn insert_cached_highlight(lang: &str, code: &str, is_dark: bool, markup: String) {
    let key = highlight_cache_key(lang, code, is_dark);
    HIGHLIGHT_CACHE.lock().insert(key, markup);
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
        insert_cached_highlight(
            "rust",
            "fn main() {}",
            true,
            "<tt>fn main()</tt>".to_string(),
        );
        assert_eq!(
            get_cached_highlight("rust", "fn main() {}", true).as_deref(),
            Some("<tt>fn main()</tt>")
        );
    }
}
