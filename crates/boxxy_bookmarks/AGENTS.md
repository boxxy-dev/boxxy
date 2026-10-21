# Bookmarks Crate (`boxxy-bookmarks`)

## Responsibility
Manages reusable Bash and Python script bookmarks, dynamic `{{{variable}}}` template parsing, AI-assisted script generation, and both the full-tab Bookmarks Gallery (`BookmarksTabComponent`) and left-sidebar Bookmarks panel (`BookmarksSidebarComponent`).

## Architecture

- **`lib.rs`**: Defines `Bookmark` and `ScriptLanguage` (`Bash` vs `Python` detection from file extension or shebang, icon mapping, and `sourceview5` language IDs).
- **`parser.rs`**: `BookmarkTemplate` — uses a `LazyLock<Regex>` to extract unique `{{{variable}}}` placeholders and render comma-separated runtime inputs into scripts.
- **`manager.rs`**: `BookmarksManager` and `BOOKMARKS_EVENT_BUS`. Persists bookmark metadata in `~/.config/boxxy-terminal/bookmarks/bookmarks.json` and individual script files in `~/.config/boxxy-terminal/bookmarks/<filename>`. Maintains an in-memory cache of script contents so `ListView` scrolling, sidebar rendering, and command palette updates incur zero synchronous disk I/O on the GTK main thread.
- **`tab.rs`**: `BookmarksTabComponent` (loaded from `resources/ui/bookmarks_tab.ui`). Uses a `GtkStack` to present a welcoming onboarding empty state (explaining `{{{variable}}}` placeholders, AI script generation, and one-click execution) when no bookmarks exist, and a searchable/filterable virtualized `GtkListView` gallery when populated. Stores row widget references via `boxxy_core_widgets::ObjectExtSafe` and uses a single shared CSS provider for preview `sourceview5::View` instances.
- **`sidebar.rs`**: `BookmarksSidebarComponent` (loaded from `resources/ui/bookmarks_sidebar.ui`). Displays an empty state with **"Open Bookmarks Tab"** and **"New Bookmark"** buttons when no bookmarks exist, and a drag-and-drop reorderable list with parameter count badges (`{N}`) plus a footer bar with **"Open Bookmarks Tab"** and **"+"** buttons when populated.
- **`editor.rs`**: `BookmarkEditor` (loaded from `resources/ui/bookmark_editor.ui`). Provides an `AdwDialog` with an `AdwToolbarView`, live `Bash (.sh)` / `Python (.py)` language auto-detection and syntax highlighting, an improved **"Generate with AI"** popover, and an interactive **Template Variables** helper bar that explains `{{{name}}}` syntax, previews live detected placeholder chips as the user types, and offers a **`+ Insert {{{var}}}`** button.
