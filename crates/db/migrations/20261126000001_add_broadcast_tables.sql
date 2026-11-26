-- Add local Boxxy News (Broadcast Service) article cache, permanent per-UUID user receipts,
-- and sync metadata tables.

CREATE TABLE IF NOT EXISTS broadcast_articles (
    id TEXT PRIMARY KEY,
    slug TEXT NOT NULL,
    title TEXT NOT NULL,
    summary TEXT NOT NULL,
    body_md TEXT NOT NULL,
    category TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('published', 'archived')),
    notify INTEGER NOT NULL DEFAULT 1,
    action_label TEXT,
    action_url TEXT,
    published_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    cached_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_broadcast_articles_published
    ON broadcast_articles (published_at DESC, id DESC);

-- Permanent lightweight per-UUID user decision (<80 bytes/row).
-- Intentionally has NO foreign key to `broadcast_articles` so pruning old Markdown bodies
-- beyond the 4-row cache cap never deletes seen/dismissed state.
CREATE TABLE IF NOT EXISTS broadcast_receipts (
    id TEXT PRIMARY KEY,
    disposition TEXT NOT NULL CHECK (disposition IN ('pending', 'read', 'dismissed', 'baseline')),
    updated_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_broadcast_receipts_disposition
    ON broadcast_receipts (disposition, updated_at_ms DESC);

CREATE TABLE IF NOT EXISTS broadcast_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
