//! Tier 3 VFS backend — symlink farm.
//!
//! Creates a directory of symlinks replicating the merged mod view.
//! No kernel privileges or FUSE required; works on any Linux system with
//! a writable temp directory.
//!
//! # Limitations (`VFS_DESIGN.md` §2.3)
//! - Game writes go to the **symlink target** (i.e. into the mod directory),
//!   not to an isolated working directory. Profile saves and game config
//!   changes may be misdirected.
//! - No atomic teardown — a crash between symlink creates leaves the merge
//!   directory in a partial state. `SymlinkFarm::unmount` handles this
//!   gracefully by skipping already-absent links.
//! - Setup time is `O(total files across all active mods)`.
//!
//! # Usage
//! ```ignore
//! let farm = SymlinkFarm::mount(&params)?;
//! farm.verify(&params)?;
//! // … game runs …
//! farm.unmount()?;
//! ```

use std::path::{Path, PathBuf};

use crate::{error::MantleError, vfs::types::MountParams};

// ─── Public struct ────────────────────────────────────────────────────────────

/// An active symlink-farm overlay.
///
/// Created by [`SymlinkFarm::mount`]. Holds the list of symlinks created so
/// that [`SymlinkFarm::unmount`] can remove exactly those links and nothing
/// else.
#[derive(Debug)]
pub struct SymlinkFarm {
    /// Absolute path to the merge directory presented to the game.
    merge_dir: PathBuf,

    /// All symlinks created during mount, in creation order.
    ///
    /// Used exclusively by `unmount` for precise teardown. The order does not
    /// matter for correctness but iteration in reverse is slightly more cache-
    /// friendly on large mod lists.
    created_links: Vec<PathBuf>,
}

impl SymlinkFarm {
    /// Create the symlink-farm overlay for the given mount parameters.
    ///
    /// # Behaviour
    /// 1. Creates `params.merge_dir` if it does not exist.
    /// 2. Iterates `params.lower_dirs` from **lowest priority to highest**
    ///    (i.e. from the last entry to index 0), symlinking each file into
    ///    `merge_dir`. Higher-priority entries overwrite existing links so
    ///    the winning file is always the highest-priority mod's copy.
    /// 3. Directories are created as real directories inside `merge_dir`
    ///    (not as symlinks) so that both high- and low-priority files under
    ///    a shared directory can coexist.
    ///
    /// # Errors
    /// Returns [`MantleError::Vfs`] if `merge_dir` cannot be created or if
    /// any symlink operation fails. Returns [`MantleError::Io`] for
    /// underlying I/O failures.
    ///
    /// # Parameters
    /// - `params`: [`MountParams`] describing priority-ordered lower dirs and
    ///   the merge target.
    pub fn mount(params: &MountParams) -> Result<Self, MantleError> {
        // Ensure the merge directory exists.
        std::fs::create_dir_all(&params.merge_dir).map_err(|e| {
            MantleError::Vfs(format!("cannot create merge dir {}: {e}", params.merge_dir.display()))
        })?;

        let mut created_links: Vec<PathBuf> = Vec::new();

        // Iterate from lowest priority (last index) to highest (index 0).
        // When the same relative path appears in multiple mods the higher-
        // priority entry writes last and wins the conflict.
        for lower_dir in params.lower_dirs.iter().rev() {
            if !lower_dir.exists() {
                return Err(MantleError::Vfs(format!(
                    "lower directory does not exist: {}",
                    lower_dir.display()
                )));
            }
            link_directory(lower_dir, lower_dir, &params.merge_dir, &mut created_links)?;
        }

        tracing::info!(
            "VFS symlink-farm mounted: {} links across {} lower dirs → {}",
            created_links.len(),
            params.lower_dirs.len(),
            params.merge_dir.display()
        );

        Ok(Self {
            merge_dir: params.merge_dir.clone(),
            created_links,
        })
    }

    /// Verify that the merge view looks correct after mount.
    ///
    /// Checks that `merge_dir` is a non-empty directory containing at least
    /// one entry for each lower directory that was non-empty. Light-weight —
    /// does not enumerate all files.
    ///
    /// # Errors
    /// Returns [`MantleError::Vfs`] if the merge directory is absent or empty
    /// when lower directories were non-empty.
    ///
    /// # Parameters
    /// - `params`: The same [`MountParams`] passed to [`SymlinkFarm::mount`].
    pub fn verify(&self, params: &MountParams) -> Result<(), MantleError> {
        if !self.merge_dir.is_dir() {
            return Err(MantleError::Vfs(format!(
                "merge dir is not a directory: {}",
                self.merge_dir.display()
            )));
        }

        // If any lower_dir had content we expect at least one symlink.
        let any_source_has_files = params.lower_dirs.iter().any(|d| has_any_file(d));

        if any_source_has_files && self.created_links.is_empty() {
            return Err(MantleError::Vfs(format!(
                "merge dir {} is empty after mount — no files were linked",
                self.merge_dir.display()
            )));
        }

        tracing::debug!(
            "VFS symlink-farm verify OK: {} links in {}",
            self.created_links.len(),
            self.merge_dir.display()
        );
        Ok(())
    }

