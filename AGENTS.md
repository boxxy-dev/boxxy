# Boxxy-Terminal Agents & Architecture

## Observability & Debugging

Boxxy provides deep, real-time visibility into LLM interactions for developers and power users:
- **Cumulative Token Analytics:** Every session tracks input, output, and total tokens, persisted in `boxxy-db` and re-rendered during session resumption.
- **Dedicated Context Debugging:** To audit the exact payloads (System Prompt, History, Tools) sent to models, run the application with:
  ```bash
  BOXXY_DEBUG_CONTEXT=1 cargo run
  ```
  This is a strictly opt-in, high-signal logging mode that works in both debug and release builds (using `log::info!` on the `model-context` target), ensuring zero noise during standard development.
- **Visual Sidebar Logs:** The Claw sidebar acts as a read-only debug log, rendering tool results, process lists, and agent reasoning steps as structured UI components.

## Technology Stack
- **Language:** Rust 2024 (v1.94+)
- **Concurrency:** tokio v1.50, async-channel v2.3
- **UI Toolkit:** GTK 4.22 + Libadwaita 1.90 (via `gtk4-rs` 0.11)
- **Terminal Engine:** Custom async-first lock-free ANSI state machine absorbed directly into the `boxxy-vte` crate — no dependency on `libvte` or any C terminal library.

## Coding Guidelines

### 1. Rust Code Style
- **Formatting:** Always format with `cargo fmt --all` after writing or editing Rust files to ensure consistency across the workspace.
- **Enforcement:** Code must pass `cargo fmt --all -- --check` with zero diff. This project strictly follows the Rust 2024 style guidelines as configured in `rustfmt.toml`.
- **Inline Comments:** Use short, concise inline comments to explain *why* a particular approach was taken, especially for complex async flows, GTK property bindings, or state machine transitions. Avoid over-commenting obvious syntax. Maintain existing comments, updating them only if they become obsolete.

### 2. Modularity & Scoping
- **File Length Limit:** Keep source files under **700 lines of code** whenever possible. If a file exceeds this, refactor it into smaller, well-scoped modules.
- **Structural Integrity:** Use directory-based modules (e.g., `pane/mod.rs`) for complex components. Avoid monolithic `lib.rs` files; use them primarily for module declarations and public API exports.

### 3. UI & Resource Management
- **Declarative UI:** Do NOT write massive XML strings inside Rust source code. Always extract GTK widget trees into `.ui` XML files located in the `resources/ui/` directory.
- **GTK CSS Specificity:** Avoid using `!important` in CSS files. GTK's CSS engine does not reliably support it; instead, use higher selector specificity (e.g., `tabbar tab.color` instead of just `.color`) to override default styles.
- **GResource Integration:** Register all `.ui`, `.md` (prompts), `.css`, and icon files in `resources/resources.gresource.xml`. Load them at runtime via `gtk::Builder::from_resource` or `gio::resources_lookup_data`.
- **Build Automation:** Ensure all resources are tracked in `crates/app/build.rs` for automatic recompilation.

### 4. Concurrency & Performance
- **Non-Blocking UI:** Never perform synchronous disk I/O, heavy parsing, or network calls on the main GTK thread. Use `glib::spawn_future_local` and delegate heavy tasks to the global Tokio runtime.
- **RefCell Safety:** Avoid holding `RefCell` borrows across `.await` points to prevent runtime panics.

## Component Responsibilities

### 1. `boxxy-app` (Binary Crate)
Entry point. Initializes GTK/Libadwaita, registers GResources, bootstraps the main window, and ensures clean process termination.

### 2. `boxxy-window` (Library Crate)
Main UI Orchestrator using a modular MVU pattern. Manages the `AdwApplicationWindow`, tabs, and global state routing via `state.rs`, `ui.rs`, and the `update/` module. Acts as the primary orchestrator for the **Global Workspace Radar**, ensuring peer-to-peer agent discovery and global intent propagation across all windows. Respects the **"Lightweight First"** configuration by initializing agents in an inactive state unless `claw_on_by_default` is enabled. It monitors the active terminal pane to reflect its specific Claw mode state.

### 3. `boxxy-terminal` (Library Crate)
Manages the split-pane terminal environment. Features a deep modular architecture (`pane/`) handling UI, gestures, events, media previews, and Claw integration.

### 4. `boxxy-agent` (Binary/Library Crate)
Host Privileged Agent. Bypasses Flatpak sandboxing to handle PTY management and host-level system administration via D-Bus IPC.

### 5. `boxxy-claw` (Library Crate)
Agentic Reasoning Engine using an **Actor Model**. Spawns isolated `ClawSession` actors per terminal pane. Features the **"Red Pony Protocol"**: each pane is assigned a unique mnemonic name (e.g., "Red Pony") mapped to its UUID.

Agents possess full **System & Environment Authority**:
- **Persistent Interaction History**: Automatically saves visual events (diagnoses, suggestions, tool results) and **turn-based token usage** to the database. These are re-rendered instantly upon session restoration, ensuring zero context loss even for sidebar logs.
- **Cross-Model Continuity**: Cumulative session analytics (Total Tokens) are persisted in the database, allowing users to switch LLM providers (e.g., Gemini to Claude) while maintaining a continuous record of the session's overall cost and context depth.
- **Soft Clear Pattern**: Clicking "Clear Screen" in the sidebar marks a session-specific timestamp. Subsequent restorations only show history generated after that point, providing a clean visual state while keeping the underlying data safe.
- **Clipboard Management**: Securely read and write to the system clipboard with user approval.
- **Process Inspection & Control**: View real-time process lists and terminate misbehaving tasks via in-terminal agent popovers and read-only sidebar tables.
- **Pane Lifecycle Management**: Spawn new sibling panes/tabs, inject raw keystrokes (Esc, Ctrl+C), and close active panes dynamically.
- **Global Workspace Radar**: Discover all active peers and share high-level objectives via the **Global Intent Blackboard**.

