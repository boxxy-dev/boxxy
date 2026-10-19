# Boxxy-Terminal Agents & Architecture

## Observability & Debugging

Boxxy provides deep, real-time visibility into LLM interactions for developers and power users:
- **Cumulative Token & Cost Analytics:** Every session tracks active context window tokens, model context window percentage (`% used`), cumulative tokens, and cumulative USD cost (`$ spent`) via `rig::catalog::Catalog::builtin()`, persisted in `boxxy-db` and re-rendered during session resumption.
- **Dedicated Context Debugging:** To audit the exact payloads (System Prompt, History, Tools) sent to models, run the application with:
  ```bash
  BOXXY_DEBUG_CONTEXT=1 cargo run
  ```
  This is a strictly opt-in, high-signal logging mode that works in both debug and release builds (using `log::info!` on the `model-context` target), ensuring zero noise during standard development.
- **Config Root Override:** All persistent state (settings, DB, characters, skills, memory, bookmarks) resolves through `boxxy_sys_utils::get_config_dir()`. Native installs use `~/.config/boxxy-terminal/`; Flatpak uses `~/.var/app/dev.boxxy.BoxxyTerminal/config/boxxy-terminal/`. To override (tests, sandbox simulation), set `BOXXY_CONFIG_DIR=<absolute-path>` — the UI propagates it to the host daemon via `flatpak-spawn --env=`. Relative or whitespace-only values are rejected with a warning.
- **Visual Sidebar Status:** The Claw sidebar displays active session status, configured models (Claw / Dreams) and active search engine, context usage analytics, MCP server status, pending tasks, and a temporary history clear button.

## Technology Stack
- **Language:** Rust 2024 (v1.98+)
- **Concurrency:** tokio v1.54, async-channel v2.5
- **UI Toolkit:** GTK 4.22 + Libadwaita 1.90 (via `gtk4-rs` 0.11)
- **Terminal Engine:** Custom async-first lock-free ANSI state machine. Optimized for Rust 1.98+ with `core::hint::cold_path` and safe stack-allocated OSC dispatch, eliminating legacy `unsafe` pointer casts.

## Coding Guidelines

### 1. Rust Code Style
- **Inline Comments:** Use short, concise inline comments to explain *why* a particular approach was taken, especially for complex async flows, GTK property bindings, or state machine transitions. Avoid over-commenting obvious syntax. Maintain existing comments, updating them only if they become obsolete.

### 2. Modularity & Scoping
- **File Length Guideline (~700 LOC):** Aim to keep source files around **700 lines of code** when natural domain boundaries exist. However, **cohesion and clarity always trump strict line counts** — do NOT artificially split a well-defined, single-responsibility file (e.g., an 800–950 line component or state machine) just to satisfy a line number if doing so fragments related logic or creates a mess. Refactor only when a file genuinely mixes distinct responsibilities.
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
- **No Hardcoded Local Paths or Assets (CRITICAL):** NEVER use hardcoded local paths (e.g., `/home/user/...`), machine-specific directories, or local user files/images in unit tests, integration tests, or application code. Tests must always use synthetic in-memory data (e.g., dynamically generated `image::RgbImage` buffers), `tempfile::tempdir()`, or generic RFC-2606 URLs (`https://example.com/...`).
- **Database Logic:** ALL database operations MUST have corresponding unit tests in their respective crates (using `Db::new_in_memory()`). NEVER assume a query is correct without verification.
- **Memory Hygiene:** The system employs a threshold-based reinforcement model to prevent hallucinations. Implicit memories (per-turn extractions) are stored as `verified: false` candidates and surfaced as `[PROVISIONAL]` recall until reinforced. To ensure environmental awareness, core system facts (OS, shell, package manager) are unconditionally injected into the context via bootstrap recall. Additionally, past interactions are labeled as unverified context to prevent stored prompt injections from influencing agent behavior. A background hygiene pass automatically prunes noise entries (unverified candidates observed only once and not updated in 14 days).
- **Validation:** Changes to core engine logic must be verified via `cargo test` and, where applicable, by adding a new YAML scenario to `scenario-runner`.

## Component Responsibilities

### 1. `boxxy-app` (Binary Crate)
Entry point. Initializes GTK/Libadwaita, registers GResources, bootstraps the main window, and ensures clean process termination.

### 2. `boxxy-window` (Library Crate)
Main UI Orchestrator using a modular MVU pattern. Manages the `AdwApplicationWindow`, tabs, and global state routing via `state.rs`, `ui.rs`, and the `update/` module. Acts as the primary orchestrator for the **Global Workspace Radar**, ensuring peer-to-peer agent discovery and global intent propagation across all windows. It monitors the active terminal pane to reflect its specific Claw mode state. It provides the **Primary Notification Surface** for agent events, emitting rich desktop alerts (character avatars, first-name attribution, and response summaries) for timers and long-running tasks, while intelligently suppressing redundant in-app toasts for task completions already visible in the overlay.

