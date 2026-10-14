# Viewer Crate (`boxxy-viewer`)

## Responsibility
Provides a **headless**, unified GTK4 rendering library that transforms raw text (Markdown), JSON, and Custom Proposals into rich, native UI widgets.

The term **headless** in this context means that the `viewer` crate is completely agnostic to Boxxy-Terminal's specific business logic. It does not know what a "Claw Agent" is, nor does it interact with the PTY or terminal. It is a pure UI library that other Boxxy components (like the sidebar or terminal overlays) use to render structured data efficiently.

## Architecture
The crate operates as a pipeline: `Raw String Input` -> `Parser` -> `Abstract Syntax Tree (ContentBlock)` -> `Renderer Traits` -> `gtk::Widget`.

### 1. Abstract Syntax Tree (`ContentBlock`)
Instead of treating everything as a single string, the viewer splits input into distinct, logical blocks (`Paragraph`, `Heading`, `List`, `Code`, `Custom`). 

### 2. Extensible Renderers (`BlockRenderer` & `ViewerRegistry`)
The crate uses a plugin architecture. It provides default renderers for standard Markdown blocks, but allows consuming crates to inject custom renderers via the `ViewerRegistry`.
- For example, `boxxy-claw` injects a custom renderer to turn `ContentBlock::Custom { schema: "list_processes", .. }` into a rich GTK ListBox with CPU and RAM bars.

### 3. The GTK Component (`StructuredViewer`)
The primary entry point is the `StructuredViewer` struct, which wraps a `gtk::Box`. Consuming components can either replace its content entirely (`set_content`) or stream data into it continuously (`append_markdown_stream`).

## Key Performance Features

To ensure the UI remains buttery smooth even when a local LLM is generating hundreds of tokens per second, the viewer implements several advanced techniques:

- **Zero-Copy Parsing:** Uses `pulldown-cmark` for blazing-fast Markdown parsing that avoids unnecessary string allocations by borrowing slices of the input buffer.
- **"Active Block" Streaming Strategy:** Rebuilding the entire GTK DOM for every new token causes massive flickering. Instead, `StructuredViewer` only updates the *last* (currently streaming) widget in the DOM. Once the parser signals a block is complete (e.g., closing a ` ``` ` code fence), the widget is "sealed" permanently.
- **Debounced Rendering (Polling-like updates):** During fast token generation, updating the GTK main thread per token starves the UI. The viewer uses a "push-then-poll" strategy: `append_markdown_stream` buffers incoming text immediately but only queues a DOM update if one isn't already pending. It flushes these updates via a `glib` timeout at a maximum of 60Hz (~16ms), effectively batching high-velocity streams into smooth, frame-synchronized UI updates.
- **Asynchronous Syntax Highlighting:** Fenced code blocks are syntax-highlighted using `syntect`. Because highlighting a 1,000-line code block would freeze the main UI thread, the code renderer offloads the highlighting to a background thread. It renders plain text immediately, and seamlessly swaps in the colored Pango XML once the background thread finishes.
- **Off-Thread Hardened Image Pipeline (`renderer/image.rs`):** HTTP and local images are fetched on the shared Tokio runtime via a reusable `LazyLock<reqwest::Client>` with a 10 MB streaming byte cap (`resp.chunk().await`), then decoded off-thread inside `tokio::task::spawn_blocking` (enforcing a 4096×4096 dimension guard) before constructing a `gdk::MemoryTexture` on the GTK thread.
- **Pango Markup Flattening:** Instead of creating a separate `gtk::Label` for every single **bold** or *italic* word, the parser folds inline styles into a single Pango XML string (e.g., `<b>Text</b>`), guaranteeing exactly *one* GTK widget is created per paragraph.
