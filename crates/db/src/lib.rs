#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
use anyhow::{Context, Result};
use directories::ProjectDirs;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::path::PathBuf;

pub mod models;
pub mod store;

#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    pub async fn new() -> Result<Self> {
        let db_path = Self::get_db_path()?;

        // Ensure directory exists
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .context("Failed to create database directory")?;
        }

        let options = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true)
            // Optimize for concurrent reads and writes
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .context("Failed to connect to database")?;

        let db = Self { pool };
        db.initialize_schema().await?;

        Ok(db)
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
        let schema = r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

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

            CREATE VIRTUAL TABLE IF NOT EXISTS interactions_fts USING fts5(
                content,
                content='interactions',
                content_rowid='id'
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

            CREATE TABLE IF NOT EXISTS memories (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                key TEXT NOT NULL,
                project_path TEXT DEFAULT 'global',
                content TEXT NOT NULL,
                category TEXT,
                verified BOOLEAN DEFAULT true,
                pinned BOOLEAN DEFAULT false,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                last_accessed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                access_count INTEGER DEFAULT 0,
                UNIQUE(key, project_path)
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                key,
                content,
                content='memories',
                content_rowid='id'
            );

            -- Triggers to keep FTS index updated for long-term memories
            DROP TRIGGER IF EXISTS memories_ai;
            CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN
              INSERT INTO memories_fts(rowid, key, content) VALUES (new.id, new.key, new.content);
            END;

            DROP TRIGGER IF EXISTS memories_ad;
            CREATE TRIGGER memories_ad AFTER DELETE ON memories BEGIN
              INSERT INTO memories_fts(memories_fts, rowid, key, content) VALUES('delete', old.id, old.key, old.content);
            END;

            DROP TRIGGER IF EXISTS memories_au;
            CREATE TRIGGER memories_au AFTER UPDATE ON memories BEGIN
              INSERT INTO memories_fts(memories_fts, rowid, key, content) VALUES('delete', old.id, old.key, old.content);
              INSERT INTO memories_fts(rowid, key, content) VALUES (new.id, new.key, new.content);
            END;

            CREATE TABLE IF NOT EXISTS skills (
                name TEXT PRIMARY KEY,
                description TEXT NOT NULL,
                triggers TEXT NOT NULL, -- Comma-separated or space-separated triggers
                content TEXT NOT NULL,
                pinned BOOLEAN DEFAULT false,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS skills_fts USING fts5(
                name,
                description,
                triggers,
                content='skills',
                content_rowid='name'
            );

            -- Triggers to keep FTS index updated for skills
            DROP TRIGGER IF EXISTS skills_ai;
            CREATE TRIGGER skills_ai AFTER INSERT ON skills BEGIN
              INSERT INTO skills_fts(rowid, name, description, triggers) VALUES (new.name, new.name, new.description, new.triggers);
            END;

            DROP TRIGGER IF EXISTS skills_ad;
            CREATE TRIGGER skills_ad AFTER DELETE ON skills BEGIN
              INSERT INTO skills_fts(skills_fts, rowid, name, description, triggers) VALUES('delete', old.name, old.name, old.description, old.triggers);
            END;

            DROP TRIGGER IF EXISTS skills_au;
            CREATE TRIGGER skills_au AFTER UPDATE ON skills BEGIN
              INSERT INTO skills_fts(skills_fts, rowid, name, description, triggers) VALUES('delete', old.name, old.name, old.description, old.triggers);
              INSERT INTO skills_fts(rowid, name, description, triggers) VALUES (new.name, new.name, new.description, new.triggers);
            END;
            "#;

        sqlx::query(schema)
            .execute(&self.pool)
            .await
            .context("Failed to initialize database schema")?;

        // Ignore errors for this migration in case the column already exists
        let _ = sqlx::query("ALTER TABLE skills ADD COLUMN pinned BOOLEAN DEFAULT false")
            .execute(&self.pool)
            .await;
            
        Ok(())
    }

    #[must_use] 
    pub const fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}