### 6. `boxxy-vte` (Library Crate)
Headless pure-Rust terminal emulator. Renders via GSK Snapshot and supports Kittygraphics natively. OSC 7/8/133 support. Features native semantic prompt tracking (`Flags::SEMANTIC_*`) embedded directly into the terminal cell grid to provide structured context blocks (`[PROMPT]`, `[COMMAND]`, `[OUTPUT]`).

### 7. `boxxy-ai-core` (Library Crate)
Unified AI interface layer. Abstracts multiple providers (Gemini, Anthropic, Ollama) behind a single `BoxxyAgent` interface. Manages `AiCredentials` mapping and the global multi-threaded Tokio runtime.

### 8. `boxxy-core-toolbox` (Library Crate)
Provides a structured library of high-level tools for Boxxy agents, completely decoupled from the reasoning engine. Includes:
- **Host Operations:** File management (with line-range limits), process inspection, system info (structured JSON), and clipboard access.
- **Web/Network:** HTTP fetching with built-in timeouts and 1MB size limits.
- **Web Search:** A modular `SearchProvider` trait allowing real-time web queries (currently implemented via Tavily).
- **Approval Protocol:** Uses the `ApprovalHandler` trait to ensure dangerous actions (like `rm` or `kill`) always prompt the user via the GTK UI before execution.

### 9. `boxxy-preferences` (Library Crate)
Settings management using an `AdwNavigationSplitView` architecture. UI is defined in `resources/ui/preferences.ui` and supports real-time search filtering. Implements the **"Master Switch vs Local Toggle"** design pattern (e.g., for Web Search), separating global capability authorization from per-pane activation.

### 10. `boxxy-msgbar` (Library Crate)
Provides the `MsgBarComponent`, an inline GTK input overlay for interacting with Boxxy-Claw. Triggered globally via `Ctrl+/`, it anchors a native text entry precisely over the active terminal cursor. It features a robust GTK-based autocompletion system (`AutocompleteController`) for `@agent` names and seamlessly inherits the active terminal theme's background and foreground colors. It includes native toggle buttons for **Claw Mode**, **Proactive Diagnosis**, **Session Pinning**, and **Web Search** (allowing per-pane, isolated capability activation). It also includes a fully persistent, lazy-loaded history system (`MsgHistory`) that preserves text and rich multimedia attachments across sessions using `boxxy-db`, featuring automatic RAM pruning for large payloads.

### 11. `boxxy-model-selection` (Library Crate)
Data-driven model configuration UI. Uses a registry pattern to dynamically build selection dialogs and dropdowns based on registered `AiProvider` traits. Decouples AI capability discovery from the main application window.

## Distribution & Updates

Boxxy supports two primary distribution channels:

1. **Flatpak (Flathub):** The primary sandboxed distribution. Updates are managed externally by the Flatpak system.
2. **Native (GitHub Nightly):** A standalone installation method via `scripts/install.sh` that targets `~/.local/boxxy-terminal/`. 

### Auto-Update Protocol (Native Only)
For native installations, Boxxy implements an **Atomic Swap** update mechanism located in `boxxy_window::updater`:
- **Detection:** Reuses `is_flatpak()` to disable the internal updater when sandboxed.
- **Verification:** Tracks the `published_at` date of the GitHub `nightly` release in `~/.local/boxxy-terminal/.last_update` to avoid redundant prompts.
- **Persistence:** Downloads and extracts updates silently in the background.
- **Execution:** Performs an atomic rename of the running `boxxy-terminal` and `boxxy-agent` binaries before spawning the new process and exiting. This bypasses "text file busy" errors and requires no root privileges.

## State Machine & UI Sync Protocol (Claw)

To enforce clarity and predictability, Boxxy strictly adheres to the following UI/Agent sync protocol:

1. **Single Source of Truth:** The `ClawSession` actor in the background is the ultimate source of truth for the state of a proposal.
2. **In-Terminal Interaction:** All interactive agent approvals and proposals (e.g., executing commands, writing files) occur exclusively through **in-terminal popovers**. The Claw Sidebar acts solely as a **read-only debug log** for tracking history and system events.
3. **Explicit Resolution:** Every proposal *must* be resolved via `ClawMessage` as either:
   - `Approved / Executed`
   - `CancelPending` (via the Reject button, hitting Escape, or the user manually typing in the terminal to "ghost dismiss").
   - `UserMessage` (providing feedback instead of accepting/rejecting).
4. **The "Silent Reject" Pattern:** When a user explicitly rejects a proposal (via `CancelPending`), the LLM receives an error. To prevent the LLM from chatty, unnecessary follow-ups (e.g. "I'm sorry you didn't like that!"), the system dictates a `[SILENT_ACK]` flow. If the agent yields a `[SILENT_ACK]` token, the UI silently drops the event and returns the agent to a sleep state without prompting the user further.

## Development Protocol
- **MCP:** Use Context7 MCP for all library documentation and code generation.
- **Git:** NEVER automatically commit changes. Commits must be performed manually.
- **Documentation:** Keep this file and crate-level `AGENTS.md` files updated with all architectural changes.
