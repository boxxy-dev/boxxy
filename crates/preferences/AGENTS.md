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
- **Off by Default Strategy**: Supports a "Lightweight First" philosophy. Users can toggle `claw_on_by_default` or `web_search_by_default` in settings to ensure Boxxy-Claw or its heavier tools only load when explicitly requested, preserving system resources and API credits.
- **Automatic Persistence**: All changes are immediately saved to disk and broadcasted through the `SETTINGS_EVENT_BUS`.
- **UI Decoupling**: Logic is separated from the UI layout by using GResource-based XML templates.
