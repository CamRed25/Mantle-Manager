//! Steam library scanning — locates installed games via `steamlocate`.
//!
//! # Design
//! The public API is split into two layers:
//!
//! 1. **[`detect_all`]** — takes a `&steamlocate::SteamDir` and returns every
//!    recognised game in all Steam libraries. Accepts the `SteamDir` by reference
//!    so callers (or tests) can construct it from any path via
//!    [`steamlocate::SteamDir::from_dir`].
//!
//! 2. **[`detect_all_steam`]** — convenience wrapper that calls
//!    `SteamDir::locate()` and delegates to `detect_all`. Use this in
//!    production code; use `detect_all` in tests.
//!
//! 3. **[`detect_game_at_path`]** — checks whether a given directory looks
//!    like a valid installation of a [`GameDef`]. Pure filesystem check;
//!    requires no Steam installation. Suitable for unit tests with `TempDir`.
//!
//! # `PLATFORM_COMPAT.md` §6 — Proton
//! After locating each game, [`detect_all`] calls [`super::proton::proton_prefix`]
//! to attach the Proton prefix path if one exists. The prefix is attached
//! opportunistically — a `None` prefix does not make the `GameInfo` invalid.

use std::path::{Path, PathBuf};

use steamlocate::SteamDir;

use super::proton;
use super::{
    games::{self, GameDef},
    GameInfo,
};
use crate::error::MantleError;

// ─── Public API ───────────────────────────────────────────────────────────────

/// Scan all Steam libraries via `SteamDir::locate()` and return every
/// recognised game found.
///
/// Convenience wrapper around [`detect_all`]. Returns an empty `Vec` (not an
/// error) if Steam is not installed.
///
/// # Errors
/// Returns [`MantleError::Game`] only for hard I/O failures reading the Steam
/// library manifest. A missing Steam installation always yields `Ok(vec![])`.
pub fn detect_all_steam() -> Result<Vec<GameInfo>, MantleError> {
    let Ok(steam) = SteamDir::locate() else {
        tracing::debug!("Steam not found — returning empty game list");
        return Ok(vec![]);
    };
    detect_all(&steam)
}

