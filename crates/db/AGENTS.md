# boxxy-db

## Role
Provides a persistent SQLite database for Boxxy-Terminal, serving as the Long-Term Memory (RAG) backend for `boxxy-claw` and the persistent history store for the `boxxy-msgbar`.

## Responsibilities
- **Connection Management**: Handles connecting to the local `claw_memory.db` SQLite file, creating the database and directories if they don't exist.
- **Migrations**: During the **Preview Phase**, formal migrations are bypassed in favor of an **Auto-Drop Strategy**. The database tracks its own version via `PRAGMA user_version`. If a mismatch is detected (e.g., after an update with breaking schema changes), the application automatically drops the `.db` file and recreates it. A system notification is then sent to the user via the first initialized Claw agent.
- **Data Access**: Exposes asynchronous CRUD operations for memories (RAG facts), sessions, and persistent visual logs.
- **Testing Standard**: ALL database operations must have corresponding unit tests in `store.rs`. New schema changes must be verified using the `Db::new_in_memory()` pattern to prevent regressions.
- **Memory Schema (Long-term Facts)**:
  - **Verified Status**: Employs a threshold-based reinforcement model. Extracted facts are stored as unverified candidates and only auto-promoted to `verified = true` if they cross strict `observation_count` and `confidence_score` thresholds.
  - **Reinforcement Tracking**: The `memories` table includes `observation_count` (frequency of extraction) and `confidence_score` (max LLM certainty) to drive the automated promotion system.
  - **FTS5 Integration**: All memories are automatically indexed in a virtual FTS5 table (`memories_fts`) with `project_path` scoping for fast semantic retrieval.
  - **Automatic Pruning**: Supported via `access_count` and `last_accessed_at` tracking, with enhanced hygiene rules for pruning unverified noise.
- **Interaction Schema (Episodic Memory & Dreaming)**:
  - Tracks raw conversation and command history in the `interactions` table.
  - Uses a `processing_state` column (`raw`, `seeded`, `dreamed`) to power the async Memory Consolidation pipeline, allowing the background `DreamOrchestrator` to batch-process un-consolidated interactions.
- **Session Persistence (Schema v9)**:
  - **Pinned Sessions**: The `sessions` table includes a `pinned` column. Pinned sessions are excluded from the "last 10" limit and sorted to the top.
  - **Total Tokens**: Tracks the cumulative context cost of a session across different model providers and application restarts.
  - **Character Identity**: The `sessions` table stores `character_id` and `character_display_name` recorded at session-creation time so `/resume` can display the original character even if the live catalog changes.
  - **Dream History**: Includes `last_dream_at` to orchestrate when a session's history was last consolidated.
  - **Soft Clear**: A `cleared_at` timestamp allows users to hide past history visuals without losing the underlying message context.
  - **Interaction Logs**: The `claw_events` table stores serialized UI events (diagnoses, proposals). It is indexed by `session_id` for fast restoration during session resumption.
  - Note: there is no `active_pane_assignments` table. Character claim state is volatile in-memory only (owned by `boxxy-agent`).
- **Session-Scoped Task Persistence**: Scheduled tasks are serialized atomically on every turn and saved alongside the conversation history. These tasks are only re-hydrated and executed when the specific session is actively resumed in a pane.

## Key Modules
- `db`: The core connection pool and initialization logic.
- `models`: Defines the data structures stored in the database.
- `store`: Implements the SQL queries and operations.