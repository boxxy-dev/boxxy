# Core Widgets Crate (`boxxy-core-widgets`)

## Responsibility
Shared, domain-agnostic GTK4/Libadwaita UI primitives and GObject utilities reused across `boxxy-preferences`, `boxxy-claw-widget`, `boxxy-claw-ui`, and `boxxy-bookmarks`. Kept free of D-Bus, database, preferences, and LLM dependencies, depending only on `boxxy-assets`, `boxxy-sys-utils`, `gtk4`, `libadwaita`, `glib`, and `log`.

## Architecture & Modules

```text
src/
├── lib.rs            # Public module declarations & re-exports
├── utils.rs          # Sound TypeId + ThreadGuard + RefCell `ObjectExtSafe` & cached Quarks
├── autocomplete/
│   ├── mod.rs        # `AutocompleteController`, `AutocompleteInput`, lifecycle & key/cursor signals
│   ├── engine.rs     # Pure headless trigger matching, UTF-8 cursor bounds & completion splicing
│   └── style.rs      # Deduplicated badge `GtkCssProvider`, color sanitization/contrast & TTL avatar cache
└── avatar_cropper/
    ├── mod.rs        # Re-exports + bounded local/portal `gio::File` byte reader
    ├── editable.rs   # `EditableAvatar` composite widget + single-flight `prompt_and_crop_avatar`
    ├── geometry.rs   # Pure headless `CropState`, `DragMode`, `Viewport` & `hit_test` math
    ├── render.rs     # Premultiplied-alpha backdrop & preview samplers + Cairo canvas drawing
    └── dialog.rs     # `AvatarCropDialog` (`avatar_crop_dialog.ui`), spinner state machine & controllers
```

### 1. `ObjectExtSafe` (`src/utils.rs`)
Provides memory-safe, thread-checked typed storage on any `glib::Object` (`set_safe_data`, `get_safe_data -> Option<Ref<'_, T>>`, `steal_safe_data`):
- **Namespaced & Thread-Cached Quarks:** Prefixes keys with `boxxy-core-widgets::safe-data::` so caller keys (e.g. `"title"`, `"icon"`, `"header"` on `SignalListItemFactory` slots) never collide with GTK/GLib internal C quarks, caching resolved `glib::Quark`s per thread (`<= 512` entries) for zero-allocation virtualized list binds.
- **Zero-UB Invariants:** Each quark stores a write-once `ThreadGuard<RefCell<Option<Box<dyn Any>>>>` whose outer allocation lives until `GObject` finalization. Type mismatches return `None` via `Any` (leaving stored values untouched on mismatched `steal_safe_data`), active borrows panic deterministically on concurrent same-key mutation instead of causing use-after-free, and off-thread access to `!Send` data panics safely via `ThreadGuard`.

### 2. `autocomplete` (`src/autocomplete/`)
Reusable popover completion controller (`AutocompleteController`) supporting `gtk::Entry`, `gtk::TextView`, and custom `AutocompleteInput` implementors:
- **Leak-Free Signal Lifecycle:** All GTK signal handlers (`changed`, `notify::cursor-position`, `row-activated`, `EventControllerKey`) capture `Weak<AutocompleteController>`, and both `Drop` and `widget.connect_destroy` cleanly `unparent()` the custom `GtkPopover`.
- **Modifier & Cursor Awareness:** `handle_key_pressed_with_state` respects `Shift+Enter` (passes through for multi-line newlines), `Shift+Tab` / `ISO_Left_Tab` (moves selection up), `Page_Up` / `Page_Down`, and ignores `Ctrl`/`Alt`/`Super` chords. Cursor movement re-validates active trigger bounds while open so moving outside a token closes the popup instead of splicing against a stale `start_idx`.
- **Zero-Cascade Styling & Row Reuse:** Shares a single `GtkSizeGroup` across updates, indexes `last_items` directly instead of mutating `GtkWidget::widget_name` CSS IDs, preserves user arrow-key selection across background cache refreshes of the same query, deduplicates badge colors through a single `GtkCssProvider` (validating `#hex` and `rgb(...)` with WCAG luminance foreground contrast), and caches `gdk::Texture` avatars with a 2 s `stat()` TTL (including negative misses).

### 3. `avatar_cropper` (`src/avatar_cropper/`)
Interactive square avatar selection and cropping pipeline backed by `resources/ui/widgets/avatar_crop_dialog.ui` and `boxxy_assets::image`:
- **`EditableAvatar` & Bounded File Loading:** 52 px circular avatar button with protruding edit/remove badges, accessible labels, and an RAII `InFlightGuard` preventing duplicate stacked `GtkFileDialog`s (auto-resetting on apply, error, or cancel). Reads local files (`is_file()` + `Read::take(MAX_INPUT_BYTES + 1)`) and GVFS/portal streams (`64 KiB` bounded chunks) without risking OOM on special or oversized files.
- **Premultiplied Sampling & 120 Hz Responsiveness:** Both `AvatarCropDialog::present` and `present_from_file` prepare `<= 1024 px` Cairo ARGB32 backdrops off-thread on `boxxy_sys_utils::runtime()` behind a flash-free `180 ms` delay / `250 ms` min-hold spinner state machine, guard `Apply` completion against cancel/close races, sample 4-tap downscales in premultiplied alpha space (`R8g8b8a8Premultiplied`, preventing dark halos on transparent PNG edges), and deduplicate preview texture uploads across sub-pixel drag frames.
