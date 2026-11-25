# boxxy-db

## Role
Provides the persistent SQLite storage layer for Boxxy-Terminal, backing Long-Term Memory (RAG), episodic interaction history, conversation/session resumption, skill FTS5 indexing, input history (`msgbar_history`), and the durable OpenTelemetry event journal (`telemetry_journal`).

All SQLite queries across the workspace live inside `boxxy-db` (`Store`) — higher-level crates (`boxxy-claw`, `boxxy-agent`, `boxxy-claw-ui`, `boxxy-claw-widget`, `boxxy-telemetry`) interact strictly through typed `Db` and `Store` APIs rather than raw `sqlx` queries.

## Architecture & Module Layout

```text
crates/db/
├── migrations/
│   ├── schema_latest.sql                                # Canonical schema for fresh installs & Db::new_in_memory()
│   ├── 20261115000001_harden_schema_fts_and_indexes.sql # Non-destructive v15 -> journaled schema upgrade
│   └── 20261115000002_upgrade_telemetry_journal.sql     # Normalized coalescing telemetry_journal with ms timestamps & event_id
└── src/
    ├── lib.rs          # Db pool lifecycle, 0700/0600 POSIX permissions, WAL/MEMORY pragmas, Db::open_at(), db.store()
    ├── migrations.rs   # OpenCode-style `migration` journal runner, pre-upgrade `.bak` snapshot, zero-drift tests
    ├── models.rs       # Session, SessionUpsert, Interaction, Memory, SkillRecord, MsgBarHistory, TelemetryJournal*, HygieneStats
    └── store/
        ├── mod.rs          # Store<'a> handle & SQLITE_BIND_CHUNK_SIZE batch ceiling
        ├── sessions.rs     # Sessions, transactional character migration, pending tasks, claw_events
        ├── memories.rs     # Long-term facts, candidate reinforcement, promotion, conflict demotion, FTS5, hygiene
        ├── interactions.rs # Episodic summaries, dreaming state transitions, embeddings, FTS5 search
        ├── skills.rs       # Diff-aware transactional sync_skills & weighted FTS5 search
        ├── msgbar.rs       # Message bar input history & single-statement atomic pruning
        └── telemetry.rs    # Coalescing telemetry_journal upserts, atomic delta ack, TTL/row-cap prune, and opt-out purge
```

## Connection & Security Posture (`lib.rs`)
- **Location**: Resolves via `boxxy_sys_utils::get_config_dir().join("boxxyclaw/boxxy.db")`. Use `Db::open_at(path)` for isolated file-backed tests and `Db::new_in_memory()` (`max_connections = 1`, `idle_timeout = None`, `max_lifetime = None`) for unit tests.
- **Owner-Only Permissions**: On Unix, parent directories are restricted to `0700` and `boxxy.db` (plus `-wal`, `-shm`, `.bak`, and `.migrate.lock` sidecars) are restricted to `0600` without opening/closing existing files (preserving POSIX `fcntl` advisory locks).
- **SQLite Pragmas**: Configured with `journal_mode = WAL`, `synchronous = NORMAL`, `busy_timeout = 10s`, `foreign_keys = ON`, `temp_store = MEMORY`, and `cache_size = -8000` (8 MiB per connection).

## Journaled Migration System (`migrations.rs` & `migrations/`)
Boxxy uses an OpenCode-style `migration` journal table designed for continuous commit-based updates across Native Nightly, Flatpak, and Nix without sequential integer collisions across Git branches:

```sql
CREATE TABLE IF NOT EXISTS migration (
    id TEXT PRIMARY KEY,           -- e.g. "20261115000001_harden_schema_fts_and_indexes"
    time_completed INTEGER NOT NULL
);
```

