//! Integration tests for VFS mount/unmount lifecycle.
//!
//! Tests that require real kernel features (overlayfs, FUSE) skip gracefully
//! at runtime when the requirement is unavailable, per TESTING_GUIDE.md §5.
//!
//! These tests access only the public API of `mantle_core` — no `use super::*`.

// ─── Smoke tests (run on every machine) ──────────────────────────────────────

/// Verify that the VFS type system compiles and the backend selection path
/// used by mount is reachable. Does not perform any actual mount operation.
#[test]
fn backend_selection_reachable_without_mount() {
    let _kind = mantle_core::vfs::select_backend();
}

// ─── Symlink-farm lifecycle (always available) ────────────────────────────────

/// A symlink-farm mount + unmount round trip. Requires no kernel features and
/// must pass on every machine.
#[test]
fn symlink_farm_mount_unmount_round_trip() {
    let lower = tempfile::TempDir::new().unwrap();
    let merge = tempfile::TempDir::new().unwrap();
    std::fs::write(lower.path().join("plugin.esp"), b"TES4").unwrap();

    let params = mantle_core::vfs::MountParams {
        lower_dirs: vec![lower.path().to_owned()],
        merge_dir: merge.path().to_owned(),
    };
    let handle = mantle_core::vfs::mount_with(
        mantle_core::vfs::BackendKind::SymlinkFarm,
        params,
    )
    .expect("symlink-farm mount must succeed");

    assert_eq!(handle.backend_kind(), mantle_core::vfs::BackendKind::SymlinkFarm);
    assert!(
        merge.path().join("plugin.esp").exists(),
        "plugin.esp must be visible in the merge dir after mount"
    );

    handle.unmount().expect("symlink-farm unmount must succeed");

    assert!(
        !merge.path().join("plugin.esp").exists(),
        "plugin.esp must be gone from the merge dir after unmount"
    );
}

/// Verify that priority order is honoured: the highest-priority mod's file
/// wins a conflict and is visible in the merge view after mount.
#[test]
fn symlink_farm_conflict_winner_is_visible() {
    let high = tempfile::TempDir::new().unwrap();
    let low = tempfile::TempDir::new().unwrap();
    std::fs::write(high.path().join("shared.esp"), b"HIGH").unwrap();
    std::fs::write(low.path().join("shared.esp"), b"LOW").unwrap();

    let params = mantle_core::vfs::MountParams {
        lower_dirs: vec![high.path().to_owned(), low.path().to_owned()],
        merge_dir: tempfile::TempDir::new().unwrap().keep(),
    };
    let handle = mantle_core::vfs::mount_with(
        mantle_core::vfs::BackendKind::SymlinkFarm,
        params,
    )
    .expect("mount");

    let target =
        std::fs::read_link(handle.merge_dir().join("shared.esp")).expect("read_link");
    assert!(
        target.starts_with(high.path()),
        "conflict winner must be the high-priority mod, got {target:?}"
    );

    handle.unmount().expect("unmount");
}

/// Pre-flight validation: a missing lower directory must be rejected before
/// the backend is even invoked.
#[test]
fn mount_with_missing_lower_dir_returns_error() {
    let merge = tempfile::TempDir::new().unwrap();
    let params = mantle_core::vfs::MountParams {
        lower_dirs: vec![std::path::PathBuf::from("/nonexistent/mod/dir")],
        merge_dir: merge.path().to_owned(),
    };
    let result = mantle_core::vfs::mount_with(
        mantle_core::vfs::BackendKind::SymlinkFarm,
        params,
    );
    assert!(
        result.is_err(),
        "mount_with a missing lower dir must return Err"
    );
}

