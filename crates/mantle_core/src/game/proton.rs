//! Proton compatibility prefix detection.
//!
//! All Bethesda titles on Linux run via Proton (Valve's Wine-based
//! compatibility layer). The Proton prefix holds the Wine C: drive tree,
//! registry hives, and user data.
//!
//! # Prefix layout
//! ```text
//! <steamapps>/compatdata/<app_id>/
//! └── pfx/                         ← Wine prefix root (this is what we return)
//!     ├── drive_c/                 ← Wine C: drive
//!     │   ├── users/
//!     │   └── windows/
//!     ├── user.reg                 ← Wine user registry hive
//!     ├── system.reg               ← Wine system registry hive
//!     └── userdef.reg
//! ```
//!
//! # References
//! - `PLATFORM_COMPAT.md` §6 — Proton and Wine integration
//! - `PLATFORM_COMPAT.md` §6.1 — `proton_prefix()` canonical snippet

use std::path::{Path, PathBuf};

// ─── Public API ───────────────────────────────────────────────────────────────

/// Locate the Proton Wine prefix for a given Steam App ID.
///
/// Looks for the compat data directory relative to the main Steam root
/// (i.e. `~/.steam/steam/`). This is the directory that contains `pfx/`.
///
/// Returns `None` if:
/// - The compatdata directory does not exist (game has never been launched
///   via Proton, or the app runs natively).
/// - The `pfx/` subdirectory is absent (Proton is configured but the prefix
///   has not been initialised yet).
///
/// # Parameters
/// - `steam_root`: The root path of the Steam installation (`SteamDir::path()`).
/// - `app_id`: Steam App ID of the game (e.g. `489830` for Skyrim SE).
///
/// # Example
/// ```ignore
/// use mantle_core::game::proton::proton_prefix_in_dir;
/// let pfx = proton_prefix_in_dir(steam.path(), 489830);
/// ```
#[must_use]
pub fn proton_prefix_in_dir(steam_root: &Path, app_id: u32) -> Option<PathBuf> {
    // Structure: <steam_root>/steamapps/compatdata/<app_id>/pfx
    let pfx = steam_root
        .join("steamapps")
        .join("compatdata")
        .join(app_id.to_string())
        .join("pfx");

    if pfx.is_dir() {
        Some(pfx)
    } else {
        None
    }
}

/// Locate the Proton Wine prefix using the standard Steam install path from
/// `steamlocate::SteamDir::locate()`.
///
/// Convenience wrapper for the common production case.
///
/// Returns `None` if Steam is not installed or the prefix does not exist.
/// Logs at `DEBUG` level if the prefix is absent.
///
/// # Parameters
/// - `app_id`: Steam App ID of the game.
pub fn proton_prefix(app_id: u32) -> Option<PathBuf> {
    let steam = steamlocate::SteamDir::locate().ok()?;
    let pfx = proton_prefix_in_dir(steam.path(), app_id);

    if pfx.is_none() {
        tracing::debug!(
            "No Proton prefix found for app_id {app_id} (game may not have been launched yet)"
        );
    }

    pfx
}

/// Returns `true` if the given Wine prefix directory contains the expected
/// registry hive files, indicating a fully initialised Proton prefix.
///
/// Checks for `user.reg` and `system.reg` inside `pfx_path`.
/// Does **not** validate the registry contents.
///
/// # Parameters
/// - `pfx_path`: Absolute path to the `pfx/` directory.
#[must_use]
pub fn is_prefix_initialised(pfx_path: &Path) -> bool {
    pfx_path.join("user.reg").is_file() && pfx_path.join("system.reg").is_file()
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Build a fake Steam root suitable for testing, with a compat prefix for
    /// the given app_id.
    fn fake_steam_root_with_prefix(app_id: u32) -> TempDir {
        let root = TempDir::new().unwrap();
        let pfx = root
            .path()
            .join("steamapps")
            .join("compatdata")
            .join(app_id.to_string())
            .join("pfx");
        std::fs::create_dir_all(&pfx).unwrap();
        root
    }

    #[test]
    fn finds_prefix_when_present() {
        let root = fake_steam_root_with_prefix(489830);
        let pfx = proton_prefix_in_dir(root.path(), 489830)
            .expect("must find prefix that was just created");
        assert!(pfx.is_dir());
        assert!(pfx.to_str().unwrap().contains("489830"));
    }

    #[test]
    fn returns_none_when_prefix_absent() {
        let root = TempDir::new().unwrap();
        // No compatdata directory created.
        assert!(proton_prefix_in_dir(root.path(), 489830).is_none());
    }

    #[test]
    fn returns_none_for_different_app_id() {
        let root = fake_steam_root_with_prefix(489830);
        // Prefix exists for 489830 but not for 377160.
        assert!(proton_prefix_in_dir(root.path(), 377160).is_none());
    }

    #[test]
    fn prefix_path_ends_with_pfx() {
        let root = fake_steam_root_with_prefix(377160);
        let pfx = proton_prefix_in_dir(root.path(), 377160).unwrap();
        assert_eq!(pfx.file_name().unwrap(), "pfx");
    }

    #[test]
    fn is_prefix_initialised_true_when_registry_hives_present() {
        let dir = TempDir::new().unwrap();
        let pfx = dir.path();
        std::fs::write(pfx.join("user.reg"), b"WINE REGISTRY").unwrap();
        std::fs::write(pfx.join("system.reg"), b"WINE REGISTRY").unwrap();
        assert!(is_prefix_initialised(pfx));
    }

    #[test]
    fn is_prefix_initialised_false_when_hives_absent() {
        let dir = TempDir::new().unwrap();
        assert!(!is_prefix_initialised(dir.path()));
    }

    #[test]
    fn is_prefix_initialised_false_when_only_one_hive_present() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("user.reg"), b"WINE REGISTRY").unwrap();
        // system.reg is missing.
        assert!(!is_prefix_initialised(dir.path()));
    }

    #[test]
    fn proton_prefix_returns_ok_regardless_of_steam_presence() {
        // This must not panic whether or not Steam is installed.
        // Returns None when Steam is absent, Some(…) when present and prefix
        // exists. Either is valid.
        let _result = proton_prefix(489830); // Skyrim SE
    }
}
