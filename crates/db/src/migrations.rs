//! OpenCode-style journaled SQLite migration engine for `boxxy-db`.
//!
//! Tracks applied schema/data migrations in `migration (id TEXT PRIMARY KEY, time_completed INTEGER NOT NULL)`
//! so commit-based continuous updates, multi-version jumps, and parallel Git branches never collide on
//! sequential `PRAGMA user_version` integers.
//!
//! - **Fresh databases** (`:memory:` or new installs) apply `migrations/schema_latest.sql` in one
//!   transaction and seed the `migration` journal with all current migration IDs.
//! - **Existing databases (`v15+`)** serialize pending upgrades across `boxxy-agent` and
//!   `boxxy-app` via `<db_path>.migrate.lock` (`flock`), create an atomic pre-migration backup
//!   (`<db_path>.bak`), and apply only unrecorded `Migration` steps in individual
//!   `BEGIN IMMEDIATE` transactions.
//! - **Ancient preview databases (`user_version < 15` without a `migration` table)** are backed up
//!   to `<db_path>.bak` and cleanly bootstrapped to the latest schema.

use anyhow::{Context, Result};
use sqlx::{AssertSqlSafe, SqliteConnection, SqlitePool};
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{SystemTime, UNIX_EPOCH};

/// Minimum legacy `PRAGMA user_version` eligible for non-destructive journaled migration.
/// Preview databases older than v15 (without a `migration` table) are reset cleanly.
const LEGACY_MIN_SUPPORTED_PRAGMA_VERSION: i32 = 15;

/// Informational `PRAGMA user_version` written alongside journal updates for external SQLite tools.
const INFORMATIONAL_USER_VERSION: i32 = 18;

const SCHEMA_LATEST_SQL: &str = include_str!("../migrations/schema_latest.sql");

pub type CustomMigrationFn =
    for<'a> fn(&'a mut SqliteConnection) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

#[derive(Clone, Copy)]
pub enum MigrationUp {
    Sql(&'static str),
    Custom(CustomMigrationFn),
}

#[derive(Clone, Copy)]
pub struct Migration {
    /// Unique, immutable identifier (`YYYYMMDDHHMMSS_<slug>`).
    pub id: &'static str,
    /// When `false`, the runner suspends `PRAGMA foreign_keys` outside the transaction before
    /// executing `up` (required by SQLite's 12-step table-rebuild pattern) and runs
    /// `PRAGMA foreign_key_check` before committing.
    pub foreign_keys: bool,
    pub up: MigrationUp,
}

/// Ordered registry of all post-v15 migrations.
///
/// Every new schema change MUST update `migrations/schema_latest.sql` AND append a `Migration`
/// entry here. Unit tests enforce structural equivalence between fresh bootstraps and migrated DBs.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        id: "20261115000001_harden_schema_fts_and_indexes",
        foreign_keys: false,
        up: MigrationUp::Sql(include_str!(
            "../migrations/20261115000001_harden_schema_fts_and_indexes.sql"
        )),
    },
    Migration {
        id: "20261115000002_upgrade_telemetry_journal",
        foreign_keys: true,
        up: MigrationUp::Sql(include_str!(
            "../migrations/20261115000002_upgrade_telemetry_journal.sql"
        )),
    },
    Migration {
        id: "20261126000001_add_broadcast_tables",
        foreign_keys: true,
        up: MigrationUp::Sql(include_str!(
            "../migrations/20261126000001_add_broadcast_tables.sql"
        )),
    },
];

enum BootstrapState {
    FreshBootstrapped,
    ReadyForPending(HashSet<String>),
}

struct MigrationLockGuard {
    file: File,
}

impl Drop for MigrationLockGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Ensures `pool` is initialized and migrated to the current schema.
pub async fn apply(pool: &SqlitePool, db_path: Option<&Path>) -> Result<()> {
    apply_with_migrations(pool, db_path, MIGRATIONS).await
}

