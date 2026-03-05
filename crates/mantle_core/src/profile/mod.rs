//! Profile management — create, activate, clone, and delete profiles.
//!
//! A profile bundles a mod list + load order + per-profile INI overrides.
//! Activation triggers VFS teardown of the old profile and mount of the
//! new one via the `vfs` module, keeping the game directory clean between
//! profile switches.
//!
//! # Typical workflow
//! ```ignore
//! use mantle_core::profile;
//! use std::path::Path;
//!
//! // Create a profile at first run.
//! let id = profile::create_profile(&conn, "Default", None)?;
//!
//! // Activate it when the user clicks "Launch".
//! let handle = profile::activate_profile(&conn, id, Path::new("/game/Data"), None, &bus)?;
//!
//! // … game runs …
//! handle.unmount()?;
//!
//! // Clone for a second load order.
//! let id2 = profile::clone_profile(&conn, id, "Playthrough 2")?;
//!
//! // Delete when no longer needed.
//! profile::delete_profile(&conn, id2)?;
//! ```

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rusqlite::Connection;

use crate::{
    config::data_dir,
    data::profiles::{
        delete_profile as db_delete_profile, get_active_profile, get_profile_by_id, insert_profile,
        set_active_profile, InsertProfile,
    },
    error::MantleError,
    game::ini::apply_profile_ini,
    mod_list::{add_mod_to_profile, list_profile_mods},
    plugin::{EventBus, ModManagerEvent},
    vfs::{
        backend::select_backend, cleanup::teardown_stale, mount::mount_with, types::MountParams,
        MountHandle,
    },
};

// ─── Public API ───────────────────────────────────────────────────────────────

/// Create a new (inactive) profile.
///
/// The profile is inserted with `is_active = 0`. Use [`activate_profile`] to
/// mount its mod overlay and make it the active one.
///
/// # Parameters
/// - `conn`: Open, migrated `rusqlite::Connection`.
/// - `name`: Human-readable name — must be unique across all profiles.
/// - `game_slug`: Optional game slug to lock this profile to a specific game.
///
/// # Returns
/// The `rowid` of the newly created profile.
///
/// # Errors
/// Returns [`MantleError::Database`] on constraint violation (duplicate name)
/// or other SQL failures.
pub fn create_profile(
    conn: &Connection,
    name: &str,
    game_slug: Option<&str>,
) -> Result<i64, MantleError> {
    insert_profile(conn, &InsertProfile { name, game_slug })
}

/// Clone an existing profile into a new one with a different name.
///
/// Copies the source profile's `game_slug` and full mod list (preserving
/// priority order and enabled state) into a fresh inactive profile.
///
/// # Parameters
/// - `conn`: Open, migrated `rusqlite::Connection`.
/// - `source_id`: Primary key of the profile to clone.
/// - `new_name`: Name for the cloned profile — must be unique.
///
/// # Returns
/// The `rowid` of the newly created clone.
///
/// # Errors
/// - [`MantleError::NotFound`] if `source_id` does not exist.
/// - [`MantleError::Database`] on SQL failure or duplicate `new_name`.
pub fn clone_profile(
    conn: &Connection,
    source_id: i64,
    new_name: &str,
) -> Result<i64, MantleError> {
    // Retrieve the source profile so we can copy its game_slug.
    let source = get_profile_by_id(conn, source_id)?
        .ok_or_else(|| MantleError::NotFound(format!("profile id {source_id} not found")))?;

    // Create the new profile with the same game binding.
    let new_id = insert_profile(
        conn,
        &InsertProfile {
            name: new_name,
            game_slug: source.game_slug.as_deref(),
        },
    )?;

    // Copy the source mod list.  list_profile_mods returns entries in
    // priority-ascending order (1 = highest), so adding them in iteration
    // order via add_mod_to_profile (which appends at the next lowest priority)
    // preserves the original load order exactly.
    let source_mods = list_profile_mods(conn, source_id)?;
    for entry in &source_mods {
        add_mod_to_profile(conn, new_id, entry.mod_id)?;
    }

    Ok(new_id)
}

