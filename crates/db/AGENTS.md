# boxxy-db

## Role
Provides a persistent SQLite database for Boxxy-Terminal, serving as the Long-Term Memory (RAG) backend for `boxxy-claw` and the persistent history store for the `boxxy-msgbar`.

## Responsibilities
- **Connection Management**: Handles connecting to the local `claw_memory.db` SQLite file, creating the database and directories if they don't exist.
- **Migrations**: During the **Preview Phase**, formal migrations are bypassed in favor of an **Auto-Drop Strategy**. The database tracks its own version via `PRAGMA user_version`. If a mismatch is detected (e.g., after an update with breaking schema changes), the application automatically drops the `.db` file and recreates it. A system notification is then sent to the user via the first initialized Claw agent.
- **Data Access**: Exposes asynchronous CRUD operations for memories (RAG facts), sessions (including serialized history and state), interactions (1-bullet summaries, no raw terminal data), and `msgbar_history` (user text/attachments only) through the `Store` struct.

## Key Modules
- `db`: The core connection pool and initialization logic.
- `models`: Defines the data structures stored in the database.
- `store`: Implements the SQL queries and operations.