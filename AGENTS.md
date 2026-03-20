# Boxxy-Terminal Agents & Architecture

## Philosophy

This project follows a modular, component-driven architecture using native **gtk4-rs** and **GTK4/Libadwaita**.
We leverage Rust's type safety and an actor-like model to enforce a strict boundary between the UI thread and background operations.

To prevent UI starvation and zombie processes, the application utilizes a **single global multi-threaded Tokio runtime** (`boxxy_ai_core::utils::runtime()`) for all I/O and CPU-heavy tasks. Communication back to the GTK main loop is handled via **bounded `async-channel`s** combined with explicit yielding (`glib::timeout_future(0).await`), ensuring the UI remains responsive under heavy load.

## Technology Stack
- **Language:** Rust 2024 (v1.94+)
- **Concurrency:** tokio v1.50, async-channel v2.3
- **UI Toolkit:** GTK 4.22 + Libadwaita 1.90 (via `gtk4-rs` 0.11)
- **Terminal Engine:** Custom async-first lock-free ANSI state machine absorbed directly into the `boxxy-vte` crate — no dependency on `libvte` or any C terminal library.

## Coding Guidelines

### 1. Rust Code Style
- **Formatting:** Always format with `cargo fmt --all` after writing or editing Rust files to ensure consistency across the workspace.
- **Enforcement:** Code must pass `cargo fmt --all -- --check` with zero diff. This project strictly follows the Rust 2024 style guidelines as configured in `rustfmt.toml`.

### 2. Modularity & Scoping
- **File Length Limit:** Keep source files under **700 lines of code** whenever possible. If a file exceeds this, refactor it into smaller, well-scoped modules.
- **Structural Integrity:** Use directory-based modules (e.g., `pane/mod.rs`) for complex components. Avoid monolithic `lib.rs` files; use them primarily for module declarations and public API exports.

### 3. UI & Resource Management
- **Declarative UI:** Do NOT write massive XML strings inside Rust source code. Always extract GTK widget trees into `.ui` XML files located in the `resources/ui/` directory.
- **GResource Integration:** Register all `.ui`, `.md` (prompts), `.css`, and icon files in `resources/resources.gresource.xml`. Load them at runtime via `gtk::Builder::from_resource` or `gio::resources_lookup_data`.
- **Build Automation:** Ensure all resources are tracked in `crates/app/build.rs` for automatic recompilation.

### 4. Concurrency & Performance
- **Non-Blocking UI:** Never perform synchronous disk I/O, heavy parsing, or network calls on the main GTK thread. Use `glib::spawn_future_local` and delegate heavy tasks to the global Tokio runtime.
- **RefCell Safety:** Avoid holding `RefCell` borrows across `.await` points to prevent runtime panics.

## Component Responsibilities

### 1. `boxxy-app` (Binary Crate)
Entry point. Initializes GTK/Libadwaita, registers GResources, bootstraps the main window, and ensures clean process termination.

### 2. `boxxy-window` (Library Crate)
Main UI Orchestrator using a modular MVU pattern. Manages the `AdwApplicationWindow`, tabs, and global state routing via `state.rs`, `ui.rs`, and the `update/` module. Acts as the primary orchestrator for the **Global Workspace Radar**, ensuring peer-to-peer agent discovery and global intent propagation across all windows. Respects the **"Lightweight First"** configuration by initializing agents in an inactive state unless `claw_on_by_default` is enabled.

### 3. `boxxy-terminal` (Library Crate)
Manages the split-pane terminal environment. Features a deep modular architecture (`pane/`) handling UI, gestures, events, media previews, and Claw integration.

### 4. `boxxy-agent` (Binary/Library Crate)
Host Privileged Agent. Bypasses Flatpak sandboxing to handle PTY management and host-level system administration via D-Bus IPC.

### 5. `boxxy-claw` (Library Crate)
Agentic Reasoning Engine using an **Actor Model**. Spawns isolated `ClawSession` actors per terminal pane. Features the **"Red Pony Protocol"**: each pane is assigned a unique mnemonic name (e.g., "Red Pony") mapped to its UUID, operating as a "Clean Slate" agent that does not leak conversational context to other panes. 

Handles context synthesis, tool execution, and LLM communication via a modular dispatcher. Features a **Hybrid Memory System** for both explicit tool-based storage and implicit background fact extraction. It utilizes OSC 133 semantic prompt tracking and Dynamic Scrollback Paging from the VTE to autonomously pull structured history context to the AI on-demand. 

Agents interact with peers via the **Global Workspace Radar**, allowing them to read buffers, delegate tasks, and autonomously coordinate multi-pane workflows across the entire PC using their mnemonic identities. Agents can proactively discover all active peers using the `list_active_agents` tool and share high-level objectives via the **Global Intent Blackboard**.

Agents also have full **Pane Lifecycle Management** authority, capable of spawning new sibling panes or tabs (passing initial intents), injecting raw keystrokes (Esc, Ctrl+C, etc.) into peer terminals, and closing active panes dynamically without IPC overhead.

### 6. `boxxy-vte` (Library Crate)
Headless pure-Rust terminal emulator. Renders via GSK Snapshot and supports Kittygraphics natively. OSC 7/8/133 support. Features native semantic prompt tracking (`Flags::SEMANTIC_*`) embedded directly into the terminal cell grid to provide structured context blocks (`[PROMPT]`, `[COMMAND]`, `[OUTPUT]`).

### 7. `boxxy-ai-core` (Library Crate)
Unified AI interface layer. Abstracts multiple providers (Gemini, Anthropic, Ollama) behind a single `BoxxyAgent` interface. Manages `AiCredentials` mapping and the global multi-threaded Tokio runtime.

### 8. `boxxy-preferences` (Library Crate)
Settings management using an `AdwNavigationSplitView` architecture. UI is defined in `resources/ui/preferences.ui` and supports real-time search filtering.

### 9. `boxxy-model-selection` (Library Crate)
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

To prevent "split-brain" states between the Terminal Popover, the Sidebar, and the LLM itself, Boxxy strictly adheres to the following UI/Agent sync protocol:

1. **Single Source of Truth:** The `ClawSession` actor in the background is the ultimate source of truth for the state of a proposal.
2. **Explicit Resolution:** Every proposal *must* be resolved via `ClawMessage` as either:
   - `Approved / Executed`
   - `CancelPending` (via the Reject button, hitting Escape, or the user manually typing in the terminal to "ghost dismiss").
   - `UserMessage` (providing feedback instead of accepting/rejecting).
3. **The "Silent Reject" Pattern:** When a user explicitly rejects a proposal (via `CancelPending`), the LLM receives an error. To prevent the LLM from chatty, unnecessary follow-ups (e.g. "I'm sorry you didn't like that!"), the system dictates a `[SILENT_ACK]` flow. If the agent yields a `[SILENT_ACK]` token, the UI silently drops the event and returns the agent to a sleep state without prompting the user further.
4. **Bi-directional State Events:** When the engine resolves or drops a proposal, it emits a `ProposalResolved` (or similar cleanup) event. Both the Terminal Popover and the Sidebar listen to this event, ensuring that rejecting a proposal in one view instantly updates and hides the UI in the other view.

## Development Protocol
- **MCP:** Use Context7 MCP for all library documentation and code generation.
- **Git:** NEVER automatically commit changes. Commits must be performed manually.
- **Documentation:** Keep this file and crate-level `AGENTS.md` files updated with all architectural changes.
