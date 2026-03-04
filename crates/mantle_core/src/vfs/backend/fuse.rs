//! Tier 2 VFS backend — fuse-overlayfs.
//!
//! Wraps the `fuse-overlayfs` userspace binary to create a rootless `OverlayFS`
//! mount. This is the primary backend inside Flatpak (where kernel mount
//! operations are sandboxed) and a preferred fallback on kernels >= 5.11.
//!
//! # Prerequisites (`VFS_DESIGN.md` §2.2)
//! - `fuse-overlayfs` binary present at a well-known path or on `$PATH`.
//! - `/dev/fuse` readable by the calling process.
//! - Kernel FUSE support enabled (module `fuse` loaded or built-in).
//!
//! # Lifecycle
//! [`FuseOverlay::mount`] spawns `fuse-overlayfs -f -o lowerdir=…,upperdir=…,
//! workdir=… <merge_dir>` as a foreground child process. The process keeps the
//! FUSE filesystem alive. [`FuseOverlay::unmount`] calls `fusermount3 -u` (or
//! `fusermount -u`) to release the mount, then waits for the child to exit.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use crate::{error::MantleError, vfs::types::MountParams};

// ─── Binary discovery ─────────────────────────────────────────────────────────

/// Locate the `fuse-overlayfs` binary on this system.
///
/// Checks static well-known paths first, then `$HOME/.local/bin`, then falls
/// back to invoking `which fuse-overlayfs`.
fn find_fuse_overlayfs() -> Option<PathBuf> {
    const WELL_KNOWN: &[&str] = &[
        "/usr/bin/fuse-overlayfs",
        "/usr/local/bin/fuse-overlayfs",
    ];
    for &path in WELL_KNOWN {
        let p = Path::new(path);
        if p.exists() {
            return Some(p.to_owned());
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home).join(".local/bin/fuse-overlayfs");
        if p.exists() {
            return Some(p);
        }
    }
    // Last resort: ask the shell.
    if let Ok(output) = Command::new("which").arg("fuse-overlayfs").output() {
        if output.status.success() {
            let s = String::from_utf8_lossy(&output.stdout);
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(PathBuf::from(trimmed));
            }
        }
    }
    None
}

// ─── Mount-point detection ─────────────────────────────────────────────────────

/// Returns `true` if `path` is a mount point by comparing its device number
/// to that of its parent directory. A healthy overlay mount always has a
/// different device number than the directory beneath it.
///
/// Returns `Ok(false)` (rather than an error) if either path cannot be stat'd.
fn is_mount_point(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let parent = path.parent().unwrap_or(path);
    let Ok(parent_meta) = std::fs::metadata(parent) else {
        return false;
    };
    meta.dev() != parent_meta.dev()
}

// ─── FuseOverlay ─────────────────────────────────────────────────────────────

/// An active fuse-overlayfs mount.
///
/// Created by [`FuseOverlay::mount`]. Holds the child process and the
/// temporary upper/work directories required by overlayfs semantics.
///
/// # Teardown
/// Explicitly call [`FuseOverlay::unmount`] to tear down the mount. There is
/// no `Drop`-based cleanup because teardown can fail and `Drop` cannot
/// propagate errors. A crash leaves a stale mount that `vfs::teardown_stale`
/// handles on the next launch.
#[derive(Debug)]
pub struct FuseOverlay {
    /// Running `fuse-overlayfs` child process; kept alive while mounted.
    child: std::process::Child,
    /// Upper directory — game writes land here (copy-on-write), leaving mod
    /// source files unmodified. Held for RAII deletion on drop.
    #[allow(dead_code)]
    upper_dir: tempfile::TempDir,
    /// Work directory required by overlayfs for atomic rename bookkeeping.
    /// Held for RAII deletion on drop.
    #[allow(dead_code)]
    work_dir: tempfile::TempDir,
    /// Merged view directory presented to the game process.
    merge_dir: PathBuf,
}

