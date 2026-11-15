-- Canonical schema for fresh Boxxy-Terminal installations and in-memory test databases.
-- Every structural change here MUST have a corresponding incremental migration in
-- `crates/db/migrations/` registered in `crates/db/src/migrations.rs` (enforced by unit tests).

CREATE TABLE IF NOT EXISTS migration (
    id TEXT PRIMARY KEY,
    time_completed INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    history_json TEXT,
    pending_tasks_json TEXT,
    agent_name TEXT,
    character_id TEXT NOT NULL DEFAULT '',
    character_display_name TEXT NOT NULL DEFAULT '',
    last_cwd TEXT,
    title TEXT,
    model_id TEXT,
    pinned BOOLEAN NOT NULL DEFAULT false,
    total_tokens BIGINT NOT NULL DEFAULT 0,
    context_tokens BIGINT NOT NULL DEFAULT 0,
    total_cost_usd REAL NOT NULL DEFAULT 0.0,
    last_dream_at DATETIME,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS claw_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    event_json TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS interactions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    project_path TEXT,
    content TEXT NOT NULL,
    metadata TEXT,
    embedding BLOB,
    processing_state TEXT NOT NULL DEFAULT 'raw',
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    last_accessed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS memories (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    key TEXT NOT NULL,
    project_path TEXT NOT NULL,
    content TEXT NOT NULL,
    category TEXT,
    verified BOOLEAN NOT NULL DEFAULT false,
    pinned BOOLEAN NOT NULL DEFAULT false,
    observation_count INTEGER NOT NULL DEFAULT 1,
    confidence_score REAL NOT NULL DEFAULT 0.0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    last_accessed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    access_count INTEGER NOT NULL DEFAULT 0,
    UNIQUE(key, project_path)
);

CREATE TABLE IF NOT EXISTS skills (
    name TEXT PRIMARY KEY,
    description TEXT NOT NULL DEFAULT '',
    triggers TEXT NOT NULL DEFAULT '',
    content TEXT NOT NULL,
    metadata TEXT,
    pinned BOOLEAN NOT NULL DEFAULT false,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS msgbar_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    text TEXT NOT NULL,
    attachments_json TEXT NOT NULL DEFAULT '[]',
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS telemetry_journal (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    install_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    day_bucket INTEGER NOT NULL,
    recorded_at_ms INTEGER NOT NULL,
    metric_name TEXT NOT NULL,
    value REAL NOT NULL CHECK (value > 0.0),
    attributes_json TEXT NOT NULL DEFAULT '{}'
);

-- Relational indexes for hot-path lookups, FK cascades, and background hygiene/dreaming
CREATE INDEX IF NOT EXISTS idx_sessions_pinned_updated ON sessions(pinned DESC, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_claw_events_session_id ON claw_events(session_id);
CREATE INDEX IF NOT EXISTS idx_interactions_session_id ON interactions(session_id);
CREATE INDEX IF NOT EXISTS idx_interactions_path_accessed ON interactions(project_path, last_accessed_at DESC);
CREATE INDEX IF NOT EXISTS idx_interactions_undreamed ON interactions(created_at ASC) WHERE processing_state IN ('raw', 'seeded');
CREATE INDEX IF NOT EXISTS idx_interactions_last_accessed ON interactions(last_accessed_at);
CREATE INDEX IF NOT EXISTS idx_memories_scope_lookup ON memories(project_path, verified, pinned, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_memories_pinned ON memories(project_path, key ASC) WHERE pinned = true;
CREATE INDEX IF NOT EXISTS idx_memories_unverified_candidates ON memories(category, observation_count, confidence_score) WHERE verified = false;
CREATE UNIQUE INDEX IF NOT EXISTS idx_telemetry_journal_coalesce
    ON telemetry_journal(schema_version, install_id, session_id, day_bucket, metric_name, attributes_json);
CREATE INDEX IF NOT EXISTS idx_telemetry_journal_recorded_at
    ON telemetry_journal(recorded_at_ms ASC, id ASC);

-- FTS5 External-Content Indexes & Column-Scoped Triggers

CREATE VIRTUAL TABLE IF NOT EXISTS interactions_fts USING fts5(
    content,
    project_path,
    content='interactions',
    content_rowid='id'
);

CREATE TRIGGER interactions_ai AFTER INSERT ON interactions BEGIN
    INSERT INTO interactions_fts(rowid, content, project_path)
    VALUES (new.id, new.content, new.project_path);
END;

CREATE TRIGGER interactions_ad AFTER DELETE ON interactions BEGIN
    INSERT INTO interactions_fts(interactions_fts, rowid, content, project_path)
    VALUES ('delete', old.id, old.content, old.project_path);
END;

CREATE TRIGGER interactions_au AFTER UPDATE OF content, project_path ON interactions
WHEN old.content IS NOT new.content OR old.project_path IS NOT new.project_path
BEGIN
    INSERT INTO interactions_fts(interactions_fts, rowid, content, project_path)
    VALUES ('delete', old.id, old.content, old.project_path);
    INSERT INTO interactions_fts(rowid, content, project_path)
    VALUES (new.id, new.content, new.project_path);
END;

CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    key,
    content,
    project_path,
    content='memories',
    content_rowid='id'
);

CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, key, content, project_path)
    VALUES (new.id, new.key, new.content, new.project_path);
END;

CREATE TRIGGER memories_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, key, content, project_path)
    VALUES ('delete', old.id, old.key, old.content, old.project_path);
END;

CREATE TRIGGER memories_au AFTER UPDATE OF key, content, project_path ON memories
WHEN old.key IS NOT new.key OR old.content IS NOT new.content OR old.project_path IS NOT new.project_path
BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, key, content, project_path)
    VALUES ('delete', old.id, old.key, old.content, old.project_path);
    INSERT INTO memories_fts(rowid, key, content, project_path)
    VALUES (new.id, new.key, new.content, new.project_path);
END;

CREATE VIRTUAL TABLE IF NOT EXISTS skills_fts USING fts5(
    name,
    description,
    triggers,
    content,
    content='skills',
    content_rowid='rowid'
);

CREATE TRIGGER skills_ai AFTER INSERT ON skills BEGIN
    INSERT INTO skills_fts(rowid, name, description, triggers, content)
    VALUES (new.rowid, new.name, new.description, new.triggers, new.content);
END;

CREATE TRIGGER skills_ad AFTER DELETE ON skills BEGIN
    INSERT INTO skills_fts(skills_fts, rowid, name, description, triggers, content)
    VALUES ('delete', old.rowid, old.name, old.description, old.triggers, old.content);
END;

CREATE TRIGGER skills_au AFTER UPDATE OF name, description, triggers, content ON skills
WHEN old.name IS NOT new.name
  OR old.description IS NOT new.description
  OR old.triggers IS NOT new.triggers
  OR old.content IS NOT new.content
BEGIN
    INSERT INTO skills_fts(skills_fts, rowid, name, description, triggers, content)
    VALUES ('delete', old.rowid, old.name, old.description, old.triggers, old.content);
    INSERT INTO skills_fts(rowid, name, description, triggers, content)
    VALUES (new.rowid, new.name, new.description, new.triggers, new.content);
END;
