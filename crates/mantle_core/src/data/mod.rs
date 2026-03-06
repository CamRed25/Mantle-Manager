//! `SQLite` data layer for Mantle Manager.
//!
//! All persistent application state is stored here. No other module reads
//! from or writes to `SQLite` directly — SQL does not appear outside of `data/`.
//!
//! # Connection Semantics
//! `Database` wraps a `Mutex<rusqlite::Connection>` so it can be shared across
//! async tasks. All blocking operations must be dispatched via
//! `tokio::task::spawn_blocking` at call sites in the UI layer — never block
//! an async executor thread directly.
//!
//! # Database Location
//! | Deployment | Path |
//! |------------|------|
//! | Flatpak    | `~/.var/app/io.mantlemanager.MantleManager/data/mantle.db` |
//! | Native     | `~/.local/share/mantle-manager/mantle.db` |
//!
//! # Migrations
//! Migrations live in `src/data/migrations/` as numbered SQL files.
//! Call [`run_migrations`] (free function) on a fresh connection to bring
//! a database up to the latest schema version.

pub mod downloads;
pub mod mod_files;
pub mod mods;
pub mod profiles;
mod schema;

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OpenFlags};

use crate::error::MantleError;

// ---------------------------------------------------------------------------
// Public re-exports
// ---------------------------------------------------------------------------

/// Apply all pending schema migrations to `conn`.
///
/// This is the canonical public entry point used by test helpers and
/// application startup.  See [`schema::run_migrations`] for full
/// documentation.
///
/// # Parameters
/// - `conn`: An open `rusqlite::Connection`.
///
/// # Returns
/// `Ok(())` on success.  `Err(MantleError::Database(_))` on failure.
pub use schema::run_migrations;

// ---------------------------------------------------------------------------
// Database struct
// ---------------------------------------------------------------------------

/// Owned, thread-safe handle to the Mantle Manager `SQLite` database.
///
/// Wraps a `rusqlite::Connection` in a `Mutex` so the handle can be shared
/// across `tokio::task::spawn_blocking` closures without cloning.
///
/// # Example
/// ```no_run
/// use std::path::Path;
/// use mantle_core::data::Database;
///
/// let db = Database::open(Path::new("/tmp/mantle.db"))
///     .expect("failed to open database");
/// ```
pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    // -----------------------------------------------------------------------
    // Constructors
    // -----------------------------------------------------------------------

    /// Open (or create) the database at `path` and apply any pending
    /// migrations.
    ///
    /// Sets the required `SQLite` PRAGMAs:
    /// - `journal_mode = WAL`
    /// - `foreign_keys = ON`
    /// - `synchronous = NORMAL`
    ///
    /// The connection is opened with `SQLITE_OPEN_FULLMUTEX` to allow safe
    /// use from multiple threads.
    ///
    /// # Parameters
    /// - `path`: Filesystem path to the `.db` file.  The directory must
    ///   already exist.
    ///
    /// # Returns
    /// A fully-migrated `Database` on success, or
    /// `Err(MantleError::Database(_))` / `Err(MantleError::Io(_))` on
    /// failure.
    ///
    /// # Side Effects
    /// Creates `path` if it does not exist.  Modifies schema if migrations
    /// are pending.
    ///
    /// # Errors
    /// Returns [`MantleError::Database`] if the connection cannot be opened,
    /// PRAGMAs fail, or migrations fail.
    pub fn open(path: &Path) -> Result<Self, MantleError> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_FULL_MUTEX,
        )
        .map_err(MantleError::Database)?;

        set_pragmas(&conn)?;
        run_migrations(&conn)?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Open an in-memory database and apply all migrations.
    ///
    /// Intended for unit tests and integration tests only.  The database
    /// disappears when the `Database` is dropped.
    ///
    /// # Returns
    /// A fully-migrated, empty `Database` on success.
    ///
    /// # Panics
    /// Does not panic (in-memory open always succeeds in rusqlite).
    ///
    /// # Errors
    /// Returns [`MantleError::Database`] if PRAGMAs or migrations fail.
    pub fn open_in_memory() -> Result<Self, MantleError> {
        let conn = Connection::open_in_memory().map_err(MantleError::Database)?;
        set_pragmas(&conn)?;
        run_migrations(&conn)?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    // -----------------------------------------------------------------------
    // Migration
    // -----------------------------------------------------------------------

    /// Re-run the migration runner against this database's connection.
    ///
    /// Normally migrations are applied at `open` time, so this method is
    /// only needed in tests or tooling that constructs a `Database` manually.
    ///
    /// # Returns
    /// `Ok(())` if all migrations are up to date after the call.
    ///
    /// # Panics
    /// Panics if the database `Mutex` has been poisoned (should never occur
    /// in normal operation).
    ///
    /// # Errors
    /// Returns [`MantleError::Database`] if a migration SQL fails.
    pub fn run_migrations(&self) -> Result<(), MantleError> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        run_migrations(&conn)
    }

    // -----------------------------------------------------------------------
    // Internal connection accessor
    // -----------------------------------------------------------------------

    /// Borrow the inner connection for use by submodule CRUD functions.
    ///
    /// The lock is held only for the duration of the closure.  Do not call
    /// across an `await` point.
    ///
    /// # Parameters
    /// - `f`: Closure receiving `&Connection`.
    ///
    /// # Returns
    /// Whatever `f` returns.
    ///
    /// # Panics
    /// Panics if the connection mutex has been poisoned (should never occur
    /// in normal operation).
    pub fn with_conn<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&Connection) -> T,
    {
        let conn = self.conn.lock().expect("database mutex poisoned");
        f(&conn)
    }
}

