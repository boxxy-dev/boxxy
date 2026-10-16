# Viewer Crate (`boxxy-viewer`)

## Responsibility
Provides a **headless**, unified GTK4 rendering library that transforms raw text (Markdown), JSON, and Custom Proposals into rich, native UI widgets.

The term **headless** in this context means that the `viewer` crate is completely agnostic to Boxxy-Terminal's specific business logic. It does not know what a "Claw Agent" is, nor does it interact with the PTY or terminal. It is a pure UI library that other Boxxy components (like the sidebar or terminal overlays) use to render structured data efficiently.

## Architecture
The crate operates as a pipeline: `Raw String Input` -> `Parser` -> `Abstract Syntax Tree (ContentBlock)` -> `Renderer Traits` -> `gtk::Widget`.

### 1. Abstract Syntax Tree (`ContentBlock`)
Instead of treating everything as a single string, the viewer splits input into distinct, logical blocks (`Paragraph`, `Heading`, `Blockquote`, `List`, `Rule`, `Image`, `ImageGallery`, `Table`, `Code`, `Custom`).
- **Automatic Gallery Coalescing:** The parser post-processes `ContentBlock::List` items that contain images (with optional short filename/caption paragraphs) as well as consecutive standalone `ContentBlock::Image` blocks into `ContentBlock::ImageGallery { items: Vec<GalleryItem> }`, while preserving introductory prose paragraphs above image runs.
- **GFM Table Parsing:** Full support for GitHub-Flavored Markdown tables (`Tag::Table`, `TableHead`, `TableRow`, `TableCell`), including streaming partial tables, inline `<br>` / `<br/>` line breaks, and images embedded directly inside table cells (`TableCell { markup, images }`).
- **Clickable Hyperlinks:** Converts Markdown links `[text](url)` into Pango `<a href="...">` tags wired to `gtk::UriLauncher` (for web URLs) and `gtk::FileLauncher` (for local files).

### 2. Extensible Renderers (`BlockRenderer` & `ViewerRegistry`)
The crate uses a plugin architecture. It provides default renderers (`TextRenderer`, `TableRenderer`, `CodeRenderer`, `ImageRenderer`) for standard Markdown blocks, and allows consuming crates to inject custom renderers via the `ViewerRegistry`.
- `TableRenderer` (`renderer/table.rs`): Renders GFM tables inside a `.viewer-table-frame` card using a `HeightForWidth` `TableContainer` around `gtk::Grid` (preventing unconstrained 1-character minimum-width height inflation inside horizontal `GtkBox` rows), alternating row shading, smart prose wrapping, embedded `.viewer-image-card` thumbnails inside cells, and a one-click Copy TSV button.
- `ImageRenderer` (`renderer/image.rs`): Renders standalone/table-cell images (via `BoundedCard` to prevent `GtkPicture` aspect-ratio height inflation in wide containers) and responsive 1/2/3-column `GalleryGrid` media galleries (`.viewer-image-card`). Each card features a footer with middle-ellipsized filename/caption, original resolution tooltip (`width×height`), click-to-open, and an action menu / right-click context menu (`Open`, `Open With…` via `gtk::FileLauncher::set_always_ask(true)`, `Show in Folder`, `Copy Image`, `Copy Path / URL`).

### 3. The GTK Component (`StructuredViewer`)
The primary entry point is the `StructuredViewer` struct, which wraps a `gtk::Box`. Consuming components can either replace its content entirely (`set_content`) or stream data into it continuously (`append_markdown_stream`).

## Key Performance Features

To ensure the UI remains buttery smooth during high-velocity LLM token streaming and `GtkListView` history scrolling:

- **Diff-Aware Incremental Streaming (`ViewerState::apply_blocks`):** Tracks `rendered_blocks: Vec<(ContentBlock, gtk::Widget)>` and diffs new AST blocks against the existing widget list, preserving unchanged prefix widgets and cleanly replacing coalesced tail blocks (such as two consecutive images merging into an `ImageGallery`).
- **Two-Tier Thumbnail Cache (`cache.rs`):**
  - **Tier-1 (Main-Thread `MemoryTexture` LRU):** Stores up to 96 downscaled `gdk::MemoryTexture`s in memory. When scrolling back in `GtkListView`, `connect_bind` hits Tier-1 synchronously in 0 ms, setting the exact `Picture` size and texture before layout measurement—eliminating spinners and scroll-position jumps.
  - **Tier-2 (Persistent Disk Cache):** Stores pre-scaled thumbnails in `<config_dir>/cache/thumbnails/<sha256>.bxt` keyed by file path/URL, modification time, file size, and target `ThumbnailSpec`, avoiding re-decoding 4K/8K source files across session restores.
- **Off-Thread Thumbnailing via `boxxy-assets`:** Delegates image validation and downscaling (up to 50 MB / 16384×16384 source images) to `boxxy_assets::image::generate_thumbnail` inside `tokio::task::spawn_blocking`, transferring only the compact downscaled RGBA8 thumbnail buffer (`<= 480×320` or `<= 360×220`) to the GTK thread.
- **Parsed AST & Syntax Highlighting LRU Caches (`cache.rs`):** Caches parsed `Arc<[ContentBlock]>` ASTs (256 entries) and `syntect` Pango markup strings (128 entries) so `GtkListView` row recycling renders markdown and syntax-highlighted code blocks synchronously in 0 ms on cache hit.
