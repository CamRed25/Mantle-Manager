//! Download queue persistence layer.
//!
//! Stores download job history in the `downloads` table (added by migration
//! `m002_add_downloads`).  The in-memory `DownloadQueue` in `mantle_ui` uses
//! these functions to:
//! - Persist each new job via [`upsert_download`].
//! - Update status on every terminal (Complete/Failed/Cancelled) transition
//!   via [`update_download_status`].
//! - Reload the non-completed queue across restarts via [`load_active_downloads`].
//!
//! # Status values (stored as TEXT)
//! | Rust variant          | DB string       |
//! |-----------------------|-----------------|
//! | `Queued`              | `"queued"`      |
//! | `InProgress{…}`       | `"in_progress"` |
//! | `Complete{bytes}`     | `"complete"`    |
//! | `Failed(msg)`         | `"failed"`      |
//! | `Cancelled`           | `"cancelled"`   |

use rusqlite::{Connection, OptionalExtension};

use crate::error::MantleError;

// ─── Persisted download record ────────────────────────────────────────────────

/// A single download job as stored in the `downloads` table.
///
/// Mirrors the in-memory `DownloadJob` in `mantle_ui`, serialised for `SQLite`.
#[derive(Debug, Clone)]
pub struct PersistedDownload {
    /// Stable UUID — matches the in-memory `DownloadJob.id` (as string).
    pub id: String,
    /// Remote HTTPS URL that was or will be fetched.
    pub url: String,
    /// Human-readable mod / file name shown in the UI.
    pub filename: String,
    /// Absolute destination path for the archive.
    pub dest_path: String,
    /// One of: `"queued"`, `"in_progress"`, `"complete"`, `"failed"`, `"cancelled"`.
    pub status: String,
    /// Download progress in the range `[0.0, 1.0]`.
    pub progress: f64,
    /// Total file size in bytes, if known.
    pub total_bytes: Option<u64>,
    /// Unix timestamp (seconds) when the job was first created.
    pub added_at: i64,
}

impl PersistedDownload {
    /// Returns `true` if the status represents a non-retryable terminal state.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        matches!(self.status.as_str(), "complete" | "cancelled")
    }

    /// Returns `true` if the download should be re-enqueued on app restart.
    ///
    /// A "queued" or "`in_progress`" job was interrupted mid-session; we offer
    /// to resume it on the next launch.
    #[must_use]
    pub fn should_resume(&self) -> bool {
        matches!(self.status.as_str(), "queued" | "in_progress")
    }
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Insert or replace a download record.
///
/// Uses `INSERT OR REPLACE` so calling this with an existing `id` is
/// equivalent to a full-row update.
///
/// # Parameters
/// - `conn`: Open `SQLite` connection.
/// - `d`:    The download record to persist.
///
/// # Errors
/// Returns [`MantleError::Database`] on any `SQLite` failure.
pub fn upsert_download(conn: &Connection, d: &PersistedDownload) -> Result<(), MantleError> {
    conn.execute(
        "INSERT OR REPLACE INTO downloads
             (id, url, filename, dest_path, status, progress, total_bytes, added_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, unixepoch())",
        rusqlite::params![
            d.id,
            d.url,
            d.filename,
            d.dest_path,
            d.status,
            d.progress,
            d.total_bytes.map(|b| i64::try_from(b).unwrap_or(i64::MAX)),
            d.added_at,
        ],
    )
    .map_err(MantleError::Database)?;
    Ok(())
}

/// Update the status and progress of an existing download.
///
/// This is cheaper than [`upsert_download`] for monitoring updates where
/// only status/progress change.
///
/// # Parameters
/// - `conn`:     Open `SQLite` connection.
/// - `id`:       UUID of the job to update.
/// - `status`:   New status string (e.g. `"complete"`).
/// - `progress`: New progress value `[0.0, 1.0]`.
///
/// # Errors
/// Returns [`MantleError::Database`] on any `SQLite` failure.
pub fn update_download_status(
    conn: &Connection,
    id: &str,
    status: &str,
    progress: f64,
) -> Result<(), MantleError> {
    conn.execute(
        "UPDATE downloads SET status = ?1, progress = ?2, updated_at = unixepoch()
         WHERE id = ?3",
        rusqlite::params![status, progress, id],
    )
    .map_err(MantleError::Database)?;
    Ok(())
}

/// Load all non-cleared download records ordered by `added_at`.
///
/// Returns every row where `status` is NOT `"complete"` and NOT `"cancelled"`.
/// This provides the queue to display in the UI on startup.
///
/// # Parameters
/// - `conn`: Open `SQLite` connection.
///
/// # Returns
/// All active/interrupted download records in insertion order.
///
/// # Errors
/// Returns [`MantleError::Database`] on any `SQLite` failure.
pub fn load_active_downloads(conn: &Connection) -> Result<Vec<PersistedDownload>, MantleError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, url, filename, dest_path, status, progress, total_bytes, added_at
             FROM downloads
             WHERE status NOT IN ('complete', 'cancelled')
             ORDER BY added_at ASC",
        )
        .map_err(MantleError::Database)?;

    let rows = stmt
        .query_map([], |row| {
            Ok(PersistedDownload {
                id: row.get(0)?,
                url: row.get(1)?,
                filename: row.get(2)?,
                dest_path: row.get(3)?,
                status: row.get(4)?,
                progress: row.get(5)?,
                total_bytes: row.get::<_, Option<i64>>(6)?.map(|b| u64::try_from(b).unwrap_or(0)),
                added_at: row.get(7)?,
            })
        })
        .map_err(MantleError::Database)?
        .filter_map(Result::ok)
        .collect();

    Ok(rows)
}

