//! CRUD operations for the `mods` table.
//!
//! Provides insert, lookup, list, and delete for installed mod records.
//! All functions operate synchronously on a `&rusqlite::Connection`.
//! Callers in the UI / async layer must dispatch via
//! `tokio::task::spawn_blocking`.

use rusqlite::{Connection, OptionalExtension};

use crate::error::MantleError;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// A row from the `mods` table.
///
/// Optional fields map to SQL `NULLable` columns — `None` means the column is
/// NULL in the database.
#[derive(Debug, Clone, PartialEq)]
pub struct ModRecord {
    /// Auto-increment primary key.
    pub id: i64,
    /// Stable slug identifier (UNIQUE in DB).
    pub slug: String,
    /// Human-readable name.
    pub name: String,
    /// Version string (semver or raw tag), `None` if unknown.
    pub version: Option<String>,
    /// Author name, `None` if unknown.
    pub author: Option<String>,
    /// Long description, `None` if not provided.
    pub description: Option<String>,
    /// Nexus Mods mod ID, `None` if not from Nexus.
    pub nexus_mod_id: Option<i64>,
    /// Nexus Mods file ID, `None` if not from Nexus.
    pub nexus_file_id: Option<i64>,
    /// Source URL, `None` if unknown.
    pub source_url: Option<String>,
    /// Path to the original archive file, `None` if not retained.
    pub archive_path: Option<String>,
    /// Absolute path to the extracted data directory.
    pub install_dir: String,
    /// XXH3 hex hash of the original archive, `None` if not computed.
    pub archive_hash: Option<String>,
    /// Unix timestamp of installation.
    pub installed_at: i64,
    /// Unix timestamp of last update.
    pub updated_at: i64,
}

/// Parameters for inserting a new mod record.
///
/// Uses `Option` for all nullable columns. `installed_at` and `updated_at`
/// default to the current Unix time if not provided.
#[derive(Debug, Clone)]
pub struct InsertMod<'a> {
    pub slug: &'a str,
    pub name: &'a str,
    pub version: Option<&'a str>,
    pub author: Option<&'a str>,
    pub description: Option<&'a str>,
    pub nexus_mod_id: Option<i64>,
    pub nexus_file_id: Option<i64>,
    pub source_url: Option<&'a str>,
    pub archive_path: Option<&'a str>,
    pub install_dir: &'a str,
    pub archive_hash: Option<&'a str>,
    /// Override for `installed_at` timestamp.  `None` = use current time.
    pub installed_at: Option<i64>,
}

// ---------------------------------------------------------------------------
// Write operations
// ---------------------------------------------------------------------------

/// Insert a new mod record into the `mods` table.
///
/// # Parameters
/// - `conn`: An open, migrated `rusqlite::Connection`.
/// - `rec`: Fields for the new row.
///
/// # Returns
/// The `rowid` of the newly inserted row on success, or
/// `Err(MantleError::Database(_))` on failure (including UNIQUE constraint
/// violations on `slug`).
///
/// # Side Effects
/// Inserts one row into `mods`.
///
/// # Errors
/// Returns [`MantleError::Database`] if the INSERT fails (e.g. UNIQUE
/// constraint violation on `slug`).
pub fn insert_mod(conn: &Connection, rec: &InsertMod<'_>) -> Result<i64, MantleError> {
    let now = unix_now();
    let installed_at = rec.installed_at.unwrap_or(now);

    conn.execute(
        "INSERT INTO mods (
             slug, name, version, author, description,
             nexus_mod_id, nexus_file_id, source_url,
             archive_path, install_dir, archive_hash,
             installed_at, updated_at
         ) VALUES (
             :slug, :name, :version, :author, :description,
             :nexus_mod_id, :nexus_file_id, :source_url,
             :archive_path, :install_dir, :archive_hash,
             :installed_at, :updated_at
         )",
        rusqlite::named_params! {
            ":slug":          rec.slug,
            ":name":          rec.name,
            ":version":       rec.version,
            ":author":        rec.author,
            ":description":   rec.description,
            ":nexus_mod_id":  rec.nexus_mod_id,
            ":nexus_file_id": rec.nexus_file_id,
            ":source_url":    rec.source_url,
            ":archive_path":  rec.archive_path,
            ":install_dir":   rec.install_dir,
            ":archive_hash":  rec.archive_hash,
            ":installed_at":  installed_at,
            ":updated_at":    now,
        },
    )
    .map_err(MantleError::Database)?;

    Ok(conn.last_insert_rowid())
}