/// [`mount`] (auto-select) must succeed on every machine: if the auto-selected
/// backend is a stub, it should at minimum not panic.
///
/// This test is informational — it prints the selected backend and either
/// confirms a successful mount/unmount or prints the error from a stub backend.
#[test]
fn auto_select_mount_does_not_panic() {
    let lower = tempfile::TempDir::new().unwrap();
    let merge = tempfile::TempDir::new().unwrap();
    std::fs::write(lower.path().join("data.esp"), b"TES4").unwrap();

    let params = mantle_core::vfs::MountParams {
        lower_dirs: vec![lower.path().to_owned()],
        merge_dir: merge.path().to_owned(),
    };

    match mantle_core::vfs::mount(params) {
        Ok(handle) => {
            eprintln!("INFO: auto-select chose {:?} — mount succeeded", handle.backend_kind());
            handle.unmount().expect("unmount after auto-select must succeed");
        }
        Err(e) => {
            // Stub backends return "not yet implemented" — that is expected.
            eprintln!("INFO: auto-select mount returned Err (stub backend): {e}");
        }
    }
}

// ─── Kernel overlayfs lifecycle (kernel 6.6+, non-Flatpak) ──────────────────

/// A kernel overlayfs mount + unmount round trip.
///
/// # Skip condition
/// Skipped if running inside Flatpak, kernel < 6.6, or `has_new_mount_api()`
/// returns false.
///
/// # Skip condition
/// Skipped inside Flatpak, on kernel < 6.6, if `has_new_mount_api()` returns
/// false, or if `fsopen("overlay")` is blocked by missing `CAP_SYS_ADMIN`.
#[test]
fn kernel_overlayfs_mount_unmount_round_trip() {
    if std::env::var("FLATPAK_ID").is_ok() {
        eprintln!("SKIP: inside Flatpak");
        return;
    }
    let (major, minor, _) = mantle_core::vfs::kernel_version();
    if (major, minor) < (6, 6) {
        eprintln!("SKIP: kernel {major}.{minor} < 6.6 — kernel overlayfs unavailable");
        return;
    }
    if !mantle_core::vfs::has_new_mount_api() {
        eprintln!("SKIP: fsopen not available on this kernel");
        return;
    }

    let lower = tempfile::TempDir::new().unwrap();
    let merge = tempfile::TempDir::new().unwrap();
    std::fs::write(lower.path().join("plugin.esp"), b"TES4").unwrap();

    let params = mantle_core::vfs::MountParams {
        lower_dirs: vec![lower.path().to_owned()],
        merge_dir: merge.path().to_owned(),
    };
    let handle = match mantle_core::vfs::mount_with(
        mantle_core::vfs::BackendKind::KernelOverlayfs,
        params,
    ) {
        Ok(h) => h,
        Err(e) => {
            // EPERM / EACCES = API present but unprivileged overlayfs disabled.
            let msg = e.to_string();
            if msg.contains("Operation not permitted") || msg.contains("Permission denied") {
                eprintln!("SKIP: unprivileged kernel overlayfs not permitted: {e}");
                return;
            }
            panic!("kernel overlayfs mount must succeed: {e}");
        }
    };

    assert!(merge.path().join("plugin.esp").exists());
    handle.unmount().expect("kernel overlayfs unmount must succeed");
    assert!(!merge.path().join("plugin.esp").exists());
}

// ─── FUSE overlayfs lifecycle ─────────────────────────────────────────────────

/// A fuse-overlayfs mount + unmount round trip.
///
/// # Skip condition
/// Skipped if `fuse_overlayfs_available()` returns false.
#[test]
fn fuse_overlayfs_mount_unmount_round_trip() {
    if !mantle_core::vfs::fuse_overlayfs_available() {
        eprintln!("SKIP: fuse-overlayfs binary not available");
        return;
    }

    let lower = tempfile::TempDir::new().unwrap();
    let merge = tempfile::TempDir::new().unwrap();
    std::fs::write(lower.path().join("plugin.esp"), b"TES4").unwrap();

    let params = mantle_core::vfs::MountParams {
        lower_dirs: vec![lower.path().to_owned()],
        merge_dir: merge.path().to_owned(),
    };
    let handle = mantle_core::vfs::mount_with(
        mantle_core::vfs::BackendKind::FuseOverlayfs,
        params,
    )
    .expect("fuse-overlayfs mount must succeed");

    assert!(merge.path().join("plugin.esp").exists());
    handle.unmount().expect("fuse-overlayfs unmount must succeed");
    assert!(!merge.path().join("plugin.esp").exists());
}