pub(crate) async fn apply_with_migrations(
    pool: &SqlitePool,
    db_path: Option<&Path>,
    migrations: &[Migration],
) -> Result<()> {
    if is_fully_migrated_fast_path(pool, migrations).await? {
        return Ok(());
    }

    // Serialize bootstrap, `.bak` creation, and multi-step migrations across concurrent
    // `boxxy-agent` and `boxxy-app` startups so a second process never snapshots a
    // half-migrated database over `<db_path>.bak`.
    let migration_lock = match db_path {
        Some(path) => Some(acquire_migration_lock(path).await?),
        None => None,
    };

    if migration_lock.is_some() && is_fully_migrated_fast_path(pool, migrations).await? {
        return Ok(());
    }

    let mut conn = pool
        .acquire()
        .await
        .context("Failed to acquire SQLite connection for migrations")?;

    let state = prepare_bootstrap_state(&mut conn, db_path, migrations).await?;
    if let BootstrapState::ReadyForPending(completed) = state {
        let has_pending = migrations.iter().any(|m| !completed.contains(m.id));
        if has_pending {
            if let Some(path) = db_path {
                create_pre_migration_backup(&mut conn, path).await;
            }
            apply_only(&mut conn, migrations, &completed).await?;
            sqlx::raw_sql(AssertSqlSafe(format!(
                "PRAGMA user_version = {INFORMATIONAL_USER_VERSION}"
            )))
            .execute(&mut *conn)
            .await?;
        }
    }

    Ok(())
}

async fn acquire_migration_lock(db_path: &Path) -> Result<MigrationLockGuard> {
    let lock_path = migration_lock_path_for(db_path);
    tokio::task::spawn_blocking(move || {
        if let Some(parent) = lock_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts.open(&lock_path).with_context(|| {
            format!(
                "Failed to open database migration lock file {}",
                lock_path.display()
            )
        })?;
        crate::restrict_owner_permissions(&lock_path);
        file.lock().with_context(|| {
            format!(
                "Failed to acquire database migration lock on {}",
                lock_path.display()
            )
        })?;
        Ok(MigrationLockGuard { file })
    })
    .await
    .context("Database migration lock task panicked")?
}

/// Lock-free read check for the 99.9% startup case where all migrations are already applied.
async fn is_fully_migrated_fast_path(pool: &SqlitePool, migrations: &[Migration]) -> Result<bool> {
    let has_journal: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'migration' LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;

    if has_journal.is_none() {
        return Ok(false);
    }

    let rows: Vec<(String,)> = sqlx::query_as("SELECT id FROM migration")
        .fetch_all(pool)
        .await?;
    let completed: HashSet<String> = rows.into_iter().map(|(id,)| id).collect();
    Ok(migrations.iter().all(|m| completed.contains(m.id)))
}

async fn prepare_bootstrap_state(
    conn: &mut SqliteConnection,
    db_path: Option<&Path>,
    migrations: &[Migration],
) -> Result<BootstrapState> {
    sqlx::raw_sql("BEGIN IMMEDIATE").execute(&mut *conn).await?;

    let outcome: Result<Option<BootstrapState>> = async {
        let tables: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        )
        .fetch_all(&mut *conn)
        .await?;

        if tables.is_empty() {
            bootstrap_fresh_schema_in_tx(conn, migrations).await?;
            sqlx::raw_sql("COMMIT").execute(&mut *conn).await?;
            return Ok(Some(BootstrapState::FreshBootstrapped));
        }

        if tables.iter().any(|(name,)| name == "migration") {
            let completed = fetch_completed_ids(conn).await?;
            sqlx::raw_sql("COMMIT").execute(&mut *conn).await?;
            return Ok(Some(BootstrapState::ReadyForPending(completed)));
        }

        let (user_version,): (i32,) = sqlx::query_as("PRAGMA user_version")
            .fetch_one(&mut *conn)
            .await?;

        if user_version < LEGACY_MIN_SUPPORTED_PRAGMA_VERSION {
            // Release transaction before snapshotting `.bak` and toggling `foreign_keys = OFF`.
            sqlx::raw_sql("COMMIT").execute(&mut *conn).await?;
            return Ok(None);
        }

        log::info!(
            "Adopting existing v{user_version} database into the journaled migration system."
        );
        // Defer creating `migration` until `apply_only` runs so `<db_path>.bak` captures the
        // untouched pre-migration database prior to any DDL.
        sqlx::raw_sql("COMMIT").execute(&mut *conn).await?;
        Ok(Some(BootstrapState::ReadyForPending(HashSet::new())))
    }
    .await;

    match outcome {
        Ok(Some(state)) => Ok(state),
        Ok(None) => {
            if let Some(path) = db_path {
                create_pre_migration_backup(conn, path).await;
            }
            reset_legacy_pre_v15_database(conn, migrations).await?;
            Ok(BootstrapState::FreshBootstrapped)
        }
        Err(err) => {
            let _ = sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await;
            Err(err)
        }
    }
}

