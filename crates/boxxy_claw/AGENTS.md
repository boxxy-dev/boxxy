# Claw Crate (`boxxy-claw`)

## Responsibility
Provides the agentic reasoning engine for Boxxy-Terminal. It bridges the gap between terminal snapshots and host-level system administration by leveraging LLMs, specialized tools, and a highly scalable multi-agent architecture.

## Architecture
The crate uses an **Actor Model** mixed with a **Shared-Everything State** strategy to ensure state isolation while enabling workspace orchestration:

- **Context Hygiene (CRITICAL):**
  - **Static System Prompts:** The `SYSTEM PROMPT` is strictly static across all panes. Dynamic data (pane identity, time, active skills) is appended to the `USER PROMPT` (`## CURRENT TURN CONTEXT`). This maximizes implicit prefix caching for models like Gemini 3.1 and reduces token churn for all other models.
  - **Freshness Buffer:** Only the last 4 turns of history retain full terminal snapshots and global radar data. Older turns are aggressively stripped of these blocks to prevent linear context growth.
  - **Snapshot Deduplication:** The terminal snapshot is hashed every turn. If the hash hasn't changed since the previous turn, the payload is replaced with `[TERMINAL_SNAPSHOT: NO_CHANGE]`.
  - **Semantic Deduplication ("Clone Killer"):** Before storing a new episodic memory, a background agent checks recent interactions for the current project path. If a new summary is semantically identical (>= 90% match) to an existing one, the engine `touch`es the existing interaction (updates the timestamp) rather than inserting a duplicate row.
  - **Chit-Chat Filtering:** The background summarizer is strictly instructed to discard social greetings or non-technical chatter by outputting `NO_TECHNICAL_CHANGE`.
  - **Robotic Frame:** Background memory extraction agents (Observer/Summarizer) are isolated from the main agent's persona using `[DATA_START]`/`[DATA_END]` tags and explicit anchor commands to enforce rigid JSON/technical outputs and prevent hallucinations.
- **Agent Identification:** Agents identify themselves strictly by their unique pane name (e.g., "plentiful bream"), not by the underlying engine name ("Boxxy-Claw").
- **`engine/session.rs`**: Implements the `ClawSession` actor. Each terminal pane owns its own dedicated session loop and tracks local capability states.
  - **Finite State Machine (FSM):** Agent states are strictly managed via a priority-aware FSM (`engine/fsm/`). Transitions (e.g., `Sleep` -> `Waiting`) are executed via `TransitionRequest` messages, ensuring deterministic behavior, lock safety, and cycle detection (DAG) during Swarm operations.
  - **Interrupt & Replace (Human-in-the-Loop):** User actions (e.g., `CancelPending`, `SleepToggle`) are routed to a high-priority `urgent_backlog`. If the agent is `Working` on a background Swarm task, the User's interrupt instantly `.abort()`s the ongoing LLM task, gracefully downgrades the state to `Waiting`, and regains interactive control.
  - **Wholesome Sleep Mode & The Dreamer:** Agents can be put to `Sleep` (passive observers, 0 tokens). When awoken via a "Hard Wake" (User interaction), the FSM triggers a background "Dreamer" task (`engine/summarization/wake.rs`). This reads the terminal delta since sleep, uses the configured `memory_model` to generate a concise catch-up summary, and injects it as a `[WHILE_YOU_SLEPT]` context block before returning control to the user.
  - **Phantom Swarm (Transient Headless Workers):** Interactive agents can use the `summon_headless_worker` tool to spawn invisible, parallel compute sessions. These headless workers are strictly parented to the interactive agent, share its CWD, and are automatically reaped upon task completion, enabling true concurrent orchestration without cluttering the UI.
- **`engine/agent.rs`**: Wraps the `rig-core` framework. Conditionally injects capabilities based on global preferences and local pane state.
- **`engine/dispatcher.rs`**: Modular parser that cleans LLM output and routes suggestions back to the UI.
- **`engine/context.rs`**: Assembles the agent's context by merging terminal snapshots, memories, and the Workspace Radar.
- **`engine/tools/orchestration.rs`**: Houses the **Swarm Toolbox**, including `subscribe_to_pane`, `delegate_task_async`, `await_tasks`, `acquire_lock`, `release_lock`, and `request_assistance`.
- **`registry/`**: Houses the global singletons that share data across all `ClawSession` actors efficiently without blocking.
  - **`skills.rs`**: Manages the `SkillRegistry` for semantic skill retrieval.
  - **`workspace.rs`**: Manages the `WorkspaceRegistry`, acting as the **Global Radar**. It now includes a central **EventBus** for pub/sub messaging and a **LockTable** for resource management across the entire application.
