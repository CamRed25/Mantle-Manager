//! Root error type for `mantle_core`.
//!
//! All public-facing functions in this crate return `Result<T, MantleError>`.
//! Callers in the application layer may convert with `anyhow::Error` via the
//! automatic `From<MantleError>` impl that `thiserror` generates.

use thiserror::Error;

/// Unified error enum for all `mantle_core` operations.
///
/// Each variant captures the subsystem it originates from, providing
/// structured context without losing the underlying cause.
#[derive(Debug, Error)]
pub enum MantleError {
    /// VFS mount, unmount, or namespace operation failed.
    #[error("VFS error: {0}")]
    Vfs(String),

    /// Archive extraction or inspection failed.
    #[error("Archive error: {0}")]
    Archive(String),

    /// `SQLite` / rusqlite database error.
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// Filesystem I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Game detection failed or game directory is invalid.
    #[error("Game error: {0}")]
    Game(String),

    /// Plugin (ESP/ESM/ESL) parsing error.
    #[error("Plugin error: {0}")]
    Plugin(String),

    /// Configuration parse or write error.
    #[error("Config error: {0}")]
    Config(String),

    /// Conflict graph construction error.
    #[error("Conflict error: {0}")]
    Conflict(String),

    /// Profile operation failed.
    #[error("Profile error: {0}")]
    Profile(String),

    /// A required resource was not found.
    #[error("Not found: {0}")]
    NotFound(String),

    /// SKSE installer error (download, extraction, or validation failure).
    #[error("SKSE installer error: {0}")]
    Skse(String),
}
