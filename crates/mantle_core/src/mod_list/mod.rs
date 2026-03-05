//! Mod list — ordered collection of installed mods for a profile.
//!
//! Wraps the `mods` and `profile_mods` tables from `DATA_MODEL.md`.
//! Exposes add / remove / enable / disable / move / reorder operations that
//! keep `priority` values consistent in a single transaction.
//!
//! Priority 1 = highest-priority (leftmost in the VFS `lowerdir`).
//! All mutations that change multiple priorities go through
//! [`replace_all_profile_mods`], which deletes and re-inserts the
//! `profile_mods` rows atomically to avoid `UNIQUE(profile_id, priority)`
//! constraint violations during in-place shifts.

use std::collections::HashMap;

use rusqlite::Connection;

use crate::error::MantleError;

// ─── Public types ─────────────────────────────────────────────────────────────

/// A row from the joined `profile_mods` + `mods` view for a single profile.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileModEntry {
    /// `mods.id`
    pub mod_id: i64,
    /// `mods.slug`
    pub mod_slug: String,
    /// `mods.name`
    pub mod_name: String,
    /// `profile_mods.priority` (1 = highest)
    pub priority: i64,
    /// `profile_mods.is_enabled`
    pub is_enabled: bool,
    /// `mods.install_dir`
    pub install_dir: String,
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Return all mods in a profile, ordered by priority ascending (1 first).
///
/// Both enabled and disabled mods are included.
///
/// # Errors
/// Returns [`MantleError::Database`] if the query fails.
pub fn list_profile_mods(
    conn: &Connection,
    profile_id: i64,
) -> Result<Vec<ProfileModEntry>, MantleError> {
    let mut stmt = conn
        .prepare(
            "SELECT m.id, m.slug, m.name, pm.priority, pm.is_enabled, m.install_dir
             FROM profile_mods pm
             INNER JOIN mods m ON m.id = pm.mod_id
             WHERE pm.profile_id = :profile_id
             ORDER BY pm.priority ASC",
        )
        .map_err(MantleError::Database)?;

    let rows = stmt
        .query_map(rusqlite::named_params! { ":profile_id": profile_id }, |row| {
            Ok(ProfileModEntry {
                mod_id: row.get(0)?,
                mod_slug: row.get(1)?,
                mod_name: row.get(2)?,
                priority: row.get(3)?,
                is_enabled: row.get::<_, i64>(4)? != 0,
                install_dir: row.get(5)?,
            })
        })
        .map_err(MantleError::Database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(MantleError::Database)?;

    Ok(rows)
}

/// Add a mod to a profile at the lowest priority (appended to end of list).
///
/// If the mod is already in the profile, returns `Ok(false)` without
/// modifying the database.
///
/// # Errors
/// Returns [`MantleError::Database`] if the insert fails.
pub fn add_mod_to_profile(
    conn: &Connection,
    profile_id: i64,
    mod_id: i64,
) -> Result<bool, MantleError> {
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM profile_mods WHERE profile_id = :pid AND mod_id = :mid",
            rusqlite::named_params! { ":pid": profile_id, ":mid": mod_id },
            |row| row.get::<_, i64>(0),
        )
        .map_err(MantleError::Database)?
        > 0;

    if exists {
        return Ok(false);
    }

    let next_priority: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(priority), 0) + 1 FROM profile_mods WHERE profile_id = :pid",
            rusqlite::named_params! { ":pid": profile_id },
            |row| row.get(0),
        )
        .map_err(MantleError::Database)?;

    conn.execute(
        "INSERT INTO profile_mods (profile_id, mod_id, priority, is_enabled)
         VALUES (:pid, :mid, :pri, 1)",
        rusqlite::named_params! {
            ":pid": profile_id,
            ":mid": mod_id,
            ":pri": next_priority,
        },
    )
    .map_err(MantleError::Database)?;

    Ok(true)
}

