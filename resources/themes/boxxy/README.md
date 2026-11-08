# Boxxy Modular GTK CSS (`resources/themes/boxxy/`)

This directory contains the modular GTK 4.24 stylesheets imported by `resources/themes/boxxy_skeleton.css` (Layer 2 of the Boxxy App Theme cascade, loaded at `STYLE_PROVIDER_PRIORITY_USER = 800` when `AppThemeMode::Boxxy` is active).

## Module Ownership Table

| Order | File | Responsibility |
| :--- | :--- | :--- |
| `01` | `tokens.css` | Shared GTK 4.24 CSS custom properties (`--boxxy-border-*`, `--boxxy-radius-*`, `--boxxy-control-*`, `--boxxy-pill-*`, motion transforms). |
| `02` | `reset.css` | Global `transition: none` rule across interactive controls so GTK 4 never interpolates `background-color` through dark grey. |
| `03` | `chrome.css` | `.sidebar-pane`, `.sidebar-toolbar`, `.terminal-header`, `.content-header`, `dialog headerbar`, and frosted-glass header exclusions. |
| `04` | `tabs.css` | `AdwTabBar` (`tabbar tab`) rest/hover/selected states and embedded circular tab close buttons (`×`). |
| `05` | `sidebar.css` | `.navigation-sidebar > row` capsule pills, embedded `.past-session-options-btn` (`⋮`), and `viewswitcher` / `stackswitcher` header toggles. |
| `06` | `buttons.css` | Canonical button hierarchy: windowcontrol icon reset (`16×16`), standard tactile buttons (`24px` / `5px 12px`), `.suggested-action`, `.destructive-action`, `.compact-btn`, square headerbar/group `.image-button`s (`32×32px`), `.pill` CTAs (`26px` / `6px 22px`), and `.color-dot-btn` swatches. |
| `07` | `inputs.css` | `searchentry`, `entry:not(.flat)`, `switch`, `scale` sliders (`24×20px` block thumb), and `checkbutton`. |
| `08` | `lists-cards.css` | `.boxed-list`, `.boxed-list-separate`, `preferencesgroup listbox`, `row.activatable` hover/active states, `.card`, and `.bookmark-card`. |
| `09` | `menus-popovers.css` | `popover > contents`, `popover.menu modelbutton`, linked horizontal button bars (`box.linked > button.model`), and `GtkDropDown` / `AdwComboRow` popover `listview > row` items. |
| `10` | `scrollbars.css` | Squared Boxxy overlay & standard scrollbars with inset troughs (`3px 5px`) and reset slider margins. |
| `11` | `claw.css` | `.claw-drawer`, `.boxxy-msgbar`, `.character-btn`, `.claw-status-pill`, and `.viewer-block` containers. |
| `12` | `dialogs-widgets.css` | `dialog sheet`, `.boxxy-theme-tag-pill`, What's New dialog hero/tiles/rows/buttons, and `EditableAvatar` buttons/badges. |

## Authoring & Specificity Rules

1. **One Owner Per Component:** Always edit the module that owns the widget family. Never add widget selectors to `boxxy_skeleton.css`.
2. **Never Chain 5+ `:not()` Exclusions on Base Controls:** Every `:not(.foo)` adds `(0, 1, 0)` class specificity. Keep base rules in `buttons.css` at `<= (0, 3, 2)` and place semantic/size/component modifiers *after* the base rule so they override naturally without specificity wars.
3. **`border` vs. `outline` Box-Model Discipline:**
   - Use box-model `border: var(--boxxy-border-width) solid @boxxy_ink; outline: none;` on containers, cards, boxed-lists, popovers, dialog sheets, menu items, and compact pills (`.boxxy-theme-tag-pill`, `.whats-new-commit-pill`, `.character-btn`) so the stroke never eats into inner padding.
   - Use `outline: var(--boxxy-border-width) solid @boxxy_ink; outline-offset: -2px;` only on controls whose outerpadding explicitly budgets `>= 2px` clearance for the inward stroke (`tabbar tab`, standard tactile buttons, `.navigation-sidebar > row`, `searchentry`).
4. **Colors via Layer 3/4 `@define-color` Only:** Never hardcode theme hex colors in CSS modules. Always reference `@window_bg_color`, `@card_bg_color`, `@sidebar_bg_color`, `@headerbar_bg_color`, `@popover_bg_color`, `@accent_bg_color`, `@accent_fg_color`, `@boxxy_ink`, `@boxxy_shadow`, and `@boxxy_surface_hover`.
5. **GResource Auto-Registration:** `crates/app/build.rs` automatically discovers every `.css` file in `resources/themes/boxxy/` and bundles it into `resources/resources.gresource.xml`.
