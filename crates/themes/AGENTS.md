# boxxy-themes Agents & Architecture

## Responsibilities
Palette-driven terminal theme engine and multi-layer Custom Boxxy App Theme engine (`app_theme.rs`).
> **Full Specification:** See [`BOXXY_THEME_API.md`](./BOXXY_THEME_API.md) for the complete 4-layer CSS cascade, `Boxxy Dark` / `Boxxy Light` color tables, `@define-color` variable-to-component mapping, Shadow vs. Non-Shadow matrix, unified button geometry rules, and Community `.toml` Theme API.
- **Terminal Palettes:** The terminal palette is the single source of truth for terminal foreground/background, ANSI 16 colors, `.terminal-header` / `.claw-drawer` CSS overrides, and GtkSourceView XML schemes generated at compile-time and runtime.
- **Custom Boxxy App Themes (`app_theme.rs`):** Styles the application chrome (Sidebars, Preferences, HeaderBars, Popovers, Dialogs, Buttons, Navigation pills) independently of the terminal canvas using a 4-layer GTK `CssProvider` cascade:
  1. `resources/style.css` (`PRIORITY_APPLICATION` = 600) — Always-active geometry and Default GTK fallback.
  2. `resources/themes/boxxy_skeleton.css` (`STYLE_PROVIDER_PRIORITY_USER` = 800) — Playful neo-brutalist Boxxy skeleton (zero-lag `transition: none` interactive states, crisp `@boxxy_ink` outlines, offset `@boxxy_shadow` drop-shadows, capsule sidebar rows, `button.pill` advertising CTA buttons, `.boxed-list` preference cards with dark ink row grooves, `GtkDropDown` listview rows, `24×20px` scale sliders, and squared overlay scrollbars) loaded only when `AppThemeMode::Boxxy` is active.
  3. Theme Color Tokens (`STYLE_PROVIDER_PRIORITY_USER + 10` = 810) — Generated `@define-color` CSS block (including Libadwaita semantic destructive/warning/success tokens whose foreground contrast follows `is_hex_dark(card_bg)` and publishes `"boxxy-card-is-dark"` on `gdk::Display::default()` so `boxxy-viewer` syntax highlighting adapts even on hybrid themes where `card_bg` has the opposite luminance of `window_bg`) from the two official built-in presets (`boxxy-dark`, `boxxy-light`) or community `.toml` files discovered in `boxxy_sys_utils::get_config_dir().join("themes")` (`~/.config/boxxy-terminal/themes/`, seeded with `cotton-candy.toml`, `bauhaus.toml`, and `README.md` on first creation and cached via an mtime/size stamp). Community themes are strictly validated hex color definitions (`sanitize_hex_color`) so they can never inject arbitrary CSS or break widget geometry.
  4. Custom Accent Color Override (`STYLE_PROVIDER_PRIORITY_USER + 20` = 820) — Optional user accent color override (`@accent_color`, `@accent_bg_color`, and auto-contrasted `@accent_fg_color`).
- **Texture Cache:** Provides centralized texture caching for efficient background image rendering.

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

