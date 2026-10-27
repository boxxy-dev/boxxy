# boxxy-themes Agents & Architecture

## Responsibilities
Palette-driven theme engine. The terminal palette is the single source of truth — GTK CSS
overrides and GtkSourceView XML schemes are generated at compile-time and runtime.
It also provides centralized texture caching for efficient background image rendering.

## Data Structures

### `PaletteVariantStatic`
A statically allocated light-or-dark variant of a palette:
```rust
pub struct PaletteVariantStatic {
    pub background: &'static str,  // hex
    pub foreground: &'static str,
    pub cursor:     &'static str,
    pub colors:     [&'static str; 16],  // ANSI Color0–Color15
    pub gtk_css:    &'static str,
    pub sourceview_xml: &'static str,
}
```

### `ParsedPaletteStatic`
A complete palette generated at compile time:
```rust
pub struct ParsedPaletteStatic {
    pub name: &'static str,   // display name, e.g. "Dracula"
    pub id:   &'static str,   // slug, e.g. "dracula"
    pub light: PaletteVariantStatic,
    pub dark:  PaletteVariantStatic,
}
```

### `TextureCache`
A global thread-safe cache (`Arc<parking_lot::Mutex<HashMap<String, gdk::Texture>>>`) for background images. Ensures that even if multiple terminal tabs use the same high-resolution background image, the texture is only decoded and uploaded to the GPU (VRAM) once.

## Public API

### `list_palettes() -> Vec<ParsedPaletteStatic>`
Enumerates all palettes compiled into the binary via `build.rs`.

### `load_palette(name: &str) -> Option<ParsedPaletteStatic>`
Loads a single palette by exact or normalized display name stored in settings (e.g. "Dracula",
"Adventure Time"). Returns `None` for "System" / "" (pure Adwaita, no custom palette).

### `apply_palette(palette: Option<&ParsedPaletteStatic>, dark_mode: bool)`
Applies GTK/Libadwaita theming derived from the palette by loading the pre-generated CSS string.
- Manages a `thread_local!` `CssProvider` so old providers are removed before the new one is added.
- Passing `None` clears the custom CSS provider.

### `apply_sourceview_palette(buffer: &sourceview5::Buffer, palette: Option<&ParsedPaletteStatic>, dark_mode: bool)`
Writes the pre-generated GtkSourceView XML style scheme to `~/.config/boxxy-terminal/styles/{id}.xml`, and activates it on the buffer.

### Background Image Helpers
- `get_texture_from_path(path: &str) -> Option<gdk::Texture>`: Retrieves a texture from the global cache, loading it from disk if necessary.
- `copy_background_image(src_path: &Path) -> Option<String>`: Copies a user-selected image into `~/.config/boxxy-terminal/backgrounds/` to bypass Flatpak sandbox portal limitations upon application restart.

## Palette File Format (TOML)
Stored in `resources/palettes/*.toml` and parsed at compile-time by `build.rs`:
```toml
[colors.primary]
background = '#282a36'
foreground = '#f8f8f2'

[colors.normal]
black = '#21222c'
# ...
```

## Build Process (`build.rs`)
To avoid expensive string parsing and CSS/XML generation at runtime, `build.rs` reads all TOML files in `resources/palettes/` during compilation. It generates `OUT_DIR/generated_themes.rs`, which contains a static `THEMES` array holding all `ParsedPaletteStatic` instances, complete with pre-calculated HSL-adjusted CSS and SourceView XML strings.

## Design Notes
- No per-theme CSS/XML files shipped with the binary — everything is generated from TOML and baked directly into the static executable data.
- HSL math helpers (`darken`, `lighten`, `transform_lightness`) are executed entirely within `build.rs`.
- `thread_local! { CURRENT_PROVIDER }` tracks the active `gtk::CssProvider` for removal.

