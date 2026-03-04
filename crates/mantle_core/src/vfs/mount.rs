//! VFS mount lifecycle — unified public entry point and [`MountHandle`].
//!
//! [`mount`] auto-selects the best available backend via [`select_backend`],
//! validates the mount parameters, calls the backend's `mount` + `verify`
//! sequence, and returns a [`MountHandle`] that encapsulates the active overlay.
//!
//! Use [`mount_with`] when you need an explicit backend — primarily in tests
//! and in cases where backend selection has already been done once at session
//! start and you want to reuse that decision.
//!
//! # Lifecycle
//! ```ignore
//! // Select backend once at session start (or use mount() which does it internally).
//! let kind = mantle_core::vfs::select_backend();
//! tracing::info!(vfs.backend = %kind, "VFS backend ready");
//!
//! // Mount when the user clicks Launch.
//! let params = MountParams { lower_dirs: mod_dirs, merge_dir: game_data_dir };
//! let handle = mantle_core::vfs::mount_with(kind, params)?;
//!
//! // … game process runs …
//!
//! // Tear down when the game exits.
//! handle.unmount()?;
//! ```
//!
//! # Error model
//! If the backend's `mount` step fails, no partial state is left behind —
//! the backend constructors are responsible for their own cleanup on error.
//! If `verify` fails after a successful `mount`, the backend is torn down
//! before the error is returned.

use std::path::{Path, PathBuf};

use crate::{
    error::MantleError,
    vfs::{
        backend::{fuse::FuseOverlay, kernel::KernelOverlay, symlink::SymlinkFarm, BackendKind},
        select_backend, MountParams,
    },
};

// ─── Private backend holder ───────────────────────────────────────────────────

/// Type-erased holder for one of the three active backend instances.
///
/// Not exposed publicly — callers interact through [`MountHandle`].
enum ActiveMount {
    Kernel(KernelOverlay),
    Fuse(FuseOverlay),
    Symlink(SymlinkFarm),
}

impl std::fmt::Debug for ActiveMount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Kernel(_) => f.write_str("Kernel"),
            Self::Fuse(_) => f.write_str("Fuse"),
            Self::Symlink(_) => f.write_str("Symlink"),
        }
    }
}

// ─── Public handle ────────────────────────────────────────────────────────────

/// An active VFS overlay mount.
///
/// Created by [`mount`] or [`mount_with`]. Holds whatever state is needed to
/// tear down the overlay when the game exits, regardless of which backend is
/// running under the hood.
///
/// # Teardown
/// Always call [`MountHandle::unmount`] when the game process exits.
///
/// There is no `Drop`-based cleanup: teardown can fail (an I/O error on
/// `umount2`, for example), and `Drop` cannot propagate errors to the caller.
/// A future cleanup pass (`vfs::teardown_stale`) handles mounts whose process
/// crashed before `unmount` was called.
#[derive(Debug)]
pub struct MountHandle {
    inner: ActiveMount,
    kind: BackendKind,
    merge_dir: PathBuf,
}

impl MountHandle {
    /// Tear down the overlay and release all associated resources.
    ///
    /// For kernel and FUSE backends this unmounts the filesystem. For the
    /// symlink farm this removes all created symlinks and prunes empty
    /// directories.
    ///
    /// # Errors
    /// Returns [`MantleError::Vfs`] or [`MantleError::Io`] if teardown fails.
    pub fn unmount(self) -> Result<(), MantleError> {
        tracing::info!(
            "VFS unmounting: {} (backend: {})",
            self.merge_dir.display(),
            self.kind,
        );
        let result = match self.inner {
            ActiveMount::Kernel(b) => b.unmount(),
            ActiveMount::Fuse(b) => b.unmount(),
            ActiveMount::Symlink(b) => b.unmount(),
        };
        if result.is_ok() {
            tracing::info!("VFS unmounted: {}", self.merge_dir.display());
        }
        result
    }

    /// The backend tier serving this mount.
    #[must_use]
    pub fn backend_kind(&self) -> BackendKind {
        self.kind
    }

    /// Path to the merge directory presented to the game process.
    #[must_use]
    pub fn merge_dir(&self) -> &Path {
        &self.merge_dir
    }
}

// ─── Mount entry points ───────────────────────────────────────────────────────

/// Mount a virtual overlay using the automatically selected backend.
///
/// Calls [`select_backend`] to determine the best available tier, logs the
/// choice, and delegates to [`mount_with`]. Prefer this over `mount_with` in
/// production code — explicit backend selection should only be needed in tests.
///
/// # Errors
/// Propagates errors from [`mount_with`].
pub fn mount(params: MountParams) -> Result<MountHandle, MantleError> {
    let kind = select_backend();
    tracing::info!("VFS: selected backend {kind}");
    mount_with(kind, params)
}

/// Mount a virtual overlay using a specific backend tier.
///
/// Called by [`mount`] internally, and useful when the caller has already
/// determined the correct backend at session start (avoiding a repeated probe)
/// or when a specific backend must be forced in tests.
///
/// # Pre-mount validation
/// Verifies every path in `params.lower_dirs` is a readable directory before
/// invoking the backend, per `VFS_DESIGN.md` §4.1.
///
/// # Post-mount verification
/// If the backend's `mount` succeeds but `verify` fails, the backend is torn
/// down before the error is returned so no partial state is left behind.
///
/// # Errors
/// Returns [`MantleError::Vfs`] if:
/// - Any lower directory does not exist or is not a directory.
/// - The backend's `mount` or `verify` step returns an error.
// clippy::needless_pass_by_value: MountParams is intentionally consumed here —
// its Vec fields are moved into the backend, so taking by value avoids an
// otherwise unnecessary clone at every call site.
#[allow(clippy::needless_pass_by_value)]
pub fn mount_with(kind: BackendKind, params: MountParams) -> Result<MountHandle, MantleError> {
    // ── Pre-flight: verify all lower dirs are readable ────────────────────────
    for dir in &params.lower_dirs {
        if !dir.is_dir() {
            return Err(MantleError::Vfs(format!(
                "lower directory not found or is not a directory: {}",
                dir.display()
            )));
        }
    }

    let lower_count = params.lower_dirs.len();
    let merge_dir = params.merge_dir.clone();

    tracing::debug!(
        "VFS: mounting {} lower dir(s) → {} via {kind}",
        lower_count,
        merge_dir.display(),
    );

    // ── Dispatch to backend: mount then verify ────────────────────────────────
    let inner = match kind {
        BackendKind::KernelOverlayfs => {
            let b = KernelOverlay::mount(&params)?;
            if let Err(e) = b.verify(&params) {
                // verify failed — tear down the mount before returning
                let _ = b.unmount();
                return Err(e);
            }
            ActiveMount::Kernel(b)
        }
        BackendKind::FuseOverlayfs => {
            let b = FuseOverlay::mount(&params)?;
            if let Err(e) = b.verify(&params) {
                // verify failed — tear down the mount before returning the error
                let _ = b.unmount();
                return Err(e);
            }
            ActiveMount::Fuse(b)
        }
        BackendKind::SymlinkFarm => {
            let b = SymlinkFarm::mount(&params)?;
            if let Err(e) = b.verify(&params) {
                // verify failed — tear down the mount before returning the error
                let _ = b.unmount();
                return Err(e);
            }
            ActiveMount::Symlink(b)
        }
    };

    tracing::info!(
        "VFS: mounted {} lower dir(s) → {} ({})",
        lower_count,
        merge_dir.display(),
        kind,
    );

    Ok(MountHandle { inner, kind, merge_dir })
}
