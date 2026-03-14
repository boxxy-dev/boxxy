# boxxy-db

## Role
Provides a persistent SQLite database for Boxxy-Terminal, primarily serving as the Long-Term Memory (RAG) backend for `boxxy-claw`.

## Responsibilities
- **Connection Management**: Handles connecting to the local `claw_memory.db` SQLite file, creating the database and directories if they don't exist.
- **Migrations**: Automatically applies database schema migrations on startup.
- **Data Access**: Exposes asynchronous CRUD operations for memories, sessions, and other persistent state.

## Key Modules
- `db`: The core connection pool and initialization logic.
- `models`: Defines the data structures stored in the database.
- `store`: Implements the SQL queries and operations.