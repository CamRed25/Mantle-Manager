//! Integration tests for `mantle_core::data` — migration system and CRUD.
//!
//! Covers:
//!  - Clean migration apply and idempotency
//!  - `schema_version` row presence
//!  - All expected tables and indices
//!  - Mod, profile, and mod_file round-trips
//!  - Active-profile exactly-one invariant
//!  - Foreign key cascade behaviour
//!  - The `temp_db()` helper pattern used throughout the test suite

use mantle_core::data::{
    mod_files::{insert_mod_files, InsertModFile},
    mods::{delete_mod, get_mod_by_slug, insert_mod, list_mods, InsertMod},
    profiles::{
        delete_profile, get_active_profile, insert_profile, list_profiles, set_active_profile,
        InsertProfile,
    },
    run_migrations,
};
use rusqlite::Connection;

// ── helpers ───────────────────────────────────────────────────────────────────

/// The canonical `temp_db()` helper as documented in TESTING_GUIDE.md §4.1.
///
/// Returns an in-memory, fully-migrated `rusqlite::Connection` with
/// foreign key enforcement enabled.
fn temp_db() -> Connection {
    let conn = Connection::open_in_memory().expect("in-memory db always succeeds");
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    run_migrations(&conn).expect("migrations must apply cleanly to fresh db");
    conn
}

/// Insert a minimal mod and return its row ID.
fn test_mod(conn: &Connection, slug: &str) -> i64 {
    insert_mod(
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
            install_dir: "/tmp/mods/test",
            archive_hash: None,
            installed_at: None,
        },
    )
    .unwrap_or_else(|e| panic!("insert_mod({slug}) failed: {e}"))
}

/// Insert a minimal profile and return its row ID.
fn test_profile(conn: &Connection, name: &str) -> i64 {
    insert_profile(
        conn,
        &InsertProfile {
            name,
            game_slug: None,
        },
    )
    .unwrap_or_else(|e| panic!("insert_profile({name}) failed: {e}"))
}

// ── migration: clean apply ────────────────────────────────────────────────────

#[test]
fn m001_clean_apply_succeeds() {
    temp_db(); // must not panic
}

#[test]
fn m001_schema_version_row_is_1() {
    let conn = temp_db();
    let version: u32 = conn
        .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 1, "schema_version must be 1 after m001");
}

#[test]
fn m001_all_tables_exist() {
    let conn = temp_db();
    for table in &[
        "schema_version",
        "mods",
        "mod_files",
        "profiles",
        "profile_mods",
        "load_order",
        "downloads",
        "plugin_settings",
        "conflicts",
    ] {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                rusqlite::params![table],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "table '{table}' must exist after m001");
    }
}

#[test]
fn m001_is_idempotent() {
    let conn = temp_db();
    run_migrations(&conn).expect("re-running migrations must succeed");

    let row_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0)).unwrap();
    assert_eq!(row_count, 1, "schema_version must have exactly 1 row");
}

// ── mods: round-trip ──────────────────────────────────────────────────────────

#[test]
fn mod_insert_get_roundtrip() {
    let conn = temp_db();
    let id = insert_mod(
        &conn,
        &InsertMod {
            slug: "skyui",
            name: "SkyUI",
            version: Some("5.2.1"),
            author: Some("schlangster"),
            description: Some("UI overhaul"),
            nexus_mod_id: Some(3863),
            nexus_file_id: Some(12345),
            source_url: Some("https://nexusmods.com/skyrimspecialedition/mods/3863"),
            archive_path: Some("/downloads/SkyUI.7z"),
            install_dir: "/mods/skyui",
            archive_hash: Some("aabbccddeeff"),
            installed_at: Some(1_700_000_000),
        },
    )
    .unwrap();

    assert!(id > 0);
    let r = get_mod_by_slug(&conn, "skyui").unwrap().unwrap();
    assert_eq!(r.slug, "skyui");
    assert_eq!(r.name, "SkyUI");
    assert_eq!(r.version.as_deref(), Some("5.2.1"));
    assert_eq!(r.author.as_deref(), Some("schlangster"));
    assert_eq!(r.nexus_mod_id, Some(3863));
    assert_eq!(r.archive_hash.as_deref(), Some("aabbccddeeff"));
    assert_eq!(r.installed_at, 1_700_000_000);
}

#[test]
fn mod_list_and_delete() {
    let conn = temp_db();
    test_mod(&conn, "mod-a");
    test_mod(&conn, "mod-b");
    assert_eq!(list_mods(&conn).unwrap().len(), 2);

    assert!(delete_mod(&conn, "mod-a").unwrap());
    assert_eq!(list_mods(&conn).unwrap().len(), 1);
    assert!(get_mod_by_slug(&conn, "mod-a").unwrap().is_none());
}

