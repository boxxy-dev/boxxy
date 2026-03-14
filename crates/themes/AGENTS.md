# boxxy-themes Agents & Architecture

## Responsibilities
Palette-driven theme engine. The terminal palette is the single source of truth — GTK CSS
overrides and GtkSourceView XML schemes are generated at runtime from the palette data.
Palette files are compiled into the binary as GResources (`resources/palettes/*.palette`).

## Data Structures

### `PaletteVariant`
One light-or-dark variant of a palette:
```rust
pub struct PaletteVariant {
    pub background: String,  // hex
    pub foreground: String,
    pub cursor:     String,
    pub colors:     [String; 16],  // ANSI Color0–Color15
}
```
- `to_vte_colors()` — converts to `Vec<gdk::RGBA>` for `vte4`.

### `ParsedPalette`
A complete palette loaded from a `.palette` GResource:
```rust
pub struct ParsedPalette {
    pub name: String,   // display name, e.g. "Dracula"
    pub id:   String,   // slug, e.g. "dracula"
    pub light: PaletteVariant,
    pub dark:  PaletteVariant,
}
```

### `Palette` (type alias)
`pub type Palette = PaletteVariant;` — kept for backward compatibility with `boxxy-terminal`.

## Public API

### `list_palettes() -> Vec<ParsedPalette>`
Enumerates all palettes compiled into the GResource bundle
(`/play/mii/Boxxy/palettes/`). Returns them sorted by name. Includes a synthetic
"System" entry (no overrides, pure Adwaita).

### `load_palette(id: &str) -> Option<ParsedPalette>`
Loads a single palette by id slug (e.g. `"dracula"`). Handles legacy display-name
lookups (e.g. `"Dracula"` → `"dracula"`). Returns `None` for `"System"` / unknown ids.

### `apply_palette(palette: Option<&ParsedPalette>, dark_mode: bool)`
Applies GTK/Libadwaita theming derived from the palette:
- Generates `@define-color` CSS overrides (accent, headerbar, card, sidebar, etc.).
- Directly embeds palette-derived color rules for widgets that need explicit hex colors (bypassing `@define-color` cross-provider scoping limitations):
  - `switch:checked { background-color: {accent}; }` — switch toggle uses palette accent
  - `checkbutton:checked > check { background-color: {accent}; border-color: {accent}; }` — checkbox uses palette accent
  - `.user-message { background-color: {accent}; color: {bg}; }` — AI chat user bubbles use palette accent/background
- Manages a `thread_local!` `CssProvider` so old providers are removed before the
  new one is added.
- Sets `StyleManager::color_scheme` to `ForceDark` / `ForceLight` based on palette variant.
- Passing `None` resets to pure Adwaita (no overrides, `Default` color scheme).

**GTK4 CSS Provider Scoping:** `@define-color` variables are provider-scoped — a variable defined in one CSS provider cannot be referenced by rules in a different provider. Therefore, any rule that needs the palette's exact hex color (e.g. accent) **must** be placed inside `generate_gtk_css` where Rust string formatting interpolates the hex value directly. Placing such rules in `resources/style.css` (a separate provider) will see the system's `@accent_bg_color`, not the palette color.

### `apply_sourceview_palette(buffer: &sourceview5::Buffer, palette: Option<&ParsedPalette>, dark_mode: bool)`
Generates a GtkSourceView XML style scheme from the palette's ANSI colors, writes it
to `~/.local/share/boxxy-terminal/styles/{id}.xml`, and activates it on the buffer.
Color roles: Color8=comments, Color2=strings, Color3=numbers, Color5=keywords,
Color6=types, Color1=errors.

## Palette File Format (Ptyxis INI)
Stored in `resources/palettes/*.palette`, compiled as GResources:
```ini
[Palette]
Name=Dracula

[Dark]
Background=#282a36
Foreground=#f8f8f2
Cursor=#f8f8f2
Color0=#21222c
...
Color15=#ffffff

[Light]
Background=#f8f8f2
...
```

## Available Palettes
Dracula, Nord, Solarized, Gruvbox, One Dark, Tokyo Night, Catppuccin, Monokai,
Synthwave, Material Ocean, System (Adwaita).

## Design Notes
- No per-theme hardcoded CSS/XML files — everything is generated from `PaletteVariant` fields.
- HSL math helpers (`darken`, `lighten`, `transform_lightness`) used for derived colors
  (e.g. headerbar = background darkened 8%).
- `thread_local! { CURRENT_PROVIDER }` tracks the active `gtk::CssProvider` for removal.
- `use gtk4 as gtk;` at the top resolves the `gtk::` namespace.
