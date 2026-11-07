# Boxxy Theme Engine & Community Theme API (`BOXXY_THEME_API.md`)

This document is the authoritative specification for the **Boxxy App Theme Engine** (`crates/themes/src/app_theme.rs` and `resources/themes/boxxy_skeleton.css`). It documents the 4-layer GTK CSS cascade, the official Dark and Light color palettes, the complete `@define-color` variable catalog and every UI component that consumes it, the Shadow vs. Non-Shadow visual hierarchy, the unified HeaderBar button geometry rules, and the `.toml` Community Theme API.

---

## 1. Architecture: Separation of Terminal Palettes & App Chrome

Boxxy decouples **Terminal Palettes** from **Application Chrome Themes**:

- **Terminal Palettes (`crates/themes/src/lib.rs`, `resources/palettes/*.toml`):** Control the terminal canvas foreground/background, cursor, 16 ANSI colors, and `GtkSourceView` XML schemes.
- **Boxxy App Themes (`crates/themes/src/app_theme.rs`, `resources/themes/boxxy_skeleton.css`):** Style the surrounding application chrome (HeaderBars, Sidebars, Tabs, Popovers, Menus, Preferences, Dialogs, Claw Overlay Drawer, and Buttons) using a 4-layer GTK 4 `CssProvider` cascade.

### The 4-Layer GTK `CssProvider` Cascade

| Layer | GTK Priority | Source | Lifecycle | Responsibility |
| :--- | :--- | :--- | :--- | :--- |
| **Layer 1 (Base)** | `600` (`PRIORITY_APPLICATION`) | `resources/style.css` | Always loaded | Base widget geometry, spacing, layout rules, and fallback `@define-color` tokens for Legacy Adwaita (`AppThemeMode::DefaultGtk`). |
| **Layer 1b (Palette)** | `610` (`PRIORITY_APPLICATION + 10`) | `build.rs` (`generate_gtk_css`) | Loaded when a terminal palette is selected | Terminal-coupled `.terminal-header` and `.claw-drawer` colors when running in Legacy Adwaita mode. |
| **Layer 2 (Skeleton)** | `800` (`PRIORITY_USER`) | `resources/themes/boxxy_skeleton.css` | Loaded **only** when `AppThemeMode::Boxxy` is active | Playful neo-brutalist outlines (`@boxxy_ink`), tactile offset drop-shadows (`@boxxy_shadow`), capsule navigation pills, and active button press states. Completely unloaded in Legacy Adwaita mode. |
| **Layer 3 (Theme Tokens)** | `810` (`PRIORITY_USER + 10`) | `AppThemeColors::to_gtk_css()` | Loaded **only** when `AppThemeMode::Boxxy` is active | Defines all semantic `@define-color` variables from the active built-in or community `.toml` theme. |
| **Layer 4 (Accent Override)** | `820` (`PRIORITY_USER + 20`) | `generate_accent_override_css()` | Loaded when `custom_accent_color` is set | Overrides `@accent_color`, `@accent_bg_color`, and auto-contrasted `@accent_fg_color` on top of any active Boxxy or Community theme. |

> **Why `PRIORITY_USER` (`800+`)?**
> Libadwaita registers its internal stylesheet and `AdwStyleManager` color definitions above `PRIORITY_APPLICATION` (`600`). Placing Layers 2–4 at `800 / 810 / 820` ensures Boxxy's `@define-color` tokens and neo-brutalist rules cleanly override Libadwaita without ever needing `!important` (which GTK 4's CSS parser does not support).

---

## 2. Official Built-in Palettes (`Boxxy Dark` & `Boxxy Light`)

Boxxy ships with two official built-in themes in `builtin_app_themes()` (`crates/themes/src/app_theme.rs`):

