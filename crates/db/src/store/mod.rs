//! Domain-organized SQLite data access methods over a [`sqlx::SqlitePool`].

mod broadcasts;
mod interactions;
mod memories;
mod msgbar;
mod sessions;
mod skills;
mod telemetry;

pub use broadcasts::{MAX_CACHED_BROADCAST_ARTICLES, NEW_NOTIFICATION_MAX_AGE_DAYS};

use sqlx::{QueryBuilder, Sqlite, SqlitePool};

/// Maximum number of bound values per SQL `IN (...)` batch, safely below SQLite's
/// `SQLITE_MAX_VARIABLE_NUMBER` ceiling (`32,766` modern / `999` legacy).
pub(crate) const SQLITE_BIND_CHUNK_SIZE: usize = 500;

/// Normalizes an optional project directory scope, treating blank or `"global"` values as `None`
/// (global-only scope) and trimming trailing `'/'` separators on concrete paths.
#[must_use]
pub(crate) fn normalize_scope(project_path: Option<&str>) -> Option<&str> {
    let trimmed = project_path?.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("global") {
        return None;
    }
    if trimmed == "/" {
        Some("/")
    } else {
        let stripped = trimmed.trim_end_matches('/');
        (!stripped.is_empty()).then_some(stripped)
    }
}

/// Appends a SQLite `WHERE` clause predicate restricting `col` (`project_path`) to:
/// - `'global'` / `NULL` / `''` when `scope` is `None`;
/// - `'global'` / `NULL` / `''` or exact `'/'` when `scope` is `Some("/")` (never matching all of `/`);
/// - `'global'`, exact directory match, ancestor directory match, or child directory match when
///   `scope` is a concrete workspace path — using `substr` rather than `LIKE` so `_` and `%`
///   characters in directory names are never treated as wildcards.
pub(crate) fn push_scope_filter(
    builder: &mut QueryBuilder<Sqlite>,
    col: &str,
    scope: Option<&str>,
) {
    match scope {
        None => {
            builder.push(format!("({col} IS NULL OR {col} = '' OR {col} = 'global')"));
        }
        Some("/") => {
            builder.push(format!(
                "({col} IS NULL OR {col} = '' OR {col} = 'global' OR {col} = '/')"
            ));
        }
        Some(path) => {
            builder.push(format!(
                "({col} IS NULL OR {col} = '' OR {col} = 'global' OR rtrim({col}, '/') = "
            ));
            builder.push_bind(path.to_owned());
            builder.push(format!(
                " OR (rtrim({col}, '/') != '' AND rtrim({col}, '/') != '/' AND substr("
            ));
            builder.push_bind(path.to_owned());
            builder.push(format!(
                ", 1, length(rtrim({col}, '/')) + 1) = rtrim({col}, '/') || '/') OR substr(rtrim({col}, '/'), 1, length("
            ));
            builder.push_bind(path.to_owned());
            builder.push(") + 1) = ");
            builder.push_bind(path.to_owned());
            builder.push(" || '/')");
        }
    }
}

/// Appends an `ORDER BY` expression ranking exact `project_path` matches (`0`) ahead of
/// same-repository ancestor/child directory matches (`1`) and global entries (`2`).
pub(crate) fn push_scope_rank(
    builder: &mut QueryBuilder<Sqlite>,
    col: &str,
    scope: Option<&str>,
) {
    match scope {
        None => {
            builder.push("NULL");
        }
        Some(path) => {
            builder.push(format!("CASE WHEN rtrim({col}, '/') = "));
            builder.push_bind(path.to_owned());
            builder.push(format!(
                " THEN 0 WHEN ({col} IS NULL OR {col} = '' OR {col} = 'global') THEN 2 ELSE 1 END"
            ));
        }
    }
}

#[derive(Clone, Copy)]
pub struct Store<'a> {
    pub(crate) pool: &'a SqlitePool,
}

impl<'a> Store<'a> {
    #[must_use]
    pub const fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }
}
