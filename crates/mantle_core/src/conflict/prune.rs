//! Conflict-loser pruner — removes files that are fully overridden by higher-priority mods.
//!
//! Given a [`ConflictMap`] and a mods directory, this module moves the
//! conflict-losing files from their source mod directories to a timestamped
//! backup directory.  The originals are only removed after the backup copy
//! succeeds, so the operation is safe against partial failures.
//!
//! # Workflow
//! 1. Build a [`ConflictMap`] with [`crate::conflict::build_conflict_map`].
//! 2. Call [`prune_losers`] with the map, the mods root, and a backup root.
//! 3. Inspect [`PruneResult`] for counts and any errors.
//!
//! # File layout
//! Backup files are placed at `{backup_dir}/{mod_id}/{relative_path}`,
//! preserving the original directory structure so backups can be restored.
//!
//! # Note on path casing
//! The conflict map stores paths in lowercase (matching the case-fold
//! normalizer output).  Files are looked up on disk with the lowercase path
//! directly.  If the original file was not case-folded before installation the
//! lookup will fail gracefully and the entry will appear in
//! [`PruneResult::skipped_missing`] rather than erroring.

use std::path::{Path, PathBuf};

use crate::conflict::ConflictMap;

// ── Public types ──────────────────────────────────────────────────────────────

/// Outcome of a [`prune_losers`] call.
#[derive(Debug, Default)]
pub struct PruneResult {
    /// `(src, dst)` pairs for files successfully moved to the backup directory.
    pub moved: Vec<(PathBuf, PathBuf)>,
    /// `(mod_id, relative_path)` for files that did not exist on disk
    /// (already deleted, not yet extracted, or stored under a different case).
    pub skipped_missing: Vec<(String, String)>,
    /// `(src_path, error_message)` for files that could not be moved.
    pub errors: Vec<(PathBuf, String)>,
}

impl PruneResult {
    /// Number of files successfully moved to the backup directory.
    #[must_use]
    pub fn moved_count(&self) -> usize {
        self.moved.len()
    }

