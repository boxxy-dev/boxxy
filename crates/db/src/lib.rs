#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
use anyhow::{Context, Result};
use directories::ProjectDirs;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::OnceCell;

pub mod models;
pub mod store;

static DB: OnceCell<Db> = OnceCell::const_new();
pub static DATABASE_WAS_RESET: AtomicBool = AtomicBool::new(false);

const CURRENT_SCHEMA_VERSION: i32 = 8;

#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    pub async fn new() -> Result<Self> {
        let db = DB
            .get_or_try_init(|| async {
                let db_path = Self::get_db_path()?;

                // Ensure directory exists
                if let Some(parent) = db_path.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .context("Failed to create database directory")?;
                }

                // 1. Initial connection to check version
                let options = SqliteConnectOptions::new()
                    .filename(&db_path)
                    .create_if_missing(true)
                    .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
                    .synchronous(sqlx::sqlite::SqliteSynchronous::Normal);

                let temp_pool = SqlitePoolOptions::new()
                    .max_connections(1)
                    .connect_with(options.clone())
                    .await?;

                let version: (i32,) = sqlx::query_as("PRAGMA user_version")
                    .fetch_one(&temp_pool)
                    .await?;

                temp_pool.close().await;

                // 2. Validate version
                if version.0 != CURRENT_SCHEMA_VERSION && version.0 != 0 {
                    log::warn!(
                        "Database version mismatch (found {}, expected {}). DROPPING DATABASE.",
                        version.0,
                        CURRENT_SCHEMA_VERSION
                    );

                    // Close any potential lingering handles by waiting a tiny bit if needed
                    // though temp_pool.close() should be enough.
                    let _ = tokio::fs::remove_file(&db_path).await;
                    // Also remove the WAL and SHM files if they exist
                    let _ = tokio::fs::remove_file(format!("{}-wal", db_path.display())).await;
                    let _ = tokio::fs::remove_file(format!("{}-shm", db_path.display())).await;

                    DATABASE_WAS_RESET.store(true, Ordering::SeqCst);
                } else if version.0 == 0 {
                    // If it's 0, we check if tables exist to decide if it's a "Legacy Unversioned" DB or a fresh one.
                    // For this preview phase, we'll just treat 0 as "Fresh" and set the version.
                    // But if sessions table exists, it might be legacy.
                    // To be safe, if version is 0 and the file existed, we can just let it proceed to initialize_schema.
                }

                // 3. Re-open (or initial open) official pool
                let pool = SqlitePoolOptions::new()
                    .max_connections(5)
                    .connect_with(options)
                    .await
                    .context("Failed to connect to database")?;

                let db = Self { pool };
                db.initialize_schema().await?;

                // 4. Set/Update version
                sqlx::query(&format!("PRAGMA user_version = {CURRENT_SCHEMA_VERSION}"))
                    .execute(&db.pool)
                    .await?;

                Ok::<Self, anyhow::Error>(db)
            })
            .await?;

        Ok(db.clone())
    }

    fn get_db_path() -> Result<PathBuf> {
        let proj_dirs = ProjectDirs::from("org", "boxxy", "boxxy-terminal")
            .context("Could not determine project directories")?;
        let config_dir = proj_dirs.config_dir();
        Ok(config_dir.join("boxxyclaw").join("boxxy.db"))
    }

    async fn initialize_schema(&self) -> Result<()> {
        // Direct table creation without sqlx migration tracking.
        // This makes development faster and avoids checksum/versioning conflicts.
        let schema = r"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                history_json TEXT,
                pending_tasks_json TEXT,
                agent_name TEXT,
                last_cwd TEXT,
                title TEXT,
                model_id TEXT,
                pinned BOOLEAN DEFAULT false,
                cleared_at DATETIME,
                total_tokens BIGINT DEFAULT 0,
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

            CREATE INDEX IF NOT EXISTS idx_claw_events_session_id ON claw_events(session_id);

            CREATE TABLE IF NOT EXISTS interactions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                project_path TEXT,
                content TEXT NOT NULL,
                metadata TEXT,
                embedding BLOB,
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
                verified BOOLEAN DEFAULT false,
                pinned BOOLEAN DEFAULT false,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                last_accessed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                access_count INTEGER DEFAULT 0,
                UNIQUE(key, project_path)
            );

            CREATE TABLE IF NOT EXISTS skills (
                name TEXT PRIMARY KEY,
                description TEXT NOT NULL DEFAULT '',
                triggers TEXT NOT NULL DEFAULT '',
                content TEXT NOT NULL,
                metadata TEXT,
                pinned BOOLEAN DEFAULT false,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS msgbar_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                text TEXT NOT NULL,
                attachments_json TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS telemetry_journal (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                metric_name TEXT NOT NULL,
                value REAL NOT NULL,
                attributes_json TEXT NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            -- Search indexes for RAG and Skills
            CREATE VIRTUAL TABLE IF NOT EXISTS interactions_fts USING fts5(
                content='interactions',
                content,
                project_path
            );

            -- Triggers to keep FTS index updated for interactions
            DROP TRIGGER IF EXISTS interactions_ai;
            CREATE TRIGGER interactions_ai AFTER INSERT ON interactions BEGIN
              INSERT INTO interactions_fts(rowid, content) VALUES (new.id, new.content);
            END;

            DROP TRIGGER IF EXISTS interactions_ad;
            CREATE TRIGGER interactions_ad AFTER DELETE ON interactions BEGIN
              INSERT INTO interactions_fts(interactions_fts, rowid, content) VALUES('delete', old.id, old.content);
            END;

            DROP TRIGGER IF EXISTS interactions_au;
            CREATE TRIGGER interactions_au AFTER UPDATE ON interactions BEGIN
              INSERT INTO interactions_fts(interactions_fts, rowid, content) VALUES('delete', old.id, old.content);
              INSERT INTO interactions_fts(rowid, content) VALUES (new.id, new.content);
            END;

            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                content='memories',
                content,
                project_path
            );

            -- Triggers for memories
            DROP TRIGGER IF EXISTS memories_ai;
            CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN
              INSERT INTO memories_fts(rowid, content, project_path) VALUES (new.id, new.content, new.project_path);
            END;

            DROP TRIGGER IF EXISTS memories_ad;
            CREATE TRIGGER memories_ad AFTER DELETE ON memories BEGIN
              INSERT INTO memories_fts(memories_fts, rowid, content, project_path) VALUES('delete', old.id, old.content, old.project_path);
            END;

            DROP TRIGGER IF EXISTS memories_au;
            CREATE TRIGGER memories_au AFTER UPDATE ON memories BEGIN
              INSERT INTO memories_fts(memories_fts, rowid, content, project_path) VALUES('delete', old.id, old.content, old.project_path);
              INSERT INTO memories_fts(rowid, content, project_path) VALUES (new.id, new.content, new.project_path);
            END;

            CREATE VIRTUAL TABLE IF NOT EXISTS skills_fts USING fts5(
                content='skills',
                name,
                content
            );

            -- Triggers for skills
            DROP TRIGGER IF EXISTS skills_ai;
            CREATE TRIGGER skills_ai AFTER INSERT ON skills BEGIN
              INSERT INTO skills_fts(rowid, name, content) VALUES (new.rowid, new.name, new.content);
            END;

            DROP TRIGGER IF EXISTS skills_ad;
            CREATE TRIGGER skills_ad AFTER DELETE ON skills BEGIN
              INSERT INTO skills_fts(skills_fts, rowid, name, content) VALUES('delete', old.rowid, old.name, old.content);
            END;

            DROP TRIGGER IF EXISTS skills_au;
            CREATE TRIGGER skills_au AFTER UPDATE ON skills BEGIN
              INSERT INTO skills_fts(skills_fts, rowid, name, content) VALUES('delete', old.rowid, old.name, old.content);
              INSERT INTO skills_fts(rowid, name, content) VALUES (new.rowid, new.name, new.content);
            END;
        ";

        sqlx::query(schema).execute(&self.pool).await?;

        // Manual column migrations for existing tables if needed (not dropping)
        // Check if pinned column exists in skills (added in v3)
        let _ = sqlx::query("ALTER TABLE skills ADD COLUMN pinned BOOLEAN DEFAULT false")
            .execute(&self.pool)
            .await;

        Ok(())
    }

    pub async fn new_in_memory() -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await?;

        let db = Self { pool };
        db.initialize_schema().await?;
        
        sqlx::query(&format!("PRAGMA user_version = {CURRENT_SCHEMA_VERSION}"))
            .execute(&db.pool)
            .await?;
            
        Ok(db)
    }

    #[must_use]
    pub const fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}
