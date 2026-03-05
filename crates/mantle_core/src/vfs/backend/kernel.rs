//! Tier 1 VFS backend — kernel overlayfs via the new mount API.
//!
//! Uses `fsopen(2)` / `fsconfig(2)` / `fsmount(2)` / `move_mount(2)` — the
//! "new mount API" introduced in Linux 5.2 and fully available for rootless
//! overlay without privileges since kernel 6.6.
//!
//! # Prerequisites (`VFS_DESIGN.md` §2.1)
//! - Linux kernel >= 6.6 (rootless overlayfs, `userxattr`, stable new mount API).
//! - **Not** running inside a Flatpak sandbox.
//! - User namespaces enabled (`CONFIG_USER_NS=y`).
//!
//! # Syscall numbers
//! Raw `libc::syscall` is used because `nix` 0.28 does not yet wrap the new
//! mount API. Numbers below are for **x86\_64** only.

use std::{
    ffi::CString,
    path::{Path, PathBuf},
};

use crate::{error::MantleError, vfs::types::MountParams};

// ─── Syscall numbers (x86_64) ─────────────────────────────────────────────

const SYS_MOVE_MOUNT: libc::c_long = 429;
const SYS_FSOPEN: libc::c_long = 430;
const SYS_FSCONFIG: libc::c_long = 431;
const SYS_FSMOUNT: libc::c_long = 432;

// ─── fsopen / fsconfig / fsmount / move_mount constants ───────────────────

const FSOPEN_CLOEXEC: libc::c_int = 1;
const FSCONFIG_SET_STRING: libc::c_int = 1;
const FSCONFIG_CMD_CREATE: libc::c_int = 6;
const FSMOUNT_CLOEXEC: libc::c_int = 1;
const MOVE_MOUNT_F_EMPTY_PATH: libc::c_int = 0x0000_0004;

// ─── Raw syscall wrappers ────────────────────────────────────────────────

/// Open a filesystem context for `fs_name`. Returns the context fd.
///
/// # Errors
/// Returns [`MantleError::Vfs`] if the syscall fails.
fn sys_fsopen(fs_name: &str, flags: libc::c_int) -> Result<libc::c_int, MantleError> {
    let name = CString::new(fs_name)
        .map_err(|_| MantleError::Vfs("fsopen: invalid filesystem name".to_owned()))?;
    // SAFETY: valid arguments for fsopen(2); pointer lifetime covers the call.
    let fd = unsafe { libc::syscall(SYS_FSOPEN, name.as_ptr(), libc::c_long::from(flags)) };
    if fd < 0 {
        Err(MantleError::Vfs(format!(
            "fsopen({fs_name}): {}",
            std::io::Error::last_os_error()
        )))
    } else {
        libc::c_int::try_from(fd)
            .map_err(|_| MantleError::Vfs("fsopen: returned oversized fd".to_owned()))
    }
}

/// Set a string-valued parameter on a filesystem context (`FSCONFIG_SET_STRING`).
///
/// # Errors
/// Returns [`MantleError::Vfs`] if the syscall fails.
fn sys_fsconfig_set_string(fd: libc::c_int, key: &str, value: &str) -> Result<(), MantleError> {
    let key_c = CString::new(key)
        .map_err(|_| MantleError::Vfs(format!("fsconfig: invalid key '{key}'")))?;
    let val_c = CString::new(value)
        .map_err(|_| MantleError::Vfs(format!("fsconfig: invalid value for '{key}'")))?;
    // SAFETY: valid arguments for fsconfig(2) FSCONFIG_SET_STRING.
    let ret = unsafe {
        libc::syscall(
            SYS_FSCONFIG,
            libc::c_long::from(fd),
            libc::c_long::from(FSCONFIG_SET_STRING),
            key_c.as_ptr().cast::<libc::c_char>(),
            val_c.as_ptr().cast::<libc::c_void>(),
            0i64,
        )
    };
    if ret < 0 {
        Err(MantleError::Vfs(format!(
            "fsconfig(SET_STRING, '{key}'): {}",
            std::io::Error::last_os_error()
        )))
    } else {
        Ok(())
    }
}