    /// `true` if every file was either moved or skipped; no I/O errors occurred.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Move all conflict-losing files from `mods_dir` into `backup_dir`.
///
/// Iterates every [`crate::conflict::ConflictEntry`] in `conflict_map`,
/// locates the losing mod's file on disk at `{mods_dir}/{mod_id}/{path}`,
/// copies it to `{backup_dir}/{mod_id}/{path}`, and removes the original.
///
/// Parent directories in `backup_dir` are created automatically.  If a source
/// file does not exist it is silently recorded in
/// [`PruneResult::skipped_missing`] (not an error).  If the copy succeeds but
/// the removal fails the error is recorded in [`PruneResult::errors`].
///
/// # Parameters
/// - `conflict_map`: Conflict scan result; provides the loser lists.
/// - `mods_dir`: Root directory containing all installed mod directories
///   (each subdirectory is one mod, named by its slug).
/// - `backup_dir`: Destination root for backup files.  Created if absent.
///
/// # Returns
/// A [`PruneResult`] with full per-file outcomes.
#[must_use]
pub fn prune_losers(conflict_map: &ConflictMap, mods_dir: &Path, backup_dir: &Path) -> PruneResult {
    let mut result = PruneResult::default();

    for entry in conflict_map.all_entries() {
        for loser in &entry.losers {
            let src = mods_dir.join(loser).join(&entry.path);
            if !src.exists() {
                result.skipped_missing.push((loser.clone(), entry.path.clone()));
                continue;
            }

            let dst = backup_dir.join(loser).join(&entry.path);

            // Create parent directories in backup destination.
            if let Some(parent) = dst.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    result.errors.push((src, e.to_string()));
                    continue;
                }
            }

            // Copy to backup, then remove original.
            if let Err(e) = std::fs::copy(&src, &dst) {
                result.errors.push((src, e.to_string()));
                continue;
            }

            if let Err(e) = std::fs::remove_file(&src) {
                result.errors.push((src.clone(), format!("backup ok but remove failed: {e}")));
                // Don't count as moved — file still exists at source.
                continue;
            }

            result.moved.push((src, dst));
        }
    }

    result
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conflict::{build_conflict_map, ModEntry};
    use std::fs;
    use tempfile::TempDir;

    fn entry(id: &str, files: &[&str]) -> ModEntry {
        ModEntry {
            id: id.to_owned(),
            files: files.iter().map(|&s| s.to_owned()).collect(),
        }
    }

    /// Create a temp mods directory with the given layout:
    /// `layout` is a list of `(mod_slug, relative_file_path)` pairs.
    fn build_mods_dir(layout: &[(&str, &str)]) -> TempDir {
        let mods = TempDir::new().unwrap();
        for (slug, rel) in layout {
            let p = mods.path().join(slug).join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(p, b"file content").unwrap();
        }
        mods
    }

    // ── PruneResult helpers ───────────────────────────────────────────────

    #[test]
    fn is_ok_true_when_no_errors() {
        let r = PruneResult::default();
        assert!(r.is_ok());
        assert_eq!(r.moved_count(), 0);
    }

    #[test]
    fn is_ok_false_when_errors_present() {
        let mut r = PruneResult::default();
        r.errors.push((PathBuf::from("x"), "oops".into()));
        assert!(!r.is_ok());
    }

    // ── prune_losers ──────────────────────────────────────────────────────

    #[test]
    fn no_conflicts_moves_nothing() {
        let mods = build_mods_dir(&[("mod_a", "data/a.esp")]);
        let backup = TempDir::new().unwrap();
        let map = build_conflict_map(&[entry("mod_a", &["data/a.esp"])]);
        let result = prune_losers(&map, mods.path(), backup.path());
        assert_eq!(result.moved_count(), 0);
        assert!(result.is_ok());
        // File still in mod_a
        assert!(mods.path().join("mod_a/data/a.esp").exists());
    }

    #[test]
    fn losing_file_moved_to_backup() {
        let mods = build_mods_dir(&[
            ("high_prio", "data/shared.esp"),
            ("low_prio", "data/shared.esp"),
        ]);
        let backup = TempDir::new().unwrap();
        let map = build_conflict_map(&[
            entry("high_prio", &["data/shared.esp"]),
            entry("low_prio", &["data/shared.esp"]),
        ]);
        let result = prune_losers(&map, mods.path(), backup.path());

        assert_eq!(result.moved_count(), 1, "one loser file must be moved");
        assert!(result.is_ok());

        // Winner's file must remain.
        assert!(mods.path().join("high_prio/data/shared.esp").exists());
        // Loser's file must be gone from mod dir.
        assert!(!mods.path().join("low_prio/data/shared.esp").exists());
        // Loser's file must be in backup.
        assert!(backup.path().join("low_prio/data/shared.esp").exists());
    }

    #[test]
    fn multiple_losers_all_moved() {
        let mods = build_mods_dir(&[
            ("a", "data/shared.nif"),
            ("b", "data/shared.nif"),
            ("c", "data/shared.nif"),
        ]);
        let backup = TempDir::new().unwrap();
        let map = build_conflict_map(&[
            entry("a", &["data/shared.nif"]),
            entry("b", &["data/shared.nif"]),
            entry("c", &["data/shared.nif"]),
        ]);
        let result = prune_losers(&map, mods.path(), backup.path());
        assert_eq!(result.moved_count(), 2, "two losers must be moved");
        assert!(!mods.path().join("b/data/shared.nif").exists());
        assert!(!mods.path().join("c/data/shared.nif").exists());
        assert!(backup.path().join("b/data/shared.nif").exists());
        assert!(backup.path().join("c/data/shared.nif").exists());
    }

    #[test]
    fn missing_file_recorded_in_skipped_missing() {
        // The conflict map claims "low_prio" loses, but the file is absent on disk.
        let mods = build_mods_dir(&[("high_prio", "data/shared.esp")]);
        // low_prio directory exists but does NOT contain the file.
        fs::create_dir_all(mods.path().join("low_prio")).unwrap();
        let backup = TempDir::new().unwrap();
        let map = build_conflict_map(&[
            entry("high_prio", &["data/shared.esp"]),
            entry("low_prio", &["data/shared.esp"]),
        ]);
        let result = prune_losers(&map, mods.path(), backup.path());
        assert_eq!(result.moved_count(), 0);
        assert_eq!(result.skipped_missing.len(), 1);
        assert!(result.is_ok());
    }

    #[test]
    fn only_conflicted_files_are_pruned() {
        // mod_a wins shared.esp; mod_b has a unique file that must be untouched.
        let mods = build_mods_dir(&[
            ("mod_a", "data/shared.esp"),
            ("mod_b", "data/shared.esp"),
            ("mod_b", "data/unique.nif"),
        ]);
        let backup = TempDir::new().unwrap();
        let map = build_conflict_map(&[
            entry("mod_a", &["data/shared.esp"]),
            entry("mod_b", &["data/shared.esp", "data/unique.nif"]),
        ]);
        let result = prune_losers(&map, mods.path(), backup.path());
        assert_eq!(result.moved_count(), 1);
        // Unique file must be untouched.
        assert!(mods.path().join("mod_b/data/unique.nif").exists());
    }

    #[test]
    fn backup_dir_created_automatically() {
        let mods = build_mods_dir(&[("winner", "data/file.esp"), ("loser", "data/file.esp")]);
        let parent = TempDir::new().unwrap();
        let backup = parent.path().join("deep/nested/backup");
        // backup does NOT exist yet
        assert!(!backup.exists());
        let map = build_conflict_map(&[
            entry("winner", &["data/file.esp"]),
            entry("loser", &["data/file.esp"]),
        ]);
        let result = prune_losers(&map, mods.path(), &backup);
        assert_eq!(result.moved_count(), 1);
        assert!(backup.join("loser/data/file.esp").exists());
    }
}
