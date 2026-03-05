//! Address Library version mismatch detection.
//!
//! Detects when mods in the active set ship copies of the Address Library for
//! SKSE plugins (`versionlib-*.bin` / `versionlib64-*.bin`) that target
//! different game versions, or that do not match the installed game version.
//!
//! # Problem statement
//! Each Skyrim binary version has a unique set of memory addresses. The Address
//! Library maps stable offset IDs to those addresses so that SKSE plugins do
//! not hard-code addresses. A plugin compiled against game 1.6.659 needs the
//! `versionlib-1-6-659-0.bin` database — if a different version is present in
//! the overlay the plugin will crash at load time.
//!
//! # Detection strategy
//! Given the full file manifest of every active mod:
//!
//! 1. Scan all file paths for the pattern `versionlib[64]-<v1>-<v2>-<v3>-<v4>.bin`
//!    (case-insensitive; paths are lowercased by convention in the conflict layer).
//! 2. Group by library type (`versionlib` vs `versionlib64`).
//! 3. **Inter-mod mismatch** — if two or more mods ship the same library type
//!    but different game-version strings, flag a conflict. Only the winning
//!    (highest-priority) version will be visible in the VFS overlay, so any
//!    SKSE plugin that expects a different version will crash.
//! 4. **Game-version mismatch** — if the caller provides the installed game
//!    version string and none of the shipped files match it, flag a conflict.
//!
//! # Filename format
//! ```text
//! versionlib-<major>-<minor>-<patch>-<build>.bin      (SSE / AE unified)
//! versionlib64-<major>-<minor>-<patch>-<build>.bin    (older AE split format)
//! ```
//! Paths in mod manifests are lowercase relative paths, e.g.
//! `data/skse/plugins/versionlib-1-6-659-0.bin`.
//!
//! # References
//! - ARCHITECTURE.md §4.4 — conflict/ module layout
//! - <https://www.nexusmods.com/skyrimspecialedition/mods/32444> — Address Library mod page

use std::collections::HashMap;

use crate::error::MantleError;

// ─── Public types ─────────────────────────────────────────────────────────────

/// The specific kind of Address Library version mismatch detected.
#[derive(Debug, Clone, PartialEq)]
pub enum AddressLibMismatch {
    /// Two or more mods in the active set ship the same library type but for
    /// different game versions. The VFS overlay will only expose the
    /// highest-priority mod's version, breaking any SKSE plugin that expects
    /// a different one.
    ///
    /// `versions` lists every distinct game-version string found across all
    /// shipping mods, sorted ascending.
    InterModMismatch { versions: Vec<String> },

    /// The library shipped by the active mod set does not match the version
    /// string reported by the installed game binary. SKSE plugins will crash
    /// at load time.
    GameVersionMismatch {
        /// Version string as reported by game detection.
        game_version: String,
        /// Version string embedded in the Address Library filename.
        library_version: String,
    },
}

/// A detected Address Library version conflict.
///
/// Produced by [`detect_address_lib_conflicts`] when the active mod set
/// contains Address Library files that are incompatible with each other or
/// with the installed game binary.
#[derive(Debug, Clone)]
pub struct AddressLibConflict {
    /// Library type: `"versionlib"` (SSE / AE unified) or
    /// `"versionlib64"` (older AE split format).
    pub library_type: String,

    /// `(mod_id, game_version_string)` pairs for every mod that ships this
    /// library type. Use these to tell the user which mods are involved.
    pub shipping_mods: Vec<(String, String)>,

