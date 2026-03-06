//! SKSE/F4SE/xNVSE/OBSE DLL collision detection.
//!
//! A "DLL conflict" occurs when two or more enabled mods in a profile each
//! ship a copy of the **same DLL file path** into a script-extender plugin
//! directory.  The VFS layer silently makes one copy win (highest-priority
//! mod), leaving the losing mod's plugin unloaded — almost always a silent
//! runtime failure with no user-visible error.
//!
//! # Scope
//! Only paths beneath known script-extender plugin directories are classified
//! as SE-plugin DLLs and examined for collisions:
//!
//! | Prefix                  | Script extender          |
//! |-------------------------|--------------------------|
//! | `data/skse/plugins/`    | SKSE64 (Skyrim SE / AE)  |
//! | `data/f4se/plugins/`    | F4SE (Fallout 4)         |
//! | `data/nvse/plugins/`    | xNVSE (Fallout: NV)      |
//! | `data/fose/plugins/`    | FOSE (Fallout 3)         |
//! | `data/obse/plugins/`    | OBSE (Oblivion)          |
//! | `data/mwse/`            | MWSE (Morrowind)         |
//!
//! Engine-level DLLs outside those directories (e.g. `steam_api64.dll`) are
//! already captured by the general file-conflict detector in [`super::detector`].
//!
//! # References
//! - ARCHITECTURE.md §4.4 — conflict/ module layout
//! - ARCHITECTURE.md §8   — Address Library version mismatch (see `address_lib`)

use std::collections::HashMap;

use rusqlite::Connection;

use crate::error::MantleError;

// ─── Script-extender directory prefixes ──────────────────────────────────────

/// Lowercase, `/`-separated path prefixes for known script-extender plugin
/// directories.  Paths stored in `mod_files` already use this convention.
const SE_PLUGIN_DIRS: &[&str] = &[
    "data/skse/plugins/",
    "data/f4se/plugins/",
    "data/nvse/plugins/",
    "data/fose/plugins/",
    "data/obse/plugins/",
    "data/mwse/",
];

// ─── Public types ─────────────────────────────────────────────────────────────