**Frosted Glass:** `widgets/frosted_glass.rs` wraps the whole sidebar/content split view. It filters only the backdrop paste, then paints a flat tint; native Libadwaita window clipping owns corners and CSD offsets. Keep the blur radius at 40 for GTK 4.25's radius-to-sigma conversion. See `DEV/plans/frosted_glass_blur.md` for validation status and compositor diagnostics.

### 3. `boxxy-terminal` (Library Crate)
Manages the split-pane terminal environment. Features a deep modular architecture (`pane/`) handling UI, gestures, events, and media previews. Claw UI (drawer, indicator, merged msgbar, event dispatch) does **not** live here — it lives in `boxxy-claw-widget`. The terminal provides `PaneClawHost` as a thin adapter that implements `boxxy_claw_widget::ClawHost` so the drawer can drive the pane without depending on `TerminalWidget` or `PaneInner`.

### 4. `boxxy-agent` (Binary/Library Crate)
Host-privileged daemon that sits between the sandboxed UI and the operating system. Exposes four D-Bus interfaces on the session bus (`Agent`, `Pty`, `Claw`, `Maintenance`) from a single process. The `Agent` interface exposes `GetVersion()` and `GetConfigDir()` — on startup the UI compares both to its own expectations and respawns the daemon on mismatch, preventing a stale host process from reading or writing a divergent config tree. It is the **single source of truth for character assignments**, maintaining a volatile in-memory `CharacterRegistry` of active claims keyed by `holder_id`. Claims are automatically released when a UI process disconnects or crashes via `ipc/client_tracker.rs`, which subscribes to `org.freedesktop.DBus.NameOwnerChanged` and calls `release_owner(bus_name)` on disconnection, cascading swarm lock cleanup for any `HolderKind::Pane` claims. Orphan character migration (sessions referencing a deleted character UUID) is handled both at daemon startup and at runtime during catalog hot-reloads by mapping them to the first available catalog entry. Hosts `ClawSession` actors (per-pane AI reasoning), owns the PTY registry (viewer ref-count, persistence flag, 4 MB ring buffer per detached session, zombie-sweep on a 4 h TTL), and runs the background Dream Cycle at `niceness 19` gated on UPower + ghost-mode (see §"Memory Consolidation" below). The binary also provides CLI subcommands — `start`, `stop`, `restart`, `list-sessions` — used by the UI's updater and for manual inspection.

The `Claw` D-Bus interface also exposes `spawn_detached(command, cwd) → pid` for launching GUI apps, servers, and scripts completely detached from the daemon's process group (via `setsid()`). A 250ms early-crash window catches "command not found" failures before returning the PID. The multiplexer env-scrubbing below is **not** applied to detached spawns — they inherit the daemon's full host environment so `DISPLAY`/`WAYLAND_DISPLAY` are present and GUI apps render correctly.

**PTY Environment Scrubbing:** On `SpawnPty`, the daemon surgically removes inherited multiplexer identity vars (`TMUX*`, `ZELLIJ*`, `KITTY_*`, `ALACRITTY_*`, `VSCODE_*`, `STY`, `WINDOW`, `WT_SESSION`) via `env_remove`. `env_clear()` is explicitly forbidden — in Flatpak mode the UI passes only terminal-identity vars and the daemon's host environment is the sole source of `PATH`, `DISPLAY`, etc.

The daemon runs a **Character Catalog Watcher** (`character_watcher.rs`) that monitors `~/.config/boxxy-terminal/boxxyclaw/characters/` for filesystem changes using `notify-debouncer-mini` with a 500 ms debounce. On any `.toml`, `.png`, or `.json` modification, it calls `CharacterRegistry::reload_catalog()`, which: reloads the catalog from disk; computes a duties+personality hash to identify changed characters; migrates orphaned claims to the first available character; bumps the registry revision and broadcasts a `claims_changed` D-Bus signal. Sessions whose character's personality changed receive a `SettingsInvalidated` message (matched by `character_id`); sessions whose character was deleted and remapped receive it via their `holder_id` — keeping the two cases separate to avoid spurious rebuilds of unrelated sessions. Changes to character `duties` or `personality` take effect on the very next agent turn without restarting the daemon or UI.

### 5. `boxxy-claw` (Library Crate)
Agentic Reasoning Engine using an **Actor Model**. Spawns isolated `ClawSession` actors per terminal pane. Features the **"Red Pony Protocol"**: each pane's `CharacterClaim` carries a stable two-word `petname` (e.g., "red-pony") minted by the `petname` crate at claim time; swarm tools address peers by petname, character display name, or holder ID via `resolve_peer`.