    /// The specific kind of mismatch (inter-mod or game-version).
    pub kind: AddressLibMismatch,
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

/// Parsed metadata from an Address Library filename.
struct ParsedAddressLib {
    /// `"versionlib"` or `"versionlib64"`.
    library_type: String,
    /// Dot-separated version string, e.g. `"1.6.659.0"`.
    game_version: String,
}

/// Attempt to parse Address Library metadata from a lowercase relative file path.
///
/// Returns `Some(ParsedAddressLib)` if the filename matches the expected pattern,
/// `None` otherwise.
///
/// Accepted patterns (filename only, path prefix ignored):
/// - `versionlib-<v1>-<v2>-<v3>-<v4>.bin`
/// - `versionlib64-<v1>-<v2>-<v3>-<v4>.bin`
fn parse_address_lib_path(path: &str) -> Option<ParsedAddressLib> {
    // Extract the filename component — everything after the last '/'.
    let fname = path.rsplit('/').next().unwrap_or(path);

    let stem = fname.strip_suffix(".bin")?;

    let (library_type, version_part) = if let Some(rest) = stem.strip_prefix("versionlib64-") {
        ("versionlib64", rest)
    } else if let Some(rest) = stem.strip_prefix("versionlib-") {
        ("versionlib", rest)
    } else {
        return None;
    };

    // Version must be exactly four dash-separated non-negative integers.
    let parts: Vec<&str> = version_part.split('-').collect();
    if parts.len() != 4 || parts.iter().any(|p| p.parse::<u32>().is_err()) {
        return None;
    }

    Some(ParsedAddressLib {
        library_type: library_type.to_owned(),
        game_version: format!("{}.{}.{}.{}", parts[0], parts[1], parts[2], parts[3]),
    })
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Scan the active mod set for Address Library version mismatches.
///
/// # Parameters
/// - `mod_files`: Slice of `(mod_id, file_paths)` tuples. `mod_id` is the
///   mod slug; `file_paths` contains lowercase relative paths from the mod's
///   file manifest.
/// - `game_version`: The installed game binary version string
///   (e.g. `"1.6.659.0"`), if known. Pass `None` to skip game-version
///   matching.
///
/// # Returns
/// A `Vec<AddressLibConflict>` containing one entry per library type that has
/// a mismatch. Returns an empty vec if the active set is clean.
///
/// # Errors
/// Currently infallible. The return type is `Result` for API consistency with
/// future implementations that may perform I/O (e.g. reading version data
/// from the BSA archives themselves).
pub fn detect_address_lib_conflicts(
    mod_files: &[(String, Vec<String>)],
    game_version: Option<&str>,
) -> Result<Vec<AddressLibConflict>, MantleError> {
    // Build: library_type → { game_version_str → [mod_ids that ship it] }
    let mut by_type: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();

    for (mod_id, files) in mod_files {
        for path in files {
            if let Some(parsed) = parse_address_lib_path(path) {
                by_type
                    .entry(parsed.library_type)
                    .or_default()
                    .entry(parsed.game_version)
                    .or_default()
                    .push(mod_id.clone());
            }
        }
    }

    let mut conflicts = Vec::new();

    for (lib_type, version_map) in &by_type {
        // Flatten to (mod_id, version) pairs for the conflict report.
        let mut shipping_mods: Vec<(String, String)> = version_map
            .iter()
            .flat_map(|(ver, mods)| mods.iter().map(move |m| (m.clone(), ver.clone())))
            .collect();
        shipping_mods.sort_unstable();

        if version_map.len() > 1 {
            // Multiple distinct versions present — inter-mod mismatch.
            let mut versions: Vec<String> = version_map.keys().cloned().collect();
            versions.sort_unstable();

            conflicts.push(AddressLibConflict {
                library_type: lib_type.clone(),
                shipping_mods,
                kind: AddressLibMismatch::InterModMismatch { versions },
            });
        } else if let Some(gv) = game_version {
            // Single version — check it against the installed game.
            let Some((lib_ver, _)) = version_map.iter().next() else {
                continue; // map is non-empty by construction; belt-and-suspenders guard
            };
            if lib_ver != gv {
                conflicts.push(AddressLibConflict {
                    library_type: lib_type.clone(),
                    shipping_mods,
                    kind: AddressLibMismatch::GameVersionMismatch {
                        game_version: gv.to_owned(),
                        library_version: lib_ver.clone(),
                    },
                });
            }
        }
        // If version_map.len() == 1 and game_version is None → no conflict detectable.
    }

    // Sort output for deterministic ordering.
    conflicts.sort_unstable_by(|a, b| a.library_type.cmp(&b.library_type));

    Ok(conflicts)
}

// ─── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_address_lib_path ───────────────────────────────────────────────

    #[test]
    fn parse_versionlib_sse_root() {
        let p = parse_address_lib_path("data/skse/plugins/versionlib-1-6-659-0.bin").unwrap();
        assert_eq!(p.library_type, "versionlib");
        assert_eq!(p.game_version, "1.6.659.0");
    }

    #[test]
    fn parse_versionlib64_ae() {
        let p = parse_address_lib_path("versionlib64-1-6-640-0.bin").unwrap();
        assert_eq!(p.library_type, "versionlib64");
        assert_eq!(p.game_version, "1.6.640.0");
    }

    #[test]
    fn parse_no_path_prefix() {
        let p = parse_address_lib_path("versionlib-1-5-97-0.bin").unwrap();
        assert_eq!(p.game_version, "1.5.97.0");
    }

    #[test]
    fn parse_returns_none_for_non_versionlib() {
        assert!(parse_address_lib_path("data/skse/plugins/some_plugin.dll").is_none());
        assert!(parse_address_lib_path("plugin.esp").is_none());
        assert!(parse_address_lib_path("versionlib.bin").is_none()); // no version
    }

    #[test]
    fn parse_returns_none_for_non_numeric_version() {
        assert!(parse_address_lib_path("versionlib-1-6-abc-0.bin").is_none());
    }

    #[test]
    fn parse_returns_none_for_wrong_part_count() {
        assert!(parse_address_lib_path("versionlib-1-6-659.bin").is_none()); // 3 parts
        assert!(parse_address_lib_path("versionlib-1-6-659-0-extra.bin").is_none());
        // 5 parts
    }

    // ── detect_address_lib_conflicts ─────────────────────────────────────────

    fn files(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|&s| s.to_owned()).collect()
    }

