//! CRUD operations for the `profiles` table.
//!
//! Enforces the exactly-one-active-profile invariant in Rust code.
//! All functions operate synchronously on a `&rusqlite::Connection`.
//! Callers in the UI / async layer must dispatch via
//! `tokio::task::spawn_blocking`.
//!
//! # Active Profile Invariant
//! `SQLite` does not support single-row constraints without triggers, so Rust
//! enforces the rule: at any point in time, exactly one profile may have
//! `is_active = 1`.  [`set_active_profile`] is the only function that
//! changes `is_active`; it clears all other profiles in the same transaction.

use rusqlite::{Connection, OptionalExtension};

use crate::error::MantleError;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// A row from the `profiles` table.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileRecord {
    /// Auto-increment primary key.
    pub id: i64,
    /// Human-readable unique name.
    pub name: String,
    /// Game slug this profile is locked to, or `None`.
    pub game_slug: Option<String>,
    /// Whether this is the currently active profile.
    pub is_active: bool,
    /// Unix timestamp of creation.
    pub created_at: i64,
    /// Unix timestamp of last modification.
    pub updated_at: i64,
}

/// Parameters for inserting a new profile.
#[derive(Debug, Clone)]
pub struct InsertProfile<'a> {
    pub name: &'a str,
    pub game_slug: Option<&'a str>,
}

// ---------------------------------------------------------------------------
// Write operations
// ---------------------------------------------------------------------------

/// Insert a new profile.
///
/// The new profile is created with `is_active = 0`.  Use
/// [`set_active_profile`] to activate it after insertion.
///
/// # Parameters
/// - `conn`: An open, migrated `rusqlite::Connection`.
/// - `rec`: Name and optional game slug for the new profile.
///
/// # Returns
/// The `rowid` of the newly inserted row, or
/// `Err(MantleError::Database(_))` on failure (including UNIQUE constraint
/// violations on `name`).
///
/// # Side Effects
/// Inserts one row into `profiles`.
///
/// # Errors
/// Returns [`MantleError::Database`] if the INSERT fails (e.g. UNIQUE
/// constraint violation on `name`).
pub fn insert_profile(conn: &Connection, rec: &InsertProfile<'_>) -> Result<i64, MantleError> {
    let now = unix_now();
    conn.execute(
        "INSERT INTO profiles (name, game_slug, is_active, created_at, updated_at)
         VALUES (:name, :game_slug, 0, :created_at, :updated_at)",
        rusqlite::named_params! {
            ":name":       rec.name,
            ":game_slug":  rec.game_slug,
            ":created_at": now,
            ":updated_at": now,
        },
    )
    .map_err(MantleError::Database)?;
    Ok(conn.last_insert_rowid())
}

/// Set the profile identified by `profile_id` as the sole active profile.
///
/// Executes within a single transaction:
/// 1. Clears `is_active` on all profiles.
/// 2. Sets `is_active = 1` on the target profile.
///
/// This guarantees the exactly-one-active invariant atomically.
///
/// # Parameters
/// - `conn`: An open, migrated `rusqlite::Connection`.
/// - `profile_id`: Primary key of the profile to activate.
///
/// # Returns
/// `Ok(())` on success, `Err(MantleError::NotFound(_))` if `profile_id`
/// does not exist, or `Err(MantleError::Database(_))` on query failure.
///
/// # Side Effects
/// Modifies `is_active` on every row in `profiles`.
///
/// # Errors
/// Returns [`MantleError::NotFound`] if `profile_id` does not exist, or
/// [`MantleError::Database`] if any SQL operation fails.
pub fn set_active_profile(conn: &Connection, profile_id: i64) -> Result<(), MantleError> {
    // Verify the target profile exists before touching anything.
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM profiles WHERE id = :id",
            rusqlite::named_params! { ":id": profile_id },
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .map_err(MantleError::Database)?;

    if !exists {
        return Err(MantleError::NotFound(format!("profile id {profile_id} not found")));
    }

    let now = unix_now();
    conn.execute_batch(&format!(
        "BEGIN;
         UPDATE profiles SET is_active = 0, updated_at = {now};
         UPDATE profiles SET is_active = 1, updated_at = {now} WHERE id = {profile_id};
         COMMIT;"
    ))
    .map_err(MantleError::Database)
}

/// Delete the profile with the given `profile_id`.
///
/// If the deleted profile was active, no profile will be active afterwards.
/// The caller is responsible for activating another profile.
///
/// Associated `profile_mods` and `load_order` rows are removed via
/// `ON DELETE CASCADE`.
///
/// # Parameters
/// - `conn`: An open, migrated `rusqlite::Connection`.
/// - `profile_id`: Primary key of the profile to delete.
///
/// # Returns
/// `Ok(true)` if deleted, `Ok(false)` if not found, or
/// `Err(MantleError::Database(_))` on failure.
///
/// # Side Effects
/// Deletes from `profiles` and cascaded child tables.
///
/// # Errors
/// Returns [`MantleError::Database`] if the DELETE fails.
pub fn delete_profile(conn: &Connection, profile_id: i64) -> Result<bool, MantleError> {
    let rows = conn
        .execute(
            "DELETE FROM profiles WHERE id = :id",
            rusqlite::named_params! { ":id": profile_id },
        )
        .map_err(MantleError::Database)?;
    Ok(rows > 0)
}

// ---------------------------------------------------------------------------
// Read operations
// ---------------------------------------------------------------------------

