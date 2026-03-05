//! Shared helpers used by multiple page modules.
//!
//! Currently houses the database-access utility functions that would
//! otherwise be duplicated in every page that performs write operations.

use mantle_core::{config::default_db_path, data::Database, Error as CoreError};

// ─── Database helpers ─────────────────────────────────────────────────────────

/// Open the database, run `f`, and return its result unchanged.
///
/// # Errors
/// Returns `Err(CoreError)` if `Database::open` fails or if `f` returns an error.
///
/// # Parameters
/// - `f`: Closure receiving a `&Database` and returning `Result<T, CoreError>`.
pub(crate) fn with_db<F, T>(f: F) -> Result<T, CoreError>
where
    F: FnOnce(&Database) -> Result<T, CoreError>,
{
    let db = Database::open(&default_db_path())?;
    f(&db)
}

/// Open the database, run `f`, and map any error to [`String`].
///
/// Convenience wrapper around [`with_db`] for call sites that propagate
/// errors as strings (e.g., via `tracing::warn!("{e}")`).
///
/// # Parameters
/// - `f`: Closure receiving a `&Database` and returning `Result<T, CoreError>`.
pub(crate) fn with_db_s<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce(&Database) -> Result<T, CoreError>,
{
    with_db(f).map_err(|e| e.to_string())
}
