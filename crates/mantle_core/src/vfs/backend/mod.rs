//! VFS backend selection — maps detected environment to a `BackendKind`.
//!
//! The selection follows the priority order defined in ARCHITECTURE.md §4.2:
//!
//! ```text
//! is_flatpak()
//!     → FuseOverlayfs  (kernel mount ops blocked by sandbox)
//!
//! NOT is_flatpak() AND kernel >= 6.6 AND has_new_mount_api()
//!     → KernelOverlayfs  (zero-overhead, native)
//!
//! NOT is_flatpak() AND (kernel >= 5.11 OR fuse_overlayfs_available())
//!     → FuseOverlayfs  (rootless, portable)
//!
//! fallback
//!     → SymlinkFarm  (always available)
//! ```
//!
//! # Note on `SteamOS`
//! `SteamOS` 3.x ships kernel 6.1 and runs all non-Steam applications inside
//! Flatpak. It always selects `FuseOverlayfs`. This is correct and expected.

pub mod fuse;
pub mod kernel;
pub mod symlink;

pub use symlink::SymlinkFarm;

use crate::vfs::detect;

/// The VFS backend tier chosen for the current runtime environment.
///
/// Variants are ordered from highest to lowest performance. The implementation
/// modules for each tier live in `vfs/backend/kernel.rs`, `vfs/backend/fuse.rs`,
/// and `vfs/backend/symlink.rs` respectively — they are stubs until the mount
/// lifecycle is implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// Tier 1: kernel overlayfs via `fsopen`/`fsconfig` new mount API.
    ///
    /// Available when: not inside Flatpak, kernel >= 6.6, `fsopen` not ENOSYS.
    /// Zero FUSE overhead; the overlay is a direct kernel VFS operation.
    KernelOverlayfs,

    /// Tier 2: `fuse-overlayfs` userspace overlay.
    ///
    /// Available when: inside Flatpak (always), or kernel >= 5.11, or the
    /// `fuse-overlayfs` binary is present on PATH.
    /// Requires `/dev/fuse` and the binary; works rootless.
    FuseOverlayfs,

    /// Tier 3: symlink farm.
    ///
    /// Always available. No kernel or FUSE dependency. Lower performance for
    /// large mod lists as changes require re-linking; no true overlay semantics.
    SymlinkFarm,
}

impl std::fmt::Display for BackendKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendKind::KernelOverlayfs => write!(f, "kernel-overlayfs"),
            BackendKind::FuseOverlayfs => write!(f, "fuse-overlayfs"),
            BackendKind::SymlinkFarm => write!(f, "symlink-farm"),
        }
    }
}