    /// Tear down the symlink-farm overlay, removing all created symlinks.
    ///
    /// Skips links that are already absent (handles crash-recovery scenario).
    /// Does **not** remove the merge directory itself — it may have been
    /// pre-existing.
    ///
    /// After removing symlinks a best-effort sweep removes directories inside
    /// `merge_dir` that are now empty (walking bottom-up).
    ///
    /// # Errors
    /// Returns [`MantleError::Io`] if a symlink that is still present cannot
    /// be removed.
    pub fn unmount(self) -> Result<(), MantleError> {
        let mut first_error: Option<std::io::Error> = None;

        // Remove symlinks in reverse creation order (minor locality benefit).
        for link in self.created_links.iter().rev() {
            match std::fs::remove_file(link) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Already absent — no action needed (crash recovery path).
                }
                Err(e) => {
                    tracing::warn!("VFS symlink-farm: failed to remove {}: {e}", link.display());
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            }
        }

        // Best-effort: prune now-empty directories inside merge_dir.
        if self.merge_dir.is_dir() {
            prune_empty_dirs(&self.merge_dir);
        }

        if let Some(e) = first_error {
            return Err(MantleError::Io(e));
        }

        tracing::info!("VFS symlink-farm unmounted: {}", self.merge_dir.display());
        Ok(())
    }

    /// Number of symlinks currently held by this mount.
    ///
    /// Primarily useful for logging and tests.
    #[must_use]
    pub fn link_count(&self) -> usize {
        self.created_links.len()
    }

    /// Path to the merge directory.
    #[must_use]
    pub fn merge_dir(&self) -> &Path {
        &self.merge_dir
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Recursively walk `source_dir`, symlinking all files into `merge_dir`
/// preserving directory structure. `root` is the original lower-dir root used
/// to compute relative paths.
///
/// Directories are created (not symlinked) to allow multiple mods to
/// contribute files under the same subdirectory.
///
/// # Parameters
/// - `root`: The root of the lower directory (used to strip prefix for
///   relative path computation).
/// - `source_dir`: The current directory being walked (starts equal to `root`).
/// - `merge_dir`: Absolute path to the merge output directory.
/// - `created_links`: Accumulator — every symlink path is appended here.
fn link_directory(
    root: &Path,
    source_dir: &Path,
    merge_dir: &Path,
    created_links: &mut Vec<PathBuf>,
) -> Result<(), MantleError> {
    let entries = std::fs::read_dir(source_dir).map_err(MantleError::Io)?;

    for entry in entries {
        let entry = entry.map_err(MantleError::Io)?;
        let source_path = entry.path();
        let relative = source_path.strip_prefix(root).expect("entry is always under root");
        let dest_path = merge_dir.join(relative);

        let file_type = entry.file_type().map_err(MantleError::Io)?;

        if file_type.is_dir() {
            // Create real directory in merge so subdirectory files from
            // multiple mods can be placed alongside each other.
            if !dest_path.exists() {
                std::fs::create_dir_all(&dest_path).map_err(MantleError::Io)?;
            }
            // Recurse.
            link_directory(root, &source_path, merge_dir, created_links)?;
        } else if file_type.is_file() || file_type.is_symlink() {
            // If a lower-priority symlink already exists here, remove it so
            // this higher-priority mod's file can take its place.
            if dest_path.exists() || dest_path.symlink_metadata().is_ok() {
                std::fs::remove_file(&dest_path).map_err(MantleError::Io)?;
                // Remove from created_links — it will be re-added below.
                created_links.retain(|p| p != &dest_path);
            }

            std::os::unix::fs::symlink(&source_path, &dest_path).map_err(MantleError::Io)?;
            created_links.push(dest_path);
        }
        // Ignore other types (device files, sockets) — not relevant for mods.
    }

    Ok(())
}

/// Returns `true` if `dir` contains at least one regular file anywhere in its
/// tree. Used by `verify` to check whether we should expect symlinks.
fn has_any_file(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            return true;
        }
        if path.is_dir() && has_any_file(&path) {
            return true;
        }
    }
    false
}