| Token (`AppThemeColors`) | `Boxxy Dark` (`id = "boxxy-dark"`) | `Boxxy Light` (`id = "boxxy-light"`) | Surface Role |
| :--- | :--- | :--- | :--- |
| `window_bg` | `#1d1d20` (Carbon Dark) | `#ecfafb` (Warm Mint Paper) | Primary window, dialog, and Claw overlay drawer canvas background. |
| `window_fg` | `#f2f4f2` | `#111418` | Primary text and icon foreground on `window_bg`. |
| `sidebar_bg` | `#2e2e32` (Elevated Slate) | `#ffffff` (Crisp White) | Left sidebar (`AI Chat`, `Claw`, `Bookmarks`, `Colors`) and Preferences navigation sidebar background. |
| `sidebar_fg` | `#f2f4f2` | `#111418` | Text and icon foreground on `sidebar_bg`. |
| `card_bg` | `#26262a` (Card Surface) | `#ffffff` (Crisp White) | Tactile buttons, unselected tabs, preference `boxed-list` cards, search/text entries, and linked button strips inside menus. |
| `card_fg` | `#f2f4f2` | `#111418` | Text and icon foreground on `card_bg`. |
| `headerbar_bg` | `#1d1d20` | `#ffffff` | Main `.content-header`, dialog headerbars, and non-frosted `.terminal-header`. |
| `headerbar_fg` | `#f2f4f2` | `#111418` | Text and icon foreground on `headerbar_bg`. |
| `popover_bg` | `#2e2e32` *(matches `sidebar_bg`)* | `#ffffff` *(matches `sidebar_bg`)* | Right-click terminal menu (`popover.terminal-menu`), hamburger menu (`popover.menu`), command palette, search popover, and autocomplete popover. |
| `popover_fg` | `#f2f4f2` | `#111418` | Text and icon foreground on `popover_bg`. |
| `accent` | `#7ec868` (Peanut Green) | `#00acc1` (Boxxy Cyan) | Selected tabs, active sidebar rows, active `AdwViewSwitcher` buttons, `.suggested-action` buttons, checked switches/checkboxes, and focus rings. |
| `accent_fg` | `#0d140a` | `#0d1117` | High-contrast ink text/icons rendered on top of `accent` surfaces. |
| `ink_border` | `#050505` | `#111418` | Crisp `2px` neo-brutalist outlines and structural pane dividers (`@boxxy_ink`). |
| `shadow` | `#050505` | `#111418` | Solid offset drop-shadows (`@boxxy_shadow`). |

### Surface Contrast Rule: Why `popover_bg` Matches `sidebar_bg`
In `Boxxy Dark` (and by default in community themes), `popover_bg` (`#2e2e32`) matches `sidebar_bg` (`#2e2e32`) rather than `card_bg` (`#26262a`). This serves two purposes:
1. **Visual Continuity:** Context menus (`popover.terminal-menu`, `popover.menu`) visually match the elevated sidebar surface next to the terminal.
2. **Inner Control Separation:** Grouped horizontal button strips inside menus (`Copy | Copy Link` and the 4 pane split/close buttons in `popover.menu box.linked`) use `@card_bg_color` (`#26262a`), giving them crisp contrast against the `#2e2e32` menu background.

---

## 3. Complete `@define-color` Variable Catalog & Component Mapping

**Rule:** Components must **never** hardcode hex or RGB colors for surfaces, borders, or semantic states. All component stylesheets (`resources/style.css`, `resources/themes/boxxy_skeleton.css`, and inline Rust `CssProvider` rules) must reference the semantic `@define-color` variables below.

### 3.1 Core Surface & Chrome Variables