#[test]
fn mod_cascade_deletes_mod_files() {
    let conn = temp_db();
    let mid = test_mod(&conn, "cascade-mod");
    insert_mod_files(
        &conn,
        &[InsertModFile {
            mod_id: mid,
            rel_path: "data/test.esp",
            file_hash: "aabb",
            file_size: 256,
            archive_name: None,
        }],
    )
    .unwrap();

    delete_mod(&conn, "cascade-mod").unwrap();

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM mod_files WHERE mod_id = ?1",
            rusqlite::params![mid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "mod_files must cascade-delete with the mod");
}

// ── profiles: round-trip ─────────────────────────────────────────────────────

#[test]
fn profile_insert_and_activate() {
    let conn = temp_db();
    let id = test_profile(&conn, "Default");
    assert!(get_active_profile(&conn).unwrap().is_none(), "no active profile yet");

    set_active_profile(&conn, id).unwrap();
    let active = get_active_profile(&conn).unwrap().unwrap();
    assert_eq!(active.id, id);
    assert!(active.is_active);
}

#[test]
fn exactly_one_active_profile() {
    let conn = temp_db();
    let a = test_profile(&conn, "Alpha");
    let b = test_profile(&conn, "Beta");
    let c = test_profile(&conn, "Gamma");

    set_active_profile(&conn, a).unwrap();
    set_active_profile(&conn, b).unwrap();
    set_active_profile(&conn, c).unwrap();

    let active_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM profiles WHERE is_active = 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(active_count, 1, "exactly one profile must be active");

    let active = get_active_profile(&conn).unwrap().unwrap();
    assert_eq!(active.name, "Gamma");
}

#[test]
fn profile_delete_cascade_clears_mods() {
    let conn = temp_db();
    let pid = test_profile(&conn, "DeleteMe");
    let mid = test_mod(&conn, "some-mod");

    // Link the mod to the profile.
    conn.execute_batch(&format!(
        "INSERT INTO profile_mods (profile_id, mod_id, priority, is_enabled) \
         VALUES ({pid}, {mid}, 1, 1);"
    ))
    .unwrap();

    delete_profile(&conn, pid).unwrap();

    let pm_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM profile_mods WHERE profile_id = ?1",
            rusqlite::params![pid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pm_count, 0, "profile_mods must cascade-delete with the profile");
}

#[test]
fn profile_list_returns_all_profiles() {
    let conn = temp_db();
    test_profile(&conn, "P1");
    test_profile(&conn, "P2");
    test_profile(&conn, "P3");
    assert_eq!(list_profiles(&conn).unwrap().len(), 3);
}

// ── mod_files: round-trip ─────────────────────────────────────────────────────

#[test]
fn mod_files_insert_batch_and_retrieve() {
    let conn = temp_db();
    let mid = test_mod(&conn, "batch-mod");

    // Pre-build owned strings to avoid lifetime issues with InsertModFile<'_>.
    let paths: Vec<String> = (0..10).map(|i| format!("Data/file_{i:02}.esp")).collect();
    let hashes: Vec<String> = (0..10).map(|i| format!("hash_{i:04x}")).collect();

    let files: Vec<InsertModFile<'_>> = (0..10)
        .map(|i| InsertModFile {
            mod_id: mid,
            rel_path: &paths[i],
            file_hash: &hashes[i],
            file_size: (i as i64) * 100,
            archive_name: None,
        })
        .collect();

    insert_mod_files(&conn, &files).unwrap();

    let records = mantle_core::data::mod_files::files_for_mod(&conn, mid).unwrap();
    assert_eq!(records.len(), 10);
    // Paths must be lowercase.
    assert!(records[0].rel_path.starts_with("data/"));
}

#[test]
fn mod_files_path_is_lowercase() {
    let conn = temp_db();
    let mid = test_mod(&conn, "case-mod");
    insert_mod_files(
        &conn,
        &[InsertModFile {
            mod_id: mid,
            rel_path: "Data/UPPER/MixedCase.DDS",
            file_hash: "ff",
            file_size: 4096,
            archive_name: None,
        }],
    )
    .unwrap();

    let rec = &mantle_core::data::mod_files::files_for_mod(&conn, mid).unwrap()[0];
    assert_eq!(rec.rel_path, "data/upper/mixedcase.dds");
}

// ── foreign key enforcement ───────────────────────────────────────────────────

#[test]
fn mod_files_reject_invalid_mod_id() {
    let conn = temp_db();
    let result = insert_mod_files(
        &conn,
        &[InsertModFile {
            mod_id: 99999,
            rel_path: "data/ghost.esp",
            file_hash: "00",
            file_size: 1,
            archive_name: None,
        }],
    );
    assert!(result.is_err(), "FK violation must be rejected");
}

// ── temp_db helper pattern ────────────────────────────────────────────────────

/// Confirms that the `temp_db()` helper as documented in TESTING_GUIDE.md
/// §4.1 works correctly: returns a connection that can immediately accept
/// writes without further migration calls.
#[test]
fn temp_db_helper_is_ready_for_writes() {
    let conn = temp_db();
    let id = test_mod(&conn, "ready-check");
    assert!(id > 0);
    let id2 = test_profile(&conn, "ready-profile");
    assert!(id2 > 0);
}
