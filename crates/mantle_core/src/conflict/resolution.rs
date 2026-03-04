//! Conflict resolution helpers — per-mod role queries.
//!
//! After a [`super::ConflictMap`] has been built, the resolution layer answers
//! per-mod role questions used by the UI layer to annotate the mod list with
//! colour-coded win/loss/clean indicators.

use super::{ConflictMap, ModId};

// ─── ModRole ──────────────────────────────────────────────────────────────────

/// The role of a mod in the current conflict map.
///
/// Used by the UI to colour-code the mod list:
/// - **Winner** — this mod provides the active (visible to the game) version
///   of at least one contested file, and does not lose any.
/// - **Loser** — this mod provides no contested files that are active; all
///   its contested files are hidden by a higher-priority mod.
/// - **Both** — this mod wins some conflicts (beats lower-priority mods) but
///   also loses some (overridden by higher-priority mods).
/// - **Clean** — this mod has no contested files whatsoever.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModRole {
    /// Wins at least one conflict and loses none.
    Winner,
    /// Loses at least one conflict and wins none.
    Loser,
    /// Wins some conflicts and loses others.
    Both,
    /// No contested files — no conflicts at all.
    Clean,
}

impl std::fmt::Display for ModRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Winner => write!(f, "Winner"),
            Self::Loser => write!(f, "Loser"),
            Self::Both => write!(f, "Both"),
            Self::Clean => write!(f, "Clean"),
        }
    }
}

// ─── ConflictSummary ──────────────────────────────────────────────────────────

/// A per-mod summary of conflict statistics for UI display.
///
/// Returned by [`conflict_summary_for_mod`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictSummary {
    /// The mod this summary describes.
    pub mod_id: ModId,
    /// Number of contested file paths this mod wins.
    pub wins: usize,
    /// Number of contested file paths this mod loses.
    pub losses: usize,
    /// The overall role derived from `wins` / `losses`.
    pub role: ModRole,
}

impl ConflictSummary {
    /// Returns `true` if the mod has no wins or losses.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.role == ModRole::Clean
    }
}

// ─── Public functions ─────────────────────────────────────────────────────────

/// Determine the role of `mod_id` in the given conflict map.
///
/// | wins | losses | result        |
/// |------|--------|---------------|
/// | 0    | 0      | [`ModRole::Clean`]   |
/// | > 0  | 0      | [`ModRole::Winner`]  |
/// | 0    | > 0    | [`ModRole::Loser`]   |
/// | > 0  | > 0    | [`ModRole::Both`]    |
///
/// # Parameters
/// - `map`: The conflict map to query.
/// - `mod_id`: The slug of the mod to classify.
#[must_use]
pub fn role_of_mod(map: &ConflictMap, mod_id: &str) -> ModRole {
    let wins = map.win_count_for_mod(mod_id);
    let losses = map.loss_count_for_mod(mod_id);
    match (wins > 0, losses > 0) {
        (false, false) => ModRole::Clean,
        (true, false) => ModRole::Winner,
        (false, true) => ModRole::Loser,
        (true, true) => ModRole::Both,
    }
}

/// Build a detailed [`ConflictSummary`] for `mod_id`.
///
/// Provides counts alongside the role, useful for UI tooltips.
///
/// # Parameters
/// - `map`: The conflict map to query.
/// - `mod_id`: The slug of the mod to summarise.
#[must_use]
pub fn conflict_summary_for_mod(map: &ConflictMap, mod_id: &str) -> ConflictSummary {
    let wins = map.win_count_for_mod(mod_id);
    let losses = map.loss_count_for_mod(mod_id);
    let role = role_of_mod(map, mod_id);
    ConflictSummary {
        mod_id: mod_id.to_owned(),
        wins,
        losses,
        role,
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conflict::{build_conflict_map, ModEntry};

    fn entry(id: &str, files: &[&str]) -> ModEntry {
        ModEntry {
            id: id.to_owned(),
            files: files.iter().map(|&s| s.to_owned()).collect(),
        }
    }

    #[test]
    fn clean_mod_gets_clean_role() {
        let map = build_conflict_map(&[entry("only", &["data/x.esp"])]);
        assert_eq!(role_of_mod(&map, "only"), ModRole::Clean);
    }

    #[test]
    fn winner_role_when_wins_and_zero_losses() {
        let map = build_conflict_map(&[entry("a", &["data/x.esp"]), entry("b", &["data/x.esp"])]);
        assert_eq!(role_of_mod(&map, "a"), ModRole::Winner);
    }

    #[test]
    fn loser_role_when_losses_and_zero_wins() {
        let map = build_conflict_map(&[entry("a", &["data/x.esp"]), entry("b", &["data/x.esp"])]);
        assert_eq!(role_of_mod(&map, "b"), ModRole::Loser);
    }

    #[test]
    fn both_role_when_wins_and_losses() {
        // "mid" wins over "low" on y.nif, loses to "high" on x.esp.
        let map = build_conflict_map(&[
            entry("high", &["data/x.esp"]),
            entry("mid", &["data/x.esp", "data/y.nif"]),
            entry("low", &["data/y.nif"]),
        ]);
        assert_eq!(role_of_mod(&map, "mid"), ModRole::Both);
    }

    #[test]
    fn unknown_mod_is_clean() {
        let map = build_conflict_map(&[entry("a", &["data/x.esp"])]);
        assert_eq!(role_of_mod(&map, "nonexistent"), ModRole::Clean);
    }

    #[test]
    fn conflict_summary_correct_counts() {
        let map = build_conflict_map(&[
            entry("a", &["data/x.esp", "data/y.nif"]),
            entry("b", &["data/x.esp"]),
            entry("c", &["data/y.nif"]),
        ]);
        let s = conflict_summary_for_mod(&map, "a");
        assert_eq!(s.wins, 2);
        assert_eq!(s.losses, 0);
        assert_eq!(s.role, ModRole::Winner);
        assert!(!s.is_clean());
    }

    #[test]
    fn conflict_summary_clean_mod() {
        let map = build_conflict_map(&[entry("solo", &["data/unique.nif"])]);
        let s = conflict_summary_for_mod(&map, "solo");
        assert_eq!(s.wins, 0);
        assert_eq!(s.losses, 0);
        assert_eq!(s.role, ModRole::Clean);
        assert!(s.is_clean());
    }

    #[test]
    fn mod_role_display() {
        assert_eq!(ModRole::Winner.to_string(), "Winner");
        assert_eq!(ModRole::Loser.to_string(), "Loser");
        assert_eq!(ModRole::Both.to_string(), "Both");
        assert_eq!(ModRole::Clean.to_string(), "Clean");
    }
}
