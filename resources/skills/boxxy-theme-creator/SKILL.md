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

## 2. Exact `.toml` File Schema
Boxxy uses a playful **Neo-Brutalist / Tactile Comic UI** skeleton (`2px` crisp ink outlines and hard offset drop-shadows). Community themes are strictly **color-only `.toml` files**:

```toml
[meta]
name = "Ghibli Meadow"
author = "Boxxy Claw"
description = "Sunlit parchment, summer meadow greens, and warm Totoro forest ink"
mode = "light" # MUST be "light" or "dark"
tags = ["ghibli", "warm", "botanical"] # Up to 3 short lowercase tags

[colors]
window_bg = "#f4f1de"      # Main window & preferences page canvas
window_fg = "#1d261f"      # Primary text color (high contrast against window_bg)
sidebar_bg = "#eae4c9"     # Left navigation sidebar surface
sidebar_fg = "#1d261f"     # Sidebar text color
card_bg = "#fffdf6"        # Cards, buttons, inputs, and boxed lists (must contrast with window_bg!)
card_fg = "#1d261f"        # Text inside cards and buttons
headerbar_bg = "#eae4c9"   # Top headerbar surface
headerbar_fg = "#1d261f"   # Headerbar text & icons
popover_bg = "#fffdf6"     # Menus, popovers, and command palette
popover_fg = "#1d261f"     # Popover text
accent = "#529471"         # Selected pills, active tabs, switches, suggested buttons
accent_fg = "#ffffff"      # Text/icons on top of accent (#111418 for bright/mid accents, #ffffff for dark accents)
ink_border = "#1d261f"     # 2px neo-brutalist outline color (always a deep, crisp dark ink!)
shadow = "#1d261f"         # Hard offset drop-shadow color (usually matches ink_border)
```

## 3. Design & Validation Rules (CRITICAL)
1. **Strict Hex Colors Only:** Every value under `[colors]` MUST be a 6-digit hex string starting with `#` (`#RRGGBB`). Never use named colors (`red`), `rgb()`, or CSS syntax—invalid strings are rejected by Boxxy's theme parser.
2. **Surface Separation (`window_bg` vs `card_bg`):**
   - Never make `window_bg` and `card_bg` identical.
   - In **light** themes (`mode = "light"`), make `window_bg` a tinted paper/pastel tone (e.g., `#f4f1de` cream, `#eef6f0` sage, `#fff0f5` blossom) and `card_bg` a brighter cream/white (`#fffdf6` or `#ffffff`) so cards and buttons pop off the page.
   - In **dark** themes (`mode = "dark"`), make `window_bg` a deep base (e.g., `#1d1d20`), `sidebar_bg` slightly distinct (`#2e2e32`), and `card_bg` elevated (`#26262a`).
3. **Crisp Ink Borders (`ink_border` & `shadow`):**
   - Boxxy's neo-brutalist aesthetic relies on dark, high-contrast outlines (`ink_border`) and offset shadows (`shadow`). Even in light or pastel themes, `ink_border` and `shadow` should be a rich dark ink (`#1a1d1a`, `#231917`, `#111418`), never a washed-out light gray.
4. **Live Reload in Preferences:**
   - Once you write `~/.config/boxxy-terminal/themes/<slug>.toml`, tell the user that the theme is immediately available under **Preferences → Themes → Community Themes** (Boxxy hot-reloads the `themes/` folder automatically without restarting).
