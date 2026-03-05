//! Per-game SKSE configuration table.
//!
//! Maps each supported [`GameKind`] to the download and version-check URLs,
//! expected loader filenames, Wine DLL override names, and local version-file
//! name written after a successful install.
//!
//! # Supported titles
//!
//! | Game         | Project | Site                          |
//! |--------------|---------|-------------------------------|
//! | Skyrim LE    | SKSE    | skse.silverlock.org           |
//! | Skyrim SE/AE | SKSE64  | skse.silverlock.org           |
//! | Skyrim VR    | SKSEVR  | skse.silverlock.org           |
//! | Enderal SE   | SKSE64  | skse.silverlock.org (same SE) |
//! | Fallout 4    | F4SE    | f4se.silverlock.org           |
//! | Fallout NV   | NVSE    | nvse.silverlock.org           |
//! | Fallout 3    | FOSE    | fose.silverlock.org           |
//! | Oblivion     | OBSE    | obse.silverlock.org           |
//!
//! Morrowind (MWSE) and Starfield have no stable download endpoint on a
//! silverlock-style site; [`config_for_game`] returns `None` for both.

use crate::game::GameKind;

// ── Public types ──────────────────────────────────────────────────────────────

/// Static configuration for one game's script extender.
#[derive(Debug)]
pub struct SkseGameConfig {
    /// Game variant this config applies to.
    pub kind: GameKind,
    /// Human-readable project name shown in UI, e.g. `"SKSE64"`.
    pub display_name: &'static str,
    /// URL that returns the latest version as a plain-text string
    /// (space- or dot-separated, e.g. `"2 2 6 0"`).
    pub version_url: &'static str,
    /// Direct download URL for the latest archive (7z or zip).
    pub download_url: &'static str,
    /// Filename written inside the game directory after installation that
    /// records the installed version, e.g. `"skse64_version.txt"`.
    pub version_file: &'static str,
    /// Loader executable names to look for in `{game_dir}/` after extraction.
    /// At least one must be present for validation to pass.
    pub loader_names: &'static [&'static str],
    /// Wine DLL names that need a `native,builtin` override in the Proton
    /// prefix so the script-extender Steam-loader DLL is preferred.
    pub dll_overrides: &'static [&'static str],
}

// ── Static table ──────────────────────────────────────────────────────────────

