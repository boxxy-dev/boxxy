-- Upgrades legacy v15 databases to the hardened journaled schema without losing any data.
-- Runs with `foreign_keys = OFF` outside the transaction (SQLite 12-step table rebuild),
-- followed by an automatic `PRAGMA foreign_key_check` before commit.

-- 1. Drop legacy FTS triggers & virtual tables prior to rebuilding backing tables
DROP TRIGGER IF EXISTS interactions_ai;
DROP TRIGGER IF EXISTS interactions_ad;
DROP TRIGGER IF EXISTS interactions_au;
DROP TABLE IF EXISTS interactions_fts;

DROP TRIGGER IF EXISTS memories_ai;
DROP TRIGGER IF EXISTS memories_ad;
DROP TRIGGER IF EXISTS memories_au;
DROP TABLE IF EXISTS memories_fts;

DROP TRIGGER IF EXISTS skills_ai;
DROP TRIGGER IF EXISTS skills_ad;
DROP TRIGGER IF EXISTS skills_au;
DROP TABLE IF EXISTS skills_fts;

-- 2. Rebuild `sessions` (enforces NOT NULL on counters/booleans and drops dead `cleared_at`)
DROP INDEX IF EXISTS idx_sessions_pinned_updated;
CREATE TABLE sessions_new (
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

INSERT INTO sessions_new (
    id, name, history_json, pending_tasks_json, agent_name,
    character_id, character_display_name, last_cwd, title, model_id,
    pinned, total_tokens, context_tokens, total_cost_usd,
    last_dream_at, created_at, updated_at
)
SELECT
    id,
    name,
    history_json,
    pending_tasks_json,
    agent_name,
    COALESCE(character_id, ''),
    COALESCE(character_display_name, ''),
    last_cwd,
    title,
    model_id,
    COALESCE(pinned, false),
    COALESCE(total_tokens, 0),
    COALESCE(context_tokens, 0),
    COALESCE(total_cost_usd, 0.0),
    last_dream_at,
    created_at,
    updated_at
FROM sessions;

DROP TABLE sessions;
ALTER TABLE sessions_new RENAME TO sessions;

-- 3. Rebuild `interactions` (enforces NOT NULL on `processing_state`)
CREATE TABLE interactions_new (
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

INSERT INTO interactions_new (
    id, session_id, project_path, content, metadata, embedding,
    processing_state, created_at, last_accessed_at
)
SELECT
    id,
    session_id,
    project_path,
    content,
    metadata,
    embedding,
    COALESCE(processing_state, 'raw'),
    created_at,
    last_accessed_at
FROM interactions;

DROP TABLE interactions;
ALTER TABLE interactions_new RENAME TO interactions;

-- 4. Rebuild `memories` (enforces NOT NULL on booleans & reinforcement counters)
CREATE TABLE memories_new (
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

INSERT INTO memories_new (
    id, key, project_path, content, category,
    verified, pinned, observation_count, confidence_score,
    created_at, updated_at, last_accessed_at, access_count
)
SELECT
    id,
    key,
    project_path,
    content,
    category,
    COALESCE(verified, false),
    COALESCE(pinned, false),
    COALESCE(observation_count, 1),
    COALESCE(confidence_score, 0.0),
    created_at,
    updated_at,
    last_accessed_at,
    COALESCE(access_count, 0)
FROM memories;

DROP TABLE memories;
ALTER TABLE memories_new RENAME TO memories;

-- 5. Rebuild `skills` (enforces NOT NULL on `pinned`)
CREATE TABLE skills_new (
    name TEXT PRIMARY KEY,
    description TEXT NOT NULL DEFAULT '',
    triggers TEXT NOT NULL DEFAULT '',
    content TEXT NOT NULL,
    metadata TEXT,
    pinned BOOLEAN NOT NULL DEFAULT false,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO skills_new (
    name, description, triggers, content, metadata, pinned, created_at, updated_at
)
SELECT
    name,
    COALESCE(description, ''),
    COALESCE(triggers, ''),
    content,
    metadata,
    COALESCE(pinned, false),
    created_at,
    updated_at
FROM skills;

DROP TABLE skills;
ALTER TABLE skills_new RENAME TO skills;

-- 6. Rebuild `msgbar_history` (enforces NOT NULL DEFAULT '[]' on `attachments_json`)
CREATE TABLE msgbar_history_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    text TEXT NOT NULL,
    attachments_json TEXT NOT NULL DEFAULT '[]',
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO msgbar_history_new (id, text, attachments_json, created_at)
SELECT id, text, COALESCE(attachments_json, '[]'), created_at
FROM msgbar_history;

DROP TABLE msgbar_history;
ALTER TABLE msgbar_history_new RENAME TO msgbar_history;

-- 7. Remove any orphaned `claw_events` / `interactions` rows from pre-v15 eras where
-- `foreign_keys` might not have been enforced so `PRAGMA foreign_key_check` succeeds cleanly.
DELETE FROM claw_events WHERE NOT EXISTS (SELECT 1 FROM sessions s WHERE s.id = claw_events.session_id);
DELETE FROM interactions WHERE NOT EXISTS (SELECT 1 FROM sessions s WHERE s.id = interactions.session_id);

-- 8. Create all relational indexes
CREATE INDEX IF NOT EXISTS idx_sessions_pinned_updated ON sessions(pinned DESC, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_claw_events_session_id ON claw_events(session_id);
CREATE INDEX IF NOT EXISTS idx_interactions_session_id ON interactions(session_id);
CREATE INDEX IF NOT EXISTS idx_interactions_path_accessed ON interactions(project_path, last_accessed_at DESC);
CREATE INDEX IF NOT EXISTS idx_interactions_undreamed ON interactions(created_at ASC) WHERE processing_state IN ('raw', 'seeded');
CREATE INDEX IF NOT EXISTS idx_interactions_last_accessed ON interactions(last_accessed_at);
CREATE INDEX IF NOT EXISTS idx_memories_scope_lookup ON memories(project_path, verified, pinned, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_memories_pinned ON memories(project_path, key ASC) WHERE pinned = true;
CREATE INDEX IF NOT EXISTS idx_memories_unverified_candidates ON memories(category, observation_count, confidence_score) WHERE verified = false;
CREATE INDEX IF NOT EXISTS idx_telemetry_journal_created_at ON telemetry_journal(created_at ASC, id ASC);

-- 9. Recreate external-content FTS5 virtual tables, column-scoped triggers, and rebuild indexes
CREATE VIRTUAL TABLE interactions_fts USING fts5(
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

CREATE VIRTUAL TABLE memories_fts USING fts5(
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

CREATE VIRTUAL TABLE skills_fts USING fts5(
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

INSERT INTO interactions_fts(interactions_fts) VALUES ('rebuild');
INSERT INTO memories_fts(memories_fts) VALUES ('rebuild');
INSERT INTO skills_fts(skills_fts) VALUES ('rebuild');