/// Delete a profile from the database.
///
/// The profile must **not** be currently active — callers should activate
/// another profile before deleting the current one.  Associated
/// `profile_mods` rows are removed automatically via `ON DELETE CASCADE`.
///
/// # Parameters
/// - `conn`: Open, migrated `rusqlite::Connection`.
/// - `profile_id`: Primary key of the profile to delete.
///
/// # Returns
/// `Ok(true)` if the profile was found and deleted, `Ok(false)` if it did
/// not exist.
///
/// # Errors
/// - [`MantleError::Profile`] if the profile is currently active.
/// - [`MantleError::Database`] on SQL failure.
pub fn delete_profile(conn: &Connection, profile_id: i64) -> Result<bool, MantleError> {
    // Guard: refuse to delete the active profile to avoid leaving the
    // application in an undefined state (no active profile, stale VFS mount).
    if let Some(active) = get_active_profile(conn)? {
        if active.id == profile_id {
            return Err(MantleError::Profile(
                "cannot delete the active profile; activate another profile first".into(),
            ));
        }
    }

    db_delete_profile(conn, profile_id)
}

/// Activate a profile: tear down any existing VFS overlay, mark the profile
/// active in the database, mount a new overlay for its enabled mod list, apply
/// per-profile INI overrides, and publish a [`ModManagerEvent::ProfileChanged`]
/// event on the shared bus.
///
/// The caller **must** hold the returned [`MountHandle`] for the lifetime of
/// the game session.  Call [`MountHandle::unmount`] after the game exits to
/// clean up the overlay.
///
/// # Parameters
/// - `conn`: Open, migrated `rusqlite::Connection`.
/// - `profile_id`: Primary key of the profile to activate.
/// - `game_data_dir`: Absolute path to the game's `Data/` directory. This
///   becomes the VFS merge point — the game reads its files from here.
/// - `proton_prefix`: Optional path to the Proton/Wine prefix root (the
///   `pfx/` directory). When `Some`, per-profile INI files are copied from
///   `{data_dir}/profiles/{profile_id}/ini/` into the Wine-prefix game
///   document directory. When `None`, INI synchronisation is skipped.
/// - `game_ini_dir`: When `proton_prefix` is `Some`, the absolute path to the
///   game's INI directory inside the Wine prefix (e.g.
///   `pfx/drive_c/users/steamuser/My Documents/My Games/Skyrim Special
///   Edition/`). Ignored when `proton_prefix` is `None`.
/// - `event_bus`: Shared event bus — a [`ModManagerEvent::ProfileChanged`]
///   event is published after successful activation.
///
/// # Returns
/// A [`MountHandle`] wrapping the active overlay.
///
/// # Errors
/// - [`MantleError::NotFound`] if `profile_id` does not exist.
/// - [`MantleError::Database`] if a SQL operation fails.
/// - [`MantleError::Vfs`] if mounting or teardown fails.
///
/// INI synchronisation errors are logged as warnings but do **not** cause this
/// function to return an error — a missing INI snapshot should never prevent
/// a profile from being activated.
pub fn activate_profile(
    conn: &Connection,
    profile_id: i64,
    game_data_dir: &Path,
    game_ini_dir: Option<&Path>,
    event_bus: &Arc<EventBus>,
) -> Result<MountHandle, MantleError> {
    // Capture the current active profile name for the ProfileChanged event.
    let old_profile_name =
        get_active_profile(conn)?.map_or_else(|| String::from("none"), |p| p.name);

    // Verify the target profile exists before touching the VFS.
    let new_profile = get_profile_by_id(conn, profile_id)?
        .ok_or_else(|| MantleError::NotFound(format!("profile id {profile_id} not found")))?;

    // Tear down any overlay that may be lingering from a previous session or
    // from the currently active profile.  teardown_stale is a no-op when
    // nothing is mounted at game_data_dir.
    teardown_stale(game_data_dir)
        .map_err(|e| MantleError::Vfs(format!("stale mount teardown failed: {e}")))?;

    // Persist the activation in the DB *after* the VFS teardown so that
    // list_profile_mods below reads the new profile's mod list.
    set_active_profile(conn, profile_id)?;

    // Build lower_dirs from the enabled mods in priority order.
    // Priority 1 = index 0 = highest priority (overlayfs leftmost lowerdir).
    let mod_entries = list_profile_mods(conn, profile_id)?;
    let lower_dirs: Vec<PathBuf> = mod_entries
        .into_iter()
        .filter(|e| e.is_enabled)
        .map(|e| PathBuf::from(&e.install_dir))
        .collect();

    // Select the best available VFS backend for this environment.
    let backend = select_backend();

    let params = MountParams {
        lower_dirs,
        merge_dir: game_data_dir.to_path_buf(),
    };

    let handle = mount_with(backend, params)?;

    // Apply per-profile INI overrides if the caller supplied a game INI dir.
    // Errors here are non-fatal — log a warning and continue.
    if let Some(ini_dir) = game_ini_dir {
        let profile_ini_dir = data_dir().join("profiles").join(profile_id.to_string()).join("ini");
        if let Err(e) = apply_profile_ini(&profile_ini_dir, ini_dir) {
            tracing::warn!(
                "activate_profile: INI apply failed for profile {profile_id} — {e}; continuing"
            );
        }
    }

    // Notify subscribers that the active profile changed.
    event_bus.publish(&ModManagerEvent::ProfileChanged {
        old: old_profile_name,
        new: new_profile.name,
    });

    Ok(handle)
}