/// Delete the mod with the given `slug` from the `mods` table.
///
/// Associated `mod_files` rows are removed automatically via `ON DELETE
/// CASCADE`.  `profile_mods` and `conflicts` referencing this mod are also
/// cascaded.
///
/// # Parameters
/// - `conn`: An open, migrated `rusqlite::Connection`.
/// - `slug`: The unique slug of the mod to delete.
///
/// # Returns
/// `Ok(true)` if a row was deleted, `Ok(false)` if no row matched, or
/// `Err(MantleError::Database(_))` on failure.
///
/// # Side Effects
/// Deletes from `mods` and any cascaded child rows.
///
/// # Errors
/// Returns [`MantleError::Database`] if the DELETE fails.
pub fn delete_mod(conn: &Connection, slug: &str) -> Result<bool, MantleError> {
    let rows = conn
        .execute("DELETE FROM mods WHERE slug = :slug", rusqlite::named_params! { ":slug": slug })
        .map_err(MantleError::Database)?;
    Ok(rows > 0)
}

// ---------------------------------------------------------------------------
// Read operations
// ---------------------------------------------------------------------------

/// Look up a single mod by its slug.
///
/// # Parameters
/// - `conn`: An open, migrated `rusqlite::Connection`.
/// - `slug`: Unique slug to search for.
///
/// # Returns
/// `Ok(Some(ModRecord))` if found, `Ok(None)` if not, or
/// `Err(MantleError::Database(_))` on query failure.
///
/// # Errors
/// Returns [`MantleError::Database`] if the query fails.
pub fn get_mod_by_slug(conn: &Connection, slug: &str) -> Result<Option<ModRecord>, MantleError> {
    let result = conn
        .query_row(
            "SELECT id, slug, name, version, author, description,
                    nexus_mod_id, nexus_file_id, source_url,
                    archive_path, install_dir, archive_hash,
                    installed_at, updated_at
             FROM mods
             WHERE slug = :slug",
            rusqlite::named_params! { ":slug": slug },
            row_to_mod_record,
        )
        .optional()
        .map_err(MantleError::Database)?;
    Ok(result)
}

/// Return all mods ordered by `name` ascending.
///
/// # Parameters
/// - `conn`: An open, migrated `rusqlite::Connection`.
///
/// # Returns
/// A `Vec<ModRecord>` (possibly empty) or `Err(MantleError::Database(_))`.
///
/// # Errors
/// Returns [`MantleError::Database`] if the query fails.
pub fn list_mods(conn: &Connection) -> Result<Vec<ModRecord>, MantleError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, slug, name, version, author, description,
                    nexus_mod_id, nexus_file_id, source_url,
                    archive_path, install_dir, archive_hash,
                    installed_at, updated_at
             FROM mods
             ORDER BY name ASC",
        )
        .map_err(MantleError::Database)?;

    let rows = stmt
        .query_map([], row_to_mod_record)
        .map_err(MantleError::Database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(MantleError::Database)?;

    Ok(rows)
}

