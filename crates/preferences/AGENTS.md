# Preferences Crate (`boxxy-preferences`)

## Responsibility
Manages application settings, persistence, and the Preferences UI. This crate ensures that user configurations (font, theme, API keys, etc.) are correctly saved to disk and applied across all components.

## Architecture
The crate uses a modern, declarative UI approach:

- **`config.rs`**: Defines the `Settings` struct, default values, and JSON serialization logic (`~/.config/boxxy-terminal/settings.json`).
- **`component.rs`**: Implements the `PreferencesComponent` using an **`AdwNavigationSplitView`** architecture. This provides a left sidebar for categories and a right-pane content area.
- **`resources/ui/preferences.ui`**: The entire widget tree is defined in this GtkBuilder XML file, which is loaded at runtime.

## Key Features
- **Categorized Navigation**: Settings are grouped into Appearance, Previews, APIs, and Advanced sections.
- **Real-Time Search**: A global search entry in the sidebar filters both categories and individual setting rows dynamically.
- **Dynamic API Support**: The APIs section automatically generates `PasswordEntryRow`s for any provider registered in `boxxy-model-selection` that requires a key. Keys are stored in a flexible `HashMap`.
- **Master Capability Switches**: Settings like `enable_web_search` act as hard locks on agent capabilities. This implements the "Master Switch vs Local Toggle" design pattern, ensuring that even if a feature is toggled on locally in a pane, it is strictly forbidden unless the global preference allows it.
- **Off by Default Strategy**: Supports a "Lightweight First" philosophy. Users can toggle `web_search_by_default` in settings to ensure heavier tools only load when explicitly requested, preserving system resources and API credits. Agents themselves initialize in an inactive state and require manual activation or interaction to wake.
- **Automatic Persistence**: All changes are immediately saved to disk and broadcasted through the `SETTINGS_EVENT_BUS`.
- **Cross-Process Sync**: `Settings::init()` hydrates the `OnceLock` cache from disk at startup; `Settings::reload()` re-reads and swaps the cache so the daemon picks up UI-side saves without restarting. Both sides call `init()` from their main.
- **Experimental Toggles**: The `Persistent Shells (Experimental)` switch in Advanced → Shell corresponds to the `pty_persistence: bool` field (default `false`). When on, closing a pane detaches its shell into the daemon instead of killing it; the infrastructure is complete but the reopen/reattach UI isn't built yet.
- **Claw UI Page**: `claw_ui/mod.rs` binds the Claw UI preference rows. The dimension settings (`claw_popover_width`, `claw_popover_max_height`) and their `CLAW_*_BOUNDS` consts were removed when the drawer redesign landed — the drawer is pane-sized, not user-sized. What remains: `maintain_overlay_history` (show the scrollable conversation log in the drawer, like the sidebar) and `claw_msgbar_shortcut` (the accelerator that opens the drawer, default `<Ctrl>slash`).
- **UI Decoupling**: Logic is separated from the UI layout by using GResource-based XML templates.