| CSS `@define-color` Variable | Source Field / Formula | Components Using This Variable |
| :--- | :--- | :--- |
| `@window_bg_color` | `colors.window_bg` | Main window background, `adw::Dialog` / `adw::AlertDialog` sheets (`@dialog_bg_color`), `.claw-drawer` overlay background, `.osd` / `.terminal-size-osd`, `.bookmark-preview-frame`, `.bookmarks-ai-input`, `.editable-avatar-badge`, `.avatar-sleep-badge`, `.avatar-vibe-badge`, `.terminal-split-container paned > separator`. |
| `@window_fg_color` | `colors.window_fg` | Primary text inside windows, dialogs (`@dialog_fg_color`), `.claw-drawer`, `.claw-drawer .claw-row`, `.osd`, `.editable-avatar-badge`. |
| `@sidebar_bg_color` | `colors.sidebar_bg` | `.sidebar-toolbar`, `navigation-split-view > .sidebar-pane`, `overlay-split-view > .sidebar-pane`, `.sidebar-pane` (Main sidebar & Preferences category sidebar). |
| `@sidebar_fg_color` | `colors.sidebar_fg` | All text, icons, and `.sidebar-header` inside `.sidebar-pane` and `.sidebar-toolbar`. |
| `@headerbar_bg_color` | `colors.headerbar_bg` | `.terminal-header`, `.content-header`, `adw-dialog headerbar`, `dialog headerbar`, `navigation-split-view > .content-pane headerbar`. |
| `@headerbar_fg_color` | `colors.headerbar_fg` | Titles, buttons, and icons inside `.terminal-header`, `.content-header`, and dialog headerbars. |
| `@card_bg_color` | `colors.card_bg` | Unselected `tabbar tab`, all standard tactile buttons (`headerbar button`, `windowcontrols > button`, `preferencespage button`, `dialog button`, `.claw-drawer button`), `preferencesgroup listbox.boxed-list`, `.card`, `.bookmark-card`, `searchentry`, `entry:not(.flat)`, `.claw-drawer .boxxy-msgbar`, `.claw-drawer frame.view`, `.claw-drawer .viewer-block`, `.claw-drawer .character-btn`, `.claw-status-pill`, `popover.menu box.linked` segmented button strips, `.whats-new-commit-pill`, `.whats-new-stat-tile`, `.diff-slide-drawer`. |
| `@card_fg_color` | `colors.card_fg` | Text, labels, and icons inside all `@card_bg_color` widgets above. |
| `@view_bg_color` / `@view_fg_color` | Aliases to `card_bg` / `card_fg` | Libadwaita scrolled views, text views, and `.claw-drawer .boxxy-msgbar` fallback hover/fill in Layer 1. |
| `@popover_bg_color` | `colors.popover_bg` | `popover > contents`, `popover.menu > contents`, `popover.terminal-menu > contents`, `popover.command-palette > contents`, `popover.search-popover > contents`, `.autocomplete-popover contents`. |
| `@popover_fg_color` | `colors.popover_fg` | Menu items (`popover.menu modelbutton`, `popover.terminal-menu modelbutton`) and labels inside all popovers. |

### 3.2 Accent & Interactive Highlight Variables

| CSS `@define-color` Variable | Source Field / Formula | Components Using This Variable |
| :--- | :--- | :--- |
| `@accent_bg_color` | `colors.accent` (or Layer 4 override) | Selected terminal tab (`tabbar tab:selected`), selected sidebar row (`.navigation-sidebar > row:selected`), active sidebar switcher button (`viewswitcher > button:checked`), active/pressed tactile buttons (`:active`, `:checked`), `button.suggested-action`, hovered popover menu items (`modelbutton:hover`, `box.linked > button.model:hover`), `switch:checked`, `checkbutton check:checked`, `.character-btn.selected-character`, `.boxxy-theme-tag-pill`, `.editable-avatar-badge:hover`, `.terminal-pane .split-drop-overlay`, `.whats-new-hero` gradient, `.diff-badge-modified`, `.diff-slide-drawer` border. |
| `@accent_fg_color` | `colors.accent_fg` (or auto-contrasted Layer 4) | High-contrast text and icons inside all `@accent_bg_color` surfaces (`tabbar tab:selected`, `.navigation-sidebar > row:selected`, `viewswitcher > button:checked`, `button.suggested-action`, `modelbutton:hover`, `.boxxy-theme-tag-pill`). |
| `@accent_color` | `colors.accent` (or Layer 4 override) | Focus outlines on `searchentry:focus-within`, `entry:focus-within`, and `.claw-drawer .boxxy-msgbar:focus-within`; `.bookmark-hero-icon`, `.bookmark-var-chip`, `.avatar-vibe-badge`, `.claw-spinner` fallback, `.diff-badge-modified` text. |

### 3.3 Neo-Brutalist Structural & Semantic Status Variables

