---
name: Boxxy Character Creator
description: Interview the user, generate and crop 16:9 portrait and 1:1 avatar artwork, and create Characters v2 packages in the Boxxy characters folder.
triggers:
  - create character
  - create a character
  - new character
  - custom character
  - make a character
  - character creator
  - ai companion
  - persona
pinned: false
---
# Boxxy Character Creator Instructions

Follow these instructions **only** when the user wants to create, customize, or package a **Boxxy Character** (ignore this skill if the user is merely chatting with or inviting an existing character).

## 1. Quick Discovery Interview First
If the user has not yet specified the character's core details (for example: *"let's create a new character"*), ask **one** concise, friendly message with these 4 questions before generating images or writing files:
1. **Name** (e.g., `"Sora"` or `"Sora Takahashi"` — used to derive a lowercase kebab-case `<slug>` like `sora-takahashi`)
2. **Role / Technical Specialty** (a punchy 1-line subtitle for `duties`, such as *"Systems & Kernel Mentor"*, *"Chaos SRE Firefighter"*, or *"Pixel-Perfect CSS Witch"*)
3. **Personality, Voice & Quirks** (tone, speech habits, catchphrases, how they address the user)
4. **Visual Style & Signature Color** (character appearance, art style such as anime / cel-shaded / cyberpunk / painterly, `#RRGGBB` hex vibe — or let them attach an `@image` / paste an image / say *"surprise me!"* for anything they leave open)

*(If the user already provided enough detail in their prompt, skip redundant questions and proceed directly to Step 2).*

## 2. Generate or Source Artwork & Crop Exact v2 Assets
Characters v2 uses two image files inside `<character_dir>`:
- **`portrait.webp`** — **`768 × 432 px` (`16:9` widescreen)** rendered in the `CharacterPicker` banner (`256×144`) and active Claw Sidebar hero (`240×135`).
- **`AVATAR.png`** — **`256 × 256 px` (`1:1` square)** rendered in chat rows, `:character` autocomplete, Past Conversations, and Preferences.

### Safe-Zone Framing for `16:9` Portraits (CRITICAL)
UI badges sit in the outer corners of the `16:9` portrait frame (top-right `✓` / `In use`, bottom-left `Custom`, bottom-right `Sleep` / `Vibe`). When calling `generate_image`:
- Pass `aspect_ratio: "16:9"`.
- Prompt for a **waist-up or chest-up character portrait with the character's face and eyes centered inside the middle 70% horizontal × 75% vertical safe zone**, expressive background atmosphere on the left/right flanks, and **strictly no text, UI borders, or watermarks**.

### Keeping `portrait.webp` and `AVATAR.png` Visually Consistent
1. **Single-Master Strategy (Recommended — works across ALL image providers at half the cost):**
   - Call `generate_image(prompt=..., aspect_ratio="16:9")` once to create the master widescreen illustration.
   - Call `prepare_character_assets` passing that same generated `path` as **both** `portrait_source_path` and `avatar_source_path`, tuning `avatar_focus_x` (`~0.5`), `avatar_focus_y` (`~0.25–0.35`, where the face sits vertically), and `avatar_zoom` (`~1.4–1.8`) to crop a crisp close-up `256×256` headshot!
2. **Optional Dual-Shot on Gemini / OpenRouter:**
   - If the user wants a separate pose/expression for the `1:1` avatar and your provider supports `reference_image_path`, call `generate_image(prompt=..., aspect_ratio="1:1", reference_image_path="<portrait_path>")` and pass each path to `prepare_character_assets`.
3. **User-Supplied Image (`@image.png` or Pasted Clipboard Image):**
   - Pass the `@image` path (or `"attached"` when the user pasted/attached an image directly in the message) to `prepare_character_assets`.
4. **If Your Active Model Lacks `generate_image`:**
   - If an image-capable teammate is listed in `invite_character`, invite them (`debate: false`) to generate the `16:9` master artwork in `<config_dir>/boxxyclaw/generated/` first; otherwise call `prepare_character_assets(slug="<slug>")` without image paths to resolve `character_dir` and create a clean initials-backed character package.

**CRITICAL:** Always use `prepare_character_assets` to crop/resize character images — never shell out to `imagemagick`, `ffmpeg`, or `python`. It runs Boxxy's built-in Rust Lanczos3 + lossless WebP/PNG pipeline and returns the exact `character_dir`, `manifest_path`, and `character_toml_path` for the active environment (Native, Flatpak, `--dev`, or `--config-dir`).

## 3. Write `manifest.toml` and `CHARACTER.toml`
Using the `manifest_path` and `character_toml_path` returned by `prepare_character_assets`, write both files directly with `file_write` (when `existing_character` is `false`, do **not** call `list_directory` first):

### `<character_dir>/manifest.toml`
```toml
schema_version = 2

[package]
version = "1.0.0"
authors = ["Boxxy Claw"]
description = "Concise one-sentence summary shown in tooltips and Preferences."
tags = ["rust", "systems", "mentor"]

[assets]
avatar = "AVATAR.png"       # Omit if no avatar image was created
portrait = "portrait.webp"  # Omit if no portrait image was created

[theme]
accent_color = "#2AA198"    # Must be a valid 6-digit #RRGGBB hex color
```

### `<character_dir>/CHARACTER.toml`
```toml
display_name = "Sora"
color = "#2AA198"
duties = "Systems & Kernel Mentor"
personality = """
Write clear, specific behavioral and voice instructions in second person. Keep technical explanations, commands, and code blocks accurate while staying unmistakably in character."""

greetings = [
  "First short, in-character welcome quote shown on empty panes.",
  "Second short, in-character welcome quote.",
  "Third short, in-character welcome quote.",
]
```

### Validation Rules
- **Slug:** Lowercase kebab-case (`[a-z0-9-]`, e.g. `sora-takahashi`). **Never** use `inco-gnito` (reserved for `/incognito`).
- **UUID (`id`):** Omit `id` in `CHARACTER.toml` — Boxxy automatically generates and persists a collision-free UUID v4 on load.
- **Hex Colors:** Both `[theme].accent_color` and `color` must be valid `#RRGGBB` hex strings (never CSS names or `rgb()`).
- **Greetings:** Always include `3` punchy single-line `greetings` (displayed at zero token cost in the character picker).

## 4. Showcase & Wrap Up
- Include **only** `portrait_markdown` and `avatar_markdown` returned by `prepare_character_assets` in your final response so the user sees just the `16:9` banner and cropped `1:1` avatar inline — **do not** also paste the raw uncropped `generate_image` markdown path (or a duplicate 3rd image will appear).
- Let the user know their character hot-reloads automatically within 500 ms and is ready to pick in any new pane (`Ctrl+/`) or view under **Preferences → Characters**, and offer quick adjustments (re-cropping `avatar_focus_y`/`avatar_zoom`, regenerating art, or tuning personality) if they want tweaks.
