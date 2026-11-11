use gtk::gdk;
use gtk4 as gtk;
use sourceview5::prelude::*;
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;

pub mod app_theme;
pub mod color_dot;
pub mod preview;
pub mod selector;

pub use color_dot::ColorDot;

use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock};

static TEXTURE_CACHE: LazyLock<Arc<Mutex<HashMap<String, gdk::Texture>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(HashMap::new())));

pub fn get_texture_from_path(path: &str) -> Option<gdk::Texture> {
    if let Some(texture) = TEXTURE_CACHE.lock().get(path).cloned() {
        return Some(texture);
    }

    let file = gtk::gio::File::for_path(path);
    if let Ok(texture) = gdk::Texture::from_file(&file) {
        TEXTURE_CACHE
            .lock()
            .insert(path.to_string(), texture.clone());
        return Some(texture);
    }
    None
}

pub fn clear_texture_cache() {
    TEXTURE_CACHE.lock().clear();
}

pub fn copy_background_image(src_path: &Path) -> Option<String> {
    let config_dir = get_schemes_dir()?.parent()?.join("backgrounds");
    fs::create_dir_all(&config_dir).ok()?;

    let extension = src_path.extension()?.to_str()?;
    let filename = format!("bg_{}.{}", uuid::Uuid::new_v4(), extension);
    let dest_path = config_dir.join(filename);

    fs::copy(src_path, &dest_path).ok()?;
    dest_path.to_str().map(|s| s.to_string())
}

pub fn delete_background_image(path: &str) {
    if let Ok(path) = Path::new(path).canonicalize()
        && let Some(config_dir) =
            get_schemes_dir().and_then(|p| p.parent().map(|p| p.join("backgrounds")))
        && let Ok(config_dir) = config_dir.canonicalize()
        && path.starts_with(config_dir)
    {
        // Ensure we only delete files within our backgrounds directory
        let _ = fs::remove_file(path);
    }
}