| CSS `@define-color` Variable | Source Field / Formula | Components Using This Variable |
| :--- | :--- | :--- |
| `@boxxy_ink` | `colors.ink_border` (Layer 1 fallback: `alpha(currentColor, 0.28)`) | `2px solid` outlines on tabs, sidebar rows, tactile buttons, `windowcontrols > button`, `searchentry`, `entry`, `listbox.boxed-list`, `.card`, `popover > contents`, `dialog sheet`, `switch`, `checkbutton check`, `.claw-drawer`, `.claw-status-pill`, `.boxxy-theme-tag-pill`, `.theme-preview-dot`, and `border-right` / `border-bottom` dividers on `.sidebar-pane` and `headerbar`. |
| `@boxxy_shadow` | `colors.shadow` (Layer 1 fallback: `rgba(0, 0, 0, 0.35)`) | Hard-edge offset drop-shadows (`2px 3px 0px`, `3px 4px 0px`, `4px 5px 0px`) on tactile buttons, selected sidebar pills, boxed lists, cards, popovers, and dialogs. |
| `@boxxy_surface_hover` | `mix(@card_bg_color, @accent_bg_color, 0.82)` | Vibrant hover background (82% accent + 18% card, paired with `@accent_fg_color` ink text) for unselected tabs (`tabbar tab:hover:not(:selected)`), unselected sidebar rows (`.navigation-sidebar > row:hover:not(:selected)`), tactile buttons (`:hover`), `.character-btn:hover`, and `.whats-new-stat-tile-btn:hover`. |
| `@destructive_bg_color` / `@error_bg_color` | Dark: `#c01c28` · Light: `#e01b24` | `button.destructive-action`, `.hold-to-confirm-progress`, `progressbar.terminal-progress-bar.error`, `.editable-avatar-remove-badge:hover`, `.diff-ratio-del`. |
| `@destructive_fg_color` / `@error_fg_color` | `#ffffff` | Foreground text/icons inside `button.destructive-action` and `.editable-avatar-remove-badge:hover`. |
| `@destructive_color` / `@error_color` | Dark: `#ff7b63` · Light: `#c01c28` | Inline error text and `.diff-stat-del` (`-R` line deletion counter). |
| `@warning_bg_color` / `@warning_color` | Bg: `#e5a50a` · Text: `#f8e45c` (Dark) / `#ae7b03` (Light) | `progressbar.terminal-progress-bar.warning`, `.model-status-warning`, `.boxxy-msgbar .msgbar-status-icon`, `.avatar-sleep-badge`. |
| `@success_bg_color` / `@success_color` | Bg: `#26a269` (Dark) / `#2ec27e` (Light) · Text: `#8ff0a4` (Dark) / `#1b8553` (Light) | `progressbar.terminal-progress-bar`, `.model-status-success`, `.whats-new-badge`, `.whats-new-hero`, `.diff-badge-created`, `.diff-stat-add` (`+A` line addition counter), `.diff-ratio-add`. |

---

## 4. Visual Hierarchy: Shadow vs. Non-Shadow Decision Matrix

Boxxy intentionally distinguishes between **floating tactile controls** (which cast an offset `@boxxy_shadow` and physically depress by `translate(1px, 1px)` when clicked) and **flush strip/inline controls** (which use crisp `@boxxy_ink` outlines with `box-shadow: none`).

| Component / Selector | Has `@boxxy_shadow`? | Shadow Offset | Rationale |
| :--- | :--- | :--- | :--- |
| **Sidebar Switcher Buttons** (`viewswitcher > button`, `:checked`) | **Yes** | `2px 3px 0px @boxxy_shadow` | Top-level mode switchers (`AI Chat`, `Claw`, `Bookmarks`, `Colors`) sit in the spacious sidebar header with a `2px` inter-button gap and act as tactile physical toggles. |
| **Sidebar & Command Palette Rows** (`.navigation-sidebar > row:selected`, `:hover`, `.menu-open`) | **Yes** | `2px 3px 0px @boxxy_shadow` | Lifts the active or hovered conversation, command palette item, bookmark, or preferences category above the flat list. Depresses to `1px 1px 0px` on `:active`. |
| **Terminal Tabs** (`tabbar tab`, `tabbar tab:selected`) | **No** (`box-shadow: none`) | `none` | Tabs live inside `AdwTabBar`'s compact `28px` strip. Keeping `box-shadow: none` (while retaining the `2px solid @boxxy_ink` outline and `@accent_bg_color` active fill) prevents bottom shadow clipping inside `AdwTabBox` and keeps the tab bar flush. |
| **Standalone Tactile Buttons** (`headerbar button:not(.flat)`, `windowcontrols > button`, `dialog button`, `preferencespage button`, `.claw-drawer button`) | **Yes** | `2px 3px 0px @boxxy_shadow` | Primary interactive buttons. Depress by `translate(1px, 1px)` with `1px 1px 0px @boxxy_shadow` when clicked. |
| **Popover Linked Button Container** (`popover.menu box.linked`) | **Yes (Container only)** | `2px 3px 0px @boxxy_shadow` | The outer segmented bar (`Copy \| Copy Link`, split/close bar) casts a single unified shadow. |
| **Individual Linked Buttons** (`popover.menu box.linked > button.model`) | **No** (`box-shadow: none`) | `none` | Inner segments are separated by `border-right: 2px solid @boxxy_ink` inside the shared container; per-segment shadows would overlap adjacent buttons. |
| **Flat / Embedded Sub-Buttons** (`button.flat`, `tab button.image-button`, `.claw-status-pill button.circular`) | **No** (`box-shadow: none`) | `none` | Embedded close/action icons inside a parent pill or tab that already has its own border. |
| **Search & Text Entries** (`searchentry`, `entry:not(.flat)`) | **Yes** | `2px 2px 0px @boxxy_shadow` (`2px 3px 0px` on focus) | Gives input fields a recessed/tactile card feel. |
| **Preference Groups & Cards** (`listbox.boxed-list`, `.card`, `.bookmark-card`) | **Yes** | `3px 4px 0px @boxxy_shadow` | Large elevated content containers. |
| **Popovers & Dialog Sheets** (`popover > contents`, `dialog sheet`) | **Yes** | `4px 5px 0px @boxxy_shadow` | Highest elevation layer floating above the window canvas. |