/// Finalise a filesystem context (`FSCONFIG_CMD_CREATE`).
///
/// # Errors
/// Returns [`MantleError::Vfs`] if the syscall fails.
fn sys_fsconfig_create(fd: libc::c_int) -> Result<(), MantleError> {
    // SAFETY: valid arguments for fsconfig(2) FSCONFIG_CMD_CREATE.
    let ret = unsafe {
        libc::syscall(
            SYS_FSCONFIG,
            libc::c_long::from(fd),
            libc::c_long::from(FSCONFIG_CMD_CREATE),
            std::ptr::null::<libc::c_char>(),
            std::ptr::null::<libc::c_void>(),
            0i64,
        )
    };
    if ret < 0 {
        Err(MantleError::Vfs(format!(
            "fsconfig(CMD_CREATE): {}",
            std::io::Error::last_os_error()
        )))
    } else {
        Ok(())
    }
}

/// Create a detached mount object from a configured filesystem context.
///
/// # Errors
/// Returns [`MantleError::Vfs`] if the syscall fails.
fn sys_fsmount(
    fsfd: libc::c_int,
    flags: libc::c_int,
    attr_flags: libc::c_int,
) -> Result<libc::c_int, MantleError> {
    // SAFETY: valid arguments for fsmount(2).
    let ret = unsafe {
        libc::syscall(
            SYS_FSMOUNT,
            libc::c_long::from(fsfd),
            libc::c_long::from(flags),
            libc::c_long::from(attr_flags),
        )
    };
    if ret < 0 {
        Err(MantleError::Vfs(format!("fsmount: {}", std::io::Error::last_os_error())))
    } else {
        libc::c_int::try_from(ret)
            .map_err(|_| MantleError::Vfs("fsmount: returned oversized fd".to_owned()))
    }
}

/// Attach a detached mount fd to a target path.
///
/// # Errors
/// Returns [`MantleError::Vfs`] if the syscall fails.
fn sys_move_mount(
    from_dfd: libc::c_int,
    from_path: &str,
    to_dfd: libc::c_int,
    to_path: &str,
    flags: libc::c_int,
) -> Result<(), MantleError> {
    let from_c = CString::new(from_path)
        .map_err(|_| MantleError::Vfs("move_mount: invalid from_path".to_owned()))?;
    let to_c = CString::new(to_path)
        .map_err(|_| MantleError::Vfs("move_mount: invalid to_path".to_owned()))?;
    // SAFETY: valid arguments for move_mount(2).
    let ret = unsafe {
        libc::syscall(
            SYS_MOVE_MOUNT,
            libc::c_long::from(from_dfd),
            from_c.as_ptr().cast::<libc::c_char>(),
            libc::c_long::from(to_dfd),
            to_c.as_ptr().cast::<libc::c_char>(),
            libc::c_long::from(flags),
        )
    };
    if ret < 0 {
        Err(MantleError::Vfs(format!(
            "move_mount(→ {to_path}): {}",
            std::io::Error::last_os_error()
        )))
    } else {
        Ok(())
    }
}

// ─── Mount-point detection ──────────────────────────────────────────────

/// Returns `true` if `path` is a mount point (different device number from its
/// parent). Returns `Ok(false)` if either path cannot be stat'd.
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

// ─── KernelOverlay ──────────────────────────────────────────────────

/// An active kernel-overlayfs mount created via the new mount API.
///
/// Holds the temporary upper and work directories. Both are removed from disk
/// when `unmount()` consumes this struct and they drop.
///
/// # Teardown
/// Always call [`KernelOverlay::unmount`]. No `Drop`-based cleanup: `umount2`
/// can fail and `Drop` cannot propagate errors.
// clippy::struct_field_names: fields `upper_dir`, `work_dir`, and `merge_dir` intentionally
// carry the `_dir` suffix for clarity — removing it yields `upper`, `work`, and `merge`,
// which are ambiguous without the type context.
#[allow(clippy::struct_field_names)]
#[derive(Debug)]
pub struct KernelOverlay {
    /// Upper directory — game writes land here (copy-on-write).
    /// Held for RAII deletion on drop.
    #[allow(dead_code)]
    upper_dir: tempfile::TempDir,
    /// Overlayfs work directory (internal kernel bookkeeping for renames).
    /// Held for RAII deletion on drop.
    #[allow(dead_code)]
    work_dir: tempfile::TempDir,
    /// Merged view directory.
    merge_dir: PathBuf,
}

