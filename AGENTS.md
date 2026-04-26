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

### 5. Technical Integrity & Testing
- **Database Logic:** ALL database operations MUST have corresponding unit tests in their respective crates (using `Db::new_in_memory()`). NEVER assume a query is correct without verification.
- **Memory Hygiene:** The "Background Observer" must be strictly tuned to exclude transient state. Implicit memories default to `unverified` and require manual promotion in `MEMORY.md`.
- **Validation:** Changes to core engine logic must be verified via `cargo test` and, where applicable, by adding a new YAML scenario to `scenario-runner`.

## Component Responsibilities

### 1. `boxxy-app` (Binary Crate)
Entry point. Initializes GTK/Libadwaita, registers GResources, bootstraps the main window, and ensures clean process termination.

### 2. `boxxy-window` (Library Crate)
Main UI Orchestrator using a modular MVU pattern. Manages the `AdwApplicationWindow`, tabs, and global state routing via `state.rs`, `ui.rs`, and the `update/` module. Acts as the primary orchestrator for the **Global Workspace Radar**, ensuring peer-to-peer agent discovery and global intent propagation across all windows. Respects the **"Lightweight First"** configuration by initializing agents in an inactive state unless `claw_on_by_default` is enabled. It monitors the active terminal pane to reflect its specific Claw mode state.

### 3. `boxxy-terminal` (Library Crate)
Manages the split-pane terminal environment. Features a deep modular architecture (`pane/`) handling UI, gestures, events, and media previews. Claw UI (drawer, indicator, merged msgbar, event dispatch) does **not** live here — it lives in `boxxy-claw-widget`. The terminal provides `PaneClawHost` as a thin adapter that implements `boxxy_claw_widget::ClawHost` so the drawer can drive the pane without depending on `TerminalWidget` or `PaneInner`.

### 4. `boxxy-agent` (Binary/Library Crate)
Host-privileged daemon that sits between the sandboxed UI and the operating system. Exposes four D-Bus interfaces on the session bus (`Agent`, `Pty`, `Claw`, `Maintenance`) from a single process. Hosts `ClawSession` actors (per-pane AI reasoning), owns the PTY registry (viewer ref-count, persistence flag, 4 MB ring buffer per detached session, zombie-sweep on a 4 h TTL), and runs the background Dream Cycle at `niceness 19` gated on UPower + ghost-mode (see §"Memory Consolidation" below). Agent identity is persisted to disk (`$XDG_DATA_HOME/boxxy-terminal/agent-registry.json`) so a pane keeps the same petname across restarts. The binary also provides CLI subcommands — `start`, `stop`, `restart`, `list-sessions` — used by the UI's updater and for manual inspection.

### 5. `boxxy-claw` (Library Crate)
Agentic Reasoning Engine using an **Actor Model**. Spawns isolated `ClawSession` actors per terminal pane. Features the **"Red Pony Protocol"**: each pane is assigned a unique mnemonic name (e.g., "Red Pony") mapped to its UUID.

Agents function as a **Collaborative Swarm**:
- **Event-Driven Pub/Sub**: Upgraded with a central **EventBus** in the Workspace Radar. Agents can use `subscribe_to_pane` to passively monitor peers for process exits or terminal output matches (regex) without consuming tokens.
- **Async Promises (Map-Reduce)**: Supports complex fanned-out workflows via `delegate_task_async` and `await_tasks`. A "Leader" agent can suspend its execution (0 tokens) until all child tasks return a result.
- **Resource Locking**: Employs a global **LockTable** to prevent race conditions. Agents can use `acquire_lock(path)` to safely refactor shared codebases, blocking peers from modifying the same resource until released.
- **Capability Routing (Service Mesh)**: Automatically routes requests to specialized experts (e.g., "rust-expert") based on active skills and working directory via the `request_assistance` tool.
- **Autonomous Suspension**: Implements a dedicated **Sleep Mode**. Agents automatically pause their reasoning loop while waiting for external events or sub-tasks, optimizing both cost and system resources.