- **`memories/`**: Manages interactions with `boxxy_db`, including the **Selective Background Observer** for implicit fact extraction and the **Dream Orchestrator**.
  - **Memory Consolidation ("Dreaming")**: Runs a background 3-phase LLM pipeline. Phase 1 (Ingestion) fetches "seeded" interactions. Phase 2 (Extraction) identifies permanent behavioral patterns and facts while resolving conflicts. Phase 3 (REM) promotes facts to the DB, syncs `MEMORY.md`, and logs high-level insights to `DREAMS.md`.
  - **Selective Extraction**: The observer is strictly instructed to IGNORE transient data (git branches, CWDs, agent names, image metadata). It focuses exclusively on long-term facts and user preferences.
  - **Memory Management**: Agents possess a `memory_delete` tool to prune outdated or incorrect facts from the long-term database.
  - **Hierarchical Scoping**: (Proposed) Memories are scoped either to `[global]` or to a Git-aware project root, preventing "Amnesia" when moving between subdirectories.

## Key Features
- **Structured Semantic Context**: By leveraging the `boxxy-vte` engine's OSC 133 semantic prompt tracking, the Claw session receives a highly structured, markdown-formatted history of the terminal buffer (e.g., `[PROMPT]`, `[COMMAND]`, `[OUTPUT]`). This prevents the AI from hallucinating boundaries and dramatically improves the accuracy of error diagnosis and command suggestions.
- **Structured Data & Table Views**: The engine supports `ToolResult` events that allow tools to return JSON data. The UI thread intercepts these events to render rich read-only components (like a **Process List Table**) directly in the sidebar history, moving beyond raw text blocks.
- **Deep Reasoning Headroom**: The engine allows for up to **100 tool-calling turns** per interaction, providing sufficient space for complex, multi-step diagnostic and repair workflows without hitting "max turn" limits.
- **Dynamic Scrollback Paging**: The AI is not limited to the bottom of the screen. It can autonomously pull older context from the terminal's history using the `ReadScrollbackTool`, enabling it to diagnose errors that occurred hundreds of lines ago without passing the entire scrollback buffer to the LLM on every turn.
- **Persistent Interaction History**: Interaction events (diagnoses, proposals, process lists) are serialized into `PersistentClawRow` objects and saved to the database. These are re-hydrated instantly upon `/resume` via a dedicated `RestoreHistory` event, ensuring the sidebar reflects the true state of the conversation.
- **Soft Clear Protocol**: Supports a visual-only clear that marks the session's `cleared_at` timestamp. Historical events before this mark are filtered out during restoration, providing a "fresh screen" without losing underlying LLM context.
- **Global Orchestration ("Red Pony" Protocol)**: Conversations are isolated per terminal pane (`ClawSession`) but coordinated via a **Global Workspace Radar**. Agents are aware of ALL active peers across the application, regardless of their current working directory. They can explicitly coordinate using a "Manager Pattern" via `read_pane_buffer(agent_name)`, `list_active_agents()`, and `delegate_task(agent_name)`.
- **TUI Modality Awareness**: By monitoring real-time D-Bus signals from the host agent, Claw detects when a terminal enters an interactive TUI (e.g., `vim`, `python`). It automatically injects strict warnings into the system prompt and enforces the use of raw keystroke tools (`send_keystrokes_to_pane`) over standard bash commands to prevent incorrect scripting of interactive states.
- **Global Intent Blackboard**: Replaces directory-scoped intents with a single, system-wide scratchpad. Agents can broadcast high-level goals via `set_global_intent`, ensuring alignment across the entire PC.
- **Pane Lifecycle & TUI Control**: Agents possess native `spawn_agent`, `close_agent`, and `send_keystrokes_to_pane` tools. This empowers them to autonomously alter the terminal layout (splits/tabs), coordinate dev-environment setups, and even control interactive TUI applications (like `vim` or `htop`) in peer panes by injecting raw escapes and control sequences.

