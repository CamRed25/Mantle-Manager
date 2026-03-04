use std::path::PathBuf;

/// Snapshot of application state used to populate widgets.
/// All fields are cloneable data — no locks, no core references.
/// Real state will be pushed from `mantle_core` via glib channels.
pub struct AppState {
    /// Steam App ID of the detected game, if any.
    ///
    /// Used by the launch button to open `steam://run/<app_id>`.  `None`
    /// means no game was detected (first launch, Steam not installed, no
    /// supported title found).
    pub steam_app_id: Option<u32>,
    pub game_name: String,
    pub game_version: String,
    pub launch_target: String,
    pub active_profile: String,
    pub mod_count: usize,
    pub plugin_count: usize,
    pub conflict_count: usize,
    pub overlay_backend: String,
    pub mods: Vec<ModEntry>,
    pub profiles: Vec<ProfileEntry>,
    pub downloads: Vec<DownloadEntry>,
    /// Loaded Mantle plugins, in load order.
    pub plugins: Vec<PluginEntry>,
    /// Filesystem path to the game's Data directory.
    ///
    /// Used as the VFS `merge_dir` target when mounting mods before launch.
    /// `None` when no game was detected.
    pub game_data_path: Option<PathBuf>,
}

pub struct ModEntry {
    /// Primary key from `mods.id`. 0 in placeholder/test data.
    pub mod_id: i64,
    /// The profile this entry belongs to. 0 in placeholder/test data.
    pub profile_id: i64,
    pub name: String,
    pub enabled: bool,
    pub version: Option<String>,
    pub has_conflict: bool,
}

pub struct ProfileEntry {
    /// Stable slug used to identify the profile in actions (e.g. activate, clone, delete).
    /// Wired to real core IDs in item y.
    pub id: String,
    pub name: String,
    pub mod_count: usize,
    pub active: bool,
}

pub enum DownloadState {
    /// Download is running; value is progress in the range `0.0..=1.0`.
    InProgress(f64),
    /// Download finished successfully.
    Complete,
    /// Download is waiting in the queue.
    Queued,
    /// Download stopped due to an error; contains a short error description.
    Failed(String),
}

pub struct DownloadEntry {
    /// Stable ID for cancel / retry action dispatch. Wired in item y.
    pub id: String,
    pub name: String,
    pub state: DownloadState,
}

// ─── Plugin types ─────────────────────────────────────────────────────────────

/// A single key/value setting displayed in the plugin settings panel.
///
/// Values are stringified for display; the core holds the real typed values.
pub struct PluginSettingEntry {
    /// The setting's stable key (from `MantlePlugin::settings()`).
    pub key: String,
    /// Human-readable label shown in the UI.
    pub label: String,
    /// Optional description shown as subtitle.
    pub description: Option<String>,
    /// Current value formatted as a display string.
    pub value: String,
}

/// Display snapshot for a single loaded Mantle plugin.
pub struct PluginEntry {
    /// Plugin's stable ID string.
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Semver version string.
    pub version: String,
    /// Author / maintainer name.
    pub author: String,
    /// Short description shown as row subtitle.
    pub description: String,
    /// Whether the plugin is currently enabled.
    pub enabled: bool,
    /// Plugin-declared settings, shown in the expander panel.
    pub settings: Vec<PluginSettingEntry>,
}