    #[test]
    fn no_mods_returns_empty() {
        let result = detect_address_lib_conflicts(&[], None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn mods_without_address_lib_are_clean() {
        let mods = vec![
            ("mod-a".to_owned(), files(&["data/plugin.esp", "data/textures/foo.dds"])),
            ("mod-b".to_owned(), files(&["data/plugin.esp"])),
        ];
        let result = detect_address_lib_conflicts(&mods, None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn single_mod_single_version_no_game_version_is_clean() {
        let mods =
            vec![("mod-a".to_owned(), files(&["data/skse/plugins/versionlib-1-6-659-0.bin"]))];
        let result = detect_address_lib_conflicts(&mods, None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn single_mod_matches_game_version_is_clean() {
        let mods =
            vec![("mod-a".to_owned(), files(&["data/skse/plugins/versionlib-1-6-659-0.bin"]))];
        let result = detect_address_lib_conflicts(&mods, Some("1.6.659.0")).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn two_mods_same_version_no_game_check_is_clean() {
        let mods = vec![
            ("mod-a".to_owned(), files(&["data/skse/plugins/versionlib-1-6-659-0.bin"])),
            ("mod-b".to_owned(), files(&["data/skse/plugins/versionlib-1-6-659-0.bin"])),
        ];
        // Same version — no inter-mod mismatch. Without game_version, no
        // game-mismatch check either.
        let result = detect_address_lib_conflicts(&mods, None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn two_mods_different_versions_is_inter_mod_conflict() {
        let mods = vec![
            ("mod-a".to_owned(), files(&["data/skse/plugins/versionlib-1-6-659-0.bin"])),
            ("mod-b".to_owned(), files(&["data/skse/plugins/versionlib-1-5-97-0.bin"])),
        ];
        let result = detect_address_lib_conflicts(&mods, None).unwrap();
        assert_eq!(result.len(), 1);

        let c = &result[0];
        assert_eq!(c.library_type, "versionlib");
        assert!(
            matches!(&c.kind, AddressLibMismatch::InterModMismatch { versions } if versions.len() == 2)
        );

        let AddressLibMismatch::InterModMismatch { versions } = &c.kind else {
            panic!("wrong kind")
        };
        assert!(versions.contains(&"1.6.659.0".to_owned()));
        assert!(versions.contains(&"1.5.97.0".to_owned()));
    }

    #[test]
    fn inter_mod_conflict_shipping_mods_contains_all_involved() {
        let mods = vec![
            ("mod-a".to_owned(), files(&["data/skse/plugins/versionlib-1-6-659-0.bin"])),
            ("mod-b".to_owned(), files(&["data/skse/plugins/versionlib-1-5-97-0.bin"])),
        ];
        let result = detect_address_lib_conflicts(&mods, None).unwrap();
        let c = &result[0];
        let mod_ids: Vec<&str> = c.shipping_mods.iter().map(|(id, _)| id.as_str()).collect();
        assert!(mod_ids.contains(&"mod-a"));
        assert!(mod_ids.contains(&"mod-b"));
    }

    #[test]
    fn single_mod_wrong_game_version_is_game_mismatch() {
        let mods =
            vec![("mod-a".to_owned(), files(&["data/skse/plugins/versionlib-1-5-97-0.bin"]))];
        let result = detect_address_lib_conflicts(&mods, Some("1.6.659.0")).unwrap();
        assert_eq!(result.len(), 1);

        let c = &result[0];
        assert!(matches!(
            &c.kind,
            AddressLibMismatch::GameVersionMismatch {
                game_version,
                library_version,
            }
            if game_version == "1.6.659.0" && library_version == "1.5.97.0"
        ));
    }

    #[test]
    fn versionlib_and_versionlib64_are_independent_types() {
        let mods = vec![
            ("mod-a".to_owned(), files(&["data/skse/plugins/versionlib-1-6-659-0.bin"])),
            ("mod-b".to_owned(), files(&["data/skse/plugins/versionlib64-1-6-640-0.bin"])),
        ];
        // These are different library types — no inter-mod conflict between them.
        let result = detect_address_lib_conflicts(&mods, None).unwrap();
        assert!(result.is_empty(), "different library types must not conflict with each other");
    }

    #[test]
    fn versionlib64_mismatch_detected_independently() {
        let mods = vec![
            ("mod-a".to_owned(), files(&["data/skse/plugins/versionlib64-1-6-659-0.bin"])),
            ("mod-b".to_owned(), files(&["data/skse/plugins/versionlib64-1-6-640-0.bin"])),
        ];
        let result = detect_address_lib_conflicts(&mods, None).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].library_type, "versionlib64");
    }

    #[test]
    fn three_mods_three_versions_lists_all_versions() {
        let mods = vec![
            ("mod-a".to_owned(), files(&["data/skse/plugins/versionlib-1-6-659-0.bin"])),
            ("mod-b".to_owned(), files(&["data/skse/plugins/versionlib-1-6-640-0.bin"])),
            ("mod-c".to_owned(), files(&["data/skse/plugins/versionlib-1-5-97-0.bin"])),
        ];
        let result = detect_address_lib_conflicts(&mods, None).unwrap();
        assert_eq!(result.len(), 1);
        let AddressLibMismatch::InterModMismatch { versions } = &result[0].kind else {
            panic!("expected InterModMismatch")
        };
        assert_eq!(versions.len(), 3);
    }

    #[test]
    fn inter_mod_mismatch_takes_priority_over_game_version_check() {
        // When there's an inter-mod mismatch, we report that — not a game-version mismatch.
        let mods = vec![
            ("mod-a".to_owned(), files(&["data/skse/plugins/versionlib-1-6-659-0.bin"])),
            ("mod-b".to_owned(), files(&["data/skse/plugins/versionlib-1-5-97-0.bin"])),
        ];
        let result = detect_address_lib_conflicts(&mods, Some("1.6.659.0")).unwrap();
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0].kind, AddressLibMismatch::InterModMismatch { .. }));
    }

    #[test]
    fn output_is_sorted_by_library_type() {
        let mods = vec![
            ("mod-a".to_owned(), files(&["data/skse/plugins/versionlib64-1-6-659-0.bin"])),
            ("mod-b".to_owned(), files(&["data/skse/plugins/versionlib64-1-6-640-0.bin"])),
            ("mod-c".to_owned(), files(&["data/skse/plugins/versionlib-1-6-659-0.bin"])),
            ("mod-d".to_owned(), files(&["data/skse/plugins/versionlib-1-5-97-0.bin"])),
        ];
        let result = detect_address_lib_conflicts(&mods, None).unwrap();
        assert_eq!(result.len(), 2);
        assert!(result[0].library_type <= result[1].library_type, "output must be sorted");
    }
}
