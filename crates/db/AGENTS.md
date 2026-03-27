# boxxy-db

## Role
Provides a persistent SQLite database for Boxxy-Terminal, serving as the Long-Term Memory (RAG) backend for `boxxy-claw` and the persistent history store for the `boxxy-msgbar`.

## Responsibilities
- **Connection Management**: Handles connecting to the local `claw_memory.db` SQLite file, creating the database and directories if they don't exist.
- **Migrations**: Automatically applies database schema migrations on startup (using an Idempotent Schema Initialization pattern).
- **Data Access**: Exposes asynchronous CRUD operations for memories (RAG facts), sessions, interactions (1-bullet summaries, no raw terminal data), and `msgbar_history` (user text/attachments only) through the `Store` struct.

## Key Modules
- `db`: The core connection pool and initialization logic.
- `models`: Defines the data structures stored in the database.
- `store`: Implements the SQL queries and operations.