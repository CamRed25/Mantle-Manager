//! Integration tests for VFS backend selection.
//!
//! Tests that require real kernel features or system binaries skip gracefully
//! at runtime when the requirement is unavailable, per TESTING_GUIDE.md §5.
//!
//! These tests access only the public API of `mantle_core` — no `use super::*`.

// ─── Smoke tests (run on every machine) ──────────────────────────────────────

#[test]
fn select_backend_does_not_panic() {
    let kind = mantle_core::vfs::select_backend();
    // Any valid variant is acceptable — the specific choice depends on the host.
    let display = kind.to_string();
    assert!(
        ["kernel-overlayfs", "fuse-overlayfs", "symlink-farm"].contains(&display.as_str()),
        "unexpected backend display string: {display}"
    );
}

#[test]
fn detection_functions_do_not_panic() {
    // All probes must complete without panicking on any machine.
    let _ = mantle_core::vfs::is_flatpak();
    let _ = mantle_core::vfs::kernel_version();
    let _ = mantle_core::vfs::has_new_mount_api();
    let _ = mantle_core::vfs::fuse_overlayfs_available();
}

#[test]
fn flatpak_implies_fuse_backend() {
    // On machines actually running inside Flatpak, select_backend must
    // never return KernelOverlayfs.
    if mantle_core::vfs::is_flatpak() {
        let kind = mantle_core::vfs::select_backend();
        assert_eq!(
            kind,
            mantle_core::vfs::BackendKind::FuseOverlayfs,
            "inside Flatpak the backend must always be FuseOverlayfs"
        );
    }
}

#[test]
fn kernel_backend_implies_new_mount_api() {
    // If select_backend chose KernelOverlayfs then has_new_mount_api must
    // also be true (invariant between the probe and the decision).
    if mantle_core::vfs::select_backend() == mantle_core::vfs::BackendKind::KernelOverlayfs {
        assert!(
            mantle_core::vfs::has_new_mount_api(),
            "KernelOverlayfs was selected but has_new_mount_api() returned false — invariant violated"
        );
    }
}

// ─── Kernel overlayfs mount test (skipped unless kernel >= 6.6 + fsopen) ─────

#[test]
fn kernel_overlayfs_mount_succeeds_on_supported_kernel() {
    if !mantle_core::vfs::has_new_mount_api() {
        eprintln!("SKIP: new mount API not available on this kernel");
        return;
    }
    if mantle_core::vfs::is_flatpak() {
        eprintln!("SKIP: running inside Flatpak — kernel mount ops unavailable");
        return;
    }
    // TODO: implement mount lifecycle test once backend/kernel.rs is implemented
    eprintln!("SKIP: kernel mount lifecycle not yet implemented");
}

// ─── fuse-overlayfs mount test (skipped unless binary present) ───────────────

#[test]
fn fuse_overlayfs_mount_succeeds_when_binary_present() {
    if !mantle_core::vfs::fuse_overlayfs_available() {
        eprintln!("SKIP: fuse-overlayfs binary not found");
        return;
    }
    // TODO: implement mount lifecycle test once backend/fuse.rs is implemented
    eprintln!("SKIP: fuse-overlayfs mount lifecycle not yet implemented");
}