impl KernelOverlay {
    /// Mount a kernel overlayfs overlay using the new mount API.
    ///
    /// Sequence: `fsopen("overlay")` → `fsconfig` (lowerdir, upperdir, workdir,
    /// userxattr) → `fsconfig CMD_CREATE` → `fsmount` → `move_mount`.
    ///
    /// Both the filesystem context fd and the mount fd are closed before
    /// returning, regardless of success or failure.
    ///
    /// # Errors
    /// Returns [`MantleError::Vfs`] if any syscall fails (`ENOSYS`, `EPERM`, …).
    /// Returns [`MantleError::Io`] if temp dirs or the merge dir cannot be created.
    pub fn mount(params: &MountParams) -> Result<Self, MantleError> {
        let upper_dir = tempfile::Builder::new()
            .prefix("mantle-upper-")
            .tempdir()
            .map_err(MantleError::Io)?;
        let work_dir = tempfile::Builder::new()
            .prefix("mantle-work-")
            .tempdir()
            .map_err(MantleError::Io)?;

        std::fs::create_dir_all(&params.merge_dir).map_err(MantleError::Io)?;

        let lowerdir = params
            .lower_dirs
            .iter()
            .map(|d| d.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(":");

        let fsfd = sys_fsopen("overlay", FSOPEN_CLOEXEC)?;

        // Configure in a closure so fsfd is always closed on any exit path.
        let fsmount_result = (|| -> Result<libc::c_int, MantleError> {
            sys_fsconfig_set_string(fsfd, "lowerdir", &lowerdir)?;
            sys_fsconfig_set_string(fsfd, "upperdir", &upper_dir.path().to_string_lossy())?;
            sys_fsconfig_set_string(fsfd, "workdir", &work_dir.path().to_string_lossy())?;
            // userxattr: required for unprivileged overlayfs (kernel >= 5.11).
            let _ = sys_fsconfig_set_string(fsfd, "userxattr", "");
            sys_fsconfig_create(fsfd)?;
            sys_fsmount(fsfd, FSMOUNT_CLOEXEC, 0)
        })();

        // SAFETY: fsfd is a valid fd from fsopen.
        unsafe { libc::close(fsfd) };

        let mntfd = fsmount_result?;

        let merge_str = params.merge_dir.to_string_lossy();
        let move_result =
            sys_move_mount(mntfd, "", libc::AT_FDCWD, &merge_str, MOVE_MOUNT_F_EMPTY_PATH);

        // SAFETY: mntfd is a valid fd from fsmount.
        unsafe { libc::close(mntfd) };

        move_result?;

        tracing::debug!("KernelOverlay: mounted at {}", params.merge_dir.display());
        Ok(Self {
            upper_dir,
            work_dir,
            merge_dir: params.merge_dir.clone(),
        })
    }

    /// Verify the kernel overlay is healthy (merge dir is a mount point).
    ///
    /// # Errors
    /// Returns [`MantleError::Vfs`] if `merge_dir` is not a mount point.
    pub fn verify(&self, _params: &MountParams) -> Result<(), MantleError> {
        if !is_mount_point(&self.merge_dir) {
            return Err(MantleError::Vfs(format!(
                "kernel overlay: {} is not a mount point after mount",
                self.merge_dir.display()
            )));
        }
        Ok(())
    }

    /// Tear down the kernel overlayfs mount via `umount2(MNT_DETACH)`.
    ///
    /// Upper and work temp directories are removed when the struct drops.
    ///
    /// # Errors
    /// Returns [`MantleError::Vfs`] if `umount2` fails.
    pub fn unmount(self) -> Result<(), MantleError> {
        nix::mount::umount2(&self.merge_dir, nix::mount::MntFlags::MNT_DETACH)
            .map_err(|e| MantleError::Vfs(format!("umount2({}): {e}", self.merge_dir.display())))?;
        // upper_dir and work_dir drop here → temp directories removed from disk.
        Ok(())
    }
}