/// Remove a mod from a profile and compact the remaining priorities.
///
/// Returns `Ok(true)` if the mod was found and removed, `Ok(false)` if the
/// mod was not in the profile.
///
/// # Errors
/// Returns [`MantleError::Database`] on failure.
pub fn remove_mod_from_profile(
    conn: &Connection,
    profile_id: i64,
    mod_id: i64,
) -> Result<bool, MantleError> {
    let entries = list_profile_mods(conn, profile_id)?;
    let before = entries.len();

    let remaining: Vec<&ProfileModEntry> = entries.iter().filter(|e| e.mod_id != mod_id).collect();

    if remaining.len() == before {
        return Ok(false);
    }

    let ids: Vec<i64> = remaining.iter().map(|e| e.mod_id).collect();
    let enabled_map: HashMap<i64, bool> =
        remaining.iter().map(|e| (e.mod_id, e.is_enabled)).collect();

    replace_all_profile_mods(conn, profile_id, &ids, &enabled_map)?;
    Ok(true)
}

/// Set the `is_enabled` flag for a mod in a profile.
///
/// Returns `Ok(true)` if the row was found and updated, `Ok(false)` if the
/// mod is not in the profile.
///
/// # Errors
/// Returns [`MantleError::Database`] on failure.
pub fn set_mod_enabled(
    conn: &Connection,
    profile_id: i64,
    mod_id: i64,
    enabled: bool,
) -> Result<bool, MantleError> {
    let rows = conn
        .execute(
            "UPDATE profile_mods SET is_enabled = :enabled
             WHERE profile_id = :pid AND mod_id = :mid",
            rusqlite::named_params! {
                ":enabled": i64::from(enabled),
                ":pid":     profile_id,
                ":mid":     mod_id,
            },
        )
        .map_err(MantleError::Database)?;
    Ok(rows > 0)
}

/// Move a mod to a specific priority position, shifting other mods as needed.
///
/// `new_priority` is 1-based and clamped to `[1, count]`.
/// If the mod is not in the profile, returns `Ok(())` without error.
///
/// # Errors
/// Returns [`MantleError::Database`] on failure.
pub fn move_mod_to(
    conn: &Connection,
    profile_id: i64,
    mod_id: i64,
    new_priority: i64,
) -> Result<(), MantleError> {
    let entries = list_profile_mods(conn, profile_id)?;

    let Some(current_pos) = entries.iter().position(|e| e.mod_id == mod_id) else {
        return Ok(());
    };

    let count = i64::try_from(entries.len()).unwrap_or(i64::MAX);
    // The clamp guarantees value is in [1, count], so try_from succeeds on any
    // 64-bit target; unwrap_or(1) keeps 32-bit targets safe if count > usize::MAX.
    let clamped = usize::try_from(new_priority.clamp(1, count)).unwrap_or(1);
    let target_idx = clamped - 1;

    if current_pos == target_idx {
        return Ok(());
    }

    let enabled_map: HashMap<i64, bool> =
        entries.iter().map(|e| (e.mod_id, e.is_enabled)).collect();

    let mut ids: Vec<i64> = entries.iter().map(|e| e.mod_id).collect();
    let item = ids.remove(current_pos);
    ids.insert(target_idx, item);

    replace_all_profile_mods(conn, profile_id, &ids, &enabled_map)?;
    Ok(())
}

/// Replace the mod list for a profile with a completely new ordering.
///
/// `ordered_mod_ids` must contain exactly the same set of mod IDs currently
/// in the profile (no additions or removals).
///
/// # Errors
/// Returns [`MantleError::Conflict`] if `ordered_mod_ids` does not contain
/// exactly the mods already in the profile. Returns [`MantleError::Database`]
/// on SQL failure.
pub fn reorder_profile_mods(
    conn: &Connection,
    profile_id: i64,
    ordered_mod_ids: &[i64],
) -> Result<(), MantleError> {
    let entries = list_profile_mods(conn, profile_id)?;

    let mut existing: Vec<i64> = entries.iter().map(|e| e.mod_id).collect();
    let mut requested: Vec<i64> = ordered_mod_ids.to_vec();
    existing.sort_unstable();
    requested.sort_unstable();

    if existing != requested {
        return Err(MantleError::Conflict(
            "reorder_profile_mods: ordered_mod_ids must contain exactly the mods in the profile"
                .into(),
        ));
    }

    let enabled_map: HashMap<i64, bool> =
        entries.iter().map(|e| (e.mod_id, e.is_enabled)).collect();

    replace_all_profile_mods(conn, profile_id, ordered_mod_ids, &enabled_map)?;
    Ok(())
}

