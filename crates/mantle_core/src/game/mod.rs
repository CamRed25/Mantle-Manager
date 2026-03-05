//! Game detection — locates installed Bethesda titles via steamlocate.
//!
//! Supports Skyrim SE/LE/VR, Fallout 4/NV/3, Oblivion, Morrowind, Starfield,
//! and Enderal SE. Each game has a static [`GameDef`] describing its Steam App
//! ID, executable name, and data subdirectory.
//!
//! # Usage
//!
//! ```ignore
//! use mantle_core::game;
//!
//! let games = game::detect_all_steam()?;
//! for g in &games {
//!     println!("{} @ {}", g.name, g.install_path.display());
//! }
//! ```
//!
//! # Module layout
//! - [`games`]  — static [`GameDef`] table for every supported title
//! - [`steam`]  — steamlocate-backed detection; injectable for unit tests
//! - [`proton`] — Proton prefix location and Wine prefix helpers

pub mod games;
pub mod ini;
pub mod proton;
pub mod registry;
pub mod steam;

pub use ini::{apply_profile_ini, snapshot_profile_ini, GameIni};
pub use registry::{load_system_reg, load_user_reg, wine_c_drive, RegistryHive, RegistryValue};
pub use steam::detect_all_steam;

use std::path::PathBuf;

// ─── GameKind ─────────────────────────────────────────────────────────────────

/// Discriminant used for game-specific behaviour (plugin ordering rules,
/// archive format selection, load-order constraints, etc.).
///
/// Each variant corresponds to one entry in the [`games::KNOWN_GAMES`] table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GameKind {
    /// The Elder Scrolls III: Morrowind — TES3 BSA format, .esp/.esm plugins.
    Morrowind,
    /// The Elder Scrolls IV: Oblivion — TES4 BSA format, Oblivion load order.
    Oblivion,
    /// The Elder Scrolls V: Skyrim (2011 / Legendary Edition) — TES4 BSA v104.
    SkyrimLE,
    /// The Elder Scrolls V: Skyrim Special Edition / Anniversary Edition — BSA v105.
    SkyrimSE,
    /// The Elder Scrolls V: Skyrim VR — same BSA as SE, separate App ID.
    SkyrimVR,
    /// Fallout 3 — TES4 BSA v104.
    Fallout3,
    /// Fallout: New Vegas — TES4 BSA v104.
    FalloutNV,
    /// Fallout 4 — BA2 GNRL + DX10.
    Fallout4,
    /// Starfield — BA2 with LZ4 compression.
    Starfield,
    /// Enderal: Forgotten Stories (Special Edition) — Skyrim SE engine, separate App ID.
    EnderalSE,
}

impl std::fmt::Display for GameKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Morrowind => write!(f, "Morrowind"),
            Self::Oblivion => write!(f, "Oblivion"),
            Self::SkyrimLE => write!(f, "SkyrimLE"),
            Self::SkyrimSE => write!(f, "SkyrimSE"),
            Self::SkyrimVR => write!(f, "SkyrimVR"),
            Self::Fallout3 => write!(f, "Fallout3"),
            Self::FalloutNV => write!(f, "FalloutNV"),
            Self::Fallout4 => write!(f, "Fallout4"),
            Self::Starfield => write!(f, "Starfield"),
            Self::EnderalSE => write!(f, "EnderalSE"),
        }
    }
}

// ─── GameInfo ─────────────────────────────────────────────────────────────────

/// A detected, installed game instance.
///
/// Produced by [`steam::detect_all`] or [`steam::detect_game_at_path`].
/// Consumed by the UI state layer, the VFS mount layer (to target `data_path`),
/// and `PluginContext::game()`.
///
/// # Invariants
/// - `install_path` exists and is a directory at the time of construction.
/// - `data_path` is a subdirectory of `install_path` (or equal to it for
///   Morrowind, which has no separate Data directory).
/// - `proton_prefix` is `Some(…)` when Proton compat data exists for this
///   App ID in the Steam installation that provided `install_path`.
#[derive(Debug, Clone)]
pub struct GameInfo {
    /// Short lowercase identifier, e.g. `"skyrim_se"`. Stable across releases.
    pub slug: String,

    /// Human-readable display name, e.g. `"The Elder Scrolls V: Skyrim Special Edition"`.
    pub name: String,

    /// Game variant — drives plugin ordering, archive format, and load-order rules.
    pub kind: GameKind,

    /// Steam App ID. `0` for non-Steam installs (future: GOG / standalone).
    pub steam_app_id: u32,

    /// Absolute path to the game's root install directory.
    ///
    /// Example: `/home/user/.steam/steam/steamapps/common/Skyrim Special Edition`
    pub install_path: PathBuf,

    /// Absolute path to the game's data directory — the VFS overlay target.
    ///
    /// Example: `<install_path>/Data`
    pub data_path: PathBuf,

    /// Proton compatibility prefix path, if available.
    ///
    /// Example: `<steamapps>/compatdata/489830/pfx`
    pub proton_prefix: Option<PathBuf>,
}

impl GameInfo {
    /// Returns `true` if the game runs via Proton (i.e. a Proton prefix is
    /// attached to this game instance).
    ///
    /// All Bethesda titles on Linux run through Proton unless the user has
    /// installed a native Linux build (none currently exist for any supported
    /// title).
    #[must_use]
    pub fn is_proton(&self) -> bool {
        self.proton_prefix.is_some()
    }

    /// Returns the Proton Wine prefix path (`pfx/` subdirectory) if present.
    ///
    /// Convenience accessor — equivalent to `self.proton_prefix.as_deref()`.
    #[must_use]
    pub fn wine_prefix(&self) -> Option<&std::path::Path> {
        self.proton_prefix.as_deref()
    }
}