async fn bootstrap_fresh_schema_in_tx(
    conn: &mut SqliteConnection,
    migrations: &[Migration],
) -> Result<()> {
    sqlx::raw_sql(SCHEMA_LATEST_SQL)
        .execute(&mut *conn)
        .await
        .context("Failed to initialize canonical database schema")?;

    let now_ms = now_epoch_ms();
    for migration in migrations {
        sqlx::query("INSERT OR IGNORE INTO migration (id, time_completed) VALUES (?, ?)")
            .bind(migration.id)
            .bind(now_ms)
            .execute(&mut *conn)
            .await?;
    }

    sqlx::raw_sql(AssertSqlSafe(format!(
        "PRAGMA user_version = {INFORMATIONAL_USER_VERSION}"
    )))
    .execute(&mut *conn)
    .await?;

    Ok(())
}

/// Atomically drops all legacy `< v15` objects and bootstraps the current schema.
async fn reset_legacy_pre_v15_database(
    conn: &mut SqliteConnection,
    migrations: &[Migration],
) -> Result<()> {
    sqlx::raw_sql("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await?;

    let res: Result<()> = async {
        sqlx::raw_sql("BEGIN IMMEDIATE").execute(&mut *conn).await?;

        // Re-check inside lock in case a concurrent process already reset and bootstrapped.
        let has_journal: Option<(i32,)> = sqlx::query_as(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'migration' LIMIT 1",
        )
        .fetch_optional(&mut *conn)
        .await?;
        if has_journal.is_some() {
            sqlx::raw_sql("COMMIT").execute(&mut *conn).await?;
            return Ok(());
        }

        log::warn!(
            "Legacy pre-v{LEGACY_MIN_SUPPORTED_PRAGMA_VERSION} database detected; bootstrapping fresh schema."
        );

        drop_all_user_objects_in_tx(conn).await?;
        bootstrap_fresh_schema_in_tx(conn, migrations).await?;

        sqlx::raw_sql("COMMIT").execute(&mut *conn).await?;
        Ok(())
    }
    .await;

    if res.is_err() {
        let _ = sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await;
    }
    let _ = sqlx::raw_sql("PRAGMA foreign_keys = ON")
        .execute(&mut *conn)
        .await;

    res.context("Failed to bootstrap legacy pre-v15 database")
}

async fn drop_all_user_objects_in_tx(conn: &mut SqliteConnection) -> Result<()> {
    // Drop triggers and views first, then non-shadow tables (dropping an FTS5 virtual table
    // automatically drops its `*_data`, `*_idx`, `*_docsize`, and `*_config` shadow tables).
    for obj_type in ["trigger", "view"] {
        let names: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master WHERE type = ? AND name NOT LIKE 'sqlite_%'",
        )
        .bind(obj_type)
        .fetch_all(&mut *conn)
        .await?;
        for (name,) in names {
            let quoted = quote_ident(&name);
            let kind = obj_type.to_ascii_uppercase();
            sqlx::query(AssertSqlSafe(format!("DROP {kind} IF EXISTS {quoted}")))
                .execute(&mut *conn)
                .await?;
        }
    }

    // Drop virtual tables before regular tables so FTS5 xDestroy cleans up shadow tables first.
    let vtables: Vec<(String,)> = sqlx::query_as(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND sql LIKE 'CREATE VIRTUAL TABLE%' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_all(&mut *conn)
    .await?;
    for (name,) in vtables {
        let quoted = quote_ident(&name);
        sqlx::query(AssertSqlSafe(format!("DROP TABLE IF EXISTS {quoted}")))
            .execute(&mut *conn)
            .await?;
    }

    let tables: Vec<(String,)> = sqlx::query_as(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_all(&mut *conn)
    .await?;
    for (name,) in tables {
        let quoted = quote_ident(&name);
        sqlx::query(AssertSqlSafe(format!("DROP TABLE IF EXISTS {quoted}")))
            .execute(&mut *conn)
            .await?;
    }

    let has_seq: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'sqlite_sequence' LIMIT 1",
    )
    .fetch_optional(&mut *conn)
    .await?;
    if has_seq.is_some() {
        sqlx::raw_sql("DELETE FROM sqlite_sequence")
            .execute(&mut *conn)
            .await?;
    }

    Ok(())
}

/// Applies any migrations in `migrations` not yet recorded in `migration`.
pub(crate) async fn apply_only(
    conn: &mut SqliteConnection,
    migrations: &[Migration],
    initial_completed: &HashSet<String>,
) -> Result<Vec<&'static str>> {
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS migration (id TEXT PRIMARY KEY, time_completed INTEGER NOT NULL)",
    )
    .execute(&mut *conn)
    .await?;

    let mut completed = initial_completed.clone();
    let mut applied_now = Vec::new();

    for migration in migrations {
        if completed.contains(migration.id) {
            continue;
        }

        if apply_single_migration(conn, migration).await? {
            applied_now.push(migration.id);
        }
        completed.insert(migration.id.to_string());
    }

    Ok(applied_now)
}