- **Agentic Task System (Session-Scoped)**: Agents can autonomously schedule persistent, cross-agent tasks and reminders using the `schedule_task` tool.
  - **Task Types & Approval**: Supports `Notification` (Zero-approval system reminders), `Command` (Deferred approval bash commands), and `Query`.
  - **Lifecycle & Execution**: Tasks are managed by a 1-second `tokio::select!` background loop within the `ClawSession` actor. When a scheduled command triggers, it emits an `InjectCommand` proposal event, forcing the user to approve the action *at execution time*.
  - **Cross-Agent Awareness**: Tasks are synced in real-time to the `WorkspaceRegistry`, allowing any agent to query the global schedule via `list_my_tasks` and `get_all_pending_tasks`.
  - **Resumption Persistence**: Tasks are strictly tied to the active session. If a pane is closed, its tasks sleep in `boxxy-db` (`pending_tasks_json`). When the session is resumed, the tasks are re-hydrated and the countdown continues.
- **Visual Identity System**: Every agent is visually represented by a color-coded top-right badge in its pane. This badge is dynamically colored based on the agent's name hash and automatically hides in "Alternate Screen" modes to ensure zero obstruction during full-screen CLI tool usage. Residents can toggle this feature in the Advanced Settings. The badge also supports an **Evicted State** (grayscale) if its session is resumed elsewhere, and displays a **Clock Icon** (`boxxy-running-symbolic`) when the agent has pending scheduled tasks.
- **Provider-Agnostic Scaling**: The reasoning engine is decouple from specific AI providers. It uses the `AiCredentials` mapping and a modular `create_claw_agent` factory, allowing for seamless integration of new LLMs without architectural changes.
- **Session Resumption & Eviction**: Supports resuming the last 10 active sessions (>= 1 interaction) or **Pinned Sessions** via `/resume`. Implements an **Eviction Model** where resuming a session in a new pane automatically deactivates the agent in the old pane to ensure state consistency. Restoration includes full visual history, **cumulative token counters**, and task countdowns.
- **Atomic Turn Saving**: Persists the entire message history, metadata, and visual interaction log to `boxxy_db` at the end of every turn, ensuring zero data loss and enabling reliable resumption.
- **Tiered Skill Injection & Compact Toolbox**: To save tokens, only the top-relevant skill is injected fully into the prompt. All other skills are listed compactly (name + 1-sentence description) in the "Toolbox". The agent can dynamically pull full instructions on-demand using the `activate_skill` tool.
- **FTS5 Semantic Search**: Built on top of a highly optimized SQLite FTS5 engine, enabling lightning-fast semantic retrieval for both memories and skills.
- **Host Privileged Operations**: Delegates administrative tasks (e.g., writing files, checking services) to the `boxxy-agent` via IPC.
- **Hybrid Memory System**: Uses a SQLite database (`boxxy.db`) with an FTS5 Key-Value "Upsert" model to store long-term user preferences and learned facts. 
  - **Explicit Tool Enforcement**: The agent is strictly mandated to use the `memory_store` tool for direct user requests to remember information.
  - **Implicit Background Observer**: A specialized `memory_model` evaluates every turn asynchronously to extract permanent facts without interrupting the main conversation.
  - **In-Memory Agent Persistence**: Instead of recreating agents per turn, `ClawSession` maintains a single, long-lived `rig::Agent` instance. This preserves internal session states and allows for faster, incremental context updates.
  - **Analytics Integration**: Visual history rows capture token `Usage` data, enabling historical cost and performance tracking. The session-level `total_tokens` counter is persisted and rehydrated to ensure continuity across model switches.

## Directives
- **Lazy Loading & Lifecycle**: BoxxyClaw follows a strict two-stage initialization to keep the terminal lightweight:
  1. **Identity Phase (`Initialize`):** When Claw mode is toggled ON for a specific pane, the session actor receives an `Initialize` message. It immediately generates a fresh agent name (e.g., "Capable Tragopan"), clears history, and announces its identity to the UI so badges can appear instantly.
  2. **Resource Phase:** Heavy background resources (SQLite Database, Skill Loading, RAG) MUST NOT be loaded until the first actual user request (e.g., `? hello`). This ensures simply toggling Claw mode "On" for a pane doesn't consume excessive memory or CPU until the agent is utilized.