The per-turn reasoning pipeline lives in `engine/turn/` and is split across four focused sub-modules (per the 700-line file limit):
- `turn/history.rs` — `prepare_query_history`: strips stale dynamic context from old messages (retaining the last 4 user turns), pops the current user message to avoid duplication, and injects synthetic `[Previous task was interrupted by the user]` assistant placeholders between any consecutive user messages (required for Anthropic/Gemini alternation compliance).
- `turn/skills.rs` — semantic + keyword skill injection into the turn context using the 0ms local FTS5 tokenizer (`build_fts5_query`).
- `turn/snapshot.rs` — hash-based snapshot deduplication (emits `[TERMINAL_SNAPSHOT: NO_CHANGE]` when the terminal hasn't changed, saving tokens).
- `turn/persistence.rs` — DB upsert, first-turn background title generation, and sliding-window memory compaction. Uses `rig_memory::SlidingWindowMemory` + `rig::memory::DemotionHook` (`DemotionExtractor`) to batch-flush evicted conversation segments into SQLite (`flush_history`), paired with lifecycle flushes on `Sleep`, `Deactivate`, and `Evict` so regular turns incur **zero** background LLM calls.

**Interrupt Continuity ("Amnesia Fix"):** The final `messages` array returned by `agent.chat().await` (containing the user prompt, tool calls, and assistant response) is synchronized back to `SessionState.history`. If the turn is cancelled (tokio task abort on a new `UserMessage`), the original intent is still handled. The `prepare_query_history` function detects and bridges any consecutive user messages with a synthetic assistant acknowledgment on the next turn.

Agents function as a **Collaborative Swarm**:
- **Event-Driven Pub/Sub**: Upgraded with a central **EventBus** in the Workspace Radar. Agents can use `subscribe_to_pane` to passively monitor peers for process exits or terminal output matches (regex) without consuming tokens.
- **Async Promises (Map-Reduce)**: Supports complex fanned-out workflows via `delegate_task_async` and `await_tasks`. A "Leader" agent can suspend its execution (0 tokens) until all child tasks return a result.
- **Resource Locking**: Employs a global **LockTable** to prevent race conditions. Agents can use `acquire_lock(path)` to safely refactor shared codebases, blocking peers from modifying the same resource until released.
- **Capability Routing (Service Mesh)**: Automatically routes requests to specialized experts (e.g., "rust-expert") based on active skills and working directory via the `request_assistance` tool.
- **Autonomous Suspension**: Implements a dedicated **Sleep Mode**. Agents automatically pause their reasoning loop while waiting for external events or sub-tasks, optimizing both cost and system resources.

**Error Handling (Origin-Aware)**: When a shell command finishes with a non-zero exit, the engine reacts based on **who wrote the command**. Agent-originated commands (via `ProposeTerminalCommand` tool or `InjectCommand` event that the user accepted) auto-diagnose — the agent owns the fix, drawer opens, `DiagnosisComplete` fires. User-typed commands (typos, wrong flags, anything the agent didn't propose) emit only a passive `LazyErrorIndicator` — a small red "Error Detected" pill appears at the pane corner. **Zero tokens are spent until the user clicks the pill**, at which point the widget sends `RequestLazyDiagnosis` and the same diagnosis pipeline runs on demand. Sleep mode short-circuits both paths. Non-error exit codes that legitimately signal "nothing matched" (`grep`, `diff`, `test`, `[`) are filtered before the split.

**Turn Completion Alerts**: The engine tracks turn duration; any turn exceeding **7 seconds** triggers a `LongTaskCompleted` event. Reminders and long tasks carry dynamic payloads (the reminder text or a truncated response summary) which are routed to the UI for distinct audio cues (`timer.wav` vs `task.wav`) and global desktop notifications.

Agents possess full **System & Environment Authority**:
- **Character Personality Injection**: Each agent session builds its system prompt (preamble) by loading `claw.md` (base instructions and critical rules), appending the character's `duties` and `personality` from their `CHARACTER.toml` file, and **injecting the user's name** from Preferences (if set) under a `[USER INFO]` block. This ensures the model knows how to address the user. This is followed by long-term memory. The preamble is constructed once per session (or after a hot-swap) and cached — subsequent turns within a session carry no additional personality token cost. The `{{available_skills}}` placeholder in `claw.md` is stripped at build time; active skills are injected per-turn into the user message context instead.
- **Location & Time Context Injection**: Agents are implicitly aware of the user's geographic location (city, country, timezone) and precise local time without requiring tool calls, achieved via a silent background fetch (`ip-api.com`) and prompt injection. If disabled by the user, a strict `[PRIVACY POLICY]` is injected forbidding the agent from attempting to deduce location or time via shell commands.
- **Persistent Interaction History**: Automatically saves visual events (diagnoses, suggestions, tool results) and **turn-based token usage** to the database. These are re-rendered instantly upon session restoration, ensuring zero context loss even for sidebar logs. Session titles are dynamically generated in the background using a dedicated LLM call summarizing the user's initial prompt.
- **Memory Consolidation ("Dreaming")**: A background orchestration pipeline that combats context bloat using an OpenClaw-inspired reinforcement model. Lives on the daemon side so it outlives the UI and can be safety-gated.
  - **Hot-Path Recall (0ms Local FTS5):** Pre-turn memory and skill retrieval uses a deterministic Rust FTS5 BM25 tokenizer (`build_fts5_query`) that strips stopwords and quotes prefix tokens (`"token"*`) with **zero LLM calls** on the critical path.
  - **Batch Flush on Eviction / Sleep / Close (`DemotionHook`):** Regular turns make **zero** background memory LLM calls (only Turn 1 spawns a lightweight session title generator). Instead, `SessionState` tracks a `flushed_watermark` and flushes conversation segments in batches via `flush_history` when: (1) `SlidingWindowMemory::last_messages(10)` evicts older messages at the 15-message boundary, or (2) the session transitions to `Sleep`, `Deactivate`, or `Evict`. Each flush extracts candidate facts (`confidence: 0.5`) and a 1-sentence episodic summary, deduplicated against recent interactions via **0-token local Jaccard word similarity** (`jaccard_similarity >= 0.70`).
  - **Phase 1 (Light Sleep):** Ingests flushed interaction logs and candidate facts from `boxxy-db`.
  - **Phase 2 (Deep Sleep):** Automatically promotes facts to the `dreamed` category (`verified: true`) only if they cross strict statistical thresholds: `observation_count >= 2` and `confidence_score >= 0.8`.
  - **Phase 3 (Conflict Resolution):** Identifies contradictions between recent logs and long-term memory. Conflicts trigger a demotion of the old fact (back to `candidate`, count reset to 0) and a high-confidence re-candidacy for the corrected fact.
  - **Protection Tiers:** User-manually edited memories (`manual_sync`) and explicit agent tool-calls (`preference`) are strictly protected and cannot be demoted or overwritten by the automated dreaming system.
  - **Auditable Logs:** Syncs insights to `MEMORY.md` and generates an auditable `DREAMS.md` log, including 🛡️ Verified status lines for every automatically promoted fact.
  - **Safety envelope:** runs at `niceness 19`, pauses on battery (via UPower), pauses while any UI is attached (ghost-mode gate), honours the `enable_auto_dreaming` setting, warm-up delay of 10 s after daemon start, 15-minute cool-down between cycles. Current status is readable via `Maintenance.GetMaintenanceStatus()` on D-Bus (`idle`/`disabled`/`paused_on_battery`/`paused_ui_attached`/`running`).
- **Concurrency-Safe Hot-Swapping**: Users can change AI providers (e.g. Gemini to Claude), update API keys, or toggle tool authorizations in Settings at any time. Active sessions will safely drop and transparently rebuild their underlying agents on the next turn, retaining conversation history and automatically sanitizing incompatible payload formats without crashing the pane.
- **Cross-Model Continuity**: Cumulative session analytics (Total Tokens) are persisted in the database, allowing users to switch LLM providers (e.g., Gemini to Claude) while maintaining a continuous record of the session's overall cost and context depth.
- **Temporary Clear Pattern**: Clicking the Clear button in the Claw sidebar temporarily clears the visible history from the main overlay view for the current session without modifying the database. Toggling "Maintain Session History on Overlay" off and on again, or resuming the session via `/resume`, restores the full conversation history.
- **Clipboard Management**: Securely read and write to the system clipboard with user approval.
- **Process Inspection & Control**: View real-time process lists and terminate misbehaving tasks via in-terminal agent popovers and read-only sidebar tables.
- **Pane Lifecycle Management**: Spawn new sibling panes/tabs, inject raw keystrokes (Esc, Ctrl+C), and close active panes dynamically.
- **Global Workspace Radar**: Discover all active peers and share high-level objectives via the **Global Intent Blackboard**.

### 6. `boxxy-vte` (Library Crate)
Headless pure-Rust terminal emulator. Renders via GSK Snapshot and supports Kittygraphics natively. OSC 7/8/133 support. Features native semantic prompt tracking (`Flags::SEMANTIC_*`) embedded directly into the terminal cell grid to provide structured context blocks (`[PROMPT]`, `[COMMAND]`, `[OUTPUT]`).

### 7. `boxxy-ai-core` (Library Crate)
Unified AI interface layer. Abstracts multiple providers (Gemini, Anthropic, OpenAI, DeepSeek, OpenRouter, Ollama) behind a single `BoxxyAgent` interface. Manages `AiCredentials` mapping and provider configuration. System utilities (runtime, Flatpak detection, location context) have been extracted to `boxxy-sys-utils`.

### 8. `boxxy-assets` (Library Crate)
Headless, synchronous asset processing pipeline with no GTK dependencies. CPU-bound; callers wrap entry points in `tokio::spawn_blocking`. Organized as sub-modules per asset type (`image/`, future `audio/`) sharing a common `AssetError`.

The **image pipeline** exposes a `Transformation` trait (implemented by `Resize` and `SquareCrop`) composed via the `Pipeline` builder, and an `Analyzer<O>` trait for future read-only passes. It provides two main entry points:
- `process_avatar(&[u8]) -> Result<AvatarOutput, AssetError>`, which validates input (file size, magic-byte format check, dimension bounds), center-crops to square, resizes to 256×256 (Lanczos3), and encodes to PNG bytes. Accepted formats: PNG, JPEG, WebP. Videos, GIFs, and unrecognised formats are rejected via `AssetError::UnsupportedFormat`. Max input dimension is 4096 px per axis (decompression-bomb guard).
- `generate_thumbnail(&[u8], ThumbnailSpec) -> Result<ThumbnailOutput, AssetError>`, which validates header dimensions before full decode (up to 50 MB / 16,384 px per axis / 64 MP) and downscales 4K/5K/8K images on a worker thread (`FilterType::Triangle`) into compact RGBA8 thumbnail buffers while preserving original resolution metadata for `boxxy-viewer`.

The **`user` module** provides `get_user_dir()` and `get_user_avatar_path()` which resolve to `~/.config/boxxy-terminal/user/AVATAR.png` — the canonical location for the user's own profile avatar, distinct from AI character directories.

### 9. `boxxy-sys-utils` (Library Crate)
Minimal, leaf-level utility crate for system environment detection and global async runtime management. Decouples system-level checks (Flatpak detection, self-updater availability) and the global Tokio runtime from the AI stack, allowing UI and terminal crates to remain lightweight. Provides the *mechanism* for location fetching; all *policy* (e.g., privacy toggles) lives in callers. Also owns `get_config_dir() -> PathBuf` — the single resolver every crate uses to locate the active config root. The resolver is infallible (env override → `ProjectDirs` → `$HOME/.config/boxxy-terminal` fallback), never caches, and validates `BOXXY_CONFIG_DIR` with `Path::is_absolute()` (no `canonicalize()` — must not perform I/O).

### 9. `boxxy-core-toolbox` (Library Crate)
Provides a structured library of high-level tools for Boxxy agents, completely decoupled from the reasoning engine. Includes:
- **Host Operations:** File management (`file_read` with 500-line default window, binary-file guard, and 64 KB slice cap; surgical `replace_in_file` with CRLF preservation and local `similar` 3.2.0 diff computation; `file_write` with existing-vs-new file detection and pre-approval diffing; `file_delete`; deterministic directory-first `list_directory` capped at 250 entries), fast workspace search (`glob_search` and regex `grep_search` on `spawn_blocking`, automatically skipping `.git`, `target`, `node_modules`, and binary files), process inspection (`list_processes` with `filter`, `sort_by` CPU/memory/IO/PID, and `limit` default 30 / max 100), system info (structured JSON via `spawn_blocking`), clipboard access, and **background process launching** (`RunBackgroundCommandTool`): spawns a detached, long-running process (GUI apps, dev servers, scripts) via the daemon's `spawn_detached` with optional/relative CWD resolution, 250ms early-crash detection, and user approval via the standard `propose_background_command` flow. Returns the PID; the agent can later manage it with the existing `KillProcessTool`.
- **Web/Network:** HTTP fetching (`http_fetch`) with a shared `reqwest::Client` connection pool, 10s timeouts, 1MB size limits, and Markdown-preserving HTML stripping.
- **Web Search:** A modular `SearchProvider` trait allowing real-time web queries (implemented via Tavily), paired with optional **Native Model Search** (`use_native_search`) in `boxxy-ai-core` that attaches built-in server-side web search (`googleSearch` on Gemini, `web_search` on OpenAI Responses, `web_search_20250305` on Anthropic) to both Boxxy Claw and AI Chat while automatically falling back to Tavily for non-native providers (Ollama, DeepSeek, OpenRouter). Both `http_fetch`, `web_search`, and `native_web_search` emit structured `ToolResult` rows in the Claw sidebar log.
- **Approval Protocol:** Uses the `ApprovalHandler` trait (including live `current_dir()` resolution across cached turns) to ensure dangerous actions (like `rm`, `kill`, file writes, or clipboard access) always prompt the user via the GTK UI before execution and return `[USER_EXPLICIT_REJECT]` on denial so `[SILENT_ACK]` drawer dismissal works uniformly. Every tool calls both `report_tool_started` (shows the tool name in the real-time indicator) and `report_tool_result` (emits the `ToolResult` sidebar row) to be fully visible in the UI.

### 10. `boxxy-preferences` (Library Crate)
Settings management using an `AdwNavigationSplitView` architecture. UI is defined in `resources/ui/preferences.ui` and supports real-time search filtering. Implements the **"Master Switch vs Local Toggle"** design pattern (e.g., for Web Search), separating global capability authorization from per-pane activation.

**Model Configuration State (`config.rs`):** Model slots (`ai_chat_model`, `claw_model`, `memory_model`) are stored as `Option<ModelProvider>` but validated through a centralized `ModelConfigState` enum (`Unconfigured`, `MissingApiKey { provider_name }`, `OllamaOffline`, `Ready(ModelProvider)`). The `Settings::resolve_model_state(&self, model: &Option<ModelProvider>) -> ModelConfigState` method is the single source of truth for whether a slot is usable — it consults `boxxy_model_selection::get_providers()` for `requires_api_key()` so the registry drives key-requirement logic rather than a hardcoded list. `check_and_unset_invalid_models()` delegates to `resolve_model_state` for all three slots. The **API Keys page** (`src/apis/mod.rs`) uses `resolve_model_state` to show provider-specific subtitles (e.g., "Gemini API key is missing." or "Ollama is offline.") and subscribes to `SETTINGS_EVENT_BUS` for real-time status updates when models change in the selector dialog.

The **Characters page** (`src/characters/mod.rs`) has two sections. At the top, the **"You"** group (`src/user_profile.rs`) displays the user's own identity: an `adw::Avatar` (showing `~/.config/boxxy-terminal/user/AVATAR.png` if present, otherwise initials), an entry for the name the model should call you ("What the model calls you"), and a color picker. Avatar uploads run through `boxxy_assets::image::process_avatar` via `glib::spawn_future_local` + `tokio::spawn_blocking` and write the result to `user/AVATAR.png`. The `UserProfile { display_name, color: Option<String> }` struct is persisted in `settings.json`; `color: None` means a hardcoded default is applied, `Some(hex)` means the user has chosen manually. Below that, the **"Available Characters"** group lists all AI characters sourced from `CHARACTER_CACHE` (the live `ArcSwap` catalog), each with an avatar upload button that writes to `<char_dir>/AVATAR.png` — the daemon's `CharacterWatcher` (500 ms debounce) detects the change and reloads automatically. Management actions: "Open Characters Folder" and "Reset to Defaults" (double-confirmation destructive). The page polls `CHARACTER_CACHE` every 500 ms. The color swatch is a `gtk::Button` rendered with a per-instance CSS class (`pref-color-dot-{n}`) using `background-image: none` to suppress libadwaita gradient overrides.

### 11. `boxxy-claw-widget` (Library Crate)
The reusable Claw drawer UI. Owns every widget the user sees when interacting with an agent in a pane: the slide-down overlay drawer, the floating `ClawIndicator` badge, the merged input bar (formerly the standalone `boxxy-msgbar` crate), the neutral `Proposal` enum, and the event-dispatch loop that turns `ClawEngineEvent`s into widget mutations.

The crate is surface-agnostic: it talks to its host through the `ClawHost` trait (inject a line, execute a script, grab focus, ask for scrollback, send a `ClawMessage`, forward an event, etc.). Today the terminal pane is the only implementor via `PaneClawHost`, but any future surface (standalone chat window, mobile shell) can drive the same UI by implementing the trait.

Key modules:
- `state.rs` — defines the explicit `OverlayState` enum (`Idle`, `Thinking`, `Pending`, `Action`) that drives the drawer's visual lifecycle. This eliminates implicit boolean states and provides a robust, single-source-of-truth state machine for the UI, easily accommodating fire-and-forget tasks like background commands via the `Pending` state.
- `overlay.rs` — the drawer (`TerminalOverlay`). Loaded from `resources/ui/claw_overlay.ui`. Hosts the merged msgbar inside its bottom area; diagnosis / proposal / bookmark forms above; optional scrollable conversation history (toggled by `maintain_overlay_history` preference). It also manages a `TipsCycle` which displays rotating usage hints centered in the action row while the agent is thinking or working in the background, providing a polished UX with smooth crossfade transitions.
- `claw_indicator.rs` — the floating pill that shows the agent's identity + status. The colored badge only surfaces once a real agent name is known — no placeholder "CLAW" text over empty panes. On `set_identity`, it looks up the character by UUID (falling back to internal name, then display name) in `CHARACTER_CACHE` and applies the character's CSS hex color to the badge. It handles real-time "thinking" feedback by reflecting the actively executing tool name directly in the UI.
- `msgbar/` — the input bar that lives inside the drawer. Carries attachments, autocomplete (`@agent`, `/resume`), history nav, Ctrl+V paste, and the four status toggles (claw/sleep/pin/web-search). The send button is a sibling widget, not a child of the bar, so the bar can render as a single rounded field with the send icon floating alongside.
- `dispatch.rs` — `spawn_dispatch(rx, host, overlay, indicator, msgbar, …)`. The single event loop for a pane; every branch is either a widget mutation or a host-trait call. It records all overlay-visible events (`PersistentClawRow::is_overlay_visible()`) into the overlay's backing store so the full session history is preserved even when `maintain_overlay_history` is toggled off or the visible history is temporarily cleared.
- `claw_host.rs` — the `ClawHost` trait (the decoupling boundary).
- `proposal.rs` — `Proposal` enum (UI shape; converted from terminal-specific `TerminalProposal` via `.into()`). Variants: `Command`, `Bookmark`, `FileWrite`, `FileDelete`, `KillProcess`, `BackgroundCommand { command, explanation }`, `GetClipboard`, `SetClipboard`.

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

### 12. `boxxy-model-selection` (Library Crate)
Data-driven model configuration UI. Uses a registry pattern to dynamically build selection dialogs and dropdowns based on registered `AiProvider` traits. Decouples AI capability discovery from the main application window.

`SingleModelSelector` (`src/ui/selector.rs`) renders a provider dropdown + model dropdown pair. **Important indexing invariant:** the provider dropdown includes "None" as index 0, so real providers start at index 1. The internal `providers: Vec<Box<dyn AiProvider>>` does **not** include "None", so all accesses into that vec use `(dropdown_selected - 1) as usize`. Selecting "None" (index 0) emits `on_change(None)` and hides the model/options rows. The selector filters the providers list at construction time to only include providers whose `requires_api_key()` is satisfied by the supplied `api_keys` map.

### 13. `boxxy-claw-protocol` (Library Crate)
Pure-data DTO layer shared between the UI and the daemon across the D-Bus boundary. Defines `ClawMessage`, `ClawEngineEvent`, `PersistentClawRow`, `AgentStatus`, `ScheduledTask`, `UsageWrapper`, and the `ClawEnvironment` trait. Contains no GTK types, no reasoning-engine internals — every field serialises cleanly via serde. Both `boxxy-claw` and `boxxy-claw-ui` depend on it; neither depends on the other.

Also owns the **Character system** types and loader:
- `characters.rs` — claim/registry protocol types: `HolderKind { Pane }` (extensible for future non-pane holders), `CharacterClaim { holder_id, holder_kind, character_id, session_id, petname, owner_bus_name }`, `RegistrySnapshot { catalog, claims, revision }`, `ClaimedSession`, `ClaimError { AlreadyTaken, UnknownCharacter, Migrated }`, `StartupToken`, `PeerQuery`, `PeerInfo`. Static catalog types: `CharacterConfig`, `CharacterInfo`, `CharacterStatus { Available, Active { pane_id } }`, `OrphanGroup`. Global lock-free caches: `CHARACTER_CACHE: ArcSwap<Vec<CharacterInfo>>` and `CLAIMS_CACHE: ArcSwap<Vec<CharacterClaim>>` — both seeded from `get_registry_snapshot()` at startup and kept current via `claims_changed` signal pushes.
- `character_loader.rs` — disk I/O for the character catalog. Characters live at `~/.config/boxxy-terminal/boxxyclaw/characters/<slug>/CHARACTER.toml` (+ optional `AVATAR.png`). Key functions: `load_characters()` (reads all dirs, auto-generates missing UUIDs, deduplicates collisions, sorts by `characters.json` order), `ensure_default_character()` (extracts bundled defaults on first run), `reset_to_defaults()` (wipes all dirs and re-extracts). The three bundled defaults (niko, levi, kuro) are embedded at compile time via `include_bytes!` from `resources/characters/`. Each character's `CHARACTER.toml` carries a `duties` field (short role description, injected under `--- CHARACTER ROLE ---`) and a `personality` field (tone and speech directives, injected under `--- CHARACTER PERSONALITY ---`); both are free-form strings received verbatim by the model as part of its system prompt.

### 14. `boxxy-claw-ui` (Library Crate)
GTK widgets for the Claw sidebar page (right-hand `ViewStack` tab) and the virtualized message list factory used by the Claw overlay drawer. Provides `ClawSidebarComponent` (the outer shell: status page, active Claw/Dreams models and search engine display, token/cost context counter, MCP status, pending-tasks drawer, and temporary "Clear" button) and `create_claw_message_list` (the virtual list of diagnosis / suggestion / user-message / system-message rows used by `TerminalOverlay`). Rows are recycled via `boxxy_core_widgets::ObjectExtSafe` so long histories stay cheap.

### 15. `boxxy-mcp` (Library Crate)
Implements the Model Context Protocol (MCP) using `rmcp 3.5.1` (MCP `2026-07-28` spec with automatic fallback to `2025-11-25`). Provides `McpClientManager` and `BoxxyClientHandler` to orchestrate connections via Stdio and Streamable HTTP (`ClientLifecycleMode::Auto`), with full config diffing, exponential-backoff reconnection, paginated tool discovery (`list_all_tools`), `notifications/tools/list_changed` cache invalidation, and `McpToolObserver` hooks so MCP tool executions surface in the Claw indicator badge, sidebar debug log, and telemetry. Bridges JSON Schema Draft 7 / 2020-12 to `rig` via `DynamicMcpTool`.

### 16. `boxxy-telemetry` (Library Crate)
Provides a privacy-first observability layer for tracking agent and system usage.
- **Durable SQLite Journaling:** UI processes (`boxxy-app`) write telemetry events instantly to a local `telemetry_journal` SQLite table, ensuring zero UI lag and zero data loss on abrupt shutdowns.
- **Agent-Delegated Flushing:** The background `boxxy-agent` daemon periodically drains this local journal and exports the metrics to the Supabase backend using the OpenTelemetry (OTLP) SDK.

## Distribution & Updates

Boxxy supports three primary distribution channels:

1. **Flatpak (Flathub):** The primary sandboxed distribution. Updates are managed externally by the Flatpak system.
2. **Native (GitHub Nightly):** A standalone installation method via `scripts/install.sh` that targets `~/.local/boxxy-terminal/`. 
3. **System Package Managers (Nix, Arch AUR, etc.):** For environments where a system package manager handles updates. Maintainers should compile with the `disable-self-update` Cargo feature (`cargo build --release --features disable-self-update`) to strip out the internal update logic and related dependencies.

### Auto-Update Protocol (Native Only)
For native installations, Boxxy implements an **Atomic Swap** update mechanism located in `boxxy_window::updater` (powered by `self_update 1.3`):
- **Detection & Throttling:** Uses `can_self_update()` to safely disable the internal updater when sandboxed via Flatpak or when explicitly compiled out via the `disable-self-update` feature. Checks are burst-throttled via `self_update::check_interval::UpdateCheckGuard` (`~/.local/boxxy-terminal/.last_update_check`, 60-second interval, recorded only when up to date) and support `GH_TOKEN`/`GITHUB_TOKEN` (with unauthenticated fallback) + `Error::RateLimited` classification.
- **Verification:** Queries the `/releases/tags/nightly` endpoint directly, tracks the `published_at` date in `~/.local/boxxy-terminal/.last_update` (migrating legacy `created_at` stamps from pre-1.3 installs), and verifies downloaded archives via `self_update::Checksum` (supporting both `.sha256` sums files and GitHub's per-asset `sha256:<hex>` digest).
- **Persistence:** Streams updates asynchronously on the Tokio runtime via `Download::download_to_async` (capped at 250 MiB via `max_download_size`) and unpacks via `self_update::Extract`.
- **Execution:** Performs a transactional multi-file swap of `boxxy-terminal` and `boxxy-agent` via `self_update::MoveAll` (rolling back both if either rename fails and cleaning up legacy `.old` backups) before stopping the old daemon, spawning the new process with `--gapplication-replace --new-window`, and exiting. On startup, `boxxy-app` waits for any restarting parent `boxxy-terminal` PID to finish exiting and release its D-Bus name before `app.run()`, ensuring the new window always opens reliably.

## State Machine & UI Sync Protocol (Claw)

To enforce clarity and predictability, Boxxy strictly adheres to the following UI/Agent sync protocol:

1. **Single Source of Truth:** The `ClawSession` actor in the background is the ultimate source of truth for the state of a proposal.
2. **In-Terminal Interaction:** All interactive agent approvals and proposals (e.g., executing commands, writing files) occur exclusively through **in-terminal popovers**. The Claw Sidebar acts solely as a **read-only debug log** for tracking history and system events.
3. **Explicit Resolution:** Every proposal *must* be resolved via `ClawMessage` as either:
   - `Approved / Executed`
   - `CancelPending` (via the Reject button, hitting Escape, or the user manually typing in the terminal to "ghost dismiss").
   - `UserMessage` (providing feedback instead of accepting/rejecting).
4. **The "Silent Reject" Pattern:** When a user explicitly rejects a proposal (via `CancelPending`), the LLM receives an error. To prevent the LLM from chatty, unnecessary follow-ups (e.g. "I'm sorry you didn't like that!"), the system dictates a `[SILENT_ACK]` flow. If the agent yields a `[SILENT_ACK]` token, the UI silently drops the event and returns the agent to a sleep state without prompting the user further.
5. **View Separation:** The background session generates a unified event log. The frontend splits this into two views: the **Claw Sidebar** displays the raw, unfiltered log (including tool executions and process lists), while the **Claw Overlay** provides a curated, high-level view containing only messages, commands, and actionable proposals.

## Development Protocol
- **Git (CRITICAL):** NEVER COMMIT. The AI must NEVER automatically commit changes or push to any branch. Commits and Git operations will be performed manually by the user ONLY.
- **MCP:** Use Context7 MCP for all library documentation and code generation.
- **GTK UI Files**: When modifying `.ui` files (XML), avoid using the ampersand character (`&`) in text labels (like `title` or `subtitle` properties) if possible. GTK's Pango markup parser will often fail to render the text if the string contains a raw ampersand after XML parsing. Prefer using the word "and".
- **Documentation:** Keep this file and crate-level `AGENTS.md` files updated with all architectural changes.
