# Claw Crate (`boxxy-claw`)

## Responsibility
Provides the agentic reasoning engine for Boxxy-Terminal. It bridges the gap between terminal snapshots and host-level system administration by leveraging LLMs, specialized tools, and a highly scalable multi-agent architecture.

## Architecture
The crate uses an **Actor Model** mixed with a **Shared-Everything State** strategy to ensure state isolation while enabling workspace orchestration:

- **`engine/session.rs`**: Implements the `ClawSession` actor. Each terminal pane owns its own dedicated session loop.
- **`engine/agent.rs`**: Wraps the `rig-core` framework to create model-specific agents with tool support.
- **`engine/dispatcher.rs`**: Modular parser that cleans LLM output and decides how to route suggestions back to the UI.
- **`engine/context.rs`**: Assembles the agent's context by merging terminal snapshots, memories, and the Workspace Radar.
- **`engine/tools/`**: Contains the implementations for `SysShellTool`, `FileWriteTool`, `ActivateSkillTool`, and workspace routing tools (`ReadPaneTool`, `SetWorkspaceIntentTool`).
- **`registry/`**: Houses the global singletons that share data across all `ClawSession` actors efficiently without blocking.
  - **`skills.rs`**: Manages the `SkillRegistry`, using an `Arc<RwLock>` and `notify` to hot-reload `SKILL.md` files instantly across all panes. Fails gracefully with a warning if system inotify limits are reached.
  - **`workspace.rs`**: Manages the `WorkspaceRegistry`, acting as the radar that tracks all active panes and their last commands for cross-pane orchestration.
- **`memories/`**: Manages interactions with `boxxy_db`, including the **Background Observer** for implicit fact extraction.

## Key Features
- **Actor Isolation with Peer Awareness**: Conversations are isolated per terminal pane (`ClawSession`). However, agents are aware of other panes within the same project directory via the "Workspace Radar" and can explicitly coordinate using `read_pane_buffer` and `set_workspace_intent` tools.
- **Tiered Skill Injection**: To save tokens, only the top 3 semantically relevant skills (or explicitly triggered/pinned ones) are injected fully into the prompt ("Active Skills"). All other skills are injected as compact metadata ("Toolbox"), which the agent can dynamically pull using the `activate_skill` tool.
- **FTS5 Semantic Search**: Built on top of a highly optimized SQLite FTS5 engine, enabling lightning-fast semantic retrieval for both memories and skills.
- **Host Privileged Operations**: Delegates administrative tasks (e.g., writing files, checking services) to the `boxxy-agent` via IPC.
- **Hybrid Memory System**: Uses a SQLite database (`boxxy.db`) with an FTS5 Key-Value "Upsert" model to store long-term user preferences and learned facts. 
  - **Explicit Tool Enforcement**: The agent is strictly mandated to use the `memory_store` tool for direct user requests to remember information.
  - **Implicit Background Observer**: A specialized `memory_model` evaluates every turn asynchronously to extract permanent facts without interrupting the main conversation.
  - **Context Hygiene**: Includes LLM-powered Semantic Query Expansion, LRU Hygiene (decaying stale memories over 30 days), Project-Scoped Context, Token-Based Budgeting, and Bidirectional Markdown Sync (`MEMORY.md`) with a User Verification Loop.

## Directives
- **Lazy Loading**: By default, BoxxyClaw (and its database/background tasks) MUST NOT run when the application starts. The AI engine should only be loaded into memory and spawned when the user explicitly requests to use the AI. This keeps the terminal as lightweight as possible.