/// Walk `dir` bottom-up, removing directories that are now empty.
///
/// Best-effort — silently ignores errors (e.g. permission denied on
/// directories not created by us).
fn prune_empty_dirs(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            prune_empty_dirs(&path);
            // Attempt removal; ignore failure (non-empty dirs return ENOTEMPTY).
            let _ = std::fs::remove_dir(&path);
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Create a temporary mod directory with the given files.
    /// `files` is a slice of `(relative_path, content)`.
    fn make_mod_dir(files: &[(&str, &[u8])]) -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        for (rel, content) in files {
            let path = dir.path().join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, content).unwrap();
        }
        dir
    }

    // ── Mount / unmount ───────────────────────────────────────────────────────

    #[test]
    fn mount_creates_symlinks_for_all_files() {
        let mod1 = make_mod_dir(&[("Data/test.esp", b"TES4")]);
        let merge = TempDir::new().expect("tempdir");

        let params = MountParams {
            lower_dirs: vec![mod1.path().to_path_buf()],
            merge_dir: merge.path().to_path_buf(),
        };
        let farm = SymlinkFarm::mount(&params).expect("mount");

        assert_eq!(farm.link_count(), 1);
        let link = merge.path().join("Data/test.esp");
        assert!(link.exists(), "symlink must exist at merge path");
        // Confirm it is a symlink, not a copy.
        assert!(
            link.symlink_metadata().unwrap().file_type().is_symlink(),
            "Data/test.esp must be a symlink"
        );
    }

    #[test]
    fn mount_creates_parent_directories_for_nested_files() {
        let mod1 = make_mod_dir(&[("Data/meshes/armor/iron.nif", b"MESH")]);
        let merge = TempDir::new().expect("tempdir");

        let params = MountParams {
            lower_dirs: vec![mod1.path().to_path_buf()],
            merge_dir: merge.path().to_path_buf(),
        };
        let farm = SymlinkFarm::mount(&params).expect("mount");

        assert_eq!(farm.link_count(), 1);
        assert!(merge.path().join("Data/meshes/armor/iron.nif").exists());
    }

    #[test]
    fn mount_high_priority_wins_conflict() {
        // Two mods, same file path. lower_dirs[0] (high_prio) should win.
        let high_prio = make_mod_dir(&[("Data/conflict.esp", b"HIGH")]);
        let low_prio = make_mod_dir(&[("Data/conflict.esp", b"LOW")]);

        let merge = TempDir::new().expect("tempdir");
        let params = MountParams {
            lower_dirs: vec![
                high_prio.path().to_path_buf(), // index 0 = highest priority
                low_prio.path().to_path_buf(),
            ],
            merge_dir: merge.path().to_path_buf(),
        };
        let farm = SymlinkFarm::mount(&params).expect("mount");

        // Only one symlink for the conflicted file.
        assert_eq!(farm.link_count(), 1);

        // The symlink must point into high_prio, not low_prio.
        let link_target = fs::read_link(merge.path().join("Data/conflict.esp")).expect("read_link");
        assert!(
            link_target.starts_with(high_prio.path()),
            "conflict winner must be high-priority mod, got {link_target:?}"
        );
    }

    #[test]
    fn mount_merges_non_conflicting_files_from_multiple_mods() {
        let mod1 = make_mod_dir(&[("Data/mod1.esp", b"MOD1")]);
        let mod2 = make_mod_dir(&[("Data/mod2.esp", b"MOD2")]);
        let merge = TempDir::new().expect("tempdir");

        let params = MountParams {
            lower_dirs: vec![mod1.path().to_path_buf(), mod2.path().to_path_buf()],
            merge_dir: merge.path().to_path_buf(),
        };
        let farm = SymlinkFarm::mount(&params).expect("mount");

        assert_eq!(farm.link_count(), 2);
        assert!(merge.path().join("Data/mod1.esp").exists());
        assert!(merge.path().join("Data/mod2.esp").exists());
    }

    #[test]
    fn mount_empty_lower_dirs_produces_zero_links() {
        let empty_mod = make_mod_dir(&[]);
        let merge = TempDir::new().expect("tempdir");

        let params = MountParams {
            lower_dirs: vec![empty_mod.path().to_path_buf()],
            merge_dir: merge.path().to_path_buf(),
        };
        let farm = SymlinkFarm::mount(&params).expect("mount");
        assert_eq!(farm.link_count(), 0);
    }

    #[test]
    fn mount_no_lower_dirs_produces_zero_links() {
        let merge = TempDir::new().expect("tempdir");
        let params = MountParams {
            lower_dirs: vec![],
            merge_dir: merge.path().to_path_buf(),
        };
        let farm = SymlinkFarm::mount(&params).expect("mount");
        assert_eq!(farm.link_count(), 0);
    }

    #[test]
    fn mount_fails_when_lower_dir_does_not_exist() {
        let merge = TempDir::new().expect("tempdir");
        let params = MountParams {
            lower_dirs: vec![PathBuf::from("/nonexistent/path/that/cannot/exist")],
            merge_dir: merge.path().to_path_buf(),
        };
        assert!(
            SymlinkFarm::mount(&params).is_err(),
            "mount with nonexistent lower dir must return Err"
        );
    }

    // ── Verify ────────────────────────────────────────────────────────────────

    #[test]
    fn verify_passes_after_valid_mount() {
        let mod1 = make_mod_dir(&[("Data/test.esp", b"TES4")]);
        let merge = TempDir::new().expect("tempdir");
        let params = MountParams {
            lower_dirs: vec![mod1.path().to_path_buf()],
            merge_dir: merge.path().to_path_buf(),
        };
        let farm = SymlinkFarm::mount(&params).expect("mount");
        farm.verify(&params).expect("verify must pass after clean mount");
    }

    #[test]
    fn verify_passes_when_all_sources_are_empty() {
        let empty_mod = make_mod_dir(&[]);
        let merge = TempDir::new().expect("tempdir");
        let params = MountParams {
            lower_dirs: vec![empty_mod.path().to_path_buf()],
            merge_dir: merge.path().to_path_buf(),
        };
        let farm = SymlinkFarm::mount(&params).expect("mount");
        // No files in source → no links expected → verify must still pass.
        farm.verify(&params).expect("verify with empty sources must pass");
    }

    #[test]
    fn verify_fails_when_merge_dir_is_removed_after_mount() {
        let mod1 = make_mod_dir(&[("Data/test.esp", b"TES4")]);
        let merge = TempDir::new().expect("tempdir");
        let params = MountParams {
            lower_dirs: vec![mod1.path().to_path_buf()],
            merge_dir: merge.path().to_path_buf(),
        };
        let farm = SymlinkFarm::mount(&params).expect("mount");

        // Simulate merge dir being removed externally.
        fs::remove_dir_all(farm.merge_dir()).unwrap();

        assert!(farm.verify(&params).is_err(), "verify must fail when merge dir is gone");
    }

    // ── Unmount ───────────────────────────────────────────────────────────────

    #[test]
    fn unmount_removes_all_created_symlinks() {
        let mod1 = make_mod_dir(&[("Data/a.esp", b"A"), ("Data/b.esp", b"B")]);
        let merge = TempDir::new().expect("tempdir");
        let params = MountParams {
            lower_dirs: vec![mod1.path().to_path_buf()],
            merge_dir: merge.path().to_path_buf(),
        };
        let farm = SymlinkFarm::mount(&params).expect("mount");
        assert_eq!(farm.link_count(), 2);

        farm.unmount().expect("unmount");

        assert!(
            !merge.path().join("Data/a.esp").exists(),
            "a.esp symlink must be removed after unmount"
        );
        assert!(
            !merge.path().join("Data/b.esp").exists(),
            "b.esp symlink must be removed after unmount"
        );
    }

    #[test]
    fn unmount_is_safe_when_symlinks_already_removed() {
        // Simulates crash-recovery: symlinks gone before unmount() is called.
        let mod1 = make_mod_dir(&[("Data/test.esp", b"TES4")]);
        let merge = TempDir::new().expect("tempdir");
        let params = MountParams {
            lower_dirs: vec![mod1.path().to_path_buf()],
            merge_dir: merge.path().to_path_buf(),
        };
        let farm = SymlinkFarm::mount(&params).expect("mount");

        // Manually delete the symlink before unmount().
        fs::remove_file(merge.path().join("Data/test.esp")).unwrap();

        // unmount() must succeed even though the link is already gone.
        farm.unmount().expect("unmount must be tolerant of already-removed links");
    }

    #[test]
    fn unmount_prunes_empty_directories() {
        let mod1 = make_mod_dir(&[("Data/meshes/iron.nif", b"MESH")]);
        let merge = TempDir::new().expect("tempdir");
        let params = MountParams {
            lower_dirs: vec![mod1.path().to_path_buf()],
            merge_dir: merge.path().to_path_buf(),
        };
        let farm = SymlinkFarm::mount(&params).expect("mount");
        farm.unmount().expect("unmount");

        // The Data/meshes/ directory should have been pruned.
        assert!(
            !merge.path().join("Data/meshes").exists(),
            "empty Data/meshes dir must be pruned after unmount"
        );
    }
}
