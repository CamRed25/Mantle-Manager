//! VFS mount namespace isolation via `unshare(CLONE_NEWNS)`.
//!
//! Entering a new mount namespace before performing VFS operations provides:
//! - **Automatic cleanup on exit:** all mounts in the namespace are torn down
//!   when the process exits, eliminating stale-mount risk on crash.
//! - **Isolation from the host:** mounts and unmounts are invisible to other
//!   processes, preventing unintended side-effects.
//!
//! Per `VFS_DESIGN.md` §5.
//!
//! # Availability
//! Requires kernel >= 3.8 and `CONFIG_USER_NS=y` (user namespace support).
//! Both are present on all modern Linux distributions. Probed via
//! [`is_namespace_available`].
//!
//! # Usage
//! Call [`enter_mount_namespace`] once, before the first [`crate::vfs::mount`]
//! call. Check with [`is_namespace_available`] beforehand if you want to
//! degrade gracefully when namespaces are unavailable.

use once_cell::sync::OnceCell;

use crate::error::MantleError;

// ─── Availability probe ───────────────────────────────────────────────────────

static NAMESPACE_AVAILABLE: OnceCell<bool> = OnceCell::new();

/// Returns `true` if the running kernel supports `unshare(CLONE_NEWNS)` for
/// unprivileged users.
///
/// Uses the presence of `/proc/self/ns/mnt` as a lightweight proxy — this
/// pseudo-file exists on any kernel that supports per-process mount namespaces.
/// The result is cached after the first call.
#[must_use]
pub fn is_namespace_available() -> bool {
    *NAMESPACE_AVAILABLE.get_or_init(|| std::path::Path::new("/proc/self/ns/mnt").exists())
}

// ─── Namespace entry ──────────────────────────────────────────────────────────

/// Enter a new mount namespace for the current process.
///
/// Calls `unshare(CLONE_NEWNS)` to create a private copy of the current
/// process's mount namespace, then remounts root as `MS_PRIVATE | MS_REC`
/// so that no future mount or unmount operations propagate to the parent
/// namespace.
///
/// # When to call
/// Once, before the first [`crate::vfs::mount`] call. The namespace persists
/// for the lifetime of the process and is cleaned up automatically when the
/// process exits — even on crash.
///
/// # Errors
/// Returns [`MantleError::Vfs`] if `unshare(CLONE_NEWNS)` fails (e.g. `EPERM`
/// when kernel user namespaces are disabled or confined by seccomp), or if
/// remounting root private fails.
pub fn enter_mount_namespace() -> Result<(), MantleError> {
    // SAFETY: unshare(2) with CLONE_NEWNS only affects this process's mount
    // namespace membership. No memory is shared, aliased, or freed.
    let ret = unsafe { libc::unshare(libc::CLONE_NEWNS) };
    if ret != 0 {
        return Err(MantleError::Vfs(format!(
            "unshare(CLONE_NEWNS): {}",
            std::io::Error::last_os_error()
        )));
    }

    // Make all mounts inherited from the parent namespace private, preventing
    // propagation of future mount/unmount events to peer mount groups.
    nix::mount::mount(
        Some(std::path::Path::new("none")),
        std::path::Path::new("/"),
        None::<&std::path::Path>,
        nix::mount::MsFlags::MS_PRIVATE | nix::mount::MsFlags::MS_REC,
        None::<&std::path::Path>,
    )
    .map_err(|e| MantleError::Vfs(format!("mount --make-rprivate /: {e}")))?;

    tracing::info!("VFS: entered isolated mount namespace");
    Ok(())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// `/proc/self/ns/mnt` should exist on any modern Linux kernel.
    #[test]
    fn namespace_available_on_linux() {
        assert!(
            is_namespace_available(),
            "/proc/self/ns/mnt must exist on a kernel with mount namespace support"
        );
    }

    /// Calling `is_namespace_available` multiple times must return the same
    /// value (the result is cached via `OnceCell`).
    #[test]
    fn namespace_available_is_idempotent() {
        let a = is_namespace_available();
        let b = is_namespace_available();
        assert_eq!(a, b);
    }
}