---

## 5. Unified Button & HeaderBar Geometry Rules

To maintain full compatibility with both **Boxxy Custom Themes** (`AppThemeMode::Boxxy`) and **Legacy Adwaita Themes** (`AppThemeMode::DefaultGtk`) without scattering per-widget stylesheet hacks:

1. **Keep `.ui` XML Files Purely Structural:**
   - `.ui` files in `resources/ui/` declare widget hierarchy, container `spacing`, and semantic classes (`suggested-action`, `destructive-action`, `flat`, `circular`, `boxed-list`).
   - Visual styling, padding overrides, and colors belong exclusively in CSS (`resources/style.css` for shared geometry; `resources/themes/boxxy_skeleton.css` for Boxxy tactile chrome).
2. **Single Source of Truth for Standard Button & Menu Item Padding (`min-height: 24px; padding: 5px 10px;`):**
   - Standard tactile buttons (`headerbar button`, `dialog button`, `preferencespage button`, `.claw-drawer button`), action buttons (`.suggested-action`, `.destructive-action`), popover menu text rows (`popover.menu modelbutton`, `popover.terminal-menu modelbutton`), and popover segmented buttons (`popover.menu box.linked > button.model`, `button.image-button.model`) all share **one single CSS rule** in `resources/themes/boxxy_skeleton.css`:
     ```css
     min-height: 24px;
     padding: 5px 10px;
     ```
   - `modelbutton` uses `border: 2px solid transparent` at rest and `border: 2px solid @boxxy_ink` on `:hover` (instead of an inward `outline-offset: -2px`) so its `34px` inner fill height matches `box.linked > button.model` and standard buttons pixel-for-pixel.
   - To make a button explicitly smaller/narrower, apply the `.compact-btn` modifier class (`min-height: 20px; padding: 2px 8px;`) rather than writing one-off per-widget padding rules.
3. **Single Unified Rule for Main Window `.content-header` Icon Buttons:**
   - The main window headerbar (`.content-header`) hosts two types of square icon buttons: standard `GtkButton.image-button` controls (Sidebar Toggle on the left, Hamburger Menu on the right) and `GtkWindowControls > button` (Minimize, Maximize, Close on the right).
   - By default, Libadwaita adds extra internal padding and a circular badge to `windowcontrols > button > image`.
   - In `resources/themes/boxxy_skeleton.css`, we first strip that inner image padding (`padding: 0; margin: 0; background: transparent;` on `windowcontrols > button > image`), which normalizes both `GtkButton.image-button` and `windowcontrols > button` to the exact same `16×16` inner icon box.
   - Both button types then share **one single CSS sizing rule** scoped strictly to `.content-header` (so dialog headerbars keep their native close button sizing):
     ```css
     .content-header button.image-button:not(.flat),
     .content-header windowcontrols > button {
         min-width: 24px;
         min-height: 24px;
         padding: 0px 6px;
         margin-top: 6px;
         margin-bottom: 6px;
     }
     ```

