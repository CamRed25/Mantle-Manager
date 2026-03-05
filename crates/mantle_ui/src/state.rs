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
    /// Themes available in the Themes tab (built-in + user-installed).
    pub themes: Vec<ThemeEntry>,
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

/// Status of a single download job.
///
/// Carried by [`DownloadEntry`] and pushed via the progress channel when
/// background tasks report updates.
#[derive(Clone)]
pub enum DownloadStatus {
    /// Waiting in the queue — not yet started.  Reserved for Tier 3/g HTTP
    /// download implementation; the queue stub currently skips this state.
    #[allow(dead_code)]
    Queued,
    /// Download is running.
    ///
    /// - `progress`    — fraction complete in `0.0..=1.0`.
    /// - `bytes_done`  — bytes received so far.
    /// - `total_bytes` — total expected bytes, if the server sent
    ///   `Content-Length`.
    #[allow(dead_code)] // fields used when HTTP fetch is implemented
    InProgress {
        progress: f64,
        bytes_done: u64,
        total_bytes: Option<u64>,
    },
    /// Download finished successfully.
    ///
    /// `bytes` is the total number of bytes written to disk.
    #[allow(dead_code)] // field used when HTTP fetch is implemented
    Complete { bytes: u64 },
    /// Download stopped due to an error; contains a short error description.
    Failed(String),
    /// Download was cancelled by the user before it completed.
    Cancelled,
}

pub struct DownloadEntry {
    /// Stable UUID string — used for cancel / retry / clear action dispatch.
    pub id: String,
    pub name: String,
    pub state: DownloadStatus,
}

// ─── Theme types ──────────────────────────────────────────────────────────────

/// Display snapshot for a single theme — built-in or user-installed.
pub struct ThemeEntry {
    /// Stable ID.  For built-in themes this is the kebab-case name
    /// (e.g. `"catppuccin-mocha"`); for user themes it is the CSS filename stem.
    pub id: String,
    /// Human-readable display name (from manifest or falls back to id).
    pub name: String,
    /// Author / maintainer name (empty for built-in themes).
    pub author: String,
    /// Short description (empty when not specified).
    pub description: String,
    /// Colour scheme hint: `"dark"`, `"light"`, or `"auto"`.
    pub color_scheme: String,
    /// Full CSS content — passed to `apply_theme` when activated.
    pub css: String,
    /// Whether this is the currently active theme.
    pub active: bool,
    /// `true` for themes shipped with Mantle Manager that cannot be deleted.
    pub builtin: bool,
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
    /// Minimal placeholder shown for the brief window between startup and the
    /// first live [`AppState`] delivery from `state_worker`.
    ///
    /// All fields are empty / `None` / zero — no fake data is shown.  The
    /// Overview page renders its "Welcome" status page (profiles empty),
    /// the launch button starts disabled (`steam_app_id` None), and all lists
    /// are empty.  This prevents any misleading info flashing before the real
    /// data arrives.
    pub fn placeholder() -> Self {
        Self {
            steam_app_id: None,
            game_name: String::new(),
            game_version: String::new(),
            launch_target: String::new(),
            active_profile: String::new(),
            mod_count: 0,
            plugin_count: 0,
            conflict_count: 0,
            overlay_backend: String::new(),
            mods: vec![],
            profiles: vec![],
            downloads: vec![],
            themes: vec![],
            game_data_path: None,
            plugins: vec![],
        }
    }
}
