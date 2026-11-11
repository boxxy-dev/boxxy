---
name: Boxxy Theme Creator
description: Create custom Boxxy community app themes (.toml) inside the Boxxy themes folder so the user can pick them in Preferences -> Themes.
triggers:
  - theme
  - themes
  - boxxy theme
  - custom theme
  - ghibli
  - color scheme
pinned: false
---
# Boxxy Theme Creator Instructions

When the user asks you to create, design, or customize a **Boxxy Theme** (for example: *"create a Boxxy theme that resembles Studio Ghibli coloring"*):

## 1. Where to Save the Theme
Write a `.toml` file using the `file_write` tool into:
`~/.config/boxxy-terminal/themes/<slug>.toml`
*(If `$BOXXY_CONFIG_DIR` is set in the environment, use `$BOXXY_CONFIG_DIR/themes/<slug>.toml` instead).*

- `<slug>` must be lowercase kebab-case (e.g., `studio-ghibli.toml`, `spirited-away.toml`, `nordic-frost.toml`).
- **Reserved IDs:** Never name the file `boxxy-dark.toml` or `boxxy-light.toml` (those IDs are reserved for built-in official themes and will be skipped by the community theme loader).

## 2. Exact `.toml` File Schema & Color Roles
Boxxy uses a playful **Neo-Brutalist / Tactile Comic UI** skeleton (`2px` crisp ink outlines and hard offset drop-shadows) with a **3-color identity triad** (`accent`, `secondary`, `highlight`). Community themes are strictly **color-only `.toml` files**:

```toml
[meta]
name = "Ghibli Meadow"
author = "Boxxy Claw"
description = "Sunlit parchment, meadow greens, summer sky blue, and Calcifer gold"
mode = "light" # MUST be "light" or "dark"
tags = ["ghibli", "warm", "botanical"] # Up to 3 short lowercase tags

[colors]
# --- Surfaces & Typography ---
window_bg = "#f4f1de"      # Main window, dialogs & Claw drawer canvas
window_fg = "#1d261f"      # Primary text color (high contrast against window_bg)
sidebar_bg = "#eae4c9"     # Left navigation sidebar surface
sidebar_fg = "#1d261f"     # Sidebar text color
card_bg = "#fdf6e2"        # Cards, buttons, inputs, boxed lists, and linked menu strips (must contrast with window_bg and popover_bg!)
card_fg = "#1d261f"        # Text inside cards and buttons
headerbar_bg = "#eae4c9"   # Top headerbar surface (defaults to sidebar_bg if omitted)
headerbar_fg = "#1d261f"   # Headerbar text & icons
popover_bg = "#eae4c9"     # Menus, popovers, and command palette (matches sidebar_bg so inner card_bg linked buttons pop!)
popover_fg = "#1d261f"     # Popover text (defaults to sidebar_fg if omitted)

# --- 3-Color Identity Triad ---
accent = "#529471"         # Primary interaction: selected tabs/rows, hovered controls, switches, suggested buttons
accent_fg = "#111418"      # Text/icons on top of accent & hover surfaces (#111418 for bright/mid fills, #ffffff for deep dark fills)

secondary = "#6baed6"      # Supporting identity: category tag pills, bookmark variable chips, task kind badges, @file tags
secondary_fg = "#111418"   # High-contrast text on top of secondary pills (#111418 or #ffffff)

highlight = "#f4a261"      # Attention & discovery: NEW/UPDATED badges, Vibe Mode badge, search matches, :character mentions
highlight_fg = "#111418"   # High-contrast text on top of highlight badges (#111418 or #ffffff)

# --- Neo-Brutalist Ink & Shadows ---
ink_border = "#1d261f"     # 2px neo-brutalist outline color (always a deep, crisp dark ink!)
shadow = "#1d261f"         # Hard offset drop-shadow color (usually matches ink_border)
```

## 3. Design & Validation Rules (CRITICAL)
1. **Strict Hex Colors Only:** Every value under `[colors]` MUST be a 6-digit hex string starting with `#` (`#RRGGBB`). Never use named colors (`red`), `rgb()`, or CSS syntax—invalid strings are rejected by Boxxy's theme parser.
2. **Coordinate `accent`, `secondary`, and `highlight` Intentionally:**
   - Give each role a distinct purpose that fits the theme's palette:
     - **`accent`:** The hero primary color for active selections and tactile hover surfaces.
     - **`secondary`:** A harmonious companion hue (e.g. cool blue/lavender/teal alongside green or pink) used on tag pills and metadata chips so they don't compete with selected items.
     - **`highlight`:** An energetic discovery/spotlight hue (e.g. warm amber, gold, coral, or neon magenta) for feature badges, Vibe Mode, and search highlights.
   - *(Only intentionally monochromatic themes should reuse gray/neutral values across all three).*
3. **Surface Hierarchy (`window_bg` vs `card_bg` vs `sidebar_bg`/`popover_bg`):**
   - Never make `window_bg`, `card_bg`, and `sidebar_bg` all identical.
   - **CRITICAL:** `popover_bg` MUST match `sidebar_bg` (or be omitted so it defaults to `sidebar_bg`) so context menus and popovers share the sidebar surface while inner segmented buttons (`card_bg`) stand out cleanly inside menus. Never set `popover_bg` to the same color as `card_bg`.
4. **High-Contrast Foregrounds (`accent_fg`, `secondary_fg`, `highlight_fg`):**
   - Always pick `#111418` (crisp dark ink) for bright/pastel/mid-tone fills and `#ffffff` (crisp white) for deep, dark saturated fills (such as crimson or navy) so pill labels and icons stay effortlessly readable.
5. **Crisp Ink Borders (`ink_border` & `shadow`):**
   - Boxxy's neo-brutalist aesthetic relies on dark, high-contrast outlines (`ink_border`) and offset shadows (`shadow`). Even in light or pastel themes, `ink_border` and `shadow` should be a rich dark ink (`#1a1d1a`, `#231917`, `#111418`), never a washed-out light gray.
6. **Live Reload in Preferences:**
   - Once you write `~/.config/boxxy-terminal/themes/<slug>.toml`, tell the user that the theme is immediately available under **Preferences → Themes → Community Themes** (Boxxy hot-reloads the `themes/` folder automatically without restarting).
