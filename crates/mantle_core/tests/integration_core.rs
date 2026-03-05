//! Cross-cutting integration tests — exercises the full core stack end-to-end.
//!
//! Three scenarios, each independent and self-contained:
//!
//! 1. **Profile + mod lifecycle** — create profile, install mod, add it to the
//!    profile, enable it, verify it's listed, then delete the profile and
//!    confirm the cascade removes the profile-mod link.
//!
//! 2. **VFS symlink round-trip** — mount one lower directory via the symlink-farm
//!    backend, assert the file is visible in the merge view, then unmount and
//!    assert the merge view is clean.
//!
//! 3. **Conflict detection basics** — two mods that share a file path produce a
//!    conflict map where neither mod is clean.
//!
//! All tests use only the public API of `mantle_core`.

use mantle_core::{
    conflict::{build_conflict_map, ModEntry},
    data::{
        mods::{insert_mod, InsertMod},
        profiles::{
            delete_profile, insert_profile, list_profiles, set_active_profile, InsertProfile,
        },
        Database,
    },
    mod_list::{add_mod_to_profile, list_profile_mods, set_mod_enabled},
    vfs::{mount_with, BackendKind, MountParams},
};

// ─── Scenario 1: Profile + mod lifecycle ─────────────────────────────────────

/// Full CRUD round-trip: create profile → install mod → add to profile →
/// enable → verify → delete profile and confirm cascade.
///
/// Uses a file-backed SQLite database so realistic FK cascade behaviour is
/// exercised (in-memory databases share the same FK enforcement but a
/// temp-file DB is closer to production).
#[test]
fn profile_mod_lifecycle() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let db = Database::open(&tmp.path().join("mantle.db")).expect("Database::open");

    // ── Create profile ────────────────────────────────────────────────────
    let pid = db
        .with_conn(|conn| {
            insert_profile(
                conn,
                &InsertProfile {
                    name: "Integration Test Profile",
                    game_slug: None,
                },
            )
        })
        .expect("insert_profile");

    // Activate the profile (mirrors what state_worker does).
    db.with_conn(|conn| set_active_profile(conn, pid)).expect("set_active_profile");

    let profiles = db.with_conn(list_profiles).expect("list_profiles after insert");
    assert_eq!(profiles.len(), 1, "exactly one profile after insert");

    // ── Install a mod ─────────────────────────────────────────────────────
    let mid = db
        .with_conn(|conn| {
            insert_mod(
                conn,
                &InsertMod {
                    slug: "integration-test-mod",
                    name: "Integration Test Mod",
                    version: Some("1.0.0"),
                    author: Some("Tester"),
                    description: None,
                    nexus_mod_id: None,
                    nexus_file_id: None,
                    source_url: None,
                    archive_path: None,
                    install_dir: "/tmp/mods/integration-test-mod",
                    archive_hash: None,
                    installed_at: None,
                },
            )
        })
        .expect("insert_mod");

    // ── Add mod to profile ────────────────────────────────────────────────
    let added = db
        .with_conn(|conn| add_mod_to_profile(conn, pid, mid))
        .expect("add_mod_to_profile");
    assert!(added, "mod should be newly added (not duplicate)");

    // Adding the same mod again must return false (idempotent).
    let duplicate = db
        .with_conn(|conn| add_mod_to_profile(conn, pid, mid))
        .expect("add_mod_to_profile duplicate");
    assert!(!duplicate, "re-adding must return false (already exists)");

    // ── Enable the mod ────────────────────────────────────────────────────
    db.with_conn(|conn| set_mod_enabled(conn, pid, mid, true))
        .expect("set_mod_enabled");

    // ── Verify the profile mod list ───────────────────────────────────────
    let mods = db.with_conn(|conn| list_profile_mods(conn, pid)).expect("list_profile_mods");
    assert_eq!(mods.len(), 1, "one mod entry after add");
    assert!(mods[0].is_enabled, "mod must be enabled after set_mod_enabled");
    assert_eq!(mods[0].mod_id, mid);

    // ── Delete profile — cascade removes profile_mods ──────────────────────
    let deleted = db.with_conn(|conn| delete_profile(conn, pid)).expect("delete_profile");
    assert!(deleted, "delete_profile must return true for existing profile");

    let mods_after = db
        .with_conn(|conn| list_profile_mods(conn, pid))
        .expect("list_profile_mods after delete");
    assert!(mods_after.is_empty(), "cascade delete must remove all profile_mods rows");

    let profiles_after = db.with_conn(list_profiles).expect("list_profiles after delete");
    assert!(profiles_after.is_empty(), "no profiles after delete");
}

// ─── Scenario 2: VFS symlink-farm round-trip ──────────────────────────────────

/// Mount one lower directory through the symlink-farm backend.
/// Asserts that the file is visible after mount and gone after unmount.
/// Does not require kernel overlayfs or FUSE — always available.
#[test]
fn vfs_symlink_roundtrip() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let lower = tmp.path().join("lower");
    let merge = tmp.path().join("merge");

    std::fs::create_dir_all(&lower).expect("create lower dir");
    std::fs::write(lower.join("plugin.esp"), b"TES4 fake esp").expect("write plugin.esp");

    let params = MountParams {
        lower_dirs: vec![lower.clone()],
        merge_dir: merge.clone(),
    };

    let handle =
        mount_with(BackendKind::SymlinkFarm, params).expect("symlink-farm mount must succeed");

    assert!(
        merge.join("plugin.esp").exists(),
        "plugin.esp must be visible in the merge dir after mount"
    );

    handle.unmount().expect("symlink-farm unmount must succeed");

    assert!(
        !merge.join("plugin.esp").exists(),
        "plugin.esp must be gone from the merge dir after unmount"
    );
}

// ─── Scenario 3: Conflict detection ──────────────────────────────────────────

/// Two mods sharing the same file path produce a conflict where the first-priority
/// mod wins and the second loses.
#[test]
fn conflict_detection_basic() {
    let map = build_conflict_map(&[
        ModEntry {
            id: "mod-high".to_string(),
            files: vec!["textures/sky.dds".to_string(), "meshes/sky.nif".to_string()],
        },
        ModEntry {
            id: "mod-low".to_string(),
            files: vec![
                "textures/sky.dds".to_string(), // shared with mod-high
                "textures/ground.dds".to_string(),
            ],
        },
    ]);

    // Exactly one conflicted path.
    assert_eq!(map.total_file_conflicts(), 1, "only textures/sky.dds is shared");

    // mod-high (index 0) wins; mod-low (index 1) loses.
    let entry = map
        .entry_for_path("textures/sky.dds")
        .expect("conflict entry must exist for shared path");
    assert_eq!(entry.winner, "mod-high");
    assert_eq!(entry.losers, ["mod-low"]);

    // Uncontested paths are absent from the map.
    assert!(
        map.entry_for_path("meshes/sky.nif").is_none(),
        "uncontested path must not appear in conflict map"
    );
    assert!(
        map.entry_for_path("textures/ground.dds").is_none(),
        "uncontested path must not appear in conflict map"
    );
}
