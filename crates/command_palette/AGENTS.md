# Command Palette Crate (`boxxy-command-palette`)

## Role
Provides `CommandPaletteComponent`, a searchable `Ctrl+Shift+P` (and right-click context menu) command launcher over application actions, the 12 top-level Preferences categories, and dynamic user script bookmarks.

## Architecture
- **`src/lib.rs`**: Public API surface (`CommandPaletteComponent`, `CommandItem`, `CommandKind`, `PREFERENCES_CATEGORIES`, `PreferencesCategorySpec`, `OPEN_PREFERENCES_PAGE_ACTION`).
- **`src/catalog.rs`**: Static command definitions sourced directly from `boxxy-keybindings` triggers (preventing shortcut drift) plus `PREFERENCES_CATEGORIES` covering all 12 main Preferences sections (`Terminal`, `Themes`, `APIs & Subscriptions`, `Characters`, `Desktop Integrations`, `MCP`, `Skills`, `Claw UI & Sounds`, `Toolbox & Permissions`, `Shortcuts`, `Advanced`, `About` via `win.open-preferences-page`), using unified monospace namespace subtitles (`win.*`, `app.*`, `pref.*`, `bookmark.*`).
- **`src/search.rs`**: Pre-indexed search normalization (`IndexedEntry`) and single-pass tiered relevance scoring (namespace ID prefix/exact -> exact title -> title prefix -> title word-boundary prefixes -> title substring -> multi-token title/subtitle/keyword/action match -> `frizbee` SIMD Smith-Waterman fuzzy fallback for acronyms and typos).
- **`src/component.rs`** & **`resources/ui/command_palette.ui`**: Declarative `GtkPopover` UI with weak-reference signal handlers (zero `Popover`/`Rc` reference cycles), incremental `set_dynamic_commands` updates that preserve static rows, $O(1)$ pointer-keyed row-to-entry lookup across sorted visual order, focus-preserving static document bounds scroll math (`Up`/`Down`/`Page_Up`/`Page_Down`), and synchronous `GAction` activation (preserving GDK Wayland input serials for clipboard commands).