// ─── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        data::{profiles::list_profiles, run_migrations},
        mod_list::list_profile_mods,
    };
    use rusqlite::Connection;

    /// Open an in-memory database with all migrations applied.
    fn temp_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        run_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn create_profile_is_inactive() {
        let conn = temp_conn();
        let id = create_profile(&conn, "Alpha", None).unwrap();
        let rec = get_profile_by_id(&conn, id).unwrap().unwrap();
        assert!(!rec.is_active, "newly created profile must be inactive");
    }

    #[test]
    fn create_profile_duplicate_name_rejected() {
        let conn = temp_conn();
        create_profile(&conn, "Dup", None).unwrap();
        assert!(create_profile(&conn, "Dup", None).is_err());
    }

    #[test]
    fn create_profile_with_game_slug() {
        let conn = temp_conn();
        let id = create_profile(&conn, "SSE", Some("skyrim_se")).unwrap();
        let rec = get_profile_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(rec.game_slug.as_deref(), Some("skyrim_se"));
    }

    #[test]
    fn delete_profile_removes_row() {
        let conn = temp_conn();
        let id = create_profile(&conn, "Gone", None).unwrap();
        assert!(delete_profile(&conn, id).unwrap());
        assert!(get_profile_by_id(&conn, id).unwrap().is_none());
    }

    #[test]
    fn delete_nonexistent_returns_false() {
        let conn = temp_conn();
        assert!(!delete_profile(&conn, 9999).unwrap());
    }

    #[test]
    fn delete_active_profile_blocked() {
        let conn = temp_conn();
        let id = create_profile(&conn, "Active", None).unwrap();
        set_active_profile(&conn, id).unwrap();
        let result = delete_profile(&conn, id);
        assert!(
            matches!(result, Err(MantleError::Profile(_))),
            "deleting the active profile must return Profile error"
        );
    }

    #[test]
    fn clone_profile_copies_metadata() {
        let conn = temp_conn();
        let src = create_profile(&conn, "Source", Some("skyrim_se")).unwrap();
        let cloned = clone_profile(&conn, src, "Clone").unwrap();

        let src_rec = get_profile_by_id(&conn, src).unwrap().unwrap();
        let clone_rec = get_profile_by_id(&conn, cloned).unwrap().unwrap();

        assert_eq!(clone_rec.game_slug, src_rec.game_slug);
        assert!(!clone_rec.is_active, "clone must be inactive");
        assert_ne!(clone_rec.name, src_rec.name);
    }

    #[test]
    fn clone_profile_copies_empty_mod_list() {
        let conn = temp_conn();
        let src = create_profile(&conn, "Src", None).unwrap();
        let cloned = clone_profile(&conn, src, "CloneEmpty").unwrap();

        assert!(list_profile_mods(&conn, cloned).unwrap().is_empty());
    }

    #[test]
    fn clone_nonexistent_source_returns_not_found() {
        let conn = temp_conn();
        let result = clone_profile(&conn, 9999, "X");
        assert!(matches!(result, Err(MantleError::NotFound(_))));
    }

    #[test]
    fn all_profiles_listed_after_create() {
        let conn = temp_conn();
        create_profile(&conn, "P1", None).unwrap();
        create_profile(&conn, "P2", None).unwrap();
        assert_eq!(list_profiles(&conn).unwrap().len(), 2);
    }
}