// ─── Private helpers ──────────────────────────────────────────────────────────

/// Delete all `profile_mods` rows for `profile_id` and re-insert them in the
/// given order, atomically.
///
/// Priority is assigned as `index + 1` (1-based). `enabled_map` carries
/// the `is_enabled` state for each mod; mods absent from the map default to
/// enabled.
fn replace_all_profile_mods(
    conn: &Connection,
    profile_id: i64,
    ordered_ids: &[i64],
    enabled_map: &HashMap<i64, bool>,
) -> Result<(), MantleError> {
    let tx = conn.unchecked_transaction().map_err(MantleError::Database)?;

    tx.execute(
        "DELETE FROM profile_mods WHERE profile_id = :pid",
        rusqlite::named_params! { ":pid": profile_id },
    )
    .map_err(MantleError::Database)?;

    for (idx, &mid) in ordered_ids.iter().enumerate() {
        let priority = i64::try_from(idx).expect("mod index fits i64") + 1;
        let is_enabled = i64::from(*enabled_map.get(&mid).unwrap_or(&true));
        tx.execute(
            "INSERT INTO profile_mods (profile_id, mod_id, priority, is_enabled)
             VALUES (:pid, :mid, :pri, :enabled)",
            rusqlite::named_params! {
                ":pid":     profile_id,
                ":mid":     mid,
                ":pri":     priority,
                ":enabled": is_enabled,
            },
        )
        .map_err(MantleError::Database)?;
    }

    tx.commit().map_err(MantleError::Database)?;
    Ok(())
}

/// Return the total mod count (enabled + disabled) for every profile in a
/// single SQL query, keyed by `profile_id`.
///
/// Intended for populating `{n} mods` labels in the profile list without
/// issuing one query per profile.
///
/// # Errors
/// Returns [`MantleError::Database`] if the query fails.
pub fn mod_counts_per_profile(
    conn: &Connection,
) -> Result<std::collections::HashMap<i64, usize>, MantleError> {
    let mut stmt = conn
        .prepare("SELECT profile_id, COUNT(*) FROM profile_mods GROUP BY profile_id")
        .map_err(MantleError::Database)?;

    let pairs = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))
        .map_err(MantleError::Database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(MantleError::Database)?;

    Ok(pairs.into_iter().map(|(id, n)| (id, usize::try_from(n).unwrap_or(0))).collect())
}

