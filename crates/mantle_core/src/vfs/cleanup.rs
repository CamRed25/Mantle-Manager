//! VFS stale mount detection and cleanup.
//!
//! On application restart after a crash, the overlay that was mounted for the
//! previous game session may still be active. [`teardown_stale`] is called at
//! startup to detect and remove any leftover overlays at a given merge path.
//!
//! Per `VFS_DESIGN.md` §4.4 — crash / stale mount recovery.

use std::path::Path;

use crate::error::MantleError;

// ─── Mountinfo queries ────────────────────────────────────────────────────────

/// Returns `true` if `path` appears as an active mount point in
/// `/proc/self/mountinfo`.
///
/// The path is canonicalised before the comparison so that relative paths and
/// symlinks are handled consistently.
///
/// # Errors
/// Returns [`MantleError::Io`] if `/proc/self/mountinfo` cannot be read.
pub fn is_mounted(path: &Path) -> Result<bool, MantleError> {
    let info =
        std::fs::read_to_string("/proc/self/mountinfo").map_err(MantleError::Io)?;
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_owned());
    let target = canonical.to_string_lossy();

    // mountinfo line format:
    //   ID parentID major:minor root mountpoint mountopts [optionals] '-' fstype source superopts
    // The mount point is field index 4 (0-based).
    Ok(info.lines().any(|line| {
        line.split_whitespace()
            .nth(4)
            .is_some_and(|mp| mp == target.as_ref())
    }))
}

/// Return the filesystem type of a mount at `path`, if present.
///
/// Parses the ` - ` section of each `/proc/self/mountinfo` line and returns the
/// first whitespace-delimited token (the `fstype` field).
///
/// Returns `None` if `path` is not currently a mount point or the type field
/// cannot be parsed.
///
/// # Errors
/// Returns [`MantleError::Io`] if `/proc/self/mountinfo` cannot be read.
pub fn mount_fstype(path: &Path) -> Result<Option<String>, MantleError> {
    let info =
        std::fs::read_to_string("/proc/self/mountinfo").map_err(MantleError::Io)?;
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_owned());
    let target = canonical.to_string_lossy();

    for line in info.lines() {
        // Split on " - " which separates mount options from the fs-type block.
        let mut parts = line.splitn(2, " - ");
        let before_dash = parts.next().unwrap_or("");
        let after_dash = parts.next().unwrap_or("");

        // Mount point is field index 4 (0-based) before the separator.
        let Some(mp) = before_dash.split_whitespace().nth(4) else {
            continue;
        };
        if mp != target.as_ref() {
            continue;
        }
        // after_dash: "fstype source superopts"
        if let Some(fstype) = after_dash.split_whitespace().next() {
            return Ok(Some(fstype.to_owned()));
        }
    }
    Ok(None)
}

// ─── Stale mount teardown ─────────────────────────────────────────────────────

/// Detect and remove a stale VFS overlay at `merge_dir`.
///
/// Intended to be called once at application startup, before any new mounts are
/// created. Reads `/proc/self/mountinfo` to check whether an overlay is present
/// at `merge_dir` and, if found, unmounts it using the appropriate tool:
///
/// - `overlay` → `umount2(MNT_DETACH)` via [`nix`]
/// - `fuse.fuse-overlayfs` / `fuse` → `fusermount3 -u` (or `fusermount -u`)
///
/// Returns `true` if a stale mount was found and removed, `false` if `merge_dir`
/// was not mounted.
///
/// # Errors
/// Returns [`MantleError::Vfs`] if the detected mount cannot be removed, or if
/// the filesystem type is unrecognised.
///
/// Returns [`MantleError::Io`] if `/proc/self/mountinfo` cannot be read.
pub fn teardown_stale(merge_dir: &Path) -> Result<bool, MantleError> {
    if !is_mounted(merge_dir)? {
        tracing::debug!(
            "teardown_stale: {} is not mounted — nothing to clean up",
            merge_dir.display()
        );
        return Ok(false);
    }

    let fstype = mount_fstype(merge_dir)?;
    tracing::warn!(
        "teardown_stale: stale mount at {} (type={:?}) — removing before new session",
        merge_dir.display(),
        fstype,
    );

    match fstype.as_deref() {
        Some("overlay") => {
            nix::mount::umount2(merge_dir, nix::mount::MntFlags::MNT_DETACH)
                .map_err(|e| {
                    MantleError::Vfs(format!(
                        "umount2({}): {e}",
                        merge_dir.display()
                    ))
                })?;
        }
        // fuse-overlayfs reports "fuse.fuse-overlayfs" in mountinfo.
        Some("fuse.fuse-overlayfs" | "fuse") | None => {
            let status = std::process::Command::new("fusermount3")
                .arg("-u")
                .arg(merge_dir)
                .status()
                .or_else(|_| {
                    std::process::Command::new("fusermount")
                        .arg("-u")
                        .arg(merge_dir)
                        .status()
                })
                .map_err(|e| MantleError::Vfs(format!("fusermount: {e}")))?;

            if !status.success() {
                return Err(MantleError::Vfs(format!(
                    "fusermount3 -u {} exited with code {:?}",
                    merge_dir.display(),
                    status.code(),
                )));
            }
        }
        Some(other) => {
            return Err(MantleError::Vfs(format!(
                "teardown_stale: unexpected fstype '{other}' at {} — \
                 refusing to unmount an unrecognised filesystem",
                merge_dir.display(),
            )));
        }
    }

    tracing::info!(
        "teardown_stale: removed stale {} mount at {}",
        fstype.as_deref().unwrap_or("unknown"),
        merge_dir.display(),
    );
    Ok(true)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A path that is definitely not a mount point (a file just created).
    #[test]
    fn is_mounted_returns_false_for_plain_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = is_mounted(dir.path());
        assert!(result.is_ok(), "is_mounted must not error on a real dir");
        assert!(
            !result.unwrap(),
            "a freshly created temp dir must not be a mount point"
        );
    }

    /// `/proc` is always mounted — verify detection works for a real mount.
    #[test]
    fn is_mounted_detects_proc() {
        let result = is_mounted(std::path::Path::new("/proc"));
        assert!(result.is_ok());
        assert!(result.unwrap(), "/proc must be detected as mounted");
    }

    /// `/proc` should report fstype "proc".
    #[test]
    fn mount_fstype_proc_is_proc() {
        let fstype = mount_fstype(std::path::Path::new("/proc")).unwrap();
        assert_eq!(fstype.as_deref(), Some("proc"));
    }

    /// A plain temp dir is not mounted — `teardown_stale` must return `false`.
    #[test]
    fn teardown_stale_returns_false_when_not_mounted() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = teardown_stale(dir.path());
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }
}
