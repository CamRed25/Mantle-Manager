//! CRUD operations for the `mod_files` table.
//!
//! Manages the per-file manifest for each installed mod.  This data drives
//! conflict detection — every file path is stored lowercase so comparisons
//! are case-insensitive across filesystems.
//!
//! All functions operate synchronously on a `&rusqlite::Connection`.
//! Callers in the UI / async layer must dispatch via
//! `tokio::task::spawn_blocking`.

use rusqlite::Connection;

use crate::error::MantleError;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// A row from the `mod_files` table.
#[derive(Debug, Clone, PartialEq)]
pub struct ModFileRecord {
    /// Auto-increment primary key.
    pub id: i64,
    /// Foreign key to `mods.id`.
    pub mod_id: i64,
    /// Relative path within the mod's data directory, stored lowercase.
    pub rel_path: String,
    /// XXH3 hex hash of the file contents.
    pub file_hash: String,
    /// File size in bytes.
    pub file_size: i64,
    /// BSA/BA2 archive name this file came from, or `None` for loose files.
    pub archive_name: Option<String>,
}

/// Input record for a single file to be inserted.
#[derive(Debug, Clone)]
pub struct InsertModFile<'a> {
    /// Foreign key to `mods.id`.
    pub mod_id: i64,
    /// Relative path — will be lowercased on insert.
    pub rel_path: &'a str,
    /// XXH3 hex hash.
    pub file_hash: &'a str,
    /// File size in bytes.
    pub file_size: i64,
    /// Archive name, `None` for loose files.
    pub archive_name: Option<&'a str>,
}

// ---------------------------------------------------------------------------
// Write operations
// ---------------------------------------------------------------------------

/// Insert a batch of file records for a single mod inside one transaction.
///
/// All `rel_path` values are lowercased before insertion.
///
/// # Parameters
/// - `conn`: An open, migrated `rusqlite::Connection`.
/// - `files`: Slice of file descriptors to insert.
///
/// # Returns
/// `Ok(())` on success, or `Err(MantleError::Database(_))` if any insert
/// fails (including `UNIQUE(mod_id, rel_path)` violations).
///
/// # Errors
/// Returns [`MantleError::Database`] if any SQL operation fails.
///
/// # Side Effects
/// Inserts `files.len()` rows into `mod_files` in a single explicit
/// transaction.
pub fn insert_mod_files(conn: &Connection, files: &[InsertModFile<'_>]) -> Result<(), MantleError> {
    if files.is_empty() {
        return Ok(());
    }

    // Use a prepared statement inside an explicit transaction for performance.
    conn.execute_batch("BEGIN;").map_err(MantleError::Database)?;

    let mut stmt = conn
        .prepare(
            "INSERT INTO mod_files (mod_id, rel_path, file_hash, file_size, archive_name)
             VALUES (:mod_id, :rel_path, :file_hash, :file_size, :archive_name)",
        )
        .map_err(MantleError::Database)?;

    for f in files {
        let lower_path = f.rel_path.to_lowercase();
        stmt.execute(rusqlite::named_params! {
            ":mod_id":       f.mod_id,
            ":rel_path":     lower_path,
            ":file_hash":    f.file_hash,
            ":file_size":    f.file_size,
            ":archive_name": f.archive_name,
        })
        .map_err(|e| {
            // Attempt to roll back before propagating the error.
            let _ = conn.execute_batch("ROLLBACK;");
            MantleError::Database(e)
        })?;
    }

    conn.execute_batch("COMMIT;").map_err(MantleError::Database)
}

/// Delete all file records belonging to `mod_id`.
///
/// Typically called before re-scanning a mod's file list after an update.
///
/// # Parameters
/// - `conn`: An open, migrated `rusqlite::Connection`.
/// - `mod_id`: Foreign key identifying the mod.
///
/// # Returns
/// The number of rows deleted, or `Err(MantleError::Database(_))`.
///
/// # Side Effects
/// Deletes rows from `mod_files`.
///
/// # Errors
/// Returns [`MantleError::Database`] if the SQL DELETE fails.
pub fn delete_mod_files(conn: &Connection, mod_id: i64) -> Result<usize, MantleError> {
    conn.execute(
        "DELETE FROM mod_files WHERE mod_id = :mod_id",
        rusqlite::named_params! { ":mod_id": mod_id },
    )
    .map_err(MantleError::Database)
}

// ---------------------------------------------------------------------------
// Read operations
// ---------------------------------------------------------------------------

/// Return all file records for `mod_id`, ordered by `rel_path`.
///
/// # Parameters
/// - `conn`: An open, migrated `rusqlite::Connection`.
/// - `mod_id`: Foreign key identifying the mod.
///
/// # Returns
/// `Vec<ModFileRecord>` (possibly empty) or `Err(MantleError::Database(_))`.
///
/// # Errors
/// Returns [`MantleError::Database`] if the query fails.
pub fn files_for_mod(conn: &Connection, mod_id: i64) -> Result<Vec<ModFileRecord>, MantleError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, mod_id, rel_path, file_hash, file_size, archive_name
             FROM mod_files
             WHERE mod_id = :mod_id
             ORDER BY rel_path ASC",
        )
        .map_err(MantleError::Database)?;

    let rows = stmt
        .query_map(rusqlite::named_params! { ":mod_id": mod_id }, row_to_mod_file)
        .map_err(MantleError::Database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(MantleError::Database)?;

    Ok(rows)
}

