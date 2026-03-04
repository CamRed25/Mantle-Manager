//! VFS environment detection — pure probes with no side effects.
//!
//! All functions are safe to call at any time. Expensive probes are cached
//! via `once_cell::sync::OnceCell` and execute at most once per process.
//!
//! # Public surface
//! - [`is_flatpak`]              — Flatpak sandbox detection
//! - [`kernel_version`]          — Running kernel version as `(major, minor, patch)`
//! - [`parse_kernel_version`]    — Parse a `uname -r`-style string (testable without root)
//! - [`has_new_mount_api`]       — Probe `fsopen` syscall availability (cached)
//! - [`fuse_overlayfs_available`] — Check for `fuse-overlayfs` binary on PATH (cached)

use once_cell::sync::OnceCell;

// ─── Flatpak detection ────────────────────────────────────────────────────────

/// Returns `true` if the current process is running inside a Flatpak sandbox.
///
/// Detection is based on the presence of `/.flatpak-info`, which the Flatpak
/// runtime always creates inside the sandbox. This file is absent on native
/// installations.
///
/// **Cached on first call.** Do not call in hot paths.
///
/// # Return value
/// `true`  — running inside Flatpak; kernel mount operations are unavailable.
/// `false` — native (host) execution; all three VFS tiers may be available.
pub fn is_flatpak() -> bool {
    static CACHE: OnceCell<bool> = OnceCell::new();
    *CACHE.get_or_init(|| std::path::Path::new("/.flatpak-info").exists())
}

// ─── Kernel version ───────────────────────────────────────────────────────────

/// Returns the running kernel version as `(major, minor, patch)`.
///
/// Uses `nix::sys::utsname::uname()` directly — never shells out to `uname -r`.
/// `uname()` is always successful on Linux, so the `.expect()` is infallible.
///
/// # Return value
/// `(major, minor, patch)` — e.g. `(6, 6, 0)` for kernel 6.6.0.
///
/// # Panics
/// Panics if `uname()` fails — this is infallible on Linux.
#[must_use]
pub fn kernel_version() -> (u32, u32, u32) {
    let uts = nix::sys::utsname::uname().expect("uname always succeeds on Linux");
    parse_kernel_version(uts.release().to_string_lossy().as_ref())
}