impl FuseOverlay {
    /// Mount a fuse-overlayfs overlay.
    ///
    /// Creates temporary upper and work directories, then spawns
    /// `fuse-overlayfs -f -o lowerdir=…,upperdir=…,workdir=… <merge_dir>`.
    /// Sleeps for 150 ms after spawning to give the process time to complete
    /// the mount before returning.
    ///
    /// # Errors
    /// Returns [`MantleError::Vfs`] if the `fuse-overlayfs` binary cannot be
    /// found or spawning the child process fails.
    ///
    /// Returns [`MantleError::Io`] if the temporary directories or the merge
    /// directory cannot be created.
    pub fn mount(params: &MountParams) -> Result<Self, MantleError> {
        let binary = find_fuse_overlayfs().ok_or_else(|| {
            MantleError::Vfs(
                "fuse-overlayfs binary not found — install it or put it on PATH"
                    .to_owned(),
            )
        })?;

        let upper_dir = tempfile::Builder::new()
            .prefix("mantle-upper-")
            .tempdir()
            .map_err(MantleError::Io)?;
        let work_dir = tempfile::Builder::new()
            .prefix("mantle-work-")
            .tempdir()
            .map_err(MantleError::Io)?;

        std::fs::create_dir_all(&params.merge_dir).map_err(MantleError::Io)?;

        // lowerdir= string: colon-separated, index 0 = highest priority.
        let lowerdir = params
            .lower_dirs
            .iter()
            .map(|d| d.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(":");

        let mount_options = format!(
            "lowerdir={lowerdir},upperdir={upper},workdir={work}",
            upper = upper_dir.path().display(),
            work  = work_dir.path().display(),
        );

        tracing::debug!(
            "FuseOverlay: spawning {} -f -o '{}' {}",
            binary.display(),
            mount_options,
            params.merge_dir.display(),
        );

        let child = Command::new(&binary)
            .arg("-f")
            .arg("-o")
            .arg(&mount_options)
            .arg(&params.merge_dir)
            .spawn()
            .map_err(|e| {
                MantleError::Vfs(format!("failed to spawn {}: {e}", binary.display()))
            })?;

        std::thread::sleep(std::time::Duration::from_millis(150));

        Ok(Self {
            child,
            upper_dir,
            work_dir,
            merge_dir: params.merge_dir.clone(),
        })
    }

    /// Verify that the fuse-overlayfs mount is healthy.
    ///
    /// Checks that `merge_dir` is a mount point (its `st_dev` differs from
    /// its parent). A running `fuse-overlayfs` process always satisfies this.
    ///
    /// # Errors
    /// Returns [`MantleError::Vfs`] if `merge_dir` is not a mount point,
    /// indicating the child process may have exited immediately.
    pub fn verify(&self, _params: &MountParams) -> Result<(), MantleError> {
        if !is_mount_point(&self.merge_dir) {
            return Err(MantleError::Vfs(format!(
                "fuse-overlayfs: {} is not a mount point after spawn — \
                 the process may have exited immediately",
                self.merge_dir.display(),
            )));
        }
        Ok(())
    }

    /// Tear down the fuse-overlayfs mount.
    ///
    /// Unmounts via `fusermount3 -u` (falling back to `fusermount -u`), then
    /// waits for the child process to exit. Upper and work temp directories
    /// are deleted when the struct drops.
    ///
    /// # Errors
    /// Returns [`MantleError::Vfs`] if the fusermount command fails or returns
    /// a non-zero exit status.
    pub fn unmount(mut self) -> Result<(), MantleError> {
        let merge = self.merge_dir.clone();
        let status = Command::new("fusermount3")
            .arg("-u")
            .arg(&merge)
            .status()
            .or_else(|_| Command::new("fusermount").arg("-u").arg(&merge).status())
            .map_err(|e| MantleError::Vfs(format!("fusermount: {e}")))?;

        if !status.success() {
            return Err(MantleError::Vfs(format!(
                "fusermount3 -u {} exited with code {:?}",
                merge.display(),
                status.code(),
            )));
        }
        let _ = self.child.wait();
        // upper_dir and work_dir drop here → deleted from disk.
        Ok(())
    }
}

