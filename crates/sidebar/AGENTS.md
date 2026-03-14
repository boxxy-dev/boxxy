# AI Sidebar Crate (`boxxy-sidebar`)

## Responsibility
Provides the primary AI Chat sidebar component (`AiSidebarComponent`). This crate handles the user-facing chat interface, markdown rendering, slash commands, and background LLM communication.

## Architecture
The crate is modularized into several scoped modules to maintain readability and separation of concerns:

- **`component.rs`**: Core `AiSidebarComponent` implementation. Manages the main GTK widget tree, state lifecycle, and the asynchronous `tokio` generation loop.
- **`commands.rs`**: Slash-command registry and implementations (e.g., `/clear`, `/model`). Handles command execution and autocomplete logic.
- **`markdown.rs`**: Text parsing engine. Handles fenced code blocks, segment parsing, and conversion of plain text to GTK Pango markup with proper XML escaping.
- **`widgets.rs`**: UI builder functions for specialized chat widgets like message bubbles, code blocks with syntax highlighting (via `sourceview5`), and labels.
- **`types.rs`**: Core data models, including `ChatMessage`, `Role`, and integration with the `rig-core` message framework.

## Key Features
- **Markdown Support**: Renders bold, italic, and inline code natively.
- **Syntax Highlighting**: Uses `sourceview5` for high-quality code block rendering with dark/light mode awareness.
- **Slash Commands**: Extensible command system with real-time autocomplete popover.
- **Streaming-Aware**: Designed to handle asynchronous LLM responses without blocking the GTK main loop.
- **External Prompts**: System prompts are loaded from GResources (`resources/prompts/ai_chat.md`).