/// Return all distinct `rel_path` values for enabled mods in a profile.
///
/// This is the primary input for conflict detection — it produces the flat
/// list of all file paths visible in a profile's overlay.  Paths are
/// already lowercased from insertion.
///
/// Mods are ordered by `profile_mods.priority` ascending (highest priority
/// first).  When the conflict detector sees duplicate paths the first-seen
/// (highest priority) mod wins.
///
/// # Parameters
/// - `conn`: An open, migrated `rusqlite::Connection`.
/// - `profile_id`: The profile to query.
///
/// # Returns
/// `Vec<(mod_id, rel_path)>` ordered by priority then path, or
/// `Err(MantleError::Database(_))`.
///
/// # Errors
/// Returns [`MantleError::Database`] if the query fails.
pub fn all_paths_for_enabled_mods_in_profile(
    conn: &Connection,
    profile_id: i64,
) -> Result<Vec<(i64, String)>, MantleError> {
    let mut stmt = conn
        .prepare(
            "SELECT mf.mod_id, mf.rel_path
             FROM mod_files mf
             INNER JOIN profile_mods pm ON pm.mod_id = mf.mod_id
             WHERE pm.profile_id = :profile_id
               AND pm.is_enabled  = 1
             ORDER BY pm.priority ASC, mf.rel_path ASC",
        )
        .map_err(MantleError::Database)?;

    let rows = stmt
        .query_map(rusqlite::named_params! { ":profile_id": profile_id }, |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(MantleError::Database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(MantleError::Database)?;

    Ok(rows)
}

/// Compute the XXH3-64 hash of a file's contents and return it as a
/// 16-character lowercase hex string, for storage in the `mod_files` table.
///
/// # Parameters
/// - `path`: Path to the file to hash.
///
/// # Returns
/// `Some(hex_string)` on success, `None` if the file cannot be read.
#[must_use]
pub fn hash_file_xxh3(path: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let hash = xxhash_rust::xxh3::xxh3_64(&bytes);
    Some(format!("{hash:016x}"))
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Map a rusqlite `Row` to a [`ModFileRecord`].
fn row_to_mod_file(row: &rusqlite::Row<'_>) -> rusqlite::Result<ModFileRecord> {
    Ok(ModFileRecord {
        id: row.get(0)?,
        mod_id: row.get(1)?,
        rel_path: row.get(2)?,
        file_hash: row.get(3)?,
        file_size: row.get(4)?,
        archive_name: row.get(5)?,
    })
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{mods::InsertMod, run_migrations};
    use rusqlite::Connection;

    fn temp_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        run_migrations(&conn).unwrap();
        conn
    }

    /// Insert a minimal mod and return its id.
    fn insert_test_mod(conn: &Connection, slug: &str) -> i64 {
        crate::data::mods::insert_mod(
            conn,
            &InsertMod {
                slug,
                name: slug,
                version: None,
                author: None,
                description: None,
                nexus_mod_id: None,
                nexus_file_id: None,
                source_url: None,
                archive_path: None,
                install_dir: "/tmp/test",
                archive_hash: None,
                installed_at: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn insert_and_retrieve_files() {
        let conn = temp_conn();
        let mod_id = insert_test_mod(&conn, "skyui");
        let files = vec![
            InsertModFile {
                mod_id,
                rel_path: "Interface/SkyUI.swf",
                file_hash: "aabbcc",
                file_size: 1024,
                archive_name: None,
            },
            InsertModFile {
                mod_id,
                rel_path: "Scripts/source/SkyUI.psc",
                file_hash: "ddeeff",
                file_size: 512,
                archive_name: None,
            },
        ];
        insert_mod_files(&conn, &files).unwrap();

        let records = files_for_mod(&conn, mod_id).unwrap();
        assert_eq!(records.len(), 2);
        // Paths must be stored lowercase.
        assert_eq!(records[0].rel_path, "interface/skyui.swf");
    }

    #[test]
    fn rel_path_stored_lowercase() {
        let conn = temp_conn();
        let mid = insert_test_mod(&conn, "caps");
        insert_mod_files(
            &conn,
            &[InsertModFile {
                mod_id: mid,
                rel_path: "Data/Textures/MyTex.dds",
                file_hash: "ff00",
                file_size: 2048,
                archive_name: None,
            }],
        )
        .unwrap();

        let records = files_for_mod(&conn, mid).unwrap();
        assert_eq!(records[0].rel_path, "data/textures/mytex.dds");
    }

    #[test]
    fn empty_batch_is_ok() {
        let conn = temp_conn();
        insert_mod_files(&conn, &[]).unwrap();
    }

    #[test]
    fn archive_name_roundtrip() {
        let conn = temp_conn();
        let mid = insert_test_mod(&conn, "bsa-mod");
        insert_mod_files(
            &conn,
            &[InsertModFile {
                mod_id: mid,
                rel_path: "sound/fx/crunch.wav",
                file_hash: "112233",
                file_size: 8192,
                archive_name: Some("Sounds.bsa"),
            }],
        )
        .unwrap();

        let records = files_for_mod(&conn, mid).unwrap();
        assert_eq!(records[0].archive_name.as_deref(), Some("Sounds.bsa"));
    }

    #[test]
    fn delete_mod_files_removes_all() {
        let conn = temp_conn();
        let mid = insert_test_mod(&conn, "del-me");
        insert_mod_files(
            &conn,
            &[InsertModFile {
                mod_id: mid,
                rel_path: "data/a.esp",
                file_hash: "aa",
                file_size: 100,
                archive_name: None,
            }],
        )
        .unwrap();
        let deleted = delete_mod_files(&conn, mid).unwrap();
        assert_eq!(deleted, 1);
        assert!(files_for_mod(&conn, mid).unwrap().is_empty());
    }

    #[test]
    fn duplicate_rel_path_per_mod_rejected() {
        let conn = temp_conn();
        let mid = insert_test_mod(&conn, "dup-path");
        let same = InsertModFile {
            mod_id: mid,
            rel_path: "data/shared.esp",
            file_hash: "aa",
            file_size: 100,
            archive_name: None,
        };
        insert_mod_files(&conn, &[same.clone()]).unwrap();
        // Both have the same (mod_id, rel_path) after lowercasing — must fail.
        assert!(insert_mod_files(&conn, &[same]).is_err());
    }

    #[test]
    fn all_paths_returns_paths_sorted_by_priority() {
        let conn = temp_conn();
        // Create two mods.
        let m1 = insert_test_mod(&conn, "high-prio");
        let m2 = insert_test_mod(&conn, "low-prio");

        // Create a profile.
        let profile_id: i64 = conn
            .query_row(
                "INSERT INTO profiles (name, is_active, created_at, updated_at) \
                 VALUES ('Test', 0, 0, 0) RETURNING id",
                [],
                |r| r.get(0),
            )
            .unwrap();

        // Link both mods to the profile with different priorities.
        conn.execute_batch(&format!(
            "INSERT INTO profile_mods (profile_id, mod_id, priority, is_enabled) \
             VALUES ({profile_id}, {m1}, 1, 1), ({profile_id}, {m2}, 2, 1);"
        ))
        .unwrap();

        // Both share a conflicting path; m1 should appear first.
        insert_mod_files(
            &conn,
            &[
                InsertModFile {
                    mod_id: m1,
                    rel_path: "data/shared.esp",
                    file_hash: "aa",
                    file_size: 1,
                    archive_name: None,
                },
                InsertModFile {
                    mod_id: m2,
                    rel_path: "data/shared.esp",
                    file_hash: "bb",
                    file_size: 2,
                    archive_name: None,
                },
            ],
        )
        .unwrap();

        let paths = all_paths_for_enabled_mods_in_profile(&conn, profile_id).unwrap();
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].0, m1, "highest priority mod must appear first");
        assert_eq!(paths[1].0, m2);
    }
}
