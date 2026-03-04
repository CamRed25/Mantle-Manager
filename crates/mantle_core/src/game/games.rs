//! Static game definition table.
//!
//! Each [`GameDef`] entry describes one supported Bethesda title: its
//! Steam App ID, short slug, display name, game variant, primary executable
//! filename, and the name of its data subdirectory relative to the install
//! root.
//!
//! Adding support for a new game is a two-step change:
//! 1. Add a `GameKind` variant in `super` (`game/mod.rs`).
//! 2. Add a `GameDef` row to [`KNOWN_GAMES`] below.
//!
//! No other code changes are required for basic detection — [`super::steam`]
//! iterates this table automatically.

use super::GameKind;

// ─── GameDef ──────────────────────────────────────────────────────────────────

/// Static description of a supported game title.
///
/// Used by [`super::steam`] to scan Steam libraries and by
/// [`super::steam::detect_game_at_path`] to verify a given directory really
/// is the expected game.
#[derive(Debug, Clone, Copy)]
pub struct GameDef {
    /// Short lowercase slug, stable across versions. Used as a database key.
    ///
    /// Example: `"skyrim_se"`
    pub slug: &'static str,

    /// Full display name shown in the UI.
    ///
    /// Example: `"The Elder Scrolls V: Skyrim Special Edition"`
    pub name: &'static str,

    /// Game variant for game-specific behaviour (archive format, load order).
    pub kind: GameKind,

    /// Steam Store App ID. Used with `steamlocate` to find the install path.
    pub steam_app_id: u32,

    /// Executable filename relative to the install root.
    ///
    /// Used as a presence sentinel: if this file exists the directory is
    /// treated as a valid installation of this game.
    ///
    /// Example: `"SkyrimSE.exe"`
    pub executable: &'static str,

    /// Name of the data subdirectory relative to the install root.
    ///
    /// `"Data"` for all titles except Morrowind, which stores loose files
    /// at the install root. Empty string `""` means `data_path == install_path`.
    pub data_subdir: &'static str,
}

impl GameDef {
    /// Compute the `data_path` for this game given the install directory.
    ///
    /// Returns `install_path` unchanged when `data_subdir` is empty.
    ///
    /// # Parameters
    /// - `install_path`: Absolute path to the game's root install directory.
    #[must_use]
    pub fn data_path(&self, install_path: &std::path::Path) -> std::path::PathBuf {
        if self.data_subdir.is_empty() {
            install_path.to_path_buf()
        } else {
            install_path.join(self.data_subdir)
        }
    }
}

// ─── KNOWN_GAMES ──────────────────────────────────────────────────────────────

/// All supported game titles, in rough release order.
///
/// The Steam App IDs are the official IDs from the Steam store. Fallout 4 VR
/// and Skyrim VR are omitted from the first implementation — tracked in
/// `futures.md`.
///
/// # Note on Enderal SE
/// Enderal SE (App ID 976620) is a standalone total-conversion mod, not
/// distributed as a Skyrim SE DLC. It uses the `SkyrimSE` engine and BSA format
/// but has its own executable and data layout. Listed separately from `SkyrimSE`.
pub const KNOWN_GAMES: &[GameDef] = &[
    GameDef {
        slug: "morrowind",
        name: "The Elder Scrolls III: Morrowind",
        kind: GameKind::Morrowind,
        steam_app_id: 22320,
        executable: "Morrowind.exe",
        data_subdir: "Data Files",
    },
    GameDef {
        slug: "oblivion",
        name: "The Elder Scrolls IV: Oblivion",
        kind: GameKind::Oblivion,
        steam_app_id: 22330,
        executable: "Oblivion.exe",
        data_subdir: "Data",
    },
    GameDef {
        slug: "skyrim_le",
        name: "The Elder Scrolls V: Skyrim",
        kind: GameKind::SkyrimLE,
        steam_app_id: 72850,
        executable: "TESV.exe",
        data_subdir: "Data",
    },
    GameDef {
        slug: "skyrim_se",
        name: "The Elder Scrolls V: Skyrim Special Edition",
        kind: GameKind::SkyrimSE,
        steam_app_id: 489_830,
        executable: "SkyrimSE.exe",
        data_subdir: "Data",
    },
    GameDef {
        slug: "skyrim_vr",
        name: "The Elder Scrolls V: Skyrim VR",
        kind: GameKind::SkyrimVR,
        steam_app_id: 611_670,
        executable: "SkyrimVR.exe",
        data_subdir: "Data",
    },
    GameDef {
        slug: "fallout3",
        name: "Fallout 3",
        kind: GameKind::Fallout3,
        steam_app_id: 22300,
        executable: "Fallout3.exe",
        data_subdir: "Data",
    },
    GameDef {
        slug: "fallout_nv",
        name: "Fallout: New Vegas",
        kind: GameKind::FalloutNV,
        steam_app_id: 22380,
        executable: "FalloutNV.exe",
        data_subdir: "Data",
    },
    GameDef {
        slug: "fallout4",
        name: "Fallout 4",
        kind: GameKind::Fallout4,
        steam_app_id: 377_160,
        executable: "Fallout4.exe",
        data_subdir: "Data",
    },
    GameDef {
        slug: "starfield",
        name: "Starfield",
        kind: GameKind::Starfield,
        steam_app_id: 1_716_740,
        executable: "Starfield.exe",
        data_subdir: "Data",
    },
    GameDef {
        slug: "enderal_se",
        name: "Enderal: Forgotten Stories (Special Edition)",
        kind: GameKind::EnderalSE,
        steam_app_id: 976_620,
        executable: "Enderal Launcher.exe",
        data_subdir: "Data",
    },
];

