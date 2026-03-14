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
- **Automatic Persistence**: All changes are immediately saved to disk and broadcasted through the `SETTINGS_EVENT_BUS`.
- **UI Decoupling**: Logic is separated from the UI layout by using GResource-based XML templates.
