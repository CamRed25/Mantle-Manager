//! SKSE / xSE cosave diagnostic.
//!
//! After a game session ends, some saves may be missing their script-extender
//! cosave file.  This indicates the game was launched without the SE active,
//! which corrupts SKSE-dependent mod data for those saves.
//!
//! # Workflow
//! 1. Call [`cosave_config_for`] to retrieve the per-game extension table.
//! 2. Call [`se_is_installed`] to confirm the script extender is present.
//! 3. Call [`scan_missing_cosaves`] to get the list of affected saves.
//!
//! # Supported games
//! | Game                                | Save ext | Cosave ext | SE dir           |
//! |-------------------------------------|----------|------------|------------------|
//! | Skyrim LE / SE / VR, Enderal SE     | `.ess`   | `.skse`    | `SKSE/Plugins`   |
//! | Fallout 4                           | `.fos`   | `.f4se`    | `F4SE/Plugins`   |
//! | Fallout: New Vegas                  | `.fos`   | `.nvse`    | `NVSE/Plugins`   |
//! | Fallout 3                           | `.fos`   | `.fose`    | `FOSE/Plugins`   |
//! | Oblivion                            | `.ess`   | `.obse`    | `OBSE/Plugins`   |
//! | Starfield                           | `.sfs`   | `.sfse`    | `SFSE/Plugins`   |
//! | Morrowind                           | *(no SE)*| —          | —                |

use std::path::{Path, PathBuf};

use crate::game::GameKind;

// ── Public types ──────────────────────────────────────────────────────────────

/// Per-game script-extender configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CosaveConfig {
    /// Extension of standard save files (e.g. `.ess`), **lowercase with dot**.
    pub save_ext: &'static str,
    /// Extension of script-extender cosave (e.g. `.skse`), **lowercase with dot**.
    pub cosave_ext: &'static str,
    /// Relative path within a mod where SE plugin DLLs live
    /// (used to detect whether the SE is installed in the mods directory).
    pub se_plugin_dir: &'static str,
}

/// Result of a cosave scan.
#[derive(Debug, Default)]
pub struct CosaveScanResult {
    /// Save files (file name only) that have no matching cosave.
    ///
    /// Sorted alphabetically for stable output.
    pub missing_cosaves: Vec<PathBuf>,
    /// Whether the script extender was detected in the mods directory.
    ///
    /// When `false` the scan is skipped; `missing_cosaves` will be empty
    /// even if saves lack cosaves (no SE installed → no expectation of cosaves).
    pub se_detected: bool,
}

impl CosaveScanResult {
    /// `true` if no saves are missing cosaves (or the SE is not installed).
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.missing_cosaves.is_empty()
    }
}

// ── Per-game config table ─────────────────────────────────────────────────────