/// Scan all libraries in `steam` and return every recognised game found.
///
/// Iterates every Steam library path, checks each library's app manifest
/// against [`games::KNOWN_GAMES`], and builds a [`GameInfo`] for each hit.
///
/// Libraries that fail to parse are logged at `WARN` and skipped; they do
/// not cause the whole scan to fail.
///
/// # Parameters
/// - `steam`: A `SteamDir` that may have been created from a real install
///   (`SteamDir::locate()`) or from a test fixture (`SteamDir::from_dir()`).
///
/// # Errors
/// Returns [`MantleError::Game`] if the top-level library list cannot be read.
pub fn detect_all(steam: &SteamDir) -> Result<Vec<GameInfo>, MantleError> {
    let proton_map = match steam.compat_tool_mapping() {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("Could not read compat tool mapping: {e}");
            std::collections::HashMap::new()
        }
    };

    let libraries = steam
        .libraries()
        .map_err(|e| MantleError::Game(format!("Failed to read Steam library list: {e}")))?;

    let mut games: Vec<GameInfo> = Vec::new();

    for library_result in libraries {
        let library = match library_result {
            Ok(lib) => lib,
            Err(e) => {
                tracing::warn!("Skipping unreadable Steam library: {e}");
                continue;
            }
        };

        for app_result in library.apps() {
            let app = match app_result {
                Ok(a) => a,
                Err(e) => {
                    tracing::debug!("Skipping unreadable app manifest: {e}");
                    continue;
                }
            };

            let Some(def) = games::by_app_id(app.app_id) else {
                continue; // Not a game we manage.
            };

            let install_path = library.resolve_app_dir(&app);
            let data_path = def.data_path(&install_path);

            // Verify the executable is actually present to guard against
            // partially-installed or corrupt entries.
            if !install_path.join(def.executable).exists() {
                tracing::debug!(
                    "Skipping {} — executable '{}' not found at {}",
                    def.slug,
                    def.executable,
                    install_path.display()
                );
                continue;
            }

            let prefix = proton::proton_prefix_in_dir(steam.path(), app.app_id);

            // Log whether this app uses Proton, purely informational.
            if proton_map.contains_key(&app.app_id) {
                tracing::debug!("{} runs via Proton", def.slug);
            }

            tracing::info!(
                "Detected {} at {} (prefix: {:?})",
                def.slug,
                install_path.display(),
                prefix
            );

            games.push(GameInfo {
                slug: def.slug.to_owned(),
                name: def.name.to_owned(),
                kind: def.kind,
                steam_app_id: app.app_id,
                install_path,
                data_path,
                proton_prefix: prefix,
            });
        }
    }

    // ── Registry fallback ─────────────────────────────────────────────────
    // steamlocate occasionally misses games whose install path is recorded in
    // the Proton/Wine registry rather than in the Steam library manifest.
    // For each already-detected game that has a Proton prefix, consult the
    // Wine system registry for an alternative install path.  If the registry
    // path points to a real directory (and differs from what steamlocate found),
    // append a secondary GameInfo with the corrected paths.
    let mut registry_extras: Vec<GameInfo> = Vec::new();
    for g in &games {
        if let Some(pfx) = &g.proton_prefix {
            if let Some(extra_install) = find_extra_install_path(pfx, g.steam_app_id) {
                if extra_install != g.install_path {
                    let def = match games::by_app_id(g.steam_app_id) {
                        Some(d) => d,
                        None => continue,
                    };
                    let extra_data = def.data_path(&extra_install);
                    registry_extras.push(GameInfo {
                        install_path: extra_install,
                        data_path: extra_data,
                        ..g.clone()
                    });
                }
            }
        }
    }
    games.extend(registry_extras);

    // Deduplicate by steam_app_id — steamlocate's entry wins (first seen wins).
    let mut seen_app_ids = std::collections::HashSet::new();
    games.retain(|g| seen_app_ids.insert(g.steam_app_id));

    Ok(games)
}

/// Check whether `install_path` is a valid installation of `def`.
///
/// Returns a populated [`GameInfo`] if the directory exists and the
/// game's expected executable is present inside it; `None` otherwise.
///
/// This function performs **no Steam calls** — it is a pure filesystem
/// probe. Use this in unit tests with a `TempDir`:
///
/// ```ignore
/// let dir = TempDir::new()?;
/// std::fs::write(dir.path().join("SkyrimSE.exe"), b"")?;
/// let def = games::by_slug("skyrim_se").unwrap();
/// let info = detect_game_at_path(dir.path(), def, None);
/// assert!(info.is_some());
/// ```
///
/// # Parameters
/// - `install_path`: Directory to inspect.
/// - `def`: The game definition to check against.
/// - `proton_prefix`: Optional Proton prefix to attach; pass `None` in tests.
#[must_use]
pub fn detect_game_at_path(
    install_path: &Path,
    def: &GameDef,
    proton_prefix: Option<PathBuf>,
) -> Option<GameInfo> {
    if !install_path.is_dir() {
        return None;
    }
    if !install_path.join(def.executable).exists() {
        return None;
    }

    let data_path = def.data_path(install_path);

    Some(GameInfo {
        slug: def.slug.to_owned(),
        name: def.name.to_owned(),
        kind: def.kind,
        steam_app_id: def.steam_app_id,
        install_path: install_path.to_path_buf(),
        data_path,
        proton_prefix,
    })
}

// ─── Registry helpers ─────────────────────────────────────────────────────────