async fn apply_single_migration(
    conn: &mut SqliteConnection,
    migration: &Migration,
) -> Result<bool> {
    if !migration.foreign_keys {
        sqlx::raw_sql("PRAGMA foreign_keys = OFF")
            .execute(&mut *conn)
            .await?;
    }

    let res: Result<bool> = async {
        sqlx::raw_sql("BEGIN IMMEDIATE").execute(&mut *conn).await?;

        // Re-check inside `BEGIN IMMEDIATE` so concurrent UI + daemon startups never double-apply.
        let exists: Option<(String,)> =
            sqlx::query_as("SELECT id FROM migration WHERE id = ? LIMIT 1")
                .bind(migration.id)
                .fetch_optional(&mut *conn)
                .await?;
        if exists.is_some() {
            sqlx::raw_sql("COMMIT").execute(&mut *conn).await?;
            return Ok(false);
        }

        log::info!("Applying database migration '{}'", migration.id);
        match migration.up {
            MigrationUp::Sql(sql) => {
                sqlx::raw_sql(sql).execute(&mut *conn).await?;
            }
            MigrationUp::Custom(run_fn) => {
                run_fn(conn).await?;
            }
        }

        if !migration.foreign_keys {
            let violations: Vec<(String, Option<i64>, String, i64)> =
                sqlx::query_as("PRAGMA foreign_key_check")
                    .fetch_all(&mut *conn)
                    .await?;
            if !violations.is_empty() {
                anyhow::bail!(
                    "Foreign key check failed during migration '{}': {violations:?}",
                    migration.id
                );
            }
        }

        sqlx::query("INSERT INTO migration (id, time_completed) VALUES (?, ?)")
            .bind(migration.id)
            .bind(now_epoch_ms())
            .execute(&mut *conn)
            .await?;

        sqlx::raw_sql("COMMIT").execute(&mut *conn).await?;
        Ok(true)
    }
    .await;

    if res.is_err() {
        let _ = sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await;
    }
    if !migration.foreign_keys {
        let _ = sqlx::raw_sql("PRAGMA foreign_keys = ON")
            .execute(&mut *conn)
            .await;
    }

    res.with_context(|| format!("Database migration '{}' failed", migration.id))
}

