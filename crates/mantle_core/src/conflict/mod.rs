//! Conflict detection — file-level ownership and conflict graph.
//!
//! Determines which mod "wins" each file path under the active profile by
//! comparing the file manifests of all enabled mods against their priority
//! ordering.
//!
//! # Priority convention
//! Index 0 in the input slice = **highest priority** (wins conflicts).
//! This matches [`crate::vfs::types::MountParams::lower_dirs`] so the
//! conflict map always agrees with what the VFS layer will actually mount.
//!
//! # Usage
//! ```ignore
//! use mantle_core::conflict::{build_conflict_map, ModEntry};
//!
//! // Mods in priority order — index 0 wins conflicts.
//! let mods = vec![
//!     ModEntry { id: "mod_a".into(), files: vec!["Data/plugin.esp".into()] },
//!     ModEntry { id: "mod_b".into(), files: vec!["Data/plugin.esp".into(),
//!                                                "Data/mesh.nif".into()] },
//! ];
//! let map = build_conflict_map(&mods);
//! assert_eq!(map.total_file_conflicts(), 1);
//! ```
//!
//! # Module layout
//! - [`detector`]    — core scan algorithm (file-path collision detection)
//! - [`resolution`]  — [`ModRole`] enum, winner/loser/clean queries
//! - [`dll`]         — SKSE/F4SE/xNVSE/OBSE DLL collision detection
//! - [`address_lib`] — Address Library version mismatch detection
//! - [`prune`]       — Move conflict-losing files to a backup directory

pub mod address_lib;
pub mod detector;
pub mod dll;
pub mod prune;
pub mod resolution;

pub use address_lib::{detect_address_lib_conflicts, AddressLibConflict, AddressLibMismatch};
pub use detector::detect;
pub use dll::{detect_dll_conflicts, dll_files_for_profile, is_se_plugin_dll, DllConflict};
pub use prune::{prune_losers, PruneResult};
pub use resolution::{ConflictSummary, ModRole};

use std::collections::HashMap;

// ─── Public types ─────────────────────────────────────────────────────────────

/// An opaque mod identifier. Usually the mod's slug string.
///
/// Stored by value to keep [`ConflictMap`] self-contained without lifetime
/// parameters. Most mod lists are ≤ 1 000 entries; clone cost is negligible.
pub type ModId = String;

/// One mod's manifest entry — the inputs fed to [`build_conflict_map`].
///
/// # Fields
/// - `id`: The mod's stable identifier (slug).
/// - `files`: All relative file paths provided by this mod. Paths should be
///   lowercase for case-insensitive matching; the detector does not normalise
///   them itself (see discussion in [`detector`]).
#[derive(Debug, Clone)]
pub struct ModEntry {
    /// Stable mod identifier (slug).
    pub id: ModId,
    /// Relative file paths provided by this mod, lowercase.
    pub files: Vec<String>,
}

/// A single file-level conflict: one winner and one or more losers for the
/// same relative path.
///
/// The winner is always the **first** (highest-priority, index 0) mod in the
/// input slice that claims this path. All subsequent mods that also claim this
/// path are losers.
#[derive(Debug, Clone)]
pub struct ConflictEntry {
    /// The conflicted path (relative, lowercase).
    pub path: String,
    /// The mod whose file the VFS will actually present at this path.
    pub winner: ModId,
    /// All mods whose files are hidden by `winner` (and by each other in
    /// priority order — a higher-indexed loser is hidden by a lower-indexed
    /// one, not only by the winner).
    pub losers: Vec<ModId>,
}

// ─── ConflictMap ──────────────────────────────────────────────────────────────