// ─── Lookup helpers ───────────────────────────────────────────────────────────

/// Find the [`GameDef`] for the given Steam App ID.
///
/// Returns `None` if the App ID is not in the [`KNOWN_GAMES`] table.
///
/// # Parameters
/// - `app_id`: Steam App ID to look up.
///
/// # Performance
/// Linear scan — the table has ≤ 20 rows and this function is called at most
/// once per Steam library scan, so O(n) is acceptable.
#[must_use]
pub fn by_app_id(app_id: u32) -> Option<&'static GameDef> {
    KNOWN_GAMES.iter().find(|d| d.steam_app_id == app_id)
}

/// Find the [`GameDef`] for the given slug.
///
/// Returns `None` if no entry matches.
#[must_use]
pub fn by_slug(slug: &str) -> Option<&'static GameDef> {
    KNOWN_GAMES.iter().find(|d| d.slug == slug)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_duplicate_app_ids() {
        let mut seen = std::collections::HashSet::new();
        for def in KNOWN_GAMES {
            assert!(
                seen.insert(def.steam_app_id),
                "duplicate steam_app_id {} in KNOWN_GAMES (slug: {})",
                def.steam_app_id,
                def.slug
            );
        }
    }

    #[test]
    fn no_duplicate_slugs() {
        let mut seen = std::collections::HashSet::new();
        for def in KNOWN_GAMES {
            assert!(seen.insert(def.slug), "duplicate slug '{}' in KNOWN_GAMES", def.slug);
        }
    }

    #[test]
    fn by_app_id_finds_skyrim_se() {
        let def = by_app_id(489830).expect("Skyrim SE must be in KNOWN_GAMES");
        assert_eq!(def.slug, "skyrim_se");
        assert_eq!(def.kind, GameKind::SkyrimSE);
    }

    #[test]
    fn by_app_id_returns_none_for_unknown() {
        assert!(by_app_id(0).is_none());
        assert!(by_app_id(9999999).is_none());
    }

    #[test]
    fn by_slug_finds_fallout4() {
        let def = by_slug("fallout4").expect("Fallout 4 must be in KNOWN_GAMES");
        assert_eq!(def.steam_app_id, 377160);
    }

    #[test]
    fn by_slug_returns_none_for_unknown() {
        assert!(by_slug("not_a_real_game").is_none());
    }

    #[test]
    fn data_path_appends_subdir() {
        let def = by_slug("skyrim_se").unwrap();
        let install = std::path::Path::new("/games/SkyrimSE");
        assert_eq!(def.data_path(install), install.join("Data"));
    }

    #[test]
    fn data_path_equals_install_when_subdir_empty() {
        // Morrowind uses "Data Files" not "" — but let's cover the empty case
        // with a synthetic check to guard regressions.
        let def = GameDef {
            slug: "test",
            name: "Test",
            kind: GameKind::Morrowind,
            steam_app_id: 1,
            executable: "test.exe",
            data_subdir: "",
        };
        let install = std::path::Path::new("/games/TestGame");
        assert_eq!(def.data_path(install), install);
    }

    #[test]
    fn all_entries_have_non_empty_fields() {
        for def in KNOWN_GAMES {
            assert!(!def.slug.is_empty(), "empty slug in {def:?}");
            assert!(!def.name.is_empty(), "empty name in {def:?}");
            assert!(!def.executable.is_empty(), "empty executable in {def:?}");
            assert!(def.steam_app_id > 0, "zero app_id in {def:?}");
        }
    }
}