// ─── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{
        mods::{insert_mod, InsertMod},
        profiles::{insert_profile, InsertProfile},
        run_migrations,
    };
    use rusqlite::Connection;

    fn temp_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        run_migrations(&conn).unwrap();
        conn
    }

    fn mk_mod(conn: &Connection, slug: &str) -> i64 {
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
        .unwrap()
    }

    fn mk_profile(conn: &Connection) -> i64 {
        insert_profile(
            conn,
            &InsertProfile {
                name: "Default",
                game_slug: None,
            },
        )
        .unwrap()
    }

    // ── list_profile_mods ────────────────────────────────────────────────────

    #[test]
    fn list_empty_profile_returns_empty_vec() {
        let conn = temp_conn();
        let pid = mk_profile(&conn);
        assert!(list_profile_mods(&conn, pid).unwrap().is_empty());
    }

    #[test]
    fn list_returns_mods_in_priority_order() {
        let conn = temp_conn();
        let pid = mk_profile(&conn);
        let a = mk_mod(&conn, "mod-a");
        let b = mk_mod(&conn, "mod-b");
        let c = mk_mod(&conn, "mod-c");

        add_mod_to_profile(&conn, pid, a).unwrap();
        add_mod_to_profile(&conn, pid, b).unwrap();
        add_mod_to_profile(&conn, pid, c).unwrap();

        let entries = list_profile_mods(&conn, pid).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].mod_id, a);
        assert_eq!(entries[1].mod_id, b);
        assert_eq!(entries[2].mod_id, c);
        assert_eq!(entries[0].priority, 1);
        assert_eq!(entries[2].priority, 3);
    }

    // ── add_mod_to_profile ───────────────────────────────────────────────────

    #[test]
    fn add_mod_appends_at_lowest_priority() {
        let conn = temp_conn();
        let pid = mk_profile(&conn);
        let a = mk_mod(&conn, "mod-a");
        let b = mk_mod(&conn, "mod-b");

        assert!(add_mod_to_profile(&conn, pid, a).unwrap());
        assert!(add_mod_to_profile(&conn, pid, b).unwrap());

        let entries = list_profile_mods(&conn, pid).unwrap();
        assert_eq!(entries[0].mod_id, a);
        assert_eq!(entries[0].priority, 1);
        assert_eq!(entries[1].mod_id, b);
        assert_eq!(entries[1].priority, 2);
    }

    #[test]
    fn add_mod_duplicate_returns_false() {
        let conn = temp_conn();
        let pid = mk_profile(&conn);
        let a = mk_mod(&conn, "dup-mod");

        assert!(add_mod_to_profile(&conn, pid, a).unwrap());
        assert!(!add_mod_to_profile(&conn, pid, a).unwrap());

        let entries = list_profile_mods(&conn, pid).unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn add_mod_new_mod_is_enabled_by_default() {
        let conn = temp_conn();
        let pid = mk_profile(&conn);
        let a = mk_mod(&conn, "enabled-mod");
        add_mod_to_profile(&conn, pid, a).unwrap();
        let entry = &list_profile_mods(&conn, pid).unwrap()[0];
        assert!(entry.is_enabled);
    }

    // ── remove_mod_from_profile ──────────────────────────────────────────────

    #[test]
    fn remove_mod_compacts_priorities() {
        let conn = temp_conn();
        let pid = mk_profile(&conn);
        let a = mk_mod(&conn, "mod-a");
        let b = mk_mod(&conn, "mod-b");
        let c = mk_mod(&conn, "mod-c");

        add_mod_to_profile(&conn, pid, a).unwrap();
        add_mod_to_profile(&conn, pid, b).unwrap();
        add_mod_to_profile(&conn, pid, c).unwrap();

        // Remove the middle mod.
        assert!(remove_mod_from_profile(&conn, pid, b).unwrap());

        let entries = list_profile_mods(&conn, pid).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].mod_id, a);
        assert_eq!(entries[0].priority, 1);
        assert_eq!(entries[1].mod_id, c);
        assert_eq!(entries[1].priority, 2, "priorities must be compacted");
    }

    #[test]
    fn remove_mod_not_in_profile_returns_false() {
        let conn = temp_conn();
        let pid = mk_profile(&conn);
        let a = mk_mod(&conn, "ghost");
        assert!(!remove_mod_from_profile(&conn, pid, a).unwrap());
    }

    // ── set_mod_enabled ──────────────────────────────────────────────────────

    #[test]
    fn set_mod_enabled_toggles_flag() {
        let conn = temp_conn();
        let pid = mk_profile(&conn);
        let a = mk_mod(&conn, "toggle-mod");
        add_mod_to_profile(&conn, pid, a).unwrap();

        assert!(set_mod_enabled(&conn, pid, a, false).unwrap());
        let entry = &list_profile_mods(&conn, pid).unwrap()[0];
        assert!(!entry.is_enabled);

        assert!(set_mod_enabled(&conn, pid, a, true).unwrap());
        let entry = &list_profile_mods(&conn, pid).unwrap()[0];
        assert!(entry.is_enabled);
    }

    #[test]
    fn set_mod_enabled_returns_false_for_missing_mod() {
        let conn = temp_conn();
        let pid = mk_profile(&conn);
        assert!(!set_mod_enabled(&conn, pid, 999, true).unwrap());
    }

    #[test]
    fn set_mod_enabled_preserves_priority_order() {
        let conn = temp_conn();
        let pid = mk_profile(&conn);
        let a = mk_mod(&conn, "prio-a");
        let b = mk_mod(&conn, "prio-b");
        add_mod_to_profile(&conn, pid, a).unwrap();
        add_mod_to_profile(&conn, pid, b).unwrap();

        set_mod_enabled(&conn, pid, a, false).unwrap();
        let entries = list_profile_mods(&conn, pid).unwrap();
        assert_eq!(entries[0].mod_id, a);
        assert_eq!(entries[0].priority, 1);
        assert_eq!(entries[1].mod_id, b);
        assert_eq!(entries[1].priority, 2);
    }

    // ── move_mod_to ──────────────────────────────────────────────────────────

    #[test]
    fn move_mod_to_higher_priority() {
        let conn = temp_conn();
        let pid = mk_profile(&conn);
        let a = mk_mod(&conn, "move-a");
        let b = mk_mod(&conn, "move-b");
        let c = mk_mod(&conn, "move-c");
        add_mod_to_profile(&conn, pid, a).unwrap();
        add_mod_to_profile(&conn, pid, b).unwrap();
        add_mod_to_profile(&conn, pid, c).unwrap();

        // Move c (priority 3) to priority 1.
        move_mod_to(&conn, pid, c, 1).unwrap();

        let entries = list_profile_mods(&conn, pid).unwrap();
        assert_eq!(entries[0].mod_id, c, "c must be first");
        assert_eq!(entries[1].mod_id, a);
        assert_eq!(entries[2].mod_id, b);
        assert!(entries.iter().map(|e| e.priority).eq(1..=3), "priorities must be 1,2,3");
    }

    #[test]
    fn move_mod_to_lower_priority() {
        let conn = temp_conn();
        let pid = mk_profile(&conn);
        let a = mk_mod(&conn, "down-a");
        let b = mk_mod(&conn, "down-b");
        let c = mk_mod(&conn, "down-c");
        add_mod_to_profile(&conn, pid, a).unwrap();
        add_mod_to_profile(&conn, pid, b).unwrap();
        add_mod_to_profile(&conn, pid, c).unwrap();

        // Move a (priority 1) to priority 3.
        move_mod_to(&conn, pid, a, 3).unwrap();

        let entries = list_profile_mods(&conn, pid).unwrap();
        assert_eq!(entries[0].mod_id, b);
        assert_eq!(entries[1].mod_id, c);
        assert_eq!(entries[2].mod_id, a, "a must be last");
        assert!(entries.iter().map(|e| e.priority).eq(1..=3));
    }

    #[test]
    fn move_mod_to_same_position_is_noop() {
        let conn = temp_conn();
        let pid = mk_profile(&conn);
        let a = mk_mod(&conn, "noop-a");
        let b = mk_mod(&conn, "noop-b");
        add_mod_to_profile(&conn, pid, a).unwrap();
        add_mod_to_profile(&conn, pid, b).unwrap();

        move_mod_to(&conn, pid, a, 1).unwrap();

        let entries = list_profile_mods(&conn, pid).unwrap();
        assert_eq!(entries[0].mod_id, a);
        assert_eq!(entries[1].mod_id, b);
    }

    #[test]
    fn move_mod_clamps_out_of_range_priority() {
        let conn = temp_conn();
        let pid = mk_profile(&conn);
        let a = mk_mod(&conn, "clamp-a");
        let b = mk_mod(&conn, "clamp-b");
        add_mod_to_profile(&conn, pid, a).unwrap();
        add_mod_to_profile(&conn, pid, b).unwrap();

        // Priority 99 should be clamped to 2 (count).
        move_mod_to(&conn, pid, a, 99).unwrap();
        let entries = list_profile_mods(&conn, pid).unwrap();
        assert_eq!(entries[1].mod_id, a, "a must be last after clamping to max");
    }

    #[test]
    fn move_mod_missing_from_profile_is_noop() {
        let conn = temp_conn();
        let pid = mk_profile(&conn);
        let a = mk_mod(&conn, "absent");
        // Not added to profile — should not error.
        move_mod_to(&conn, pid, a, 1).unwrap();
    }

    #[test]
    fn move_mod_preserves_enabled_flags() {
        let conn = temp_conn();
        let pid = mk_profile(&conn);
        let a = mk_mod(&conn, "flag-a");
        let b = mk_mod(&conn, "flag-b");
        add_mod_to_profile(&conn, pid, a).unwrap();
        add_mod_to_profile(&conn, pid, b).unwrap();
        set_mod_enabled(&conn, pid, b, false).unwrap();

        move_mod_to(&conn, pid, b, 1).unwrap();

        let entries = list_profile_mods(&conn, pid).unwrap();
        let b_entry = entries.iter().find(|e| e.mod_id == b).unwrap();
        assert!(!b_entry.is_enabled, "is_enabled must survive a move");
    }

    // ── reorder_profile_mods ─────────────────────────────────────────────────

    #[test]
    fn reorder_reverses_list() {
        let conn = temp_conn();
        let pid = mk_profile(&conn);
        let a = mk_mod(&conn, "rev-a");
        let b = mk_mod(&conn, "rev-b");
        let c = mk_mod(&conn, "rev-c");
        add_mod_to_profile(&conn, pid, a).unwrap();
        add_mod_to_profile(&conn, pid, b).unwrap();
        add_mod_to_profile(&conn, pid, c).unwrap();

        reorder_profile_mods(&conn, pid, &[c, b, a]).unwrap();

        let entries = list_profile_mods(&conn, pid).unwrap();
        assert_eq!(entries[0].mod_id, c);
        assert_eq!(entries[1].mod_id, b);
        assert_eq!(entries[2].mod_id, a);
        assert!(entries.iter().map(|e| e.priority).eq(1..=3));
    }

    #[test]
    fn reorder_with_wrong_set_returns_conflict_error() {
        let conn = temp_conn();
        let pid = mk_profile(&conn);
        let a = mk_mod(&conn, "err-a");
        let b = mk_mod(&conn, "err-b");
        add_mod_to_profile(&conn, pid, a).unwrap();
        add_mod_to_profile(&conn, pid, b).unwrap();

        let bogus_id = a + b + 999;
        let result = reorder_profile_mods(&conn, pid, &[a, bogus_id]);
        assert!(
            matches!(result, Err(MantleError::Conflict(_))),
            "wrong mod set must return Conflict error"
        );
    }

    #[test]
    fn reorder_preserves_enabled_flags() {
        let conn = temp_conn();
        let pid = mk_profile(&conn);
        let a = mk_mod(&conn, "eflag-a");
        let b = mk_mod(&conn, "eflag-b");
        add_mod_to_profile(&conn, pid, a).unwrap();
        add_mod_to_profile(&conn, pid, b).unwrap();
        set_mod_enabled(&conn, pid, a, false).unwrap();

        reorder_profile_mods(&conn, pid, &[b, a]).unwrap();

        let entries = list_profile_mods(&conn, pid).unwrap();
        let a_entry = entries.iter().find(|e| e.mod_id == a).unwrap();
        assert!(!a_entry.is_enabled, "is_enabled must survive reorder");
    }

    // ── mod_counts_per_profile ───────────────────────────────────────────────

    #[test]
    fn mod_counts_per_profile_returns_counts_for_all_profiles() {
        let conn = temp_conn();
        let p1 = mk_profile(&conn);
        let p2 = insert_profile(
            &conn,
            &InsertProfile {
                name: "Second",
                game_slug: None,
            },
        )
        .unwrap();

        let a = mk_mod(&conn, "cnt-a");
        let b = mk_mod(&conn, "cnt-b");
        let c = mk_mod(&conn, "cnt-c");

        add_mod_to_profile(&conn, p1, a).unwrap();
        add_mod_to_profile(&conn, p1, b).unwrap();
        add_mod_to_profile(&conn, p2, c).unwrap();

        let counts = mod_counts_per_profile(&conn).unwrap();
        assert_eq!(counts.get(&p1).copied(), Some(2));
        assert_eq!(counts.get(&p2).copied(), Some(1));
    }

    #[test]
    fn mod_counts_per_profile_empty_profile_not_in_map() {
        let conn = temp_conn();
        let pid = mk_profile(&conn);
        let counts = mod_counts_per_profile(&conn).unwrap();
        // Empty profile has no profile_mods rows → absent from the map.
        assert!(counts.get(&pid).is_none());
    }
}