/// Return the currently active profile, or `None` if no profile is active.
///
/// # Parameters
/// - `conn`: An open, migrated `rusqlite::Connection`.
///
/// # Returns
/// `Ok(Some(ProfileRecord))` if an active profile exists, `Ok(None)` if
/// not, or `Err(MantleError::Database(_))` on query failure.
///
/// # Errors
/// Returns [`MantleError::Database`] if the query fails.
pub fn get_active_profile(conn: &Connection) -> Result<Option<ProfileRecord>, MantleError> {
    conn.query_row(
        "SELECT id, name, game_slug, is_active, created_at, updated_at
         FROM profiles
         WHERE is_active = 1
         LIMIT 1",
        [],
        row_to_profile,
    )
    .optional()
    .map_err(MantleError::Database)
}

/// Return all profiles ordered by `created_at` ascending.
///
/// # Parameters
/// - `conn`: An open, migrated `rusqlite::Connection`.
///
/// # Returns
/// A `Vec<ProfileRecord>` (possibly empty) or `Err(MantleError::Database(_))`.
///
/// # Errors
/// Returns [`MantleError::Database`] if the query fails.
pub fn list_profiles(conn: &Connection) -> Result<Vec<ProfileRecord>, MantleError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, game_slug, is_active, created_at, updated_at
             FROM profiles
             ORDER BY created_at ASC",
        )
        .map_err(MantleError::Database)?;

    let rows = stmt
        .query_map([], row_to_profile)
        .map_err(MantleError::Database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(MantleError::Database)?;

    Ok(rows)
}

/// Look up a single profile by its primary key.
///
/// # Parameters
/// - `conn`: An open, migrated `rusqlite::Connection`.
/// - `profile_id`: Primary key to search for.
///
/// # Returns
/// `Ok(Some(ProfileRecord))` if found, `Ok(None)` if not, or
/// `Err(MantleError::Database(_))` on failure.
///
/// # Errors
/// Returns [`MantleError::Database`] if the query fails.
pub fn get_profile_by_id(
    conn: &Connection,
    profile_id: i64,
) -> Result<Option<ProfileRecord>, MantleError> {
    conn.query_row(
        "SELECT id, name, game_slug, is_active, created_at, updated_at
         FROM profiles WHERE id = :id",
        rusqlite::named_params! { ":id": profile_id },
        row_to_profile,
    )
    .optional()
    .map_err(MantleError::Database)
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Map a rusqlite `Row` to a [`ProfileRecord`].
fn row_to_profile(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProfileRecord> {
    Ok(ProfileRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        game_slug: row.get(2)?,
        is_active: {
            let v: i64 = row.get(3)?;
            v != 0
        },
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

/// Return the current Unix timestamp as seconds since epoch.
#[allow(clippy::cast_possible_wrap)] // Unix timestamp in seconds fits in i64 for ~292 billion years
fn unix_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::run_migrations;
    use rusqlite::Connection;

    fn temp_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        run_migrations(&conn).unwrap();
        conn
    }

    fn sample(name: &str) -> InsertProfile<'_> {
        InsertProfile {
            name,
            game_slug: None,
        }
    }

    #[test]
    fn insert_profile_returns_id() {
        let conn = temp_conn();
        let id = insert_profile(&conn, &sample("Default")).unwrap();
        assert!(id > 0);
    }

    #[test]
    fn new_profile_is_inactive() {
        let conn = temp_conn();
        insert_profile(&conn, &sample("Default")).unwrap();
        assert!(get_active_profile(&conn).unwrap().is_none());
    }

    #[test]
    fn set_active_profile_activates() {
        let conn = temp_conn();
        let id = insert_profile(&conn, &sample("Default")).unwrap();
        set_active_profile(&conn, id).unwrap();
        let active = get_active_profile(&conn).unwrap().unwrap();
        assert_eq!(active.id, id);
        assert!(active.is_active);
    }

    #[test]
    fn exactly_one_active_profile_invariant() {
        let conn = temp_conn();
        let a = insert_profile(&conn, &sample("A")).unwrap();
        let b = insert_profile(&conn, &sample("B")).unwrap();
        set_active_profile(&conn, a).unwrap();
        set_active_profile(&conn, b).unwrap();

        // Only B should be active.
        let active_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM profiles WHERE is_active = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(active_count, 1, "exactly one profile must be active");

        let active = get_active_profile(&conn).unwrap().unwrap();
        assert_eq!(active.id, b);
    }

    #[test]
    fn set_active_nonexistent_returns_not_found() {
        let conn = temp_conn();
        let result = set_active_profile(&conn, 9999);
        assert!(
            matches!(result, Err(MantleError::NotFound(_))),
            "non-existent profile must return NotFound"
        );
    }

    #[test]
    fn list_profiles_empty() {
        let conn = temp_conn();
        assert!(list_profiles(&conn).unwrap().is_empty());
    }

    #[test]
    fn list_profiles_returns_all() {
        let conn = temp_conn();
        insert_profile(&conn, &sample("P1")).unwrap();
        insert_profile(&conn, &sample("P2")).unwrap();
        assert_eq!(list_profiles(&conn).unwrap().len(), 2);
    }

    #[test]
    fn duplicate_name_rejected() {
        let conn = temp_conn();
        insert_profile(&conn, &sample("Dup")).unwrap();
        assert!(insert_profile(&conn, &sample("Dup")).is_err());
    }

    #[test]
    fn delete_profile_removes_row() {
        let conn = temp_conn();
        let id = insert_profile(&conn, &sample("Gone")).unwrap();
        assert!(delete_profile(&conn, id).unwrap());
        assert!(get_profile_by_id(&conn, id).unwrap().is_none());
    }

    #[test]
    fn game_slug_roundtrip() {
        let conn = temp_conn();
        let id = insert_profile(
            &conn,
            &InsertProfile {
                name: "SSE",
                game_slug: Some("skyrim_se"),
            },
        )
        .unwrap();
        let rec = get_profile_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(rec.game_slug.as_deref(), Some("skyrim_se"));
    }
}