/// Parse a kernel release string into `(major, minor, patch)`.
///
/// Handles distribution-specific suffixes such as `6.1.52-valve16-1-neptune`
/// (`SteamOS`) and `6.6.0-rc1-arch1-1` (Arch Linux). Any non-numeric suffix on
/// the patch component is ignored.
///
/// Returns `(0, 0, 0)` for strings that cannot be parsed — callers must treat
/// zero as "unknown / too old" and fall back to a safer backend tier.
///
/// # Parameters
/// - `release`: The kernel release string from `uname().release()`.
///
/// # Examples
/// ```
/// use mantle_core::vfs::detect::parse_kernel_version;
/// assert_eq!(parse_kernel_version("6.6.0-arch1-1"), (6, 6, 0));
/// assert_eq!(parse_kernel_version("6.1.52-valve16-1-neptune"), (6, 1, 52));
/// ```
#[must_use]
pub fn parse_kernel_version(release: &str) -> (u32, u32, u32) {
    let mut parts = release.split('.');
    let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    // Patch may be followed by a '-' and a distro suffix; take only the numeric prefix.
    let patch = parts
        .next()
        .and_then(|s| s.split('-').next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (major, minor, patch)
}

// ─── New mount API probe ──────────────────────────────────────────────────────

/// Returns `true` if the kernel supports the new mount API (`fsopen`/`fsconfig`).
///
/// Probes by issuing a raw `fsopen("overlay", 0)` syscall. The result is
/// interpreted as follows:
/// - `ENOSYS` — syscall not implemented; new mount API unavailable
/// - Any other result (including success or `EPERM`) — API is present
///
/// On success the returned fd is closed immediately. This probe is harmless
/// and requires no special privileges to execute.
///
/// **Cached on first call.** Do not call in hot paths.
pub fn has_new_mount_api() -> bool {
    static CACHE: OnceCell<bool> = OnceCell::new();
    *CACHE.get_or_init(probe_fsopen)
}

/// Execute the `fsopen` probe. Called at most once.
///
/// # Safety
/// Raw `libc::syscall` usage. No memory is allocated; the overlay name is a
/// static C string. The fd, if opened, is closed before returning.
fn probe_fsopen() -> bool {
    // SYS_fsopen was introduced in Linux 5.2 on all architectures we target.
    let overlay = b"overlay\0";
    let fd = unsafe { libc::syscall(libc::SYS_fsopen, overlay.as_ptr(), 0i32) };
    if fd >= 0 {
        // Successfully opened — close the fd and report available.
        // SAFETY: fd is a small non-negative integer from a successful syscall;
        // it always fits in c_int (i32) on all supported Linux targets.
        #[allow(clippy::cast_possible_truncation)]
        unsafe { libc::close(fd as libc::c_int) };
        true
    } else {
        // ENOSYS → syscall not present; anything else → API present but lack perms.
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(libc::ENOSYS);
        errno != libc::ENOSYS
    }
}

// ─── fuse-overlayfs binary probe ─────────────────────────────────────────────

/// Returns `true` if a `fuse-overlayfs` binary is available on the system.
///
/// Searches the standard fixed paths and then every directory in `$PATH`.
/// Includes `/run/host/usr/bin` for the Flatpak host-forwarded path.
///
/// **Cached on first call.** Do not call in hot paths.
pub fn fuse_overlayfs_available() -> bool {
    static CACHE: OnceCell<bool> = OnceCell::new();
    *CACHE.get_or_init(probe_fuse_overlayfs)
}

/// Walk known paths and `$PATH` for the `fuse-overlayfs` binary.
fn probe_fuse_overlayfs() -> bool {
    const BINARY: &str = "fuse-overlayfs";

    // Check well-known fixed paths first (covers most distros + Flatpak host path).
    let fixed_paths = [
        "/usr/bin/fuse-overlayfs",
        "/usr/local/bin/fuse-overlayfs",
        "/run/host/usr/bin/fuse-overlayfs", // Flatpak with --allow=host-path
    ];
    for path in &fixed_paths {
        if std::path::Path::new(path).exists() {
            return true;
        }
    }

    // Fall back to walking $PATH.
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            if std::path::Path::new(dir).join(BINARY).exists() {
                return true;
            }
        }
    }

    false
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // parse_kernel_version — no I/O, runs everywhere

    #[test]
    fn parse_kernel_version_extracts_major_minor_patch() {
        assert_eq!(parse_kernel_version("6.6.0-arch1-1"), (6, 6, 0));
    }

    #[test]
    fn parse_kernel_version_handles_steamdeck_release_string() {
        // SteamOS ships "6.1.52-valve16-1-neptune"
        assert_eq!(parse_kernel_version("6.1.52-valve16-1-neptune"), (6, 1, 52));
    }

    #[test]
    fn parse_kernel_version_handles_rc_suffix() {
        assert_eq!(parse_kernel_version("6.6.0-rc1-gentoo"), (6, 6, 0));
    }

    #[test]
    fn parse_kernel_version_returns_zeros_on_invalid_input() {
        assert_eq!(parse_kernel_version("not-a-version"), (0, 0, 0));
    }

    #[test]
    fn parse_kernel_version_handles_missing_patch() {
        assert_eq!(parse_kernel_version("6.6"), (6, 6, 0));
    }

    #[test]
    fn parse_kernel_version_handles_plain_version() {
        assert_eq!(parse_kernel_version("5.15.0"), (5, 15, 0));
    }

    // kernel_version — reads uname, always works on Linux

    #[test]
    fn kernel_version_returns_nonzero_major() {
        let (major, _minor, _patch) = kernel_version();
        // Any Linux kernel in use today has major >= 4.
        assert!(major >= 4, "kernel major version should be >= 4, got {major}");
    }

    // is_flatpak — returns deterministically based on file presence

    #[test]
    fn is_flatpak_returns_bool_without_panicking() {
        // Can't assert the value here (depends on runtime), but it must not panic.
        let _ = is_flatpak();
    }

    // has_new_mount_api — result depends on kernel; just assert it doesn't panic

    #[test]
    fn has_new_mount_api_returns_bool_without_panicking() {
        let _ = has_new_mount_api();
    }

    // fuse_overlayfs_available — result depends on system; just assert no panic

    #[test]
    fn fuse_overlayfs_available_returns_bool_without_panicking() {
        let _ = fuse_overlayfs_available();
    }

    // Consistency check: on kernel < 5.2 there should be no new mount API.
    // We can't assert this in general but we can assert the known relationship.

    #[test]
    fn new_mount_api_consistent_with_kernel_version() {
        let (major, minor, _) = kernel_version();
        // If kernel < 5.2 the API must NOT be available.
        if (major, minor) < (5, 2) {
            assert!(
                !has_new_mount_api(),
                "Kernel {major}.{minor} predates fsopen — new mount API must be absent"
            );
        }
        // If kernel >= 5.2 it *may* be available (could be missing in containers/KVM).
        // We cannot assert it IS available — only that the probe doesn't panic.
    }
}