- **Read-Only Fast Path**: On normal startups (`boxxy-app` and `boxxy-agent`), `migrations::apply` checks `SELECT id FROM migration` first. When all registered `MIGRATIONS` are present, it returns immediately with **zero write locks, zero DDL, and zero FTS rebuilds**.
- **Fresh Bootstrap**: Empty databases apply `migrations/schema_latest.sql` inside a `BEGIN IMMEDIATE` transaction and seed `migration` with all registered IDs in one shot.
- **Incremental Upgrades (`v15+`)**:
  - Serializes pending upgrade passes across concurrent `boxxy-agent` and `boxxy-app` startups via an exclusive advisory `flock` on `<db_path>.migrate.lock` (re-checking the read-only fast path once acquired so the second process returns immediately without touching `.bak` or opening write transactions).
  - Creates an atomic WAL-consistent rolling backup (`boxxy.db.bak.tmp` renamed over `boxxy.db.bak`) via `VACUUM INTO` before applying pending steps on file-backed databases, preserving the untouched pre-migration snapshot.
  - Executes each unapplied `Migration` (`MigrationUp::Sql` or `MigrationUp::Custom`) inside its own `BEGIN IMMEDIATE` transaction, re-checking `SELECT id FROM migration WHERE id = ?` under the lock so concurrent callers never double-apply.
  - When `foreign_keys: false` is set on a `Migration` (for SQLite's 12-step table-rebuild pattern), the runner disables `PRAGMA foreign_keys` outside the transaction, verifies `PRAGMA foreign_key_check` before `COMMIT`, and always restores `PRAGMA foreign_keys = ON` afterward.
- **Legacy `< v15` Bootstrap**: Ancient pre-v15 preview databases without a `migration` table are backed up to `boxxy.db.bak` and bootstrapped cleanly to `schema_latest.sql`.

### Workflow for Adding a Schema Change
1. Update `crates/db/migrations/schema_latest.sql`.
2. Add `crates/db/migrations/YYYYMMDDHHMMSS_<slug>.sql` and append a `Migration` entry to `MIGRATIONS` in `crates/db/src/migrations.rs`.
3. Run `cargo test -p boxxy-db` — `zero_drift_fresh_schema_matches_migrated_v15_schema_and_preserves_data` automatically verifies that fresh bootstrap and incremental migrations produce an identical schema and pass FTS5 `'integrity-check'`.

## FTS5 & Indexing Design
- **External-Content FTS5 Tables**:
  - `memories_fts` (`key`, `content`, `project_path`, `content_rowid='id'`), ranked with `bm25(memories_fts, 5.0, 1.0, 0.5)`.
  - `interactions_fts` (`content`, `project_path`, `content_rowid='id'`), ranked with `bm25(interactions_fts, 1.0, 0.5)`.
  - `skills_fts` (`name`, `description`, `triggers`, `content`, `content_rowid='rowid'`), joined on `m.rowid = f.rowid` and ranked with `bm25(skills_fts, 10.0, 5.0, 8.0, 1.0)`.
- **Column-Scoped `AFTER UPDATE OF ... WHEN ...` Triggers**: `memories_au`, `interactions_au`, and `skills_au` fire strictly when indexed text columns actually change, avoiding FTS5 delete+retokenize churn when `access_count`, `last_accessed_at`, `verified`, `pinned`, or `processing_state` are updated on hot paths.
- **Memory Protection & Hierarchical Scoping Invariants**: `upsert_dream_candidate` never overwrites `content` on `verified`, `pinned`, `manual_sync`, `preference`, or `system` memories; `demote_memory_by_key` resets both `observation_count = 0` and `confidence_score = 0.0` while refusing to demote protected or `pinned` rows. All memory and episodic interaction queries (`get_pinned_memories`, `get_bootstrap_memories`, `get_memories_for_scope`, `search_memories`, `search_provisional_memories`, `search_interactions`) enforce `substr`-based hierarchical directory scoping (`push_scope_filter` / `push_scope_rank` in `store/mod.rs`), inheriting `'global'` and same-project ancestor/child directories while strictly isolating unrelated workspaces (and never treating root `'/'` or `_`/`%` characters as wildcards).