/// Load all download records (active + historical) for history display.
///
/// Returns every row ordered by `added_at` descending (newest first).
///
/// # Parameters
/// - `conn`: Open `SQLite` connection.
///
/// # Errors
/// Returns [`MantleError::Database`] on any `SQLite` failure.
pub fn load_all_downloads(conn: &Connection) -> Result<Vec<PersistedDownload>, MantleError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, url, filename, dest_path, status, progress, total_bytes, added_at
             FROM downloads
             ORDER BY added_at DESC",
        )
        .map_err(MantleError::Database)?;

    let rows = stmt
        .query_map([], |row| {
            Ok(PersistedDownload {
                id: row.get(0)?,
                url: row.get(1)?,
                filename: row.get(2)?,
                dest_path: row.get(3)?,
                status: row.get(4)?,
                progress: row.get(5)?,
                total_bytes: row.get::<_, Option<i64>>(6)?.map(|b| u64::try_from(b).unwrap_or(0)),
                added_at: row.get(7)?,
            })
        })
        .map_err(MantleError::Database)?
        .filter_map(Result::ok)
        .collect();

    Ok(rows)
}

/// Delete a single download record by ID.
///
/// Used when the user explicitly clears a completed or failed job.
///
/// # Parameters
/// - `conn`: Open `SQLite` connection.
/// - `id`:   UUID of the job to remove.
///
/// # Errors
/// Returns [`MantleError::Database`] on any `SQLite` failure.
pub fn delete_download(conn: &Connection, id: &str) -> Result<(), MantleError> {
    conn.execute("DELETE FROM downloads WHERE id = ?1", rusqlite::params![id])
        .map_err(MantleError::Database)?;
    Ok(())
}

/// Check whether a download record exists by ID.
///
/// # Parameters
/// - `conn`: Open `SQLite` connection.
/// - `id`:   UUID to look up.
///
/// # Errors
/// Returns [`MantleError::Database`] on any `SQLite` failure.
pub fn download_exists(conn: &Connection, id: &str) -> Result<bool, MantleError> {
    conn.query_row("SELECT 1 FROM downloads WHERE id = ?1", rusqlite::params![id], |_| Ok(()))
        .optional()
        .map(|o| o.is_some())
        .map_err(MantleError::Database)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;
    use crate::data::run_migrations;

    /// Helper: open an in-memory DB and apply all migrations.
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory DB");
        run_migrations(&conn).expect("migrations");
        conn
    }

    fn sample_download(id: &str, status: &str) -> PersistedDownload {
        PersistedDownload {
            id: id.to_string(),
            url: format!("https://example.com/{id}.zip"),
            filename: format!("{id}.zip"),
            dest_path: format!("/tmp/downloads/{id}.zip"),
            status: status.to_string(),
            progress: if status == "complete" { 1.0 } else { 0.0 },
            total_bytes: Some(1024),
            added_at: 1_700_000_000,
        }
    }

    /// `upsert_download` inserts a new row; `load_active_downloads` returns it.
    #[test]
    fn upsert_and_load_active() {
        let conn = test_conn();
        let d = sample_download("abc123", "queued");
        upsert_download(&conn, &d).expect("upsert");

        let rows = load_active_downloads(&conn).expect("load");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "abc123");
        assert_eq!(rows[0].status, "queued");
    }

    /// Completed downloads are excluded from `load_active_downloads`.
    #[test]
    fn completed_downloads_not_active() {
        let conn = test_conn();
        upsert_download(&conn, &sample_download("x1", "complete")).expect("upsert");
        upsert_download(&conn, &sample_download("x2", "cancelled")).expect("upsert");
        upsert_download(&conn, &sample_download("x3", "queued")).expect("upsert");

        let active = load_active_downloads(&conn).expect("load");
        assert_eq!(active.len(), 1, "only queued job should be active");
        assert_eq!(active[0].id, "x3");
    }

    /// `update_download_status` transitions the status field correctly.
    #[test]
    fn update_status_persists() {
        let conn = test_conn();
        upsert_download(&conn, &sample_download("job1", "queued")).expect("upsert");
        update_download_status(&conn, "job1", "complete", 1.0).expect("update");

        let all = load_all_downloads(&conn).expect("load all");
        assert_eq!(all[0].status, "complete");
        assert!((all[0].progress - 1.0).abs() < f64::EPSILON);
    }

    /// `delete_download` removes the row; `download_exists` confirms absence.
    #[test]
    fn delete_removes_row() {
        let conn = test_conn();
        upsert_download(&conn, &sample_download("to_delete", "failed")).expect("upsert");
        assert!(download_exists(&conn, "to_delete").expect("exists"));

        delete_download(&conn, "to_delete").expect("delete");
        assert!(!download_exists(&conn, "to_delete").expect("exists after delete"));
    }

    /// `should_resume` returns true for queued/in_progress, false otherwise.
    #[test]
    fn should_resume_flags() {
        assert!(sample_download("a", "queued").should_resume());
        assert!(sample_download("b", "in_progress").should_resume());
        assert!(!sample_download("c", "complete").should_resume());
        assert!(!sample_download("d", "failed").should_resume());
        assert!(!sample_download("e", "cancelled").should_resume());
    }
}