/// All supported script-extender configurations, one entry per game.
///
/// Indexed via [`config_for_game`]. The table is intentionally `&'static` so
/// callers can hold references without cloning.
pub static SKSE_GAME_MAP: &[SkseGameConfig] = &[
    // ── Skyrim LE (SKSE 1.x) ─────────────────────────────────────────────────
    SkseGameConfig {
        kind: GameKind::SkyrimLE,
        display_name: "SKSE (Skyrim LE)",
        version_url: "https://skse.silverlock.org/download/skse/sk-version.txt",
        // LE releases as a self-extracting executable; libarchive handles it as zip.
        download_url: "https://skse.silverlock.org/download/skse/sk-latest.exe",
        version_file: "skse_version.txt",
        loader_names: &["skse_loader.exe"],
        dll_overrides: &["skse_steam_loader"],
    },
    // ── Skyrim SE / AE (SKSE64 2.x) ─────────────────────────────────────────
    SkseGameConfig {
        kind: GameKind::SkyrimSE,
        display_name: "SKSE64 (Skyrim SE/AE)",
        version_url: "https://skse.silverlock.org/download/skse64/se-version.txt",
        download_url: "https://skse.silverlock.org/download/skse64/se-latest.7z",
        version_file: "skse64_version.txt",
        loader_names: &["skse64_loader.exe"],
        dll_overrides: &["skse64_steam_loader"],
    },
    // ── Skyrim VR (SKSEVR) ───────────────────────────────────────────────────
    SkseGameConfig {
        kind: GameKind::SkyrimVR,
        display_name: "SKSEVR (Skyrim VR)",
        version_url: "https://skse.silverlock.org/download/sksevr/sksevr-version.txt",
        download_url: "https://skse.silverlock.org/download/sksevr/sksevr-latest.7z",
        version_file: "sksevr_version.txt",
        loader_names: &["sksevr_loader.exe"],
        dll_overrides: &["sksevr_steam_loader"],
    },
    // ── Enderal SE — same SKSE64 binary as Skyrim SE ─────────────────────────
    SkseGameConfig {
        kind: GameKind::EnderalSE,
        display_name: "SKSE64 (Enderal SE)",
        version_url: "https://skse.silverlock.org/download/skse64/se-version.txt",
        download_url: "https://skse.silverlock.org/download/skse64/se-latest.7z",
        version_file: "skse64_version.txt",
        loader_names: &["skse64_loader.exe"],
        dll_overrides: &["skse64_steam_loader"],
    },
    // ── Fallout 4 (F4SE) ─────────────────────────────────────────────────────
    SkseGameConfig {
        kind: GameKind::Fallout4,
        display_name: "F4SE (Fallout 4)",
        version_url: "https://f4se.silverlock.org/download/f4se/f4se-version.txt",
        download_url: "https://f4se.silverlock.org/download/f4se/f4se-latest.7z",
        version_file: "f4se_version.txt",
        loader_names: &["f4se_loader.exe"],
        dll_overrides: &["f4se_steam_loader"],
    },
    // ── Fallout: New Vegas (NVSE) ─────────────────────────────────────────────
    SkseGameConfig {
        kind: GameKind::FalloutNV,
        display_name: "NVSE (Fallout: New Vegas)",
        version_url: "https://nvse.silverlock.org/download/nvse/nvse-version.txt",
        download_url: "https://nvse.silverlock.org/download/nvse/nvse-latest.7z",
        version_file: "nvse_version.txt",
        loader_names: &["nvse_loader.exe"],
        dll_overrides: &[],
    },
    // ── Fallout 3 (FOSE) ─────────────────────────────────────────────────────
    SkseGameConfig {
        kind: GameKind::Fallout3,
        display_name: "FOSE (Fallout 3)",
        version_url: "https://fose.silverlock.org/download/fose/fose-version.txt",
        download_url: "https://fose.silverlock.org/download/fose/fose-latest.7z",
        version_file: "fose_version.txt",
        loader_names: &["fose_loader.exe"],
        dll_overrides: &[],
    },
    // ── Oblivion (OBSE) ───────────────────────────────────────────────────────
    SkseGameConfig {
        kind: GameKind::Oblivion,
        display_name: "OBSE (Oblivion)",
        version_url: "https://obse.silverlock.org/download/obse/obse-version.txt",
        download_url: "https://obse.silverlock.org/download/obse/obse-latest.7z",
        version_file: "obse_version.txt",
        loader_names: &["obse_loader.exe"],
        dll_overrides: &[],
    },
];

// ── Public API ────────────────────────────────────────────────────────────────

/// Returns the script-extender configuration for `kind`, or `None` if the
/// game has no supported script extender with a known download endpoint.
///
/// Returns `None` for [`GameKind::Morrowind`] (MWSE uses a different
/// distribution model) and [`GameKind::Starfield`] (no stable silverlock-style
/// endpoint at time of writing).
#[must_use]
pub fn config_for_game(kind: GameKind) -> Option<&'static SkseGameConfig> {
    SKSE_GAME_MAP.iter().find(|c| c.kind == kind)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_supported_games_have_config() {
        let supported = [
            GameKind::SkyrimLE,
            GameKind::SkyrimSE,
            GameKind::SkyrimVR,
            GameKind::EnderalSE,
            GameKind::Fallout4,
            GameKind::FalloutNV,
            GameKind::Fallout3,
            GameKind::Oblivion,
        ];
        for kind in supported {
            assert!(
                config_for_game(kind).is_some(),
                "{kind} should have a config entry"
            );
        }
    }

    #[test]
    fn unsupported_games_return_none() {
        assert!(config_for_game(GameKind::Morrowind).is_none());
        assert!(config_for_game(GameKind::Starfield).is_none());
    }

    #[test]
    fn all_entries_have_non_empty_urls() {
        for cfg in SKSE_GAME_MAP {
            assert!(!cfg.version_url.is_empty(), "{}: empty version_url", cfg.display_name);
            assert!(!cfg.download_url.is_empty(), "{}: empty download_url", cfg.display_name);
            assert!(!cfg.version_file.is_empty(), "{}: empty version_file", cfg.display_name);
        }
    }

    #[test]
    fn all_entries_have_at_least_one_loader() {
        for cfg in SKSE_GAME_MAP {
            assert!(
                !cfg.loader_names.is_empty(),
                "{}: loader_names must not be empty",
                cfg.display_name
            );
        }
    }

    #[test]
    fn skse64_and_enderal_share_urls() {
        let se = config_for_game(GameKind::SkyrimSE).unwrap();
        let enderal = config_for_game(GameKind::EnderalSE).unwrap();
        assert_eq!(se.download_url, enderal.download_url);
        assert_eq!(se.version_url, enderal.version_url);
    }
}