**Error Handling (Origin-Aware)**: When a shell command finishes with a non-zero exit, the engine reacts based on **who wrote the command**. Agent-originated commands (via `ProposeTerminalCommand` tool or `InjectCommand` event that the user accepted) auto-diagnose — the agent owns the fix, drawer opens, `DiagnosisComplete` fires. User-typed commands (typos, wrong flags, anything the agent didn't propose) emit only a passive `LazyErrorIndicator` — a small red "Error Detected" pill appears at the pane corner. **Zero tokens are spent until the user clicks the pill**, at which point the widget sends `RequestLazyDiagnosis` and the same diagnosis pipeline runs on demand. Sleep mode short-circuits both paths. Non-error exit codes that legitimately signal "nothing matched" (`grep`, `diff`, `test`, `[`) are filtered before the split.

Agents possess full **System & Environment Authority**:
- **Location & Time Context Injection**: Agents are implicitly aware of the user's geographic location (city, country, timezone) and precise local time without requiring tool calls, achieved via a silent background fetch (`ip-api.com`) and prompt injection. If disabled by the user, a strict `[PRIVACY POLICY]` is injected forbidding the agent from attempting to deduce location or time via shell commands.
- **Persistent Interaction History**: Automatically saves visual events (diagnoses, suggestions, tool results) and **turn-based token usage** to the database. These are re-rendered instantly upon session restoration, ensuring zero context loss even for sidebar logs. Session titles are dynamically generated in the background using a dedicated LLM call summarizing the user's initial prompt.
- **Memory Consolidation ("Dreaming")**: A background orchestration pipeline that combats context bloat. Lives on the daemon side so it outlives the UI and can be safety-gated.
  - **Phase 1 (Light Sleep):** Ingests raw interaction logs from the SQLite database.
  - **Phase 2 (Deep Sleep):** A dedicated LLM parses the logs, extracting durable facts (OS, hardware, preferred tools) and behavioral patterns, resolving semantic conflicts before promoting them to long-term memory.
  - **Phase 3 (REM):** Syncs insights to `MEMORY.md` and generates a human-readable `DREAMS.md` log.
  - **Safety envelope:** runs at `niceness 19`, pauses on battery (via UPower), pauses while any UI is attached (ghost-mode gate), honours the `enable_auto_dreaming` setting, warm-up delay of 10 s after daemon start, 15-minute cool-down between cycles. Current status is readable via `Maintenance.GetMaintenanceStatus()` on D-Bus (`idle`/`disabled`/`paused_on_battery`/`paused_ui_attached`/`running`).
- **Concurrency-Safe Hot-Swapping**: Users can change AI providers (e.g. Gemini to Claude), update API keys, or toggle tool authorizations in Settings at any time. Active sessions will safely drop and transparently rebuild their underlying agents on the next turn, retaining conversation history and automatically sanitizing incompatible payload formats without crashing the pane.
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

The **Characters page** (`src/characters/mod.rs`) lists all installed characters (avatar, display name, duties, color swatch) loaded directly from disk via `character_loader::load_characters()`. It provides two management actions: "Open Characters Folder" (launches the file manager at the characters dir) and "Reset to Defaults" (double-confirmation destructive action that calls `character_loader::reset_to_defaults()`). Changes to character files only take effect after restarting the `boxxy-agent` daemon, since the registry is seeded at daemon startup. The color swatch is a `gtk::Button` rendered with a per-instance CSS class (`pref-color-dot-{n}`) using `background-image: none` to suppress libadwaita gradient overrides.

### 10. `boxxy-claw-widget` (Library Crate)
The reusable Claw drawer UI. Owns every widget the user sees when interacting with an agent in a pane: the slide-down overlay drawer, the floating `ClawIndicator` badge, the merged input bar (formerly the standalone `boxxy-msgbar` crate), the neutral `Proposal` enum, and the event-dispatch loop that turns `ClawEngineEvent`s into widget mutations.

The crate is surface-agnostic: it talks to its host through the `ClawHost` trait (inject a line, execute a script, grab focus, ask for scrollback, send a `ClawMessage`, forward an event, etc.). Today the terminal pane is the only implementor via `PaneClawHost`, but any future surface (standalone chat window, mobile shell) can drive the same UI by implementing the trait.

Key modules:
- `overlay.rs` — the drawer (`TerminalOverlay`). Loaded from `resources/ui/claw_overlay.ui`. Hosts the merged msgbar inside its bottom area; diagnosis / proposal / bookmark forms above; optional scrollable conversation history (toggled by `maintain_overlay_history` preference).
- `claw_indicator.rs` — the floating pill that shows the agent's identity + status. The colored badge only surfaces once a real agent name is known — no placeholder "CLAW" text over empty panes. On `set_identity`, it looks up the character by UUID (falling back to internal name, then display name) in `CHARACTER_CACHE` and applies the character's CSS hex color to the badge. Unknown characters (e.g., a session resumed after the original character was deleted or replaced) migrate to the first available character in the registry rather than showing a permanent grey fallback.
- `msgbar/` — the input bar that lives inside the drawer. Carries attachments, autocomplete (`@agent`, `/resume`), history nav, Ctrl+V paste, and the four status toggles (claw/sleep/pin/web-search). The send button is a sibling widget, not a child of the bar, so the bar can render as a single rounded field with the send icon floating alongside.
- `dispatch.rs` — `spawn_dispatch(rx, host, overlay, indicator, msgbar, sidebar_store, …)`. The single event loop for a pane; every branch is either a widget mutation or a host-trait call.
- `claw_host.rs` — the `ClawHost` trait (the decoupling boundary).
- `proposal.rs` — `Proposal` enum (UI shape; converted from terminal-specific `TerminalProposal` via `.into()`).

Ctrl+/ reveals the full drawer and focuses the input — **never hides** it (the agent may be waiting for a message, silent dismissal would strand the user). The drawer's Okay/Reject/Accept buttons are the explicit dismiss paths.

The msgbar's **Unified Status Indicator**:
- The bot icon on the far left serves as both the **Claw Toggle** and a **Real-time Status Monitor**.
- It dynamically reflects the agent's current mode:
    - 🦖 **Waiting**: Awake, full context, and ready to respond to errors or queries.
    - 🧠 **Working**: Busy processing an LLM turn or executing a tool.
    - 💤 **Sleep**: Suspended. Acting as a passive background observer.
    - 🔒 **Locking**: Holding a global resource lock via an active Lease.
    - ⚠️ **Faulted**: Stuck in an unrecoverable timeout or crash state.
    - ⚪ **Off**: Claw is deactivated for this pane.

### 11. `boxxy-model-selection` (Library Crate)
Data-driven model configuration UI. Uses a registry pattern to dynamically build selection dialogs and dropdowns based on registered `AiProvider` traits. Decouples AI capability discovery from the main application window.

### 12. `boxxy-claw-protocol` (Library Crate)
Pure-data DTO layer shared between the UI and the daemon across the D-Bus boundary. Defines `ClawMessage`, `ClawEngineEvent`, `PersistentClawRow`, `AgentStatus`, `ScheduledTask`, `UsageWrapper`, and the `ClawEnvironment` trait. Contains no GTK types, no reasoning-engine internals — every field serialises cleanly via serde. Both `boxxy-claw` and `boxxy-claw-ui` depend on it; neither depends on the other.

Also owns the **Character system** types and loader:
- `characters.rs` — `CharacterConfig` (id, name, display_name, color, duties, personality), `CharacterInfo` (config + status + has_avatar), `CharacterStatus` (Available / Active { pane_id }), `OrphanGroup` (for sessions whose character was deleted). Exposes a global lock-free `CHARACTER_CACHE: ArcSwap<Vec<CharacterInfo>>` and a `CHARACTER_CACHE_VERSION: AtomicU64` counter that UI components poll to detect registry changes without subscribing to D-Bus signals directly.
- `character_loader.rs` — disk I/O for the character registry. Characters live at `~/.config/boxxy-terminal/boxxyclaw/characters/<slug>/CHARACTER.toml` (+ optional `AVATAR.png`). Key functions: `load_characters()` (reads all dirs, auto-generates missing UUIDs, deduplicates collisions, sorts by `characters.json` order), `ensure_default_character()` (extracts bundled defaults on first run), `reset_to_defaults()` (wipes all dirs and re-extracts). The three bundled defaults (niko-una, levi-kujo, kuro) are embedded at compile time via `include_bytes!` from `resources/characters/`.

### 13. `boxxy-claw-ui` (Library Crate)
All GTK widgets for the Claw sidebar page (right-hand `ViewStack` tab). Provides `ClawSidebarComponent` (the outer shell: status page, token counter, pending-tasks drawer, "Clear" button) and `create_claw_message_list` (the per-pane virtual list of diagnosis / suggestion / tool-call / process-list / user-message rows). Rows are recycled via `boxxy_core_widgets::ObjectExtSafe` so long histories stay cheap. Strictly display-only — the `add_*_approval_row` helpers format proposals as Diagnosis rows and deliberately ignore their callback args.

### 14. `boxxy-mcp` (Library Crate)
Implements the Model Context Protocol (MCP) using `rmcp`. It provides a `McpClientManager` to orchestrate connections via Stdio and HTTP(SSE/Streamable). Features dynamic tool ingestion, bridging JSON Schema Draft 7 to `rig` via `DynamicMcpTool`, and employs a "Lazy Boot" caching strategy to prevent slow startup times when configuring heavy Node.js MCP servers.

### 15. `boxxy-telemetry` (Library Crate)
Provides a privacy-first observability layer for tracking agent and system usage.
- **Durable SQLite Journaling:** UI processes (`boxxy-app`) write telemetry events instantly to a local `telemetry_journal` SQLite table, ensuring zero UI lag and zero data loss on abrupt shutdowns.
- **Agent-Delegated Flushing:** The background `boxxy-agent` daemon periodically drains this local journal and exports the metrics to the Supabase backend using the OpenTelemetry (OTLP) SDK.

## Distribution & Updates

Boxxy supports three primary distribution channels:

1. **Flatpak (Flathub):** The primary sandboxed distribution. Updates are managed externally by the Flatpak system.
2. **Native (GitHub Nightly):** A standalone installation method via `scripts/install.sh` that targets `~/.local/boxxy-terminal/`. 
3. **System Package Managers (Nix, Arch AUR, etc.):** For environments where a system package manager handles updates. Maintainers should compile with the `disable-self-update` Cargo feature (`cargo build --release --features disable-self-update`) to strip out the internal update logic and related dependencies.

### Auto-Update Protocol (Native Only)
For native installations, Boxxy implements an **Atomic Swap** update mechanism located in `boxxy_window::updater`:
- **Detection:** Uses `can_self_update()` to safely disable the internal updater when sandboxed via Flatpak or when explicitly compiled out via the `disable-self-update` feature.
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
- **Git (CRITICAL):** NEVER COMMIT. The AI must NEVER automatically commit changes or push to any branch. Commits and Git operations will be performed manually by the user ONLY.
- **MCP:** Use Context7 MCP for all library documentation and code generation.
- **GTK UI Files**: When modifying `.ui` files (XML), avoid using the ampersand character (`&`) in text labels (like `title` or `subtitle` properties) if possible. GTK's Pango markup parser will often fail to render the text if the string contains a raw ampersand after XML parsing. Prefer using the word "and".
- **Documentation:** Keep this file and crate-level `AGENTS.md` files updated with all architectural changes.