/// Select the best VFS backend for the current runtime environment.
///
/// Applies the priority logic from ARCHITECTURE.md §4.2. The result is not
/// cached here — callers that need a stable value across a session should
/// call this once at startup and store the result.
///
/// # Return value
/// The highest-tier [`BackendKind`] available on this system right now.
pub fn select_backend() -> BackendKind {
    // ── Tier 0: Flatpak sandbox ──────────────────────────────────────────────
    // The new mount API requires host kernel access which is blocked by the
    // Flatpak sandbox regardless of kernel version. Always use fuse-overlayfs.
    if detect::is_flatpak() {
        tracing::debug!("VFS: Flatpak detected → FuseOverlayfs");
        return BackendKind::FuseOverlayfs;
    }

    let (major, minor, _patch) = detect::kernel_version();
    tracing::debug!("VFS: kernel {major}.{minor}, not Flatpak");

    // ── Tier 1: kernel overlayfs ─────────────────────────────────────────────
    // Requires kernel >= 6.6 for full userxattr + unprivileged overlayfs
    // support, plus a runtime probe confirming fsopen isn't ENOSYS (it can
    // be absent in some container runtimes even on a 6.6+ host).
    if (major, minor) >= (6, 6) && detect::has_new_mount_api() {
        tracing::debug!("VFS: kernel >= 6.6 + fsopen OK → KernelOverlayfs");
        return BackendKind::KernelOverlayfs;
    }

    // ── Tier 2: fuse-overlayfs ───────────────────────────────────────────────
    // Available if kernel >= 5.11 (has FUSE passthrough mount support needed
    // for rootless fuse-overlayfs) OR if the binary is present and we can
    // try it regardless (pre-5.11 kernels may still work in practice).
    if (major, minor) >= (5, 11) || detect::fuse_overlayfs_available() {
        tracing::debug!("VFS: fuse path (kernel {major}.{minor}) → FuseOverlayfs");
        return BackendKind::FuseOverlayfs;
    }

    // ── Tier 3: symlink farm ─────────────────────────────────────────────────
    tracing::warn!(
        "VFS: kernel {major}.{minor} too old and fuse-overlayfs not found → SymlinkFarm"
    );
    BackendKind::SymlinkFarm
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // select_backend — runs on any machine, asserts only what we can know

    #[test]
    fn select_backend_returns_a_valid_variant() {
        // Must return without panic. All three variants are valid.
        let kind = select_backend();
        assert!(
            matches!(
                kind,
                BackendKind::KernelOverlayfs
                    | BackendKind::FuseOverlayfs
                    | BackendKind::SymlinkFarm
            ),
            "select_backend returned unexpected kind: {kind:?}"
        );
    }

    #[test]
    fn select_backend_returns_fuse_inside_flatpak_regardless_of_kernel() {
        // We cannot force is_flatpak() to return true here without mocking,
        // but we can test the logic directly with a synthetic decision matrix.
        // The actual runtime call is covered by select_backend_returns_a_valid_variant.
        let in_flatpak = true;
        let (major, minor) = (6u32, 9u32); // far above 6.6
        let has_mount_api = true;
        let kind = synthetic_select(in_flatpak, major, minor, has_mount_api, true);
        assert_eq!(kind, BackendKind::FuseOverlayfs);
    }

    #[test]
    fn select_backend_kernel_overlayfs_on_66_native() {
        let kind = synthetic_select(false, 6, 6, true, false);
        assert_eq!(kind, BackendKind::KernelOverlayfs);
    }

    #[test]
    fn select_backend_fuse_when_no_mount_api_on_66() {
        let kind = synthetic_select(false, 6, 6, false, true);
        assert_eq!(kind, BackendKind::FuseOverlayfs);
    }

    #[test]
    fn select_backend_fuse_on_511_native_without_mount_api() {
        let kind = synthetic_select(false, 5, 11, false, false);
        assert_eq!(kind, BackendKind::FuseOverlayfs);
    }

    #[test]
    fn select_backend_fuse_when_binary_present_on_old_kernel() {
        let kind = synthetic_select(false, 5, 4, false, true);
        assert_eq!(kind, BackendKind::FuseOverlayfs);
    }

    #[test]
    fn select_backend_symlink_when_nothing_available() {
        let kind = synthetic_select(false, 4, 19, false, false);
        assert_eq!(kind, BackendKind::SymlinkFarm);
    }

    #[test]
    fn backend_kind_display_matches_expected_strings() {
        assert_eq!(BackendKind::KernelOverlayfs.to_string(), "kernel-overlayfs");
        assert_eq!(BackendKind::FuseOverlayfs.to_string(), "fuse-overlayfs");
        assert_eq!(BackendKind::SymlinkFarm.to_string(), "symlink-farm");
    }

    /// Pure decision-logic version of `select_backend` for unit testing
    /// without requiring actual system probes.
    ///
    /// Mirrors the branch logic in `select_backend` exactly — must be kept
    /// in sync if the selection logic changes.
    fn synthetic_select(
        flatpak: bool,
        major: u32,
        minor: u32,
        has_mount_api: bool,
        fuse_binary: bool,
    ) -> BackendKind {
        if flatpak {
            return BackendKind::FuseOverlayfs;
        }
        if (major, minor) >= (6, 6) && has_mount_api {
            return BackendKind::KernelOverlayfs;
        }
        if (major, minor) >= (5, 11) || fuse_binary {
            return BackendKind::FuseOverlayfs;
        }
        BackendKind::SymlinkFarm
    }
}
