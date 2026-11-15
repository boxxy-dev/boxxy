//! Domain-organized SQLite data access methods over a [`sqlx::SqlitePool`].

mod interactions;
mod memories;
mod msgbar;
mod sessions;
mod skills;
mod telemetry;

use sqlx::SqlitePool;

/// Maximum number of bound values per SQL `IN (...)` batch, safely below SQLite's
/// `SQLITE_MAX_VARIABLE_NUMBER` ceiling (`32,766` modern / `999` legacy).
pub(crate) const SQLITE_BIND_CHUNK_SIZE: usize = 500;

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