impl AppState {
    /// Placeholder data matching the UI concept mockup.
    /// Replace with real data from `mantle_core` once the backend is wired up.
    // clippy::too_many_lines: this is a temporary fixture that initialises an
    // entire AppState inline; it will be removed when state_worker replaces it.
    #[allow(clippy::too_many_lines)]
    pub fn placeholder() -> Self {
        Self {
            // Skyrim SE App ID used as placeholder; real detection happens via
            // game::detect_all_steam() in state_worker.
            steam_app_id: Some(489_830),
            game_name: "Skyrim Special Edition".to_string(),
            game_version: "v1.6.1170".to_string(),
            launch_target: "SKSE64".to_string(),
            active_profile: "Survival Playthrough".to_string(),
            mod_count: 147,
            plugin_count: 89,
            conflict_count: 12,
            overlay_backend: "kernel 6.11".to_string(),
            mods: vec![
                ModEntry { mod_id: 0, profile_id: 0, name: "SKSE64".to_string(), enabled: true, version: Some("v2.2.6".to_string()), has_conflict: false },
                ModEntry { mod_id: 0, profile_id: 0, name: "Address Library for SKSE".to_string(), enabled: true, version: Some("v11".to_string()), has_conflict: false },
                ModEntry { mod_id: 0, profile_id: 0, name: "SkyUI".to_string(), enabled: true, version: Some("v5.2".to_string()), has_conflict: false },
                ModEntry { mod_id: 0, profile_id: 0, name: "Static Mesh Improvement Mod".to_string(), enabled: true, version: None, has_conflict: true },
                ModEntry { mod_id: 0, profile_id: 0, name: "Noble Skyrim HD-2K".to_string(), enabled: true, version: None, has_conflict: true },
            ],
            profiles: vec![
                ProfileEntry {
                    id: "survival-playthrough".to_string(),
                    name: "Survival Playthrough".to_string(),
                    mod_count: 147,
                    active: true,
                },
                ProfileEntry {
                    id: "vanilla-plus".to_string(),
                    name: "Vanilla+".to_string(),
                    mod_count: 23,
                    active: false,
                },
                ProfileEntry {
                    id: "testing".to_string(),
                    name: "Testing".to_string(),
                    mod_count: 5,
                    active: false,
                },
            ],
            downloads: vec![
                DownloadEntry {
                    id: "dl-1".to_string(),
                    name: "Requiem 5.4.0".to_string(),
                    state: DownloadState::InProgress(0.67),
                },
                DownloadEntry {
                    id: "dl-2".to_string(),
                    name: "SkyUI 5.2SE".to_string(),
                    state: DownloadState::Complete,
                },
                DownloadEntry {
                    id: "dl-3".to_string(),
                    name: "WICO — Windsong".to_string(),
                    state: DownloadState::Queued,
                },
                DownloadEntry {
                    id: "dl-4".to_string(),
                    name: "Immersive Armors SE".to_string(),
                    state: DownloadState::Failed("Connection timeout".to_string()),
                },
            ],
            game_data_path: None,
            plugins: vec![
                PluginEntry {
                    id: "skse-installer".to_string(),
                    name: "SKSE Installer".to_string(),
                    version: "1.2.0".to_string(),
                    author: "MO2 Linux".to_string(),
                    description: "Automatically copies SKSE64 binaries into the game directory before launch.".to_string(),
                    enabled: true,
                    settings: vec![
                        PluginSettingEntry {
                            key: "auto_update".to_string(),
                            label: "Auto-update SKSE".to_string(),
                            description: Some("Re-copy SKSE binaries on every launch.".to_string()),
                            value: "true".to_string(),
                        },
                        PluginSettingEntry {
                            key: "backup_originals".to_string(),
                            label: "Backup original files".to_string(),
                            description: None,
                            value: "false".to_string(),
                        },
                    ],
                },
                PluginEntry {
                    id: "conflict-reporter".to_string(),
                    name: "Conflict Reporter".to_string(),
                    version: "0.9.1".to_string(),
                    author: "Example".to_string(),
                    description: "Writes a human-readable conflict report to the plugin data directory after each VFS mount.".to_string(),
                    enabled: true,
                    settings: vec![
                        PluginSettingEntry {
                            key: "output_format".to_string(),
                            label: "Output format".to_string(),
                            description: Some("File format for the generated report.".to_string()),
                            value: "markdown".to_string(),
                        },
                    ],
                },
                PluginEntry {
                    id: "launch-logger".to_string(),
                    name: "Launch Logger".to_string(),
                    version: "1.0.0".to_string(),
                    author: "Community".to_string(),
                    description: "Logs each game launch with timestamp, profile, and mod count.".to_string(),
                    enabled: false,
                    settings: vec![],
                },
            ],
        }
    }
}
