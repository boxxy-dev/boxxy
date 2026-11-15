#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]

use anyhow::{Context, Result};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tokio::sync::OnceCell;

pub mod migrations;
pub mod models;
pub mod store;

static DB: OnceCell<Db> = OnceCell::const_new();
pub static DATABASE_WAS_RESET: AtomicBool = AtomicBool::new(false);

const DEFAULT_MAX_CONNECTIONS: u32 = 5;

#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// Opens (or returns the cached process-wide handle to) the default Boxxy SQLite database
    /// at `<config_dir>/boxxyclaw/boxxy.db`, running any pending journaled migrations.
    pub async fn new() -> Result<Self> {
        let db = DB
            .get_or_try_init(|| async {
                let db_path = Self::default_db_path();
                Self::open_at_with_max_connections(&db_path, DEFAULT_MAX_CONNECTIONS).await
            })
            .await?;

        Ok(db.clone())
    }

    /// Opens a file-backed `Db` at an explicit `db_path` without touching the process-wide
    /// `OnceCell` singleton. Useful for isolated disk/WAL/migration tests and headless tools.
    pub async fn open_at(db_path: impl AsRef<Path>) -> Result<Self> {
        Self::open_at_with_max_connections(db_path.as_ref(), DEFAULT_MAX_CONNECTIONS).await
    }

    async fn open_at_with_max_connections(db_path: &Path, max_connections: u32) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .context("Failed to create database directory")?;
            restrict_dir_permissions(parent);
        }

        // Pre-create missing DB file with 0600 permissions before SQLite opens it so another
        // local user cannot read it in the window before chmod, without opening+closing an
        // existing DB file (which would release POSIX fcntl advisory locks).
        ensure_owner_only_db_files(db_path);

        let options = SqliteConnectOptions::new()
            .filename(db_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(10))
            .foreign_keys(true)
            .pragma("temp_store", "MEMORY")
            .pragma("cache_size", "-8000");

        let pool = SqlitePoolOptions::new()
            .max_connections(max_connections)
            .connect_with(options)
            .await
            .context("Failed to connect to SQLite database")?;

        ensure_owner_only_db_files(db_path);
        migrations::apply(&pool, Some(db_path)).await?;

        Ok(Self { pool })
    }

    /// Creates an isolated in-memory SQLite database initialized with the canonical latest schema.
    pub async fn new_in_memory() -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(":memory:")
            .busy_timeout(Duration::from_secs(5))
            .foreign_keys(true)
            .pragma("temp_store", "MEMORY");

        // Keep `idle_timeout` and `max_lifetime` disabled so the single `:memory:` connection
        // is never reaped while a test or ephemeral caller holds the pool.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(options)
            .await
            .context("Failed to create in-memory SQLite database")?;

        migrations::apply(&pool, None).await?;
        Ok(Self { pool })
    }

    #[must_use]
    pub fn default_db_path() -> PathBuf {
        boxxy_sys_utils::get_config_dir()
            .join("boxxyclaw")
            .join("boxxy.db")
    }

    #[must_use]
    pub const fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    #[must_use]
    pub const fn store(&self) -> store::Store<'_> {
        store::Store::new(&self.pool)
    }
}

pub(crate) fn restrict_owner_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn restrict_dir_permissions(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    let _ = dir;
}

fn ensure_owner_only_db_files(db_path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        if !db_path.exists() {
            let _ = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(db_path);
        }
        restrict_owner_permissions(db_path);
        for suffix in ["-wal", "-shm"] {
            let mut sidecar = db_path.as_os_str().to_os_string();
            sidecar.push(suffix);
            let sidecar_path = PathBuf::from(sidecar);
            if sidecar_path.exists() {
                restrict_owner_permissions(&sidecar_path);
            }
        }
    }
    #[cfg(not(unix))]
    let _ = db_path;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn open_at_persists_across_reopen_and_enforces_owner_only_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("nested").join("boxxy.db");

        let db = Db::open_at(&db_path).await.unwrap();
        db.store()
            .add_memory("theme_mode", None, "dark", Some("preference"), true, false)
            .await
            .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&db_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        db.pool().close().await;

        let reopened = Db::open_at(&db_path).await.unwrap();
        let loaded = reopened
            .store()
            .get_memory_by_key("theme_mode", None)
            .await
            .unwrap()
            .expect("memory persisted across reopen");
        assert_eq!(loaded.content, "dark");
    }
}