/// The result of a full conflict scan across a prioritised mod list.
///
/// Produced by [`build_conflict_map`]. Consumed by the UI layer (to annotate
/// the mod list with win/loss/clean states) and by the `ConflictMapUpdated`
/// event fired after every mod state change.
///
/// # Querying
/// - [`ConflictMap::role_of_mod`] — is a mod a winner, loser, or clean?
/// - [`ConflictMap::conflicts_for_mod`] — all conflict entries involving a mod.
/// - [`ConflictMap::conflicted_paths`] — the set of all contested file paths.
/// - [`ConflictMap::total_file_conflicts`] — total number of contested paths.
#[derive(Debug, Clone, Default)]
pub struct ConflictMap {
    /// Map from lowercase relative path → conflict entry.
    /// Only paths with at least one loser are present.
    entries: HashMap<String, ConflictEntry>,
}

impl ConflictMap {
    /// Build an empty `ConflictMap`. Mostly useful for tests.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Total number of file paths that have at least one conflict.
    ///
    /// This is the count of distinct contested paths, not the total number of
    /// (winner, loser) pairs.
    #[must_use]
    pub fn total_file_conflicts(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if there are no contested file paths.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.entries.is_empty()
    }

    /// The role of `mod_id` in the current conflict map.
    ///
    /// Returns [`ModRole::Clean`] if the mod appears in no entry as either
    /// winner or loser.
    ///
    /// # Parameters
    /// - `mod_id`: The slug of the mod to query.
    #[must_use]
    pub fn role_of_mod(&self, mod_id: &str) -> ModRole {
        resolution::role_of_mod(self, mod_id)
    }

    /// All conflict entries in which `mod_id` participates (as winner or loser).
    ///
    /// Returns an empty iterator if the mod has no conflicts.
    pub fn conflicts_for_mod<'a>(
        &'a self,
        mod_id: &'a str,
    ) -> impl Iterator<Item = &'a ConflictEntry> + 'a {
        self.entries
            .values()
            .filter(move |e| e.winner == mod_id || e.losers.iter().any(|l| l == mod_id))
    }

    /// Iterator over all contested file paths.
    pub fn conflicted_paths(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Iterator over all [`ConflictEntry`] values.
    pub fn all_entries(&self) -> impl Iterator<Item = &ConflictEntry> {
        self.entries.values()
    }

    /// Look up the conflict entry for a specific path, if any.
    ///
    /// Returns `None` when the path is not contested.
    #[must_use]
    pub fn entry_for_path(&self, path: &str) -> Option<&ConflictEntry> {
        self.entries.get(path)
    }

    /// Number of file paths that `mod_id` wins (i.e. whose higher-priority file is from this mod).
    #[must_use]
    pub fn win_count_for_mod(&self, mod_id: &str) -> usize {
        self.entries.values().filter(|e| e.winner == mod_id).count()
    }

    /// Number of file paths that `mod_id` loses.
    #[must_use]
    pub fn loss_count_for_mod(&self, mod_id: &str) -> usize {
        self.entries.values().filter(|e| e.losers.iter().any(|l| l == mod_id)).count()
    }

    /// Internal: insert a [`ConflictEntry`]. Used only by [`detector`].
    pub(crate) fn insert(&mut self, entry: ConflictEntry) {
        self.entries.insert(entry.path.clone(), entry);
    }
}

// ─── Public constructor ───────────────────────────────────────────────────────