/// Query the Wine/Proton system registry for an alternative install path for
/// `app_id` within `pfx`.
///
/// Steam records game install locations in the system registry under:
/// `HKLM\Software\Valve\Steam\Apps\{app_id}\InstallPath`
///
/// This fallback is useful when steamlocate's library manifests are stale or
/// when the game is installed to a path that was remapped after the manifests
/// were written.
///
/// Returns `None` if:
/// - The system registry file cannot be read.
/// - The expected key / value is absent.
/// - The path string is empty.
/// - The resolved path does not exist on disk.
///
/// # Parameters
/// - `pfx`: Path to the Proton prefix root (the `pfx/` directory).
/// - `app_id`: Steam app ID to look up.
fn find_extra_install_path(pfx: &Path, app_id: u32) -> Option<PathBuf> {
    let hive = super::registry::load_system_reg(pfx).ok()?;

    let key = format!("Software\\Valve\\Steam\\Apps\\{app_id}");
    let raw = hive.get_str(&key, "InstallPath")?;
    if raw.is_empty() {
        return None;
    }

    // Wine registry paths use Windows-style backslashes; normalise to forward
    // slashes for Linux path handling.
    let path = PathBuf::from(raw.replace('\\', "/"));
    if path.exists() { Some(path) } else { None }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::games;
    use tempfile::TempDir;

    fn skyrim_def() -> &'static GameDef {
        games::by_slug("skyrim_se").unwrap()
    }

    // ── detect_game_at_path ───────────────────────────────────────────────────

    #[test]
    fn detects_game_when_executable_present() {
        let dir = TempDir::new().unwrap();
        let def = skyrim_def();
        std::fs::write(dir.path().join(def.executable), b"PE").unwrap();

        let info =
            detect_game_at_path(dir.path(), def, None).expect("must detect when executable exists");

        assert_eq!(info.slug, "skyrim_se");
        assert_eq!(info.kind, crate::game::GameKind::SkyrimSE);
        assert_eq!(info.install_path, dir.path());
        assert_eq!(info.data_path, dir.path().join("Data"));
        assert!(info.proton_prefix.is_none());
    }

    #[test]
    fn returns_none_when_executable_absent() {
        let dir = TempDir::new().unwrap();
        let def = skyrim_def();
        // Do NOT create the executable.
        assert!(detect_game_at_path(dir.path(), def, None).is_none());
    }

    #[test]
    fn returns_none_for_nonexistent_directory() {
        let def = skyrim_def();
        let bogus = std::path::Path::new("/this/path/cannot/exist/99999");
        assert!(detect_game_at_path(bogus, def, None).is_none());
    }

    #[test]
    fn attaches_proton_prefix_when_provided() {
        let dir = TempDir::new().unwrap();
        let def = skyrim_def();
        std::fs::write(dir.path().join(def.executable), b"PE").unwrap();

        let pfx = PathBuf::from("/fake/prefix/pfx");
        let info = detect_game_at_path(dir.path(), def, Some(pfx.clone())).unwrap();

        assert_eq!(info.proton_prefix, Some(pfx));
        assert!(info.is_proton());
    }

    #[test]
    fn data_path_joins_data_subdir() {
        let dir = TempDir::new().unwrap();
        let def = skyrim_def(); // data_subdir = "Data"
        std::fs::write(dir.path().join(def.executable), b"PE").unwrap();
        let info = detect_game_at_path(dir.path(), def, None).unwrap();
        assert_eq!(info.data_path, dir.path().join("Data"));
    }

    #[test]
    fn morrowind_data_path_uses_data_files_subdir() {
        let dir = TempDir::new().unwrap();
        let def = games::by_slug("morrowind").unwrap();
        std::fs::write(dir.path().join(def.executable), b"PE").unwrap();
        let info = detect_game_at_path(dir.path(), def, None).unwrap();
        // Morrowind data_subdir = "Data Files"
        assert_eq!(info.data_path, dir.path().join("Data Files"));
    }

    // ── detect_all_steam returns Ok when Steam absent ─────────────────────────

    #[test]
    fn detect_all_steam_returns_ok_when_no_steam() {
        // This may return games if Steam is installed, or empty if not.
        // Either way it must not panic or return Err.
        let result = detect_all_steam();
        assert!(result.is_ok(), "detect_all_steam must return Ok: {:?}", result);
    }
}