/// Return all mods linked to `profile_id`, ordered by priority ascending.
///
/// Only mods with `is_enabled = 1` in `profile_mods` are returned.
///
/// # Parameters
/// - `conn`: An open, migrated `rusqlite::Connection`.
/// - `profile_id`: ID of the profile.
///
/// # Returns
/// Ordered `Vec<ModRecord>` or `Err(MantleError::Database(_))`.
///
/// # Errors
/// Returns [`MantleError::Database`] if the query fails.
pub fn mods_for_profile(conn: &Connection, profile_id: i64) -> Result<Vec<ModRecord>, MantleError> {
    let mut stmt = conn
        .prepare(
            "SELECT m.id, m.slug, m.name, m.version, m.author, m.description,
                    m.nexus_mod_id, m.nexus_file_id, m.source_url,
                    m.archive_path, m.install_dir, m.archive_hash,
                    m.installed_at, m.updated_at
             FROM mods m
             INNER JOIN profile_mods pm ON pm.mod_id = m.id
             WHERE pm.profile_id = :profile_id
               AND pm.is_enabled = 1
             ORDER BY pm.priority ASC",
        )
        .map_err(MantleError::Database)?;

    let rows = stmt
        .query_map(rusqlite::named_params! { ":profile_id": profile_id }, row_to_mod_record)
        .map_err(MantleError::Database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(MantleError::Database)?;

    Ok(rows)
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Map a rusqlite `Row` to a [`ModRecord`].
///
/// Column order must match `SELECT` statements in this file.
fn row_to_mod_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<ModRecord> {
    Ok(ModRecord {
        id: row.get(0)?,
        slug: row.get(1)?,
        name: row.get(2)?,
        version: row.get(3)?,
        author: row.get(4)?,
        description: row.get(5)?,
        nexus_mod_id: row.get(6)?,
        nexus_file_id: row.get(7)?,
        source_url: row.get(8)?,
        archive_path: row.get(9)?,
        install_dir: row.get(10)?,
        archive_hash: row.get(11)?,
        installed_at: row.get(12)?,
        updated_at: row.get(13)?,
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

    /// Helper: open a fresh in-memory, migrated connection.
    fn temp_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        run_migrations(&conn).unwrap();
        conn
    }

    fn sample_insert(slug: &str) -> InsertMod<'_> {
        InsertMod {
            slug,
            name: "Test Mod",
            version: Some("1.0.0"),
            author: Some("Tester"),
            description: None,
            nexus_mod_id: Some(12345),
            nexus_file_id: Some(67890),
            source_url: None,
            archive_path: None,
            install_dir: "/tmp/mods/test-mod",
            archive_hash: None,
            installed_at: None,
        }
    }

    #[test]
    fn insert_and_get_by_slug() {
        let conn = temp_conn();
        let id = insert_mod(&conn, &sample_insert("test-mod")).unwrap();
        assert!(id > 0);

        let rec = get_mod_by_slug(&conn, "test-mod").unwrap().unwrap();
        assert_eq!(rec.slug, "test-mod");
        assert_eq!(rec.name, "Test Mod");
        assert_eq!(rec.nexus_mod_id, Some(12345));
    }

    #[test]
    fn get_by_slug_missing_returns_none() {
        let conn = temp_conn();
        assert!(get_mod_by_slug(&conn, "does-not-exist").unwrap().is_none());
    }

    #[test]
    fn list_mods_empty() {
        let conn = temp_conn();
        assert!(list_mods(&conn).unwrap().is_empty());
    }

    #[test]
    fn list_mods_returns_all() {
        let conn = temp_conn();
        insert_mod(&conn, &sample_insert("mod-a")).unwrap();
        insert_mod(&conn, &sample_insert("mod-b")).unwrap();
        let mods = list_mods(&conn).unwrap();
        assert_eq!(mods.len(), 2);
    }

    #[test]
    fn duplicate_slug_is_rejected() {
        let conn = temp_conn();
        insert_mod(&conn, &sample_insert("dup")).unwrap();
        let result = insert_mod(&conn, &sample_insert("dup"));
        assert!(result.is_err(), "duplicate slug must be rejected");
    }

    #[test]
    fn delete_mod_removes_row() {
        let conn = temp_conn();
        insert_mod(&conn, &sample_insert("delete-me")).unwrap();
        assert!(delete_mod(&conn, "delete-me").unwrap());
        assert!(get_mod_by_slug(&conn, "delete-me").unwrap().is_none());
    }

    #[test]
    fn delete_nonexistent_returns_false() {
        let conn = temp_conn();
        assert!(!delete_mod(&conn, "ghost").unwrap());
    }

    #[test]
    fn nullable_fields_roundtrip() {
        let conn = temp_conn();
        let rec = InsertMod {
            slug: "nulls",
            name: "Null Test",
            version: None,
            author: None,
            description: None,
            nexus_mod_id: None,
            nexus_file_id: None,
            source_url: None,
            archive_path: None,
            install_dir: "/tmp/mods/nulls",
            archive_hash: None,
            installed_at: None,
        };
        insert_mod(&conn, &rec).unwrap();
        let got = get_mod_by_slug(&conn, "nulls").unwrap().unwrap();
        assert!(got.version.is_none());
        assert!(got.nexus_mod_id.is_none());
    }
}
