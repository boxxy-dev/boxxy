use gtk::gdk;
use gtk4 as gtk;
use sourceview5::prelude::*;
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;

pub mod preview;
pub mod selector;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

lazy_static::lazy_static! {
    static ref TEXTURE_CACHE: Arc<Mutex<HashMap<String, gdk::Texture>>> = Arc::new(Mutex::new(HashMap::new()));
}

pub fn get_texture_from_path(path: &str) -> Option<gdk::Texture> {
    let mut cache = TEXTURE_CACHE.lock().unwrap();
    if let Some(texture) = cache.get(path) {
        return Some(texture.clone());
    }

    let file = gtk::gio::File::for_path(path);
    if let Ok(texture) = gdk::Texture::from_file(&file) {
        cache.insert(path.to_string(), texture.clone());
        return Some(texture);
    }
    None
}

pub fn clear_texture_cache() {
    TEXTURE_CACHE.lock().unwrap().clear();
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
    if let Ok(path) = Path::new(path).canonicalize() {
        if let Some(config_dir) =
            get_schemes_dir().and_then(|p| p.parent().map(|p| p.join("backgrounds")))
        {
            if let Ok(config_dir) = config_dir.canonicalize() {
                // Ensure we only delete files within our backgrounds directory
                if path.starts_with(config_dir) {
                    let _ = fs::remove_file(path);
                }
            }
        }
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
}

/// Apply a palette to GTK/Adwaita. Pass `None` for pure Adwaita (system theme).
/// `dark_mode` selects the [Dark] or [Light] variant and forces the Adwaita
/// colour scheme accordingly.
pub fn apply_palette(palette: Option<&ParsedPaletteStatic>, dark_mode: bool) {
    clear_css_provider();

    if let Some(p) = palette {
        let variant = if dark_mode { &p.dark } else { &p.light };
        load_css(variant.gtk_css);
    }
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
            gtk::STYLE_PROVIDER_PRIORITY_USER,
        );
    }
}

// ---------------------------------------------------------------------------
// GtkSourceView theming
// ---------------------------------------------------------------------------

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

fn write_and_load_scheme(id: &str, xml: &str) -> Option<sourceview5::StyleScheme> {
    let schemes_dir = get_schemes_dir()?;
    let path = schemes_dir.join(format!("{}.xml", id));
    fs::write(&path, xml).ok()?;

    let manager = sourceview5::StyleSchemeManager::default();
    let dir_str = schemes_dir.to_str()?;
    let mut paths = manager.search_path();
    if !paths.iter().any(|p| p == dir_str) {
        paths.push(dir_str.into());
        manager.set_search_path(&paths.iter().map(|s| s.as_str()).collect::<Vec<_>>());
    }
    manager.force_rescan();
    manager.scheme(id)
}

fn get_schemes_dir() -> Option<PathBuf> {
    let dir = boxxy_sys_utils::get_config_dir().join("styles");
    fs::create_dir_all(&dir).ok()?;
    Some(dir)
}
