//! Migration runner for the Mantle Manager `SQLite` database.
//!
//! # Design
//! Each migration is a SQL file embedded at compile time via `include_str!`.
//! The `schema_version` table tracks which migrations have been applied.
//! `run_migrations` is idempotent — calling it on an already-up-to-date
//! database is a no-op.
//!
//! # Adding a Migration
//! 1. Create `src/data/migrations/m00N_description.sql`.
//! 2. Add its `include_str!` to the `MIGRATIONS` slice below.
//! 3. The new SQL must end with:
//!    `INSERT INTO schema_version(version, applied_at) VALUES (N, unixepoch());`
//! 4. Write a round-trip test in `tests/data_migrations.rs`.

use crate::error::MantleError;
use rusqlite::Connection;

// ---------------------------------------------------------------------------
// Migration definitions
// ---------------------------------------------------------------------------

/// All migrations in order, starting from migration 1.
///
/// Index 0 → migration 1, index 1 → migration 2, …
/// Each string is a complete SQL script that, when executed, MUST insert one
/// row into `schema_version` as its last statement.
const MIGRATIONS: &[&str] = &[include_str!("migrations/m001_initial.sql")];

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Apply all pending migrations to `conn`, bringing the database up to the
/// latest schema version.
///
/// # Behaviour
/// - If `schema_version` does not yet exist, all migrations are applied.
/// - If `schema_version` tracks version N, only migrations N+1..latest are
///   applied.
/// - The function is idempotent: calling it on a fully-migrated database is a
///   no-op.
///
/// # Parameters
/// - `conn`: An open `rusqlite::Connection`.
///
/// # Returns
/// `Ok(())` on success.  `Err(MantleError::Database(_))` if any migration
/// fails, or if the `schema_version` table is in an unexpected state.
///
/// # Side Effects
/// Modifies the database schema (CREATE TABLE, CREATE INDEX, INSERT).
///
/// # Panics
/// Panics if a migration index cannot be converted to `u32` — this is
/// unreachable in practice since the `MIGRATIONS` slice has far fewer than
/// `u32::MAX` entries.
///
/// # Errors
/// Returns [`MantleError::Database`] if any migration SQL fails or if the
/// `schema_version` table is in an unexpected state.
pub fn run_migrations(conn: &Connection) -> Result<(), MantleError> {
    let current_version = current_schema_version(conn)?;
    let pending_start = current_version as usize; // 0-based index into MIGRATIONS

    if pending_start >= MIGRATIONS.len() {
        // Already up to date.
        return Ok(());
    }

    for (i, sql) in MIGRATIONS.iter().enumerate().skip(pending_start) {
        let migration_number =
            u32::try_from(i + 1).expect("migration index cannot exceed u32::MAX");
        apply_migration_sql(conn, migration_number, sql)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Return the current schema version recorded in the database.
///
/// Returns `0` if `schema_version` does not yet exist (fresh database).
///
/// # Parameters
/// - `conn`: An open `rusqlite::Connection`.
///
/// # Returns
/// The maximum version number present in `schema_version`, or `0` if the
/// table is absent or empty.
fn current_schema_version(conn: &Connection) -> Result<u32, MantleError> {
    // Check whether schema_version exists at all.
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='schema_version'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .map_err(MantleError::Database)?;

    if !table_exists {
        return Ok(0);
    }

    // Read max version; if table is empty treat as 0.
    let version: u32 = conn
        .query_row("SELECT COALESCE(MAX(version), 0) FROM schema_version", [], |row| {
            row.get::<_, u32>(0)
        })
        .map_err(MantleError::Database)?;

    Ok(version)
}

/// Execute a single migration SQL script within an exclusive transaction.
///
/// # Parameters
/// - `conn`: An open `rusqlite::Connection`.
/// - `migration_number`: Human-readable migration number (for error messages).
/// - `sql`: Complete SQL script to execute.
///
/// # Returns
/// `Ok(())` on success, or `Err(MantleError::Database(_))` if the script or
/// commit fails.
///
/// # Side Effects
/// Executes `sql` against the database inside an exclusive transaction.
fn apply_migration_sql(
    conn: &Connection,
    _migration_number: u32,
    sql: &str,
) -> Result<(), MantleError> {
    // Execute the migration SQL wrapped in an exclusive transaction.
    // The SQL itself already contains an INSERT into schema_version, so we
    // do not need to add the version bump here — just bracket with
    // BEGIN/COMMIT.  execute_batch handles multi-statement SQL.
    conn.execute_batch(&format!("BEGIN EXCLUSIVE;\n{sql}\nCOMMIT;"))
        .map_err(MantleError::Database)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that `run_migrations` applies cleanly to a blank in-memory DB
    /// and that `schema_version` ends up at 1.
    #[test]
    fn run_migrations_clean_apply() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("first apply must succeed");

        let version: u32 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as u32);
    }

    /// Verify that calling `run_migrations` twice does not error or duplicate
    /// the schema_version row.
    #[test]
    fn run_migrations_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap(); // must not panic

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, MIGRATIONS.len() as i64, "version rows must not duplicate");
    }

    /// Verify every expected table exists after `run_migrations`.
    #[test]
    fn all_tables_exist_after_migration() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        let expected = [
            "schema_version",
            "mods",
            "mod_files",
            "profiles",
            "profile_mods",
            "load_order",
            "downloads",
            "plugin_settings",
            "conflicts",
        ];

        for table in &expected {
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    rusqlite::params![table],
                    |row| row.get::<_, i64>(0),
                )
                .map(|n| n > 0)
                .unwrap_or(false);
            assert!(exists, "table '{table}' must exist after migration");
        }
    }

    /// Verify foreign_keys are enforced after applying migrations.
    #[test]
    fn foreign_key_enforcement_active() {
        let conn = Connection::open_in_memory().unwrap();
        // Enable foreign keys (normally done in Database::open).
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        run_migrations(&conn).unwrap();

        // Attempting to insert a mod_files row with a non-existent mod_id
        // must fail due to the FK constraint.
        let result = conn.execute(
            "INSERT INTO mod_files(mod_id, rel_path, file_hash, file_size)
             VALUES (9999, 'data/test.esp', 'abc123', 512)",
            [],
        );
        assert!(result.is_err(), "FK violation must be rejected");
    }
}