/// Return the [`CosaveConfig`] for a given game, or `None` for games without
/// a supported script extender (Morrowind has no MWSE cosave convention).
///
/// # Parameters
/// - `kind`: The game kind to look up.
#[must_use]
pub fn cosave_config_for(kind: GameKind) -> Option<CosaveConfig> {
    match kind {
        GameKind::SkyrimLE | GameKind::SkyrimSE | GameKind::SkyrimVR | GameKind::EnderalSE => {
            Some(CosaveConfig {
                save_ext: ".ess",
                cosave_ext: ".skse",
                se_plugin_dir: "SKSE/Plugins",
            })
        }
        GameKind::Fallout4 => Some(CosaveConfig {
            save_ext: ".fos",
            cosave_ext: ".f4se",
            se_plugin_dir: "F4SE/Plugins",
        }),
        GameKind::FalloutNV => Some(CosaveConfig {
            save_ext: ".fos",
            cosave_ext: ".nvse",
            se_plugin_dir: "NVSE/Plugins",
        }),
        GameKind::Fallout3 => Some(CosaveConfig {
            save_ext: ".fos",
            cosave_ext: ".fose",
            se_plugin_dir: "FOSE/Plugins",
        }),
        GameKind::Oblivion => Some(CosaveConfig {
            save_ext: ".ess",
            cosave_ext: ".obse",
            se_plugin_dir: "OBSE/Plugins",
        }),
        GameKind::Starfield => Some(CosaveConfig {
            save_ext: ".sfs",
            cosave_ext: ".sfse",
            se_plugin_dir: "SFSE/Plugins",
        }),
        GameKind::Morrowind => None,
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Return `true` if any mod in `mods_dir` has `.dll` files in `se_plugin_dir`.
///
/// Scans each immediate subdirectory of `mods_dir` (each is a mod directory)
/// for the presence of `.dll` files inside `se_plugin_dir`.  The extension
/// comparison is case-insensitive to handle Windows-authored mods.
///
/// # Parameters
/// - `mods_dir`: Root directory containing all installed mod directories.
/// - `se_plugin_dir`: Relative sub-path within each mod where SE DLLs live
///   (e.g. `"SKSE/Plugins"`).
#[must_use]
pub fn se_is_installed(mods_dir: &Path, se_plugin_dir: &str) -> bool {
    let Ok(top) = std::fs::read_dir(mods_dir) else {
        return false;
    };
    for entry in top.flatten() {
        let plugin_dir = entry.path().join(se_plugin_dir);
        if !plugin_dir.is_dir() {
            continue;
        }
        let Ok(inner) = std::fs::read_dir(&plugin_dir) else {
            continue;
        };
        for file in inner.flatten() {
            let is_dll = file
                .path()
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("dll"));
            if is_dll {
                return true;
            }
        }
    }
    false
}

/// Scan `saves_dir` for save files that have no matching cosave.
///
/// The scan is skipped entirely when the script extender is not detected in
/// `mods_dir` (via [`se_is_installed`]).  In that case the result has
/// `se_detected = false` and an empty `missing_cosaves` list — the absence of
/// a cosave is expected when no SE is installed.
///
/// # Parameters
/// - `saves_dir`: Directory containing save files (`.ess` / `.fos` / `.sfs`).
/// - `mods_dir`: Root of the installed mods tree, used for SE detection.
/// - `config`: Per-game [`CosaveConfig`] (obtain from [`cosave_config_for`]).
///
/// # Returns
/// A [`CosaveScanResult`] listing any saves that lack a cosave file.
#[must_use]
pub fn scan_missing_cosaves(
    saves_dir: &Path,
    mods_dir: &Path,
    config: &CosaveConfig,
) -> CosaveScanResult {
    let mut result = CosaveScanResult {
        se_detected: se_is_installed(mods_dir, config.se_plugin_dir),
        ..Default::default()
    };
    if !result.se_detected {
        return result;
    }

    let Ok(rd) = std::fs::read_dir(saves_dir) else {
        return result;
    };

    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name_os) = path.file_name() else {
            continue;
        };
        let name = name_os.to_string_lossy();
        if !name.to_lowercase().ends_with(config.save_ext) {
            continue;
        }
        // Strip save extension, then construct expected cosave filename.
        let stem_len = name.len() - config.save_ext.len();
        let stem = &name[..stem_len];
        let cosave_name = format!("{stem}{}", config.cosave_ext);
        if !saves_dir.join(&cosave_name).exists() {
            result.missing_cosaves.push(PathBuf::from(name.as_ref()));
        }
    }

    result.missing_cosaves.sort();
    result
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // ── cosave_config_for ─────────────────────────────────────────────────

    #[test]
    fn skyrim_se_returns_skse_config() {
        let cfg = cosave_config_for(GameKind::SkyrimSE).unwrap();
        assert_eq!(cfg.save_ext, ".ess");
        assert_eq!(cfg.cosave_ext, ".skse");
        assert_eq!(cfg.se_plugin_dir, "SKSE/Plugins");
    }

    #[test]
    fn skyrim_le_and_vr_share_skse_config() {
        let le = cosave_config_for(GameKind::SkyrimLE).unwrap();
        let vr = cosave_config_for(GameKind::SkyrimVR).unwrap();
        assert_eq!(le, vr);
    }

    #[test]
    fn fallout4_returns_f4se_config() {
        let cfg = cosave_config_for(GameKind::Fallout4).unwrap();
        assert_eq!(cfg.cosave_ext, ".f4se");
        assert_eq!(cfg.se_plugin_dir, "F4SE/Plugins");
    }

    #[test]
    fn falloutnv_returns_nvse_config() {
        let cfg = cosave_config_for(GameKind::FalloutNV).unwrap();
        assert_eq!(cfg.cosave_ext, ".nvse");
    }

    #[test]
    fn morrowind_returns_none() {
        assert!(cosave_config_for(GameKind::Morrowind).is_none());
    }

    #[test]
    fn all_non_morrowind_games_have_config() {
        let games = [
            GameKind::SkyrimLE,
            GameKind::SkyrimSE,
            GameKind::SkyrimVR,
            GameKind::EnderalSE,
            GameKind::Fallout4,
            GameKind::FalloutNV,
            GameKind::Fallout3,
            GameKind::Oblivion,
            GameKind::Starfield,
        ];
        for kind in games {
            assert!(cosave_config_for(kind).is_some(), "{kind:?} must have a cosave config");
        }
    }

    // ── se_is_installed ───────────────────────────────────────────────────

    fn make_mods_with_dll(se_subdir: &str) -> TempDir {
        let mods = TempDir::new().unwrap();
        let plugin_dir = mods.path().join("SKSE_Mod").join(se_subdir);
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(plugin_dir.join("plugin.dll"), b"MZ").unwrap();
        mods
    }

    #[test]
    fn se_detected_when_dll_present() {
        let mods = make_mods_with_dll("SKSE/Plugins");
        assert!(se_is_installed(mods.path(), "SKSE/Plugins"));
    }

    #[test]
    fn se_not_detected_when_no_dll() {
        let mods = TempDir::new().unwrap();
        let plugin_dir = mods.path().join("MyMod/SKSE/Plugins");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(plugin_dir.join("readme.txt"), b"").unwrap();
        assert!(!se_is_installed(mods.path(), "SKSE/Plugins"));
    }

    #[test]
    fn se_not_detected_when_dir_absent() {
        let mods = TempDir::new().unwrap();
        assert!(!se_is_installed(mods.path(), "SKSE/Plugins"));
    }

    #[test]
    fn se_dll_extension_case_insensitive() {
        let mods = TempDir::new().unwrap();
        let plugin_dir = mods.path().join("Mod/SKSE/Plugins");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(plugin_dir.join("plugin.DLL"), b"MZ").unwrap();
        assert!(se_is_installed(mods.path(), "SKSE/Plugins"));
    }

    // ── scan_missing_cosaves ──────────────────────────────────────────────

    /// Build a saves dir and a mods dir that has the SKSE SE DLL installed.
    fn make_skse_setup() -> (TempDir, TempDir, CosaveConfig) {
        let saves = TempDir::new().unwrap();
        let mods = make_mods_with_dll("SKSE/Plugins");
        let cfg = cosave_config_for(GameKind::SkyrimSE).unwrap();
        (saves, mods, cfg)
    }

    #[test]
    fn save_without_cosave_is_reported() {
        let (saves, mods, cfg) = make_skse_setup();
        fs::write(saves.path().join("save1.ess"), b"save").unwrap();
        // No matching save1.skse
        let result = scan_missing_cosaves(saves.path(), mods.path(), &cfg);
        assert!(result.se_detected);
        assert_eq!(result.missing_cosaves.len(), 1);
        assert_eq!(result.missing_cosaves[0], PathBuf::from("save1.ess"));
    }

    #[test]
    fn save_with_cosave_is_not_reported() {
        let (saves, mods, cfg) = make_skse_setup();
        fs::write(saves.path().join("save1.ess"), b"save").unwrap();
        fs::write(saves.path().join("save1.skse"), b"cosave").unwrap();
        let result = scan_missing_cosaves(saves.path(), mods.path(), &cfg);
        assert!(result.se_detected);
        assert!(result.missing_cosaves.is_empty());
    }

    #[test]
    fn scan_skipped_when_se_absent() {
        let saves = TempDir::new().unwrap();
        let mods = TempDir::new().unwrap(); // empty — no SE DLL
        let cfg = cosave_config_for(GameKind::SkyrimSE).unwrap();
        fs::write(saves.path().join("save1.ess"), b"save").unwrap();
        let result = scan_missing_cosaves(saves.path(), mods.path(), &cfg);
        assert!(!result.se_detected);
        assert!(result.missing_cosaves.is_empty(), "must not scan when SE absent");
    }

    #[test]
    fn nonexistent_saves_dir_returns_empty() {
        let mods = make_mods_with_dll("SKSE/Plugins");
        let cfg = cosave_config_for(GameKind::SkyrimSE).unwrap();
        let result = scan_missing_cosaves(Path::new("/no/such/saves/dir"), mods.path(), &cfg);
        assert!(result.se_detected);
        assert!(result.missing_cosaves.is_empty());
    }

    #[test]
    fn results_are_sorted() {
        let (saves, mods, cfg) = make_skse_setup();
        fs::write(saves.path().join("save_z.ess"), b"s").unwrap();
        fs::write(saves.path().join("save_a.ess"), b"s").unwrap();
        let result = scan_missing_cosaves(saves.path(), mods.path(), &cfg);
        assert_eq!(result.missing_cosaves.len(), 2);
        assert!(result.missing_cosaves[0] < result.missing_cosaves[1], "results must be sorted");
    }

    #[test]
    fn is_ok_true_when_empty() {
        let r = CosaveScanResult::default();
        assert!(r.is_ok());
    }

    #[test]
    fn is_ok_false_when_missing_present() {
        let r = CosaveScanResult {
            missing_cosaves: vec![PathBuf::from("save1.ess")],
            se_detected: true,
        };
        assert!(!r.is_ok());
    }
}
