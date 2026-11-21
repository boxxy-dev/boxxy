# Boxxy Character Package API (Characters v2)

This document specifies the **Characters v2** package format, asset geometry, override lifecycle, validation rules, and forward-compatibility contract for community and bundled characters in Boxxy-Terminal.

---

## 1. Directory Layout

All characters live under `<config_dir>/boxxyclaw/characters/` (`~/.config/boxxy-terminal/boxxyclaw/characters/` on native installs; `~/.var/app/dev.boxxy.BoxxyTerminal/config/boxxy-terminal/boxxyclaw/characters/` on Flatpak):

```text
boxxyclaw/characters/
├── README.md                  # Auto-synced quickstart reference
├── characters.json            # Ordered array of character slugs (`["niko-una", "levi-kujo", ...]`)
└── <slug>/
    ├── manifest.toml          # Package metadata, credits, asset map & theme defaults (v2)
    ├── CHARACTER.toml         # LLM identity, role, personality prompt & zero-token greetings
    ├── avatar.png             # 256×256 (1:1 square) — chat rows, chips, autocomplete & lists
    ├── portrait.webp          # 768×432 (16:9 landscape, optional) — picker cards & sidebar hero
    ├── custom_avatar.png      # Optional local user override on disk
    ├── custom_portrait.webp   # Optional local user override on disk
    └── custom.toml            # Optional local user override (`color = "#..."`)
```

- **`<slug>`**: Lowercase kebab-case folder name (e.g. `niko-una`, `sora-dev`). Derived from the directory name; `inco-gnito` is reserved for `/incognito` mode and skipped by the catalog scanner.
- **Hot Reload**: `boxxy-agent`'s `CharacterWatcher` monitors `.toml`, `.png`, `.webp`, `.jpg`, `.jpeg`, and `.json` changes with a 500 ms debounce and broadcasts updated `RegistrySnapshot`s live to all open windows.

---

## 2. Image Assets & Exact Dimensions