pub use selector::ThemeSelectorComponent;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct PaletteVariantStatic {
    pub background: &'static str,
    pub foreground: &'static str,
    pub cursor: &'static str,
    pub colors: [&'static str; 16],
    pub gtk_css: &'static str,
    pub sourceview_xml: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct ParsedPaletteStatic {
    pub name: &'static str,
    pub id: &'static str,
    pub light: PaletteVariantStatic,
    pub dark: PaletteVariantStatic,
}

pub type Palette = PaletteVariantStatic;
pub type ParsedPalette = ParsedPaletteStatic;

impl PaletteVariantStatic {
    pub fn to_vte_colors(&self) -> (gdk::RGBA, gdk::RGBA, Vec<gdk::RGBA>) {
        let fg = gdk::RGBA::parse(self.foreground).unwrap_or(gdk::RGBA::WHITE);
        let bg = gdk::RGBA::parse(self.background).unwrap_or(gdk::RGBA::BLACK);
        let palette = self
            .colors
            .iter()
            .map(|c| gdk::RGBA::parse(*c).unwrap_or(gdk::RGBA::BLACK))
            .collect();
        (fg, bg, palette)
    }
}

// Include the pre-compiled themes array
include!(concat!(env!("OUT_DIR"), "/generated_themes.rs"));

// ---------------------------------------------------------------------------
// Palette discovery & loading
// ---------------------------------------------------------------------------

/// Returns all pre-compiled palettes (alphabetically by display name).
pub fn list_palettes() -> Vec<ParsedPaletteStatic> {
    THEMES.to_vec()
}

/// Loads a palette by the display name stored in settings (e.g. "Dracula",
/// "Adventure Time"). Returns `None` for "System" / "" (pure Adwaita, no custom palette).
pub fn load_palette(name: &str) -> Option<ParsedPaletteStatic> {
    match name.trim() {
        "" | "System" | "system" | "none" => return None,
        _ => {}
    }

    // 1. Direct match by exact id or name
    if let Some(p) = THEMES.iter().find(|p| p.id == name || p.name == name) {
        return Some(*p);
    }

    // 2. Normalised fallback for legacy settings (e.g. "dracula" → "Dracula").
    let target = name.to_lowercase().replace('-', " ");
    if let Some(p) = THEMES.iter().find(|p| {
        p.id.to_lowercase().replace('-', " ") == target
            || p.name.to_lowercase().replace('-', " ") == target
    }) {
        return Some(*p);
    }

    None
}

// ---------------------------------------------------------------------------
// GTK / Adwaita theming
// ---------------------------------------------------------------------------

thread_local! {
    static CURRENT_PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
    static LAST_PALETTE_PTR: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// Apply a palette to GTK/Adwaita. Pass `None` for pure Adwaita (system theme).
/// `dark_mode` selects the [Dark] or [Light] variant and forces the Adwaita
/// colour scheme accordingly.
pub fn apply_palette(palette: Option<&ParsedPaletteStatic>, dark_mode: bool) {
    let next_css = palette.map(|p| if dark_mode { p.dark.gtk_css } else { p.light.gtk_css });
    let next_ptr = next_css.map(str::as_ptr).map(|ptr| ptr as usize);

    let unchanged = LAST_PALETTE_PTR.with(|cell| cell.get() == next_ptr);
    if unchanged {
        return;
    }

    clear_css_provider();
    if let Some(css) = next_css {
        load_css(css);
    }
    LAST_PALETTE_PTR.with(|cell| cell.set(next_ptr));
}

fn clear_css_provider() {
    if let Some(display) = gtk::gdk::Display::default() {
        CURRENT_PROVIDER.with(|p: &RefCell<Option<gtk::CssProvider>>| {
            if let Some(old) = p.borrow().as_ref() {
                gtk::style_context_remove_provider_for_display(&display, old);
            }
            *p.borrow_mut() = None;
        });
    }
}

fn load_css(css: &str) {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(css);
    if let Some(display) = gtk::gdk::Display::default() {
        CURRENT_PROVIDER.with(|p: &RefCell<Option<gtk::CssProvider>>| {
            *p.borrow_mut() = Some(provider.clone());
        });
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 10,
        );
    }
}

// ---------------------------------------------------------------------------
// GtkSourceView theming
// ---------------------------------------------------------------------------

static WRITTEN_SCHEMES: LazyLock<Mutex<HashMap<PathBuf, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Apply a palette to a SourceView buffer. Pass `None` for the default scheme.
pub fn apply_sourceview_palette(
    buffer: &sourceview5::Buffer,
    palette: Option<&ParsedPaletteStatic>,
    dark_mode: bool,
) {
    let scheme = palette.and_then(|p| {
        let variant = if dark_mode { &p.dark } else { &p.light };
        write_and_load_scheme(p.id, variant.sourceview_xml)
    });
    buffer.set_style_scheme(scheme.as_ref());
}

fn write_and_load_scheme(id: &str, xml: &'static str) -> Option<sourceview5::StyleScheme> {
    let schemes_dir = get_schemes_dir()?;
    let path = schemes_dir.join(format!("{id}.xml"));
    let xml_ptr = xml.as_ptr() as usize;

    let manager = sourceview5::StyleSchemeManager::default();
    let dir_str = schemes_dir.to_str()?;
    let mut needs_rescan = false;

    let mut paths = manager.search_path();
    if !paths.iter().any(|p| p == dir_str) {
        paths.push(dir_str.into());
        manager.set_search_path(&paths.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        needs_rescan = true;
    }

    let already_written = WRITTEN_SCHEMES.lock().get(&path).copied() == Some(xml_ptr);
    if !already_written {
        fs::write(&path, xml).ok()?;
        WRITTEN_SCHEMES.lock().insert(path, xml_ptr);
        needs_rescan = true;
    }

    if needs_rescan {
        manager.force_rescan();
    }
    manager.scheme(id)
}

fn get_schemes_dir() -> Option<PathBuf> {
    let dir = boxxy_sys_utils::get_config_dir().join("styles");
    fs::create_dir_all(&dir).ok()?;
    Some(dir)
}
