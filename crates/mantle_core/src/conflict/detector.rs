//! Core conflict-detection algorithm.
//!
//! Scans an ordered list of mod file manifests and identifies every file path
//! that is claimed by more than one mod. The caller-supplied order determines
//! priority: **index 0 = highest priority** (winner).
//!
//! # Algorithm
//! ```text
//! seen: HashMap<path, winner_mod_id>
//!
//! for (mod_id, files) in mods (index 0 first — highest priority):
//!     for file in files:
//!         if seen.contains(file):
//!             // conflict — this mod is a loser for this path
//!             conflict_table[file].losers.push(mod_id)
//!         else:
//!             seen.insert(file, mod_id)
//!             // no conflict yet — this mod is the current owner
//! ```
//!
//! This is a single-pass O(∑ files) scan. The output is a populated
//! [`ConflictMap`] with no entries for clean (uncontested) paths.

use std::collections::HashMap;

use super::{ConflictEntry, ConflictMap, ModEntry};

/// Scan `mods` for file-level conflicts and return a populated [`ConflictMap`].
///
/// `mods[0]` = highest priority. For each path claimed by multiple mods, the
/// first mod to claim it (lowest index) is the winner; all later claimants are
/// losers.
///
/// # Parameters
/// - `mods`: Priority-ordered slice of mod manifests. Must not contain
///   duplicate `ModId` values — behaviour is unspecified if two entries share
///   the same `id`.
pub fn detect(mods: &[ModEntry]) -> ConflictMap {
    // First pass: build a table of path → winner (first mod to claim it).
    // winner_table maps each path to the mod id that owns it.
    let mut winner_table: HashMap<&str, &str> = HashMap::new();

    // conflict_table accumulates losers per path.
    // We use an IndexMap-style structure: just a Vec of (path, loser) pairs
    // that we fold into ConflictEntry later.
    //
    // Using a Vec<(String, String)> and deduplicating into ConflictEntry at
    // the end keeps the hot loop free of HashMap entry() overhead for the
    // common (no-conflict) case.
    let mut loser_list: Vec<(&str, &str)> = Vec::new();

    for entry in mods {
        for file in &entry.files {
            match winner_table.entry(file.as_str()) {
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert(entry.id.as_str());
                }
                std::collections::hash_map::Entry::Occupied(_) => {
                    // This file is already owned by a higher-priority mod.
                    loser_list.push((file.as_str(), entry.id.as_str()));
                }
            }
        }
    }

    if loser_list.is_empty() {
        return ConflictMap::empty();
    }

    // Second pass: fold (path, loser) pairs into ConflictEntry objects.
    // We need the winner for each contested path — it is already in winner_table.
    let mut map = ConflictMap::empty();

    // Group losers by path. Use a temp HashMap to collect them.
    let mut contested: HashMap<&str, Vec<&str>> = HashMap::new();
    for (path, loser) in &loser_list {
        contested.entry(path).or_default().push(loser);
    }

    for (path, losers) in contested {
        let winner = winner_table[path];
        map.insert(ConflictEntry {
            path: path.to_owned(),
            winner: winner.to_owned(),
            losers: losers.into_iter().map(str::to_owned).collect(),
        });
    }

    map
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, files: &[&str]) -> ModEntry {
        ModEntry {
            id: id.to_owned(),
            files: files.iter().map(|&s| s.to_owned()).collect(),
        }
    }

    #[test]
    fn no_mods_is_clean() {
        assert!(detect(&[]).is_clean());
    }

    #[test]
    fn single_mod_all_files_are_clean() {
        let map = detect(&[entry("a", &["data/a.nif", "data/b.dds", "data/plugin.esp"])]);
        assert!(map.is_clean());
    }

    #[test]
    fn disjoint_mods_are_clean() {
        let map = detect(&[
            entry("a", &["data/a.esp"]),
            entry("b", &["data/b.nif"]),
            entry("c", &["data/c.dds"]),
        ]);
        assert!(map.is_clean());
    }

    #[test]
    fn two_mods_sharing_one_file_winner_is_index_zero() {
        let map = detect(&[
            entry("high", &["data/shared.esp"]),
            entry("low", &["data/shared.esp"]),
        ]);
        assert_eq!(map.total_file_conflicts(), 1);
        let e = map.entry_for_path("data/shared.esp").unwrap();
        assert_eq!(e.winner, "high");
        assert_eq!(e.losers, ["low"]);
    }

    #[test]
    fn three_mods_same_file_one_winner_two_losers() {
        let map = detect(&[
            entry("a", &["data/x.esp"]),
            entry("b", &["data/x.esp"]),
            entry("c", &["data/x.esp"]),
        ]);
        let e = map.entry_for_path("data/x.esp").unwrap();
        assert_eq!(e.winner, "a");
        let mut losers = e.losers.clone();
        losers.sort();
        assert_eq!(losers, ["b", "c"]);
    }

    #[test]
    fn multiple_conflicted_paths() {
        let map = detect(&[
            entry("a", &["data/x.esp", "data/y.nif"]),
            entry("b", &["data/x.esp", "data/z.dds"]),
            entry("c", &["data/y.nif", "data/z.dds"]),
        ]);
        assert_eq!(map.total_file_conflicts(), 3);
        assert_eq!(map.entry_for_path("data/x.esp").unwrap().winner, "a");
        assert_eq!(map.entry_for_path("data/y.nif").unwrap().winner, "a");
        assert_eq!(map.entry_for_path("data/z.dds").unwrap().winner, "b");
    }

    #[test]
    fn clean_paths_absent_from_entries() {
        let map = detect(&[
            entry("a", &["data/shared.esp", "data/only_a.nif"]),
            entry("b", &["data/shared.esp", "data/only_b.nif"]),
        ]);
        // shared.esp is contested; the others are clean.
        assert_eq!(map.total_file_conflicts(), 1);
        assert!(map.entry_for_path("data/only_a.nif").is_none());
        assert!(map.entry_for_path("data/only_b.nif").is_none());
    }

    #[test]
    fn large_disjoint_lists_no_false_positive() {
        // Stress: 100 mods × 50 unique files each → 0 conflicts.
        let mods: Vec<ModEntry> = (0..100_u32)
            .map(|m| ModEntry {
                id: format!("mod_{m}"),
                files: (0..50_u32).map(|f| format!("data/mod{m}_file{f}.nif")).collect(),
            })
            .collect();
        assert!(detect(&mods).is_clean());
    }
}