async fn fetch_completed_ids(conn: &mut SqliteConnection) -> Result<HashSet<String>> {
    let rows: Vec<(String,)> = sqlx::query_as("SELECT id FROM migration")
        .fetch_all(&mut *conn)
        .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Creates an atomic WAL-consistent single rolling backup (`<db_path>.bak`) via `VACUUM INTO`
/// right before running pending migrations on an existing file-backed database.
///
/// Writes to `<db_path>.bak.tmp` first and atomically renames over `<db_path>.bak` so an
/// existing backup is never lost if `VACUUM INTO` fails.
async fn create_pre_migration_backup(conn: &mut SqliteConnection, db_path: &Path) {
    let bak_path = backup_path_for(db_path);
    let tmp_bak_path = temp_backup_path_for(db_path);
    let _ = tokio::fs::remove_file(&tmp_bak_path).await;

    let Some(tmp_str) = tmp_bak_path.to_str() else {
        return;
    };

    match sqlx::query("VACUUM INTO ?")
        .bind(tmp_str)
        .execute(&mut *conn)
        .await
    {
        Ok(_) => {
            crate::restrict_owner_permissions(&tmp_bak_path);
            match tokio::fs::rename(&tmp_bak_path, &bak_path).await {
                Ok(()) => {
                    crate::restrict_owner_permissions(&bak_path);
                    log::info!(
                        "Created pre-migration database backup at {}",
                        bak_path.display()
                    );
                }
                Err(err) => {
                    let _ = tokio::fs::remove_file(&tmp_bak_path).await;
                    log::warn!(
                        "Could not finalize pre-migration backup at {}: {err}",
                        bak_path.display()
                    );
                }
            }
        }
        Err(err) => {
            let _ = tokio::fs::remove_file(&tmp_bak_path).await;
            log::warn!(
                "Could not create pre-migration backup at {}: {err}",
                bak_path.display()
            );
        }
    }
}

#[must_use]
pub(crate) fn backup_path_for(db_path: &Path) -> PathBuf {
    let mut os = db_path.as_os_str().to_os_string();
    os.push(".bak");
    PathBuf::from(os)
}

fn temp_backup_path_for(db_path: &Path) -> PathBuf {
    let mut os = db_path.as_os_str().to_os_string();
    os.push(".bak.tmp");
    PathBuf::from(os)
}

fn migration_lock_path_for(db_path: &Path) -> PathBuf {
    let mut os = db_path.as_os_str().to_os_string();
    os.push(".migrate.lock");
    PathBuf::from(os)
}

fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Db;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    const LEGACY_V15_SCHEMA: &str = r"
        CREATE TABLE sessions (
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
            pinned BOOLEAN DEFAULT false,
            cleared_at DATETIME,
            total_tokens BIGINT DEFAULT 0,
            context_tokens BIGINT DEFAULT 0,
            total_cost_usd REAL DEFAULT 0.0,
            last_dream_at DATETIME,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE claw_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            event_json TEXT NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );
        CREATE INDEX idx_claw_events_session_id ON claw_events(session_id);
        CREATE INDEX idx_sessions_pinned_updated ON sessions(pinned DESC, updated_at DESC);
        CREATE TABLE interactions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            project_path TEXT,
            content TEXT NOT NULL,
            metadata TEXT,
            embedding BLOB,
            processing_state TEXT DEFAULT 'raw',
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            last_accessed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );
        CREATE TABLE memories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            key TEXT NOT NULL,
            project_path TEXT NOT NULL,
            content TEXT NOT NULL,
            category TEXT,
            verified BOOLEAN DEFAULT false,
            pinned BOOLEAN DEFAULT false,
            observation_count INTEGER DEFAULT 1,
            confidence_score REAL DEFAULT 0.0,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            last_accessed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            access_count INTEGER DEFAULT 0,
            UNIQUE(key, project_path)
        );
        CREATE TABLE skills (
            name TEXT PRIMARY KEY,
            description TEXT NOT NULL DEFAULT '',
            triggers TEXT NOT NULL DEFAULT '',
            content TEXT NOT NULL,
            metadata TEXT,
            pinned BOOLEAN DEFAULT false,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE msgbar_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            text TEXT NOT NULL,
            attachments_json TEXT,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE telemetry_journal (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            metric_name TEXT NOT NULL,
            value REAL NOT NULL,
            attributes_json TEXT NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );
        CREATE VIRTUAL TABLE interactions_fts USING fts5(content='interactions', content, project_path);
        CREATE TRIGGER interactions_ai AFTER INSERT ON interactions BEGIN
            INSERT INTO interactions_fts(rowid, content) VALUES (new.id, new.content);
        END;
        CREATE VIRTUAL TABLE memories_fts USING fts5(content='memories', key, content, project_path);
        CREATE VIRTUAL TABLE skills_fts USING fts5(content='skills', name, content);
        PRAGMA user_version = 15;
    ";

    #[derive(Debug, PartialEq, Eq)]
    struct TableFootprint {
        name: String,
        columns: Vec<(String, String, i32, Option<String>, i32)>,
        foreign_keys: Vec<(String, String, String, String)>,
        indexes: Vec<(String, i32, Vec<String>)>,
    }

    async fn introspect_schema(pool: &SqlitePool) -> (Vec<TableFootprint>, Vec<(String, String)>) {
        let tables: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
               AND name NOT LIKE '%_data' AND name NOT LIKE '%_idx' \
               AND name NOT LIKE '%_docsize' AND name NOT LIKE '%_config' \
             ORDER BY name ASC",
        )
        .fetch_all(pool)
        .await
        .unwrap();

        let mut footprints = Vec::with_capacity(tables.len());
        for (tname,) in tables {
            let cols: Vec<(i32, String, String, i32, Option<String>, i32)> = sqlx::query_as(
                AssertSqlSafe(format!("PRAGMA table_info(\"{tname}\")")),
            )
            .fetch_all(pool)
            .await
            .unwrap();
            let columns = cols
                .into_iter()
                .map(|(_, name, ty, notnull, dflt, pk)| {
                    (name, ty.to_ascii_uppercase(), notnull, dflt, pk)
                })
                .collect();

            let mut fks: Vec<(String, String, String, String)> = sqlx::query_as::<
                _,
                (i32, i32, String, String, String, String, String, String),
            >(AssertSqlSafe(format!(
                "PRAGMA foreign_key_list(\"{tname}\")"
            )))
            .fetch_all(pool)
            .await
            .unwrap()
            .into_iter()
            .map(|(_, _, table, from, to, _, on_delete, _)| (table, from, to, on_delete))
            .collect();
            fks.sort();

            let idx_list: Vec<(i32, String, i32)> = sqlx::query_as(AssertSqlSafe(format!(
                "SELECT seq, name, \"unique\" FROM pragma_index_list('{tname}') \
                 WHERE origin != 'pk' ORDER BY name ASC"
            )))
            .fetch_all(pool)
            .await
            .unwrap();
            let mut indexes = Vec::new();
            for (_, idx_name, uniq) in idx_list {
                let idx_cols: Vec<(String,)> = sqlx::query_as(AssertSqlSafe(format!(
                    "SELECT COALESCE(name, '<expr>') FROM pragma_index_info('{idx_name}') ORDER BY seqno ASC"
                )))
                .fetch_all(pool)
                .await
                .unwrap();
                let label = if idx_name.starts_with("sqlite_autoindex_") {
                    "<unique_constraint>".to_string()
                } else {
                    idx_name
                };
                indexes.push((label, uniq, idx_cols.into_iter().map(|(c,)| c).collect()));
            }
            indexes.sort();

            footprints.push(TableFootprint {
                name: tname,
                columns,
                foreign_keys: fks,
                indexes,
            });
        }

        let triggers: Vec<(String, String)> = sqlx::query_as(
            "SELECT name, tbl_name FROM sqlite_master WHERE type = 'trigger' ORDER BY name ASC",
        )
        .fetch_all(pool)
        .await
        .unwrap();

        (footprints, triggers)
    }

    async fn raw_memory_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::new()
            .filename(":memory:")
            .foreign_keys(true);
        SqlitePoolOptions::new()
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(opts)
            .await
            .unwrap()
    }

    #[test]
    fn migration_registry_has_unique_non_empty_ids() {
        let mut seen = HashSet::new();
        for m in MIGRATIONS {
            assert!(!m.id.trim().is_empty(), "Migration ID must not be empty");
            assert!(
                seen.insert(m.id),
                "Duplicate migration ID '{}' in MIGRATIONS",
                m.id
            );
        }
    }

    #[tokio::test]
    async fn zero_drift_fresh_schema_matches_migrated_v15_schema_and_preserves_data() {
        let fresh_db = Db::new_in_memory().await.unwrap();

        let v15_pool = raw_memory_pool().await;
        sqlx::raw_sql(LEGACY_V15_SCHEMA)
            .execute(&v15_pool)
            .await
            .unwrap();

        // Seed v15 rows including nullable edge cases and child FK rows
        sqlx::raw_sql(
            r#"
            INSERT INTO sessions (id, name, pinned, cleared_at, total_tokens)
                VALUES ('s1', 'Niko', NULL, '2026-01-01', NULL);
            INSERT INTO claw_events (session_id, event_json) VALUES ('s1', '{"ok":true}');
            INSERT INTO interactions (session_id, project_path, content, processing_state)
                VALUES ('s1', 'src/module', 'Refactored parser traits', NULL);
            INSERT INTO memories (key, project_path, content, verified, pinned)
                VALUES ('editor', 'global', 'neovim', NULL, NULL);
            INSERT INTO skills (name, description, triggers, content, pinned)
                VALUES ('lint-helper', 'Runs clippy checks', 'clippy, lints', 'Check warnings', NULL);
            INSERT INTO msgbar_history (text, attachments_json) VALUES ('hello', NULL);
            INSERT INTO telemetry_journal (metric_name, value, attributes_json)
                VALUES (
                    'app.launch',
                    1.0,
                    '{"install_id":"11111111-1111-4111-8111-111111111111","session_id":"22222222-2222-4222-8222-222222222222","v":1,"os":"Linux"}'
                );
            "#,
        )
        .execute(&v15_pool)
        .await
        .unwrap();

        apply(&v15_pool, None).await.unwrap();
        // Second apply hits the read-only fast path idempotently
        apply(&v15_pool, None).await.unwrap();

        let fresh_fp = introspect_schema(fresh_db.pool()).await;
        let migrated_fp = introspect_schema(&v15_pool).await;
        assert_eq!(fresh_fp, migrated_fp);

        // Verify all user rows survived the table rebuilds intact and FTS5 passes integrity-check
        for fts in ["interactions_fts", "memories_fts", "skills_fts"] {
            sqlx::raw_sql(AssertSqlSafe(format!(
                "INSERT INTO {fts}({fts}) VALUES ('integrity-check')"
            )))
            .execute(&v15_pool)
            .await
            .unwrap();
        }

        let store = crate::store::Store::new(&v15_pool);
        assert_eq!(store.get_claw_events("s1").await.unwrap().len(), 1);
        assert_eq!(
            store.get_recent_msgbar_history(5).await.unwrap()[0].attachments_json,
            "[]"
        );
        // Trigger keyword 'clippy' is now indexed in skills_fts after v15 -> latest migration
        assert_eq!(store.search_skills("clippy", 5).await.unwrap().len(), 1);
        // Valid v15 telemetry_journal row was normalized and preserved across migration
        let migrated_tel = store.fetch_telemetry_batch(10).await.unwrap();
        assert_eq!(migrated_tel.len(), 1);
        assert_eq!(
            migrated_tel[0].install_id,
            "11111111-1111-4111-8111-111111111111"
        );
        assert_eq!(
            migrated_tel[0].session_id,
            "22222222-2222-4222-8222-222222222222"
        );
        assert_eq!(migrated_tel[0].attributes_json, r#"{"os":"Linux"}"#);
    }

    #[tokio::test]
    async fn pre_v15_database_bootstraps_cleanly_with_backup() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("legacy_v12.db");

        let opts = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true);
        let legacy_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE obsolete_preview_table (id INTEGER PRIMARY KEY); \
             INSERT INTO obsolete_preview_table VALUES (1); \
             PRAGMA user_version = 12;",
        )
        .execute(&legacy_pool)
        .await
        .unwrap();
        legacy_pool.close().await;

        let reopened = Db::open_at(&db_path).await.unwrap();
        assert!(backup_path_for(&db_path).exists());

        let obsolete: Option<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'obsolete_preview_table'",
        )
        .fetch_optional(reopened.pool())
        .await
        .unwrap();
        assert_eq!(obsolete, None);

        let applied_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM migration")
            .fetch_one(reopened.pool())
            .await
            .unwrap();
        assert_eq!(applied_count, i64::try_from(MIGRATIONS.len()).unwrap());
    }

    #[tokio::test]
    async fn concurrent_multi_pool_upgrade_preserves_untouched_pre_migration_backup() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("concurrent.db");

        // Seed a v15 database on disk with synthetic rows
        let seed_opts = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
        let seed_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(seed_opts)
            .await
            .unwrap();
        sqlx::raw_sql(LEGACY_V15_SCHEMA)
            .execute(&seed_pool)
            .await
            .unwrap();
        sqlx::raw_sql(
            "INSERT INTO sessions (id, name) VALUES ('s_pre', 'AgentA'); \
             INSERT INTO memories (key, project_path, content, verified) \
                 VALUES ('shell_pref', 'global', 'fish', true);",
        )
        .execute(&seed_pool)
        .await
        .unwrap();
        seed_pool.close().await;

        // Simulate `boxxy-agent` and `boxxy-app` opening the unmigrated DB concurrently,
        // including an extra custom step with an async yield between steps.
        let mut extended = MIGRATIONS.to_vec();
        extended.push(Migration {
            id: "20261201000001_slow_step",
            foreign_keys: true,
            up: MigrationUp::Custom(|conn| {
                Box::pin(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                    sqlx::raw_sql(
                        "CREATE TABLE post_upgrade_marker (marker TEXT PRIMARY KEY); \
                         INSERT INTO post_upgrade_marker VALUES ('done');",
                    )
                    .execute(&mut *conn)
                    .await?;
                    Ok(())
                })
            }),
        });

        let open_pool = || async {
            let opts = SqliteConnectOptions::new()
                .filename(&db_path)
                .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
                .busy_timeout(std::time::Duration::from_secs(5))
                .foreign_keys(true);
            SqlitePoolOptions::new()
                .max_connections(2)
                .connect_with(opts)
                .await
                .unwrap()
        };

        let (pool_a, pool_b) = tokio::join!(open_pool(), open_pool());
        let (res_a, res_b) = tokio::join!(
            apply_with_migrations(&pool_a, Some(&db_path), &extended),
            apply_with_migrations(&pool_b, Some(&db_path), &extended),
        );
        res_a.unwrap();
        res_b.unwrap();

        // Live DB must have all migrations and preserved data
        let live_applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM migration")
            .fetch_one(&pool_a)
            .await
            .unwrap();
        assert_eq!(live_applied, i64::try_from(extended.len()).unwrap());

        // `.bak` must be the exact untouched pre-migration v15 database (NOT overwritten mid-migration)
        let bak_path = backup_path_for(&db_path);
        assert!(bak_path.exists());
        let bak_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::new().filename(&bak_path))
            .await
            .unwrap();

        let bak_has_migration_tbl: Option<(i32,)> = sqlx::query_as(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'migration' LIMIT 1",
        )
        .fetch_optional(&bak_pool)
        .await
        .unwrap();
        assert_eq!(
            bak_has_migration_tbl, None,
            "Backup must capture untouched pre-migration state without mid-upgrade DDL"
        );

        let bak_sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(&bak_pool)
            .await
            .unwrap();
        assert_eq!(bak_sessions, 1);

        let bak_memories: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memories")
            .fetch_one(&bak_pool)
            .await
            .unwrap();
        assert_eq!(bak_memories, 1);
    }

    #[tokio::test]
    async fn multi_step_and_out_of_order_migrations_apply_once_with_backup() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("rolling.db");
        let db = Db::open_at(&db_path).await.unwrap();

        let step_a = Migration {
            id: "20261201000001_step_a",
            foreign_keys: true,
            up: MigrationUp::Sql("CREATE TABLE test_steps (step TEXT PRIMARY KEY); INSERT INTO test_steps VALUES ('a');"),
        };
        let step_b = Migration {
            id: "20261202000001_step_b",
            foreign_keys: true,
            up: MigrationUp::Custom(|conn| {
                Box::pin(async move {
                    sqlx::query("INSERT INTO test_steps VALUES (?)")
                        .bind("b")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            }),
        };
        let step_c = Migration {
            id: "20261203000001_step_c",
            foreign_keys: true,
            up: MigrationUp::Sql("INSERT INTO test_steps VALUES ('c');"),
        };

        let mut conn = db.pool().acquire().await.unwrap();
        create_pre_migration_backup(&mut conn, &db_path).await;
        assert!(backup_path_for(&db_path).exists());

        // Simulate branch C applied first, then branch A + B merged later
        let first_run = apply_only(&mut conn, &[step_a, step_c], &HashSet::new())
            .await
            .unwrap();
        assert_eq!(first_run, vec!["20261201000001_step_a", "20261203000001_step_c"]);

        let done = fetch_completed_ids(&mut conn).await.unwrap();
        let second_run = apply_only(&mut conn, &[step_a, step_b, step_c], &done)
            .await
            .unwrap();
        assert_eq!(second_run, vec!["20261202000001_step_b"]);

        let steps: Vec<(String,)> =
            sqlx::query_as("SELECT step FROM test_steps ORDER BY step ASC")
                .fetch_all(&mut *conn)
                .await
                .unwrap();
        assert_eq!(
            steps,
            vec![
                ("a".to_string(),),
                ("b".to_string(),),
                ("c".to_string(),)
            ]
        );
    }

    #[tokio::test]
    async fn failed_migration_rolls_back_and_restores_foreign_keys_pragma() {
        let db = Db::new_in_memory().await.unwrap();
        let mut conn = db.pool().acquire().await.unwrap();

        let broken = Migration {
            id: "20261299000001_broken_fk_migration",
            foreign_keys: false,
            up: MigrationUp::Sql(
                "CREATE TABLE rollback_probe (id INTEGER PRIMARY KEY); \
                 INSERT INTO claw_events (session_id, event_json) VALUES ('missing_session', '{}');",
            ),
        };

        let err = apply_only(&mut conn, &[broken], &HashSet::new())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("20261299000001_broken_fk_migration"));

        // `rollback_probe` must have been rolled back
        let probe: Option<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'rollback_probe'",
        )
        .fetch_optional(&mut *conn)
        .await
        .unwrap();
        assert_eq!(probe, None);

        // `PRAGMA foreign_keys` must be restored to ON (1)
        let (fk_on,): (i32,) = sqlx::query_as("PRAGMA foreign_keys")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(fk_on, 1);
    }
}
