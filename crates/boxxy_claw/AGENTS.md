# Claw Crate (`boxxy-claw`)

## Responsibility
Provides the agentic reasoning engine for Boxxy-Terminal. It bridges the gap between terminal snapshots and host-level system administration by leveraging LLMs, specialized tools, and a highly scalable multi-agent architecture.

## Architecture
The crate uses an **Actor Model** mixed with a **Shared-Everything State** strategy to ensure state isolation while enabling workspace orchestration:

- **`engine/session.rs`**: Implements the `ClawSession` actor. Each terminal pane owns its own dedicated session loop.
- **`engine/agent.rs`**: Wraps the `rig-core` framework to create model-specific agents with tool support.
- **`engine/dispatcher.rs`**: Modular parser that cleans LLM output and decides how to route suggestions back to the UI.
- **`engine/context.rs`**: Assembles the agent's context by merging terminal snapshots, memories, and the Workspace Radar.
- **`engine/tools/`**: Contains Boxxy-specific tools like `SysShellTool`, `TerminalCommandTool`, `ActivateSkillTool`, and global orchestration tools (`ReadPaneTool`, `SetGlobalIntentTool`). Standard host tools (file, system, web, clipboard) are now delegated to the external `boxxy-core-toolbox` crate, which `boxxy-claw` configures and connects to its UI.
- **`registry/`**: Houses the global singletons that share data across all `ClawSession` actors efficiently without blocking.
  - **`skills.rs`**: Manages the `SkillRegistry`, using an `Arc<RwLock>` and `notify` to hot-reload `SKILL.md` files instantly across all panes. Fails gracefully with a warning if system inotify limits are reached.
  - **`workspace.rs`**: Manages the `WorkspaceRegistry`, acting as the **Global Radar** that tracks all active agents across the entire application for system-wide orchestration.
- **`memories/`**: Manages interactions with `boxxy_db`, including the **Background Observer** for implicit fact extraction.

## Key Features
- **Structured Semantic Context**: By leveraging the `boxxy-vte` engine's OSC 133 semantic prompt tracking, the Claw session receives a highly structured, markdown-formatted history of the terminal buffer (e.g., `[PROMPT]`, `[COMMAND]`, `[OUTPUT]`). This prevents the AI from hallucinating boundaries and dramatically improves the accuracy of error diagnosis and command suggestions.
- **Structured Data & Table Views**: The engine supports `ToolResult` events that allow tools to return JSON data. The UI thread intercepts these events to render rich read-only components (like a **Process List Table**) directly in the sidebar history, moving beyond raw text blocks.
- **Dynamic Scrollback Paging**: The AI is not limited to the bottom of the screen. It can autonomously pull older context from the terminal's history using the `ReadScrollbackTool`, enabling it to diagnose errors that occurred hundreds of lines ago without passing the entire scrollback buffer to the LLM on every turn.
- **Global Orchestration ("Red Pony" Protocol)**: Conversations are isolated per terminal pane (`ClawSession`) but coordinated via a **Global Workspace Radar**. Agents are aware of ALL active peers across the application, regardless of their current working directory. They can explicitly coordinate using a "Manager Pattern" via `read_pane_buffer(agent_name)`, `list_active_agents()`, and `delegate_task(agent_name)`.
- **TUI Modality Awareness**: By monitoring real-time D-Bus signals from the host agent, Claw detects when a terminal enters an interactive TUI (e.g., `vim`, `python`). It automatically injects strict warnings into the system prompt and enforces the use of raw keystroke tools (`send_keystrokes_to_pane`) over standard bash commands to prevent incorrect scripting of interactive states.
- **Global Intent Blackboard**: Replaces directory-scoped intents with a single, system-wide scratchpad. Agents can broadcast high-level goals via `set_global_intent`, ensuring alignment across the entire PC.
- **Pane Lifecycle & TUI Control**: Agents possess native `spawn_agent`, `close_agent`, and `send_keystrokes_to_pane` tools. This empowers them to autonomously alter the terminal layout (splits/tabs), coordinate dev-environment setups, and even control interactive TUI applications (like `vim` or `htop`) in peer panes by injecting raw escapes and control sequences.

- **Visual Identity System**: Every agent is visually represented by a color-coded top-right badge in its pane. This badge is dynamically colored based on the agent's name hash and automatically hides in "Alternate Screen" modes to ensure zero obstruction during full-screen CLI tool usage. Residents can toggle this feature in the Advanced Settings.
- **Provider-Agnostic Scaling**: The reasoning engine is decouple from specific AI providers. It uses the `AiCredentials` mapping and a modular `create_claw_agent` factory, allowing for seamless integration of new LLMs without architectural changes.
- **Tiered Skill Injection**: To save tokens, only the top 3 semantically relevant skills (or explicitly triggered/pinned ones) are injected fully into the prompt ("Active Skills"). All other skills are injected as compact metadata ("Toolbox"), which the agent can dynamically pull using the `activate_skill` tool.
- **FTS5 Semantic Search**: Built on top of a highly optimized SQLite FTS5 engine, enabling lightning-fast semantic retrieval for both memories and skills.
- **Host Privileged Operations**: Delegates administrative tasks (e.g., writing files, checking services) to the `boxxy-agent` via IPC.
- **Hybrid Memory System**: Uses a SQLite database (`boxxy.db`) with an FTS5 Key-Value "Upsert" model to store long-term user preferences and learned facts. 
  - **Explicit Tool Enforcement**: The agent is strictly mandated to use the `memory_store` tool for direct user requests to remember information.
  - **Implicit Background Observer**: A specialized `memory_model` evaluates every turn asynchronously to extract permanent facts without interrupting the main conversation.
  - **Context Hygiene & "No Duplicated Data"**: Includes LLM-powered Semantic Query Expansion, LRU Hygiene (decaying stale memories over 30 days), Project-Scoped Context, Token-Based Budgeting, and Bidirectional Markdown Sync (`MEMORY.md`) with a User Verification Loop. Terminal snapshots (100-line buffer states) are treated as **strictly transient**. They are passed to the LLM during the active turn but are aggressively pruned from the in-memory conversation history and are **never** stored in the SQLite database. Instead, only a 1-sentence episodic summary is persisted, ensuring a lean database and preventing LLM context bloat.

## Directives
- **Lazy Loading & Lifecycle**: BoxxyClaw follows a strict two-stage initialization to keep the terminal lightweight:
  1. **Identity Phase (`Initialize`):** When Claw mode is toggled ON, the session actor receives an `Initialize` message. It immediately generates a fresh agent name (e.g., "Capable Tragopan"), clears history, and announces its identity to the UI so badges can appear instantly.
  2. **Resource Phase:** Heavy background resources (SQLite Database, Skill Loading, RAG) MUST NOT be loaded until the first actual user request (e.g., `? hello`). This ensures simply having Claw mode "On" doesn't consume excessive memory or CPU.