/// Build a [`ConflictMap`] from a priority-ordered slice of mod manifests.
///
/// `mods[0]` has the **highest priority** and wins all conflicts. This matches
/// the [`crate::vfs::types::MountParams::lower_dirs`] convention so the
/// conflict map is always consistent with the VFS mount view.
///
/// # Complexity
/// `O(∑ files_per_mod)` — one `HashMap` insert per file path across all mods.
///
/// # Parameters
/// - `mods`: Slice of [`ModEntry`] in priority order (index 0 = highest).
#[must_use]
pub fn build_conflict_map(mods: &[ModEntry]) -> ConflictMap {
    detector::detect(mods)
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
    fn empty_mod_list_is_clean() {
        let map = build_conflict_map(&[]);
        assert!(map.is_clean());
        assert_eq!(map.total_file_conflicts(), 0);
    }

    #[test]
    fn single_mod_no_conflicts() {
        let map = build_conflict_map(&[entry("a", &["data/plugin.esp", "data/mesh.nif"])]);
        assert!(map.is_clean());
    }

    #[test]
    fn two_mods_no_shared_files() {
        let map = build_conflict_map(&[entry("a", &["data/a.esp"]), entry("b", &["data/b.esp"])]);
        assert!(map.is_clean());
    }

    #[test]
    fn two_mods_one_conflict_winner_is_index_zero() {
        let map = build_conflict_map(&[
            entry("high", &["data/shared.esp", "data/high_only.nif"]),
            entry("low", &["data/shared.esp", "data/low_only.nif"]),
        ]);
        assert_eq!(map.total_file_conflicts(), 1);
        let e = map.entry_for_path("data/shared.esp").unwrap();
        assert_eq!(e.winner, "high");
        assert_eq!(e.losers, ["low"]);
    }

    #[test]
    fn three_mods_all_sharing_one_file() {
        let map = build_conflict_map(&[
            entry("a", &["data/shared.esp"]),
            entry("b", &["data/shared.esp"]),
            entry("c", &["data/shared.esp"]),
        ]);
        let e = map.entry_for_path("data/shared.esp").unwrap();
        assert_eq!(e.winner, "a");
        assert_eq!(e.losers.len(), 2);
        assert!(e.losers.contains(&"b".to_owned()));
        assert!(e.losers.contains(&"c".to_owned()));
    }

    #[test]
    fn role_winner_loser_clean() {
        let map = build_conflict_map(&[
            entry("winner", &["data/shared.esp"]),
            entry("loser", &["data/shared.esp"]),
            entry("clean", &["data/unique.nif"]),
        ]);
        assert_eq!(map.role_of_mod("winner"), ModRole::Winner);
        assert_eq!(map.role_of_mod("loser"), ModRole::Loser);
        assert_eq!(map.role_of_mod("clean"), ModRole::Clean);
    }

    #[test]
    fn mod_can_be_both_winner_and_loser_on_different_paths() {
        // "mid" wins against "low" but loses to "high"
        let map = build_conflict_map(&[
            entry("high", &["data/shared.esp"]),
            entry("mid", &["data/shared.esp", "data/other.nif"]),
            entry("low", &["data/other.nif"]),
        ]);
        // mid loses "shared.esp" to high, wins "other.nif" over low
        assert_eq!(map.role_of_mod("mid"), ModRole::Both);
        assert_eq!(map.win_count_for_mod("mid"), 1);
        assert_eq!(map.loss_count_for_mod("mid"), 1);
    }

    #[test]
    fn conflicts_for_mod_returns_relevant_entries() {
        let map = build_conflict_map(&[
            entry("a", &["data/x.esp", "data/y.nif"]),
            entry("b", &["data/x.esp"]),
            entry("c", &["data/y.nif"]),
        ]);
        let a_conflicts: Vec<_> = map.conflicts_for_mod("a").collect();
        assert_eq!(a_conflicts.len(), 2); // wins both
        let b_conflicts: Vec<_> = map.conflicts_for_mod("b").collect();
        assert_eq!(b_conflicts.len(), 1);
    }

    #[test]
    fn win_count_and_loss_count() {
        let map = build_conflict_map(&[
            entry("a", &["data/x.esp", "data/y.nif", "data/z.dds"]),
            entry("b", &["data/x.esp", "data/y.nif", "data/b_only.esp"]),
        ]);
        assert_eq!(map.win_count_for_mod("a"), 2);
        assert_eq!(map.loss_count_for_mod("a"), 0);
        assert_eq!(map.win_count_for_mod("b"), 0);
        assert_eq!(map.loss_count_for_mod("b"), 2);
    }

    #[test]
    fn entry_for_path_returns_none_for_clean_path() {
        let map = build_conflict_map(&[entry("a", &["data/only_in_a.esp"])]);
        assert!(map.entry_for_path("data/only_in_a.esp").is_none());
    }
}