// ---------------------------------------------------------------------------
// PRAGMA setup
// ---------------------------------------------------------------------------

/// Apply the required `SQLite` PRAGMAs to `conn`.
///
/// Must be called immediately after opening any connection, before any
/// schema-modifying statements.
///
/// # Parameters
/// - `conn`: An open `rusqlite::Connection`.
///
/// # Returns
/// `Ok(())` on success, or `Err(MantleError::Database(_))` if any PRAGMA
/// fails.
///
/// # Side Effects
/// Switches the journal mode to WAL, enables foreign keys, and sets
/// synchronous to NORMAL.
fn set_pragmas(conn: &Connection) -> Result<(), MantleError> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         PRAGMA synchronous = NORMAL;",
    )
    .map_err(MantleError::Database)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// `Database::open_in_memory()` must succeed and produce a migrated DB.
    #[test]
    fn open_in_memory_succeeds() {
        let db = Database::open_in_memory().expect("open_in_memory must not fail");
        db.with_conn(|conn| {
            let version: u32 = conn
                .query_row("SELECT MAX(version) FROM schema_version", [], |row| row.get(0))
                .unwrap();
            assert!(version >= 1);
        });
    }

    /// `Database::open` to a temp file must succeed.
    #[test]
    fn open_file_succeeds() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.db");
        let db = Database::open(&path).expect("file open must succeed");
        db.with_conn(|conn| {
            let v: u32 = conn
                .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
                .unwrap();
            assert!(v >= 1);
        });
    }

    /// PRAGMAs must be applied: foreign_keys must be ON.
    #[test]
    fn foreign_keys_pragma_is_on() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            let fk_on: i64 = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap();
            assert_eq!(fk_on, 1, "foreign_keys must be ON");
        });
    }

    /// WAL mode must be active after open.
    #[test]
    fn wal_mode_is_active() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Database::open(&dir.path().join("wal.db")).unwrap();
        db.with_conn(|conn| {
            let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
            assert_eq!(mode, "wal");
        });
    }

    /// The free-function `run_migrations` is accessible at
    /// `mantle_core::data::run_migrations`.
    #[test]
    fn free_fn_run_migrations_accessible() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        set_pragmas(&conn).unwrap();
        run_migrations(&conn).expect("free function must succeed");
    }
}