Characters v2 separates **square identity icons (`avatar`)** from **wide hero artwork (`portrait`)**. Stage/terminal `background` images are intentionally omitted in v2 (terminal backgrounds and Frosted Glass belong to the user's window theme; transparent character stage backdrops are reserved for Characters v3 Live2D rigs).

| Asset | Aspect Ratio | Canonical Size | Accepted Formats | Max Size / Bounds | UI Surfaces |
|---|---|---|---|---|---|
| **Avatar** | **`1:1`** (Square) | **`256 × 256 px`** | `.png`, `.webp`, `.jpg` | `<= 20 MiB`, `16..=4096 px` | Chat message rows (`28–32 px`), `:character` / `/debate` / `/drama` autocomplete, Past Conversations, Model Selector, desktop notifications, Preferences character rows (`52 px`), and fallback sidebar avatar (`84 px`). |
| **Portrait** *(optional)* | **`16:9`** (Landscape) | **`768 × 432 px`** | `.webp` *(preferred)*, `.png`, `.jpg` | `<= 20 MiB`, `64×36..=8192 px` (`<= 36 MP`) | Overlay `CharacterPicker` hero banner (`256 × 144` logical px) and active Claw Sidebar hero frame (`240 × 135` logical px). |

### Why `16:9` at `768 × 432 px`?
1. **3× Integer HiDPI Scaling:** `256 × 144` × `3` = `768 × 432`, rendering cleanly on `1×`, `1.5×`, `2×` (`512 × 288`), and `3×` displays with only `1.26 MiB` of uncompressed RGBA8 VRAM per portrait.
2. **Native Match for `generate_image` & Widescreen Art:** Matches `ImageAspectRatio::Landscape` (`16:9`) in `boxxy-ai-core` and standard `1920×1080` / `1280×720` illustrations with zero vertical crop loss.
3. **Pure-Rust WebP Decoding Fallback:** `boxxy_core_widgets::decode_texture_from_file` decodes `.webp` streams via `boxxy-assets` (`image-webp`) into `gdk::MemoryTexture`, ensuring `.webp` portraits and avatars render identically even on minimal Linux distributions or Nix closures without `webp-pixbuf-loader`.

### Safe Zones on `16:9` Portraits
Overlay badges sit in the outer corners of the `16:9` frame:
- **Top-Right (`32 × 32 px` logical margin):** Selected checkmark (`✓`) or `In use` pill in `CharacterPicker`.
- **Bottom-Left (`80 × 28 px` logical margin):** `Custom` model override pill in `CharacterPicker`.
- **Bottom-Right (`36 × 36 px` logical margin):** `Sleep` / `Vibe` status badge in the Claw Sidebar hero.
Keep the character's face and eyes within the central `70%` horizontal / `75%` vertical safe area so corner badges never obscure facial features.

### Visual Fallback Hierarchy
1. **Has `portrait`**: Renders `portrait` edge-to-edge (`ContentFit::Cover`) in both `CharacterPicker` (`256 × 144`) and the Claw Sidebar hero (`240 × 135`).
2. **Has `avatar` only (`has_portrait == false`)**:
   - `CharacterPicker` renders a character-tinted gradient stage (`.claw-char-portrait-avatar-stage`) with a centered `72 px` circular `adw::Avatar` (preventing `1:1` avatars from stretching or cropping foreheads/chins).
   - Claw Sidebar renders the classic `84 px` circular `adw::Avatar`.
3. **Neither `portrait` nor `avatar`**: Renders a tinted fallback card with Adwaita initials.

---

## 3. `manifest.toml` Schema (Package & Community Metadata)

`manifest.toml` owns package distribution metadata, creator attribution, relative asset paths, and default theme colors. It is **optional** on disk: directories without `manifest.toml` load seamlessly as `schema_version = 1`.

```toml
schema_version = 2

[package]
version = "1.0.0"
authors = ["creator-handle"]
description = "Concise one-line summary shown in tooltips and Preferences."
license = "CC-BY-4.0"                         # optional
homepage = "https://example.com/character"    # optional
tags = ["rust", "kernel", "mentor"]           # optional (indexed by Preferences search)
min_boxxy_version = "0.1.0"                   # optional

[assets]
avatar = "avatar.png"                         # optional; relative path inside character dir
portrait = "portrait.webp"                    # optional; relative path inside character dir

# Reserved for Characters v3 (accepted & validated now, ignored at runtime in v2):
# live2d_model = "live2d/model.model3.json"
# expressions_dir = "expressions"

[theme]
accent_color = "#4E9A06"                      # canonical #RRGGBB package brand color
```

### Security & Validation Rules
- `schema_version` must be `>= 1`. Unknown versions `> 2` log an informational notice and gracefully ignore unrecognized v3+ fields.
- Every path in `[assets]` must be relative to the character directory: leading `/`, `\`, NUL bytes, `.`, and `..` traversal components are rejected by `validate_relative_asset_path`.
- `avatar` and `portrait` must use `.png`, `.webp`, `.jpg`, or `.jpeg` extensions and pass magic-byte + header dimension checks in `boxxy-assets` before full pixel allocation.
- `[theme].accent_color` and `CHARACTER.toml` `color` are validated with `is_valid_hex_color` (`#RGB`, `#RGBA`, `#RRGGBB`, `#RRGGBBAA`); all UI labels render with `use_markup(false)` so untrusted strings never break Pango XML or inject CSS.

---

## 4. `CHARACTER.toml` Schema (Persona, Prompt & Greetings)

`CHARACTER.toml` owns the LLM persona and voice. Keeping `greetings` directly inside `CHARACTER.toml` (and omitting separate `greetings.toml` / `LORE.md` files) avoids multi-file fragmentation and protects the LLM context window from multi-kilobyte lore bloat.

```toml
# Stable identity — auto-generated on first load if omitted; do not change once assigned
id = "550e8400-e29b-41d4-a716-446655440000"

display_name = "Sora"
color = "#4E9A06"                             # optional when [theme].accent_color is set in manifest.toml
duties = "Systems & Kernel Mentor"            # UI subtitle + injected under --- CHARACTER ROLE ---
personality = """
Calm, encouraging, and technically exact. Explain root causes briefly before proposing fixes, and keep code blocks clean."""

greetings = [
  "Terminal is ready. What are we building today?",
  "Show me the failing trace whenever you're ready.",
]
```

### Zero-Token Greetings
- `greetings` (`Vec<String>`, optional, defaults to `[]`) is displayed as an italic quote in `CharacterPicker` when selecting a character on a new pane — giving each character immediate personality before a single LLM token is spent.
- Selection uses deterministic FNV-1a hashing seeded with `(pane_id, character_id)` so the greeting stays steady across settings refreshes within a pane while varying naturally across new panes.
- Editing `greetings` does **not** invalidate active `rig::Agent` session preambles (`hash_personality` hashes only `duties` and `personality`).

---

## 5. Non-Destructive User Overrides & Package Updates

Characters v2 preserves user customizations across community package updates:
1. **Model Overrides:** Stored in `settings.json` (`Settings.character_models`) keyed by stable `CHARACTER.toml` `id` UUID.
2. **Color Overrides:** Changing a v2 character's color swatch in **Preferences → Characters** writes `color = "#..."` to `custom.toml` without mutating `manifest.toml` or `CHARACTER.toml` (with a 1-click undo button to restore the package default color).
3. **Avatar / Portrait Overrides on Disk:** Dropping `custom_avatar.png`/`.webp` or `custom_portrait.webp`/`.png` into `<char_dir>/` overrides the package's bundled `avatar` or `portrait` without modifying author files.

Asset resolution priority on disk:
- **Avatar:** `custom_avatar.png` → `custom_avatar.webp` → `manifest.toml [assets].avatar` → `avatar.png` → `avatar.webp` → `AVATAR.png` → `AVATAR.webp` → bundled default (`niko-una`, `levi-kujo`, `kuro`).
- **Portrait:** `custom_portrait.webp` → `custom_portrait.png` → `manifest.toml [assets].portrait` → `portrait.webp` → `portrait.png` → `PORTRAIT.webp` → `PORTRAIT.png`.
- **Color:** `custom.toml color` → `CHARACTER.toml color` → `manifest.toml [theme].accent_color` → `#8035D4`.

---

## 6. CLI Diagnostics (`boxxy-terminal +doctor`)

Running `boxxy-terminal +doctor` (or `boxxy-terminal +doctor --json`) validates every directory in `<config_dir>/boxxyclaw/characters/` via `validate_character_package_dir`, reporting:
- Syntax or schema errors in `CHARACTER.toml` and `manifest.toml`.
- Path traversal or disallowed extension errors in `[assets]`.
- Warnings for missing declared asset files or malformed hex colors, plus summary counts of v2 packages and portraits.
