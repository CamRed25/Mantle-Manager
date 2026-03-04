//! Integration tests for the symlink-farm VFS backend (Tier 3).
//!
//! These tests exercise the full public API:
//!   `mantle_core::vfs::{SymlinkFarm, MountParams}`
//!
//! No special kernel features or FUSE are required — symlink farms work on any
//! POSIX filesystem. All tests use `TempDir` so they run cleanly in CI.

use mantle_core::vfs::{MountParams, SymlinkFarm};
use std::{fs, path::PathBuf};
use tempfile::TempDir;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Create a temporary directory populated with the given `(relative, content)` pairs.
fn make_mod(files: &[(&str, &[u8])]) -> TempDir {
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

// ── full lifecycle tests ──────────────────────────────────────────────────────

/// Happy path: mount → verify → unmount on a single-mod overlay.
#[test]
fn full_lifecycle_single_mod() {
    let mod1 = make_mod(&[
        ("Data/Skyrim.esm", b"TES5"),
        ("Data/textures/sky.dds", b"DDS"),
    ]);
    let merge = TempDir::new().expect("tempdir");

    let params = MountParams {
        lower_dirs: vec![mod1.path().to_path_buf()],
        merge_dir: merge.path().to_path_buf(),
    };

    let farm = SymlinkFarm::mount(&params).expect("mount");

    // Verify succeeds.
    farm.verify(&params).expect("verify");

    // Both files are visible via the merge directory.
    assert!(merge.path().join("Data/Skyrim.esm").is_symlink());
    assert!(merge.path().join("Data/textures/sky.dds").is_symlink());

    // Unmount removes all links.
    farm.unmount().expect("unmount");

    assert!(!merge.path().join("Data/Skyrim.esm").exists());
    assert!(!merge.path().join("Data/textures/sky.dds").exists());
}

/// Multi-mod overlay: non-conflicting files from two mods are both visible.
#[test]
fn full_lifecycle_two_mods_no_conflict() {
    let mod1 = make_mod(&[("Data/mod1.esp", b"MOD1")]);
    let mod2 = make_mod(&[("Data/mod2.esp", b"MOD2")]);
    let merge = TempDir::new().expect("tempdir");

    let params = MountParams {
        lower_dirs: vec![mod1.path().to_path_buf(), mod2.path().to_path_buf()],
        merge_dir: merge.path().to_path_buf(),
    };

    let farm = SymlinkFarm::mount(&params).expect("mount");
    farm.verify(&params).expect("verify");

    assert!(merge.path().join("Data/mod1.esp").exists());
    assert!(merge.path().join("Data/mod2.esp").exists());

    farm.unmount().expect("unmount");
}

/// Conflict resolution: `lower_dirs[0]` (highest priority) wins.
#[test]
fn full_lifecycle_conflict_winner_is_index_zero() {
    let high = make_mod(&[("Data/shared.esp", b"HIGH")]);
    let low = make_mod(&[("Data/shared.esp", b"LOW")]);
    let merge = TempDir::new().expect("tempdir");

    let params = MountParams {
        lower_dirs: vec![high.path().to_path_buf(), low.path().to_path_buf()],
        merge_dir: merge.path().to_path_buf(),
    };

    let farm = SymlinkFarm::mount(&params).expect("mount");

    let target = fs::read_link(merge.path().join("Data/shared.esp")).expect("read_link");
    assert!(
        target.starts_with(high.path()),
        "winning symlink target must be the high-priority mod; got {target:?}"
    );

    farm.unmount().expect("unmount");
}

/// Verify the symlink content matches source content.
#[test]
fn merged_file_content_matches_source() {
    let mod1 = make_mod(&[("Data/plugin.esp", b"exact_content_check")]);
    let merge = TempDir::new().expect("tempdir");
    let params = MountParams {
        lower_dirs: vec![mod1.path().to_path_buf()],
        merge_dir: merge.path().to_path_buf(),
    };

    let farm = SymlinkFarm::mount(&params).expect("mount");

    let content = fs::read(merge.path().join("Data/plugin.esp")).expect("read via symlink");
    assert_eq!(content, b"exact_content_check");

    farm.unmount().expect("unmount");
}

/// Nested directory structure is preserved after unmount (directories pruned).
#[test]
fn nested_dirs_are_pruned_after_unmount() {
    let mod1 = make_mod(&[("a/b/c/file.nif", b"NIF")]);
    let merge = TempDir::new().expect("tempdir");
    let params = MountParams {
        lower_dirs: vec![mod1.path().to_path_buf()],
        merge_dir: merge.path().to_path_buf(),
    };
    let farm = SymlinkFarm::mount(&params).expect("mount");
    farm.unmount().expect("unmount");

    // All intermediate directories are removed.
    assert!(!merge.path().join("a").exists(), "dir 'a' must be pruned");
}

/// Mount with nonexistent lower dir returns an error.
#[test]
fn mount_nonexistent_lower_dir_returns_error() {
    let merge = TempDir::new().expect("tempdir");
    let params = MountParams {
        lower_dirs: vec![PathBuf::from("/this/path/definitely/does/not/exist/12345")],
        merge_dir: merge.path().to_path_buf(),
    };
    assert!(
        SymlinkFarm::mount(&params).is_err(),
        "mount must fail when lower dir does not exist"
    );
}

/// Empty lower_dirs produces a mount with zero links; verify and unmount succeed.
#[test]
fn mount_empty_lower_dirs_succeeds() {
    let merge = TempDir::new().expect("tempdir");
    let params = MountParams {
        lower_dirs: vec![],
        merge_dir: merge.path().to_path_buf(),
    };
    let farm = SymlinkFarm::mount(&params).expect("mount with no lower dirs");
    assert_eq!(farm.link_count(), 0);
    farm.verify(&params).expect("verify with empty lower_dirs");
    farm.unmount().expect("unmount with empty lower_dirs");
}