/// A detected DLL collision between two or more mods.
///
/// Both (or all) mods ship a file at the same relative path inside a
/// script-extender plugin directory.  The VFS will only surface the
/// highest-priority copy; the others are silently shadowed.
#[derive(Debug, Clone, PartialEq)]
pub struct DllConflict {
    /// Relative path to the conflicting DLL (e.g. `data/skse/plugins/foo.dll`).
    ///
    /// Always lowercase and `/`-separated, matching the `mod_files` table.
    pub dll_path: String,
    /// Mod slugs/IDs that all ship this DLL, in priority order.
    ///
    /// Index 0 = highest-priority mod (the VFS winner); subsequent entries
    /// are silently shadowed.
    pub mods: Vec<String>,
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Returns `true` if `path` is a DLL file inside a known script-extender
/// plugin directory.
///
/// Paths must be lowercase and `/`-separated (as stored by `mod_files`).
///
/// # Examples
/// ```
/// use mantle_core::conflict::dll::is_se_plugin_dll;
///
/// assert!(is_se_plugin_dll("data/skse/plugins/mymod.dll"));
/// assert!(is_se_plugin_dll("data/f4se/plugins/console_util.dll"));
/// assert!(!is_se_plugin_dll("data/textures/sky.dds"));
/// assert!(!is_se_plugin_dll("data/skse/plugins/readme.txt"));
/// assert!(!is_se_plugin_dll("steam_api64.dll"));
/// ```
#[must_use]
pub fn is_se_plugin_dll(path: &str) -> bool {
    // Use Path::extension() for a case-insensitive `.dll` check so that
    // paths like `Foo.DLL` are still caught (archives sometimes preserve case).
    let is_dll = std::path::Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"));
    is_dll && SE_PLUGIN_DIRS.iter().any(|prefix| path.starts_with(prefix))
}

/// Scan `mod_dll_files` for DLL path collisions and return one
/// [`DllConflict`] per contested path.
///
/// The caller is responsible for pre-filtering `mod_dll_files` to paths of
/// interest (e.g. via [`is_se_plugin_dll`]).  Paths not ending in `.dll`
/// are still included in the scan — filtering is the caller's responsibility.
///
/// # Parameters
/// - `mod_dll_files`: Priority-ordered slice of `(mod_id, dll_paths)` pairs.
///   **Index 0 = highest priority** (matches the VFS `lowerdir` convention).
///   `mod_id` is any stable string identifier (typically the mod slug).
///
/// # Returns
/// `Vec<DllConflict>` sorted by `dll_path` for deterministic output.
/// Returns an empty vector when there are no collisions.
///
/// # Errors
/// Always returns `Ok`; the `Result` wrapper preserves API symmetry with
/// callers that may add I/O in the future.
pub fn detect_dll_conflicts(
    mod_dll_files: &[(String, Vec<String>)],
) -> Result<Vec<DllConflict>, MantleError> {
    // Build path → Vec<mod_id> map in input (priority) order.
    let mut path_to_mods: HashMap<&str, Vec<&str>> = HashMap::new();

    for (mod_id, paths) in mod_dll_files {
        for path in paths {
            path_to_mods.entry(path.as_str()).or_default().push(mod_id.as_str());
        }
    }

    // Collect paths claimed by more than one mod.
    let mut conflicts: Vec<DllConflict> = path_to_mods
        .into_iter()
        .filter(|(_, mods)| mods.len() > 1)
        .map(|(path, mods)| DllConflict {
            dll_path: path.to_owned(),
            mods: mods.into_iter().map(str::to_owned).collect(),
        })
        .collect();

    // Sort by path so results are deterministic regardless of HashMap order.
    conflicts.sort_unstable_by(|a, b| a.dll_path.cmp(&b.dll_path));

    Ok(conflicts)
}

/// Query `mod_files` for all SE-plugin DLLs belonging to enabled mods in
/// `profile_id`, grouped by mod slug, in priority order.
///
/// The result is ready to pass directly to [`detect_dll_conflicts`]:
/// index 0 = highest-priority mod (wins VFS conflicts).
///
/// Paths are filtered to [`is_se_plugin_dll`] — engine-level DLLs outside
/// script-extender directories are excluded here (they are handled by the
/// generic conflict detector).
///
/// # Errors
/// Returns [`MantleError::Database`] on any SQL failure.
pub fn dll_files_for_profile(
    conn: &Connection,
    profile_id: i64,
) -> Result<Vec<(String, Vec<String>)>, MantleError> {
    let mut stmt = conn
        .prepare(
            "SELECT m.slug, mf.rel_path
             FROM mod_files mf
             INNER JOIN mods        m  ON m.id   = mf.mod_id
             INNER JOIN profile_mods pm ON pm.mod_id = mf.mod_id
             WHERE pm.profile_id = :pid
               AND pm.is_enabled  = 1
               AND mf.rel_path LIKE '%.dll'
             ORDER BY pm.priority ASC, mf.rel_path ASC",
        )
        .map_err(MantleError::Database)?;

    let rows: Vec<(String, String)> = stmt
        .query_map(rusqlite::named_params! { ":pid": profile_id }, |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(MantleError::Database)?
        .collect::<Result<_, _>>()
        .map_err(MantleError::Database)?;

    // Group by mod slug, preserving priority order of first occurrence.
    let mut ordered_slugs: Vec<String> = Vec::new();
    let mut by_slug: HashMap<String, Vec<String>> = HashMap::new();

    for (slug, path) in rows {
        // Skip engine DLLs — those are covered by the generic conflict detector.
        if !is_se_plugin_dll(&path) {
            continue;
        }
        let entry = by_slug.entry(slug.clone()).or_insert_with(|| {
            ordered_slugs.push(slug.clone());
            Vec::new()
        });
        entry.push(path);
    }

    Ok(ordered_slugs
        .into_iter()
        .map(|slug| {
            let paths = by_slug.remove(&slug).unwrap_or_default();
            (slug, paths)
        })
        .collect())
}

// ─── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{
        mod_files::InsertModFile, mods::InsertMod, profiles::InsertProfile, run_migrations,
    };
    use rusqlite::Connection;

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Open an in-memory DB with migrations applied.
    fn temp_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        run_migrations(&conn).unwrap();
        conn
    }

    /// Insert a minimal mod, return its id.
    fn insert_mod(conn: &Connection, slug: &str) -> i64 {
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

    /// Insert a profile + add mods to it with sequential priorities.
    ///
    /// `mod_ids[0]` gets priority 1 (highest priority).
    fn setup_profile(conn: &Connection, mod_ids: &[i64]) -> i64 {
        let pid = crate::data::profiles::insert_profile(
            conn,
            &InsertProfile {
                name: "test-profile",
                game_slug: Some("skyrim_se"),
            },
        )
        .unwrap();
        for (idx, &mid) in mod_ids.iter().enumerate() {
            // usize-to-i64 cast; priority index never exceeds i64::MAX on any supported platform.
            #[allow(clippy::cast_possible_wrap)]
            let priority = idx as i64 + 1;
            conn.execute(
                "INSERT INTO profile_mods (profile_id, mod_id, priority, is_enabled)
                 VALUES (?1, ?2, ?3, 1)",
                rusqlite::params![pid, mid, priority],
            )
            .unwrap();
        }
        pid
    }

    // ── is_se_plugin_dll ──────────────────────────────────────────────────────

    #[test]
    fn se_dll_true_for_skse_plugins() {
        assert!(is_se_plugin_dll("data/skse/plugins/mymod.dll"));
        assert!(is_se_plugin_dll("data/f4se/plugins/console_util.dll"));
        assert!(is_se_plugin_dll("data/nvse/plugins/jip_ln_nvse.dll"));
        assert!(is_se_plugin_dll("data/fose/plugins/fose_plugin.dll"));
        assert!(is_se_plugin_dll("data/obse/plugins/obse_plugin.dll"));
        assert!(is_se_plugin_dll("data/mwse/mwse_plugin.dll"));
    }

    #[test]
    fn se_dll_false_for_non_plugin_paths() {
        // Wrong extension
        assert!(!is_se_plugin_dll("data/skse/plugins/readme.txt"));
        // Right extension, wrong directory
        assert!(!is_se_plugin_dll("steam_api64.dll"));
        assert!(!is_se_plugin_dll("data/dlls/helper.dll"));
        // Prefix match only — must actually be in the SE dir
        assert!(!is_se_plugin_dll("data/textures/sky.dds"));
    }

    // ── detect_dll_conflicts ──────────────────────────────────────────────────

    #[test]
    fn detect_empty_input_returns_empty() {
        let result = detect_dll_conflicts(&[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn detect_single_mod_no_conflict() {
        let input = vec![("mod_a".to_owned(), vec!["data/skse/plugins/foo.dll".to_owned()])];
        let result = detect_dll_conflicts(&input).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn detect_two_mods_different_dlls_no_conflict() {
        let input = vec![
            ("mod_a".to_owned(), vec!["data/skse/plugins/foo.dll".to_owned()]),
            ("mod_b".to_owned(), vec!["data/skse/plugins/bar.dll".to_owned()]),
        ];
        let result = detect_dll_conflicts(&input).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn detect_two_mods_same_dll_one_conflict() {
        let input = vec![
            ("mod_a".to_owned(), vec!["data/skse/plugins/foo.dll".to_owned()]),
            ("mod_b".to_owned(), vec!["data/skse/plugins/foo.dll".to_owned()]),
        ];
        let result = detect_dll_conflicts(&input).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].dll_path, "data/skse/plugins/foo.dll");
        // mod_a is index 0 (higher priority) — must appear first.
        assert_eq!(result[0].mods[0], "mod_a");
        assert_eq!(result[0].mods[1], "mod_b");
    }

    #[test]
    fn detect_three_way_conflict() {
        let input = vec![
            ("winner".to_owned(), vec!["data/skse/plugins/shared.dll".to_owned()]),
            ("loser_1".to_owned(), vec!["data/skse/plugins/shared.dll".to_owned()]),
            ("loser_2".to_owned(), vec!["data/skse/plugins/shared.dll".to_owned()]),
        ];
        let result = detect_dll_conflicts(&input).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].mods.len(), 3);
        assert_eq!(result[0].mods[0], "winner");
    }

    #[test]
    fn detect_mixed_conflict_and_clean() {
        // foo.dll conflicts; bar.dll is clean.
        let input = vec![
            (
                "mod_a".to_owned(),
                vec![
                    "data/skse/plugins/foo.dll".to_owned(),
                    "data/skse/plugins/bar.dll".to_owned(),
                ],
            ),
            ("mod_b".to_owned(), vec!["data/skse/plugins/foo.dll".to_owned()]),
        ];
        let result = detect_dll_conflicts(&input).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].dll_path, "data/skse/plugins/foo.dll");
    }

    #[test]
    fn detect_results_are_sorted_by_path() {
        let input = vec![
            (
                "mod_a".to_owned(),
                vec![
                    "data/skse/plugins/zzz.dll".to_owned(),
                    "data/skse/plugins/aaa.dll".to_owned(),
                ],
            ),
            (
                "mod_b".to_owned(),
                vec![
                    "data/skse/plugins/zzz.dll".to_owned(),
                    "data/skse/plugins/aaa.dll".to_owned(),
                ],
            ),
        ];
        let result = detect_dll_conflicts(&input).unwrap();
        assert_eq!(result.len(), 2);
        assert!(result[0].dll_path < result[1].dll_path);
    }

    // ── dll_files_for_profile ─────────────────────────────────────────────────

    #[test]
    fn dll_files_for_profile_empty_when_no_mods() {
        let conn = temp_conn();
        let pid = crate::data::profiles::insert_profile(
            &conn,
            &InsertProfile {
                name: "p",
                game_slug: Some("skyrim_se"),
            },
        )
        .unwrap();
        let result = dll_files_for_profile(&conn, pid).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn dll_files_for_profile_returns_se_dlls_grouped_by_slug() {
        let conn = temp_conn();
        let mid_a = insert_mod(&conn, "mod_a");
        let mid_b = insert_mod(&conn, "mod_b");
        let pid = setup_profile(&conn, &[mid_a, mid_b]);

        crate::data::mod_files::insert_mod_files(
            &conn,
            &[
                InsertModFile {
                    mod_id: mid_a,
                    rel_path: "data/skse/plugins/foo.dll",
                    file_hash: "aaa",
                    file_size: 1,
                    archive_name: None,
                },
                // Non-DLL — should be filtered out.
                InsertModFile {
                    mod_id: mid_a,
                    rel_path: "data/skse/plugins/foo.ini",
                    file_hash: "bbb",
                    file_size: 1,
                    archive_name: None,
                },
            ],
        )
        .unwrap();
        crate::data::mod_files::insert_mod_files(
            &conn,
            &[InsertModFile {
                mod_id: mid_b,
                rel_path: "data/skse/plugins/bar.dll",
                file_hash: "ccc",
                file_size: 1,
                archive_name: None,
            }],
        )
        .unwrap();

        let result = dll_files_for_profile(&conn, pid).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].0, "mod_a");
        assert_eq!(result[0].1, vec!["data/skse/plugins/foo.dll"]);
        assert_eq!(result[1].0, "mod_b");
        assert_eq!(result[1].1, vec!["data/skse/plugins/bar.dll"]);
    }

    #[test]
    fn dll_files_for_profile_excludes_engine_dlls() {
        // steam_api64.dll should not surface — it is not in an SE plugin dir.
        let conn = temp_conn();
        let mid = insert_mod(&conn, "mod_c");
        let pid = setup_profile(&conn, &[mid]);

        crate::data::mod_files::insert_mod_files(
            &conn,
            &[InsertModFile {
                mod_id: mid,
                rel_path: "steam_api64.dll", // not in SE dir
                file_hash: "ddd",
                file_size: 1,
                archive_name: None,
            }],
        )
        .unwrap();

        let result = dll_files_for_profile(&conn, pid).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn dll_files_for_profile_excludes_disabled_mods() {
        let conn = temp_conn();
        let mid = insert_mod(&conn, "mod_d");
        let pid = crate::data::profiles::insert_profile(
            &conn,
            &InsertProfile {
                name: "p2",
                game_slug: Some("skyrim_se"),
            },
        )
        .unwrap();
        // Insert mod with is_enabled = 0.
        conn.execute(
            "INSERT INTO profile_mods (profile_id, mod_id, priority, is_enabled)
             VALUES (?1, ?2, 1, 0)",
            rusqlite::params![pid, mid],
        )
        .unwrap();
        crate::data::mod_files::insert_mod_files(
            &conn,
            &[InsertModFile {
                mod_id: mid,
                rel_path: "data/skse/plugins/disabled.dll",
                file_hash: "eee",
                file_size: 1,
                archive_name: None,
            }],
        )
        .unwrap();

        let result = dll_files_for_profile(&conn, pid).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn full_pipeline_detects_conflict_via_db() {
        let conn = temp_conn();
        let mid_a = insert_mod(&conn, "plugin_a");
        let mid_b = insert_mod(&conn, "plugin_b");
        let pid = setup_profile(&conn, &[mid_a, mid_b]);

        // Both mods ship the same SKSE DLL.
        for mid in [mid_a, mid_b] {
            crate::data::mod_files::insert_mod_files(
                &conn,
                &[InsertModFile {
                    mod_id: mid,
                    rel_path: "data/skse/plugins/shared.dll",
                    file_hash: "fff",
                    file_size: 1,
                    archive_name: None,
                }],
            )
            .unwrap();
        }

        let dll_index = dll_files_for_profile(&conn, pid).unwrap();
        let conflicts = detect_dll_conflicts(&dll_index).unwrap();

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].dll_path, "data/skse/plugins/shared.dll");
        assert_eq!(conflicts[0].mods[0], "plugin_a"); // higher priority wins
        assert_eq!(conflicts[0].mods[1], "plugin_b");
    }
}