---

## 6. Community Theme `.toml` API Specification

Users and community authors can drop `.toml` files into `~/.config/boxxy-terminal/themes/` (or `$BOXXY_CONFIG_DIR/themes/`). On first launch, Boxxy seeds `README.md`, `cotton-candy.toml`, and `bauhaus.toml` into this folder.

### 6.1 File Naming & Discovery
- **Location:** `boxxy_sys_utils::get_config_dir().join("themes")`
- **Theme ID (`slug`):** Derived from the filename stem (lowercased, spaces replaced with `-`, e.g., `cotton-candy.toml` → `cotton-candy`).
- **Reserved IDs:** Files whose slug collides with a built-in ID (`boxxy-dark`, `boxxy-light`) are ignored so built-in presets can never be shadowed.
- **Zero-I/O Caching & Live Hot-Reload:** `discover_community_themes()` caches parsed themes keyed by `(filename, mtime, size)`. While Preferences → Themes is open, a 500 ms timer polls the directory and immediately hot-reloads any `.toml` edits into the live application without restarting.

### 6.2 Full `.toml` Schema

```toml
[meta]
name = "Cotton Candy"                                  # Optional (defaults to filename stem with spaces)
author = "nukeop"                                      # Optional (defaults to "community")
description = "Soft pastels in baby pink and blue"     # Optional (defaults to "")
mode = "light"                                         # Optional: "light" or "dark" (auto-detected from window_bg luminance if omitted)
tags = ["pastel", "pink", "cute"]                      # Optional: up to 6 tags (first 3 shown as pills in Preferences)

[colors]
# --- REQUIRED (3 fields) ---
window_bg = "#fff5f8"
window_fg = "#1e1b24"
accent    = "#ff7eb6"

# --- OPTIONAL (11 fields — automatically derived if omitted) ---
sidebar_bg   = "#fce8ef"   # Defaults to `window_bg`
sidebar_fg   = "#1e1b24"   # Defaults to `window_fg`
card_bg      = "#ffffff"   # Defaults to `window_bg`
card_fg      = "#1e1b24"   # Defaults to `window_fg`
headerbar_bg = "#fce8ef"   # Defaults to `sidebar_bg`
headerbar_fg = "#1e1b24"   # Defaults to `sidebar_fg`
popover_bg   = "#fce8ef"   # Defaults to `sidebar_bg` (matches sidebar surface!)
popover_fg   = "#1e1b24"   # Defaults to `sidebar_fg`
accent_fg    = "#1e1b24"   # Defaults to `contrast_fg_for_hex(accent)` (#ffffff or #111418)
ink_border   = "#1e1b24"   # Defaults to "#07080c" (dark mode) or "#111418" (light mode)
shadow       = "#1e1b24"   # Defaults to `ink_border`
```

### 6.3 Automatic Fallback Derivation Chain

A valid community theme only requires **3 colors** (`window_bg`, `window_fg`, `accent`). All remaining tokens derive automatically in `parse_community_theme_toml()`:

```text
window_bg ──┬──► sidebar_bg ──┬──► headerbar_bg
            │                 └──► popover_bg
            └──► card_bg

window_fg ──┬──► sidebar_fg ──┬──► headerbar_fg
            │                 └──► popover_fg
            └──► card_fg

accent ────────► accent_fg (via relative luminance: < 0.38 → #ffffff, >= 0.38 → #111418)

mode (is_dark) ► ink_border (#07080c if dark, #111418 if light) ──► shadow
```

### 6.4 Security & Validation (`sanitize_hex_color`)
- Every color string in `[colors]` (and any user `custom_accent_color`) is validated through `sanitize_hex_color(raw: &str) -> Option<String>`.
- Only exact 6-digit (`#RRGGBB`) or 8-digit (`#RRGGBBAA`) ASCII hexadecimal strings are accepted.
- Any invalid hex or CSS injection attempt (e.g., `#ffffff; @import ...` or arbitrary CSS properties) causes `sanitize_hex_color` to return `None`. If a required field fails validation, the `.toml` file is rejected; if an optional field fails validation, it safely falls back to its derived default.
