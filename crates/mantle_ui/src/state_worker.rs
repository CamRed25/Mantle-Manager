//! Background state loader — bridges `mantle_core` data into the UI thread.
//!
//! Spawns a `std::thread` that opens the `SQLite` database, reads profile
//! and mod data, and delivers an [`AppState`] snapshot over a
//! [`gtk4::glib::Sender`].
//!
//! # Delivery model
//! Delivers an initial snapshot on startup, then stays alive subscribed to
//! the shared [`EventBus`].  Whenever a [`ModManagerEvent`] matching any of
//! `ModInstalled`, `ModEnabled`, `ModDisabled`, or `ProfileChanged` is
//! published on the bus, the thread re-runs [`load_state`] and sends a fresh
//! [`AppState`] snapshot to the UI thread.
//!
//! # First launch
//! If neither the database nor the config file exist yet, this worker creates
//! them (the DB is initialised with all schema migrations).  The resulting
//! [`AppState`] reflects an empty setup — no profiles, no mods, no downloads
//! — and the UI will show its empty-state widgets.
//!
//! # Deferred fields
//! The following [`AppState`] fields are still incomplete.
//! See futures.md "State worker detection" for implementation notes.
//! - `downloads` — the download queue lives in-memory; no DB persistence yet.
//!
//! [`EventBus`]: mantle_core::plugin::EventBus
//! [`ModManagerEvent`]: mantle_core::plugin::ModManagerEvent

use std::sync::{mpsc::Sender, Arc, Condvar, Mutex, OnceLock};

use mantle_core::{
    config::{default_db_path, AppSettings},
    data::{
        profiles::{self, InsertProfile},
        Database,
    },
    game, mod_list,
    plugin::{EventBus, EventFilter},
    vfs,
};

use crate::state::{AppState, DownloadEntry, DownloadStatus, ModEntry, ProfileEntry, ThemeEntry};

// ─── Cached game detection ───────────────────────────────────────────────────

/// Result of the one-time Steam game scan, cached for the lifetime of the
/// process.
///
/// `detect_all_steam` scans the filesystem and registry; running it on every
/// state refresh would add latency to every mod-toggle or profile-switch.
/// The first detected game is almost never going to change during a session.
static CACHED_GAME: OnceLock<Option<mantle_core::game::GameInfo>> = OnceLock::new();

/// Return the first detected Steam game, running the scan at most once.
///
/// Subsequent calls return a reference to the cached result without touching
/// the filesystem.  The cache is intentionally process-scoped (`OnceLock`)
/// rather than invalidated on refresh — game installs and Steam re-scans are
/// expected to require an application restart.
fn load_game_state() -> Option<&'static mantle_core::game::GameInfo> {
    CACHED_GAME
        .get_or_init(|| game::detect_all_steam().unwrap_or_default().into_iter().next())
        .as_ref()
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Trigger a one-shot state reload without starting a new subscription loop.
///
/// Spawns a short-lived OS thread that runs [`load_state`] once and delivers
/// the result over `sender`.  Use this for `refresh_fn` callbacks that need
/// to force an immediate UI update after a DB-mutating user action.
///
/// Unlike [`spawn`], the spawned thread exits as soon as the state is sent
/// (or the send fails).  It does **not** subscribe to the event bus.
///
/// # Parameters
/// - `sender`: The sending end of a `std::sync::mpsc::channel::<AppState>`.
pub fn trigger_reload(sender: Sender<AppState>) {
    std::thread::spawn(move || resend_state(&sender));
}

/// Spawn a background OS thread that loads the initial [`AppState`] from
/// `mantle_core`, sends it via `sender`, then stays alive subscribed to the
/// shared [`EventBus`] to re-send the state whenever anything changes.
///
/// The thread is kept alive indefinitely using a `Condvar` that is never
/// notified.  All four [`SubscriptionHandle`]s are held inside the closure so
/// they are dropped only when the thread exits (i.e. when the process shuts
/// down), which means the handlers remain active for the full session.
///
/// # Parameters
/// - `sender`: The sending end of a `std::sync::mpsc::channel::<AppState>`.
///   The corresponding receiver must already be registered with
///   `glib::idle_add_local` before this function is called to avoid missing
///   the delivery.
/// - `event_bus`: Shared event bus — the worker subscribes to
///   `ModInstalled`, `ModEnabled`, `ModDisabled`, and `ProfileChanged`.
///
/// # Side Effects
/// Spawns one OS thread.  The thread opens (or creates) the `SQLite`
/// database and reads profiles, mods, and config on every delivery.
///
/// # Notes
/// The returned `JoinHandle` is intentionally dropped; the thread stays alive
/// until the process exits.
///
/// [`SubscriptionHandle`]: mantle_core::plugin::SubscriptionHandle
pub fn spawn(sender: Sender<AppState>, event_bus: Arc<EventBus>) {
    std::thread::spawn(move || {
        // ── Initial state load ────────────────────────────────────────────
        resend_state(&sender);

        // ── Subscribe to live events ──────────────────────────────────────
        // Each handler clones the sender and calls resend_state.
        // The SubscriptionHandle variables must be kept alive — dropping them
        // would immediately unsubscribe.
        let s1 = sender.clone();
        let _sub_mod_installed =
            EventBus::subscribe(&event_bus, EventFilter::ModInstalled, move |_| {
                resend_state(&s1);
            });

        let s2 = sender.clone();
        let _sub_mod_enabled =
            EventBus::subscribe(&event_bus, EventFilter::ModEnabled, move |_| {
                resend_state(&s2);
            });

        let s3 = sender.clone();
        let _sub_mod_disabled =
            EventBus::subscribe(&event_bus, EventFilter::ModDisabled, move |_| {
                resend_state(&s3);
            });

        let s4 = sender.clone();
        let _sub_profile_changed =
            EventBus::subscribe(&event_bus, EventFilter::ProfileChanged, move |_| {
                resend_state(&s4);
            });

        // ── Keep thread alive ─────────────────────────────────────────────
        // Block forever so the SubscriptionHandles above are not dropped.
        // Handlers are called from the EventBus's internal lock, so they do
        // not depend on this thread being runnable.
        let mutex = Mutex::new(());
        let condvar = Condvar::new();
        let guard = mutex.lock().expect("state_worker Mutex poisoned");
        drop(condvar.wait(guard));
    });
}

// ─── Private implementation ───────────────────────────────────────────────────

/// Re-run [`load_state`] and send the result over `sender`.
///
/// Called both for the initial delivery (inside [`spawn`]) and from every
/// event-bus subscription handler to push a fresh snapshot to the UI whenever
/// the manager's state changes.
///
/// Errors are logged as warnings and do not panic, keeping the UI functional
/// even if a transient DB read fails.
///
/// # Parameters
/// - `sender`: The sending end of the state MPSC channel.
fn resend_state(sender: &Sender<AppState>) {
    match load_state() {
        Ok(state) => {
            if sender.send(state).is_err() {
                tracing::warn!("state_worker: receiver dropped; state delivery skipped");
            }
        }
        Err(e) => {
            tracing::warn!("state_worker: resend failed — {e}");
        }
    }
}

/// Load an [`AppState`] snapshot from the database and config files.
///
/// Opens (or creates) the `SQLite` database, reads all profiles, then loads
/// the active profile's mod list.  Returns an error only if a critical read
/// fails; absent files (first launch) produce an empty but valid state.
///
/// # Returns
/// Populated [`AppState`] on success.
///
/// # Errors
/// Returns an error if the database cannot be opened or schema migrations
/// fail.  Individual query errors are treated as empty results rather than
/// propagated, to keep the UI functional even with partial DB corruption.
fn load_state() -> anyhow::Result<AppState> {
    // ── Database ──────────────────────────────────────────────────────────
    let db_path = default_db_path();
    if let Some(parent) = db_path.parent() {
        // Create data directory on first launch (no-op if it already exists).
        std::fs::create_dir_all(parent)?;
    }
    let db = Database::open(&db_path)?;

    // ── Config (for future game detection — currently unused here) ────────
    // AppSettings::load_or_default is called at startup for theme only.
    // Keeping this call so future items can read game paths etc.
    let settings: AppSettings = {
        use mantle_core::config::default_settings_path;
        AppSettings::load_or_default(&default_settings_path()).unwrap_or_default()
    };

    // ── Profiles — first-run bootstrap ────────────────────────────────────
    // If the database is brand new, no profiles exist.  Create a "Default"
    // profile and make it active so the rest of load_state() always has
    // something to work with, and the UI can show the mods page immediately
    // rather than a perpetual empty state.
    let first_run = db.with_conn(profiles::list_profiles)?.is_empty();
    if first_run {
        tracing::info!("state_worker: no profiles found — creating Default profile");
        let default_id = db.with_conn(|conn| {
            profiles::insert_profile(
                conn,
                &InsertProfile {
                    name: "Default",
                    game_slug: None,
                },
            )
        })?;
        db.with_conn(|conn| profiles::set_active_profile(conn, default_id))?;
    }

    let all_profiles = db.with_conn(profiles::list_profiles)?;
    let active = db.with_conn(profiles::get_active_profile)?;

    let active_profile_name = active.as_ref().map_or_else(String::new, |p| p.name.clone());
    let active_profile_id = active.as_ref().map(|p| p.id);

    // Batch-query mod counts for all profiles in one round-trip so the
    // sidebar can show "{n} mods" next to each profile without N extra queries.
    let profile_mod_counts = db.with_conn(mod_list::mod_counts_per_profile).unwrap_or_default();

    let profile_entries: Vec<ProfileEntry> = all_profiles
        .iter()
        .map(|p| ProfileEntry {
            id: p.id.to_string(),
            name: p.name.clone(),
            mod_count: profile_mod_counts.get(&p.id).copied().unwrap_or(0),
            active: p.is_active,
        })
        .collect();

    // ── Active profile mod list + conflict scan ───────────────────────────
    let (mod_entries, mod_count, conflict_count) = if let Some(pid) = active_profile_id {
        build_mod_list_with_conflicts(&db, pid)?
    } else {
        (vec![], 0_usize, 0_usize)
    };

    // ── Game detection (cached) ────────────────────────────────────────────
    // `load_game_state` runs `detect_all_steam` at most once per process;
    // subsequent refreshes (mod enable, profile switch, etc.) reuse the
    // static cache, keeping the common code-path free of filesystem I/O.
    let first_game = load_game_state();

    let (steam_app_id, game_name, launch_target) = if let Some(g) = first_game {
        // Detect SKSE/F4SE/etc. and use it as the launch target when installed.
        // Only available when the `net` feature is enabled (SKSE module is
        // gated on net).  Falls back to the game display name when off.
        #[cfg(feature = "net")]
        let launch_target = mantle_core::skse::config_for_game(g.kind)
            .and_then(|cfg| mantle_core::skse::installed_version(&g.install_path, cfg))
            .map(|_| format!("{} via SKSE", g.name))
            .unwrap_or_else(|| g.name.clone());
        #[cfg(not(feature = "net"))]
        let launch_target = g.name.clone();

        (Some(g.steam_app_id), g.name.clone(), launch_target)
    } else {
        (None, String::new(), String::new())
    };

    // ── Plugin registry ───────────────────────────────────────────────────
    let profile_names: Vec<String> = all_profiles.iter().map(|p| p.name.clone()).collect();
    let (plugin_entries, plugin_count) =
        load_plugins(&db, active_profile_id, &active_profile_name, &profile_names, first_game);

    Ok(AppState {
        steam_app_id,
        game_name,
        // Read the game version from the Steam ACF manifest or PE resource.
        game_version: first_game
            .map(|g| mantle_core::game::version::read_game_version(g))
            .unwrap_or_default(),
        launch_target,
        active_profile: active_profile_name,
        mod_count,
        plugin_count,
        conflict_count,
        overlay_backend: vfs::select_backend().to_string(),
        mods: mod_entries,
        profiles: profile_entries,
        // Load active (non-completed) downloads from the DB so the UI
        // displays their last-known status from the previous session.
        downloads: load_downloads_snapshot(&db),
        plugins: plugin_entries,
        themes: load_themes(&settings.ui.theme),
        // Data directory used as the VFS merge_dir target during launch mount.
        game_data_path: first_game.as_ref().map(|g| g.data_path.clone()),
    })
}
/// Load the active profile's mod list, run the file-level conflict scan, and
/// return `(entries, mod_count, conflict_count)`.
///
/// Extracted from [`load_state`] to keep that function under the 100-line
/// clippy limit.  All conflict scan errors are non-fatal — a query failure
/// results in empty file manifests and therefore zero reported conflicts,
/// which is an acceptable degraded state.
///
/// # Parameters
/// - `db`: Open database to query.
/// - `profile_id`: Active profile's primary key.
///
/// # Errors
/// Returns an error only if loading the `profile_mods` rows themselves fails.
fn build_mod_list_with_conflicts(
    db: &mantle_core::data::Database,
    profile_id: i64,
) -> anyhow::Result<(Vec<ModEntry>, usize, usize)> {
    use mantle_core::{
        conflict::{self, ModEntry as ConflictEntry},
        data::mod_files::all_paths_for_enabled_mods_in_profile,
    };
    use std::collections::HashMap;

    let profile_mods = db.with_conn(|conn| mod_list::list_profile_mods(conn, profile_id))?;
    let count = profile_mods.len();

    // Only enabled mods participate in conflict detection; disabled mods
    // are not mounted by the VFS layer and cannot win or lose conflicts.
    let enabled_mods: Vec<_> = profile_mods.iter().filter(|m| m.is_enabled).collect();

    // Map mod_id → index in conflict_entries (preserves priority order).
    let id_to_ci: HashMap<i64, usize> =
        enabled_mods.iter().enumerate().map(|(i, m)| (m.mod_id, i)).collect();

    let mut conflict_entries: Vec<ConflictEntry> = enabled_mods
        .iter()
        .map(|m| ConflictEntry {
            id: m.mod_slug.clone(),
            files: vec![],
        })
        .collect();

    // Load all file paths for enabled mods in priority + path order.
    // A query failure is non-fatal: empty file lists → no false conflicts.
    let file_rows = db
        .with_conn(|conn| all_paths_for_enabled_mods_in_profile(conn, profile_id))
        .unwrap_or_default();

    for (mid, path) in file_rows {
        if let Some(&ci) = id_to_ci.get(&mid) {
            conflict_entries[ci].files.push(path);
        }
    }

    let conflict_map = conflict::detect(&conflict_entries);
    let total_file_conflicts = conflict_map.total_file_conflicts();

    // Per-mod has_conflict: any role other than Clean (Winner / Loser / Both)
    // means this mod participates in at least one file-level conflict.
    let conflict_by_id: HashMap<i64, bool> = enabled_mods
        .iter()
        .map(|m| {
            let has = !matches!(conflict_map.role_of_mod(&m.mod_slug), conflict::ModRole::Clean);
            (m.mod_id, has)
        })
        .collect();

    let entries = profile_mods
        .into_iter()
        .map(|m| ModEntry {
            mod_id: m.mod_id,
            profile_id,
            name: m.mod_name,
            enabled: m.is_enabled,
            // mods.version is populated by the archive extraction install
            // pipeline when the mod is installed from an archive.
            version: m.version.clone(),
            has_conflict: conflict_by_id.get(&m.mod_id).copied().unwrap_or(false),
        })
        .collect();

    Ok((entries, count, total_file_conflicts))
}

/// Load non-completed downloads from the DB and convert to [`DownloadEntry`]
/// snapshots for initial UI display.
///
/// Errors are non-fatal: a failed query returns an empty list so the rest
/// of the state can still load normally.
///
/// # Parameters
/// - `db`: Open application database.
///
/// # Returns
/// List of download entries ordered by insertion time (oldest first).
fn load_downloads_snapshot(db: &mantle_core::data::Database) -> Vec<DownloadEntry> {
    let rows = db.with_conn(|conn| {
        mantle_core::data::downloads::load_active_downloads(conn)
            .unwrap_or_default()
    });

    rows.into_iter()
        .map(|d| DownloadEntry {
            id: d.id,
            name: d.filename,
            state: persisted_status_to_download_status(&d.status, d.progress, d.total_bytes),
        })
        .collect()
}

/// Convert a persisted status string + ancillary data to a [`DownloadStatus`].
///
/// # Parameters
/// - `status`:      Status string from the `downloads` table.
/// - `progress`:    Stored progress value `[0.0, 1.0]`.
/// - `total_bytes`: Stored total byte count, if known.
fn persisted_status_to_download_status(
    status: &str,
    progress: f64,
    total_bytes: Option<u64>,
) -> DownloadStatus {
    match status {
        "queued" => DownloadStatus::Queued,
        "in_progress" => DownloadStatus::InProgress {
            progress,
            bytes_done: (total_bytes.unwrap_or(0) as f64 * progress) as u64,
            total_bytes,
        },
        "complete" => DownloadStatus::Complete {
            bytes: total_bytes.unwrap_or(0),
        },
        "failed" => DownloadStatus::Failed("Interrupted by restart".to_string()),
        "cancelled" => DownloadStatus::Cancelled,
        other => {
            tracing::warn!(status = other, "unknown persisted download status; treating as failed");
            DownloadStatus::Failed(format!("Unknown status: {other}"))
        }
    }
}

/// Build the full theme list: built-in entries first, then user-installed.
///
/// Built-in CSS themes (Catppuccin, Nord, Skyrim, Fallout) are always present
/// so users can see reference implementations and apply them without going to
/// the Settings dialog.  User themes are discovered from the themes directory.
///
/// # Parameters
/// - `active_theme`: Current saved [`Theme`] variant; used to set
///   [`ThemeEntry::active`] on the matching entry.
fn load_themes(active_theme: &mantle_core::config::Theme) -> Vec<ThemeEntry> {
    use crate::settings::{
        builtin_id_to_theme, CATPPUCCIN_LATTE_CSS, CATPPUCCIN_MOCHA_CSS, FALLOUT_CSS, NORD_CSS,
        SKYRIM_CSS,
    };
    use mantle_core::config::Theme;

    let mut themes: Vec<ThemeEntry> = Vec::new();

    // ── Built-in themes ───────────────────────────────────────────────────────
    let builtins: &[(&str, &str, &str, &str, &str, &str)] = &[
        (
            "catppuccin-mocha",
            "Catppuccin Mocha",
            "Catppuccin",
            "Soothing pastel theme — dark variant.",
            "dark",
            CATPPUCCIN_MOCHA_CSS,
        ),
        (
            "catppuccin-latte",
            "Catppuccin Latte",
            "Catppuccin",
            "Soothing pastel theme — light variant.",
            "light",
            CATPPUCCIN_LATTE_CSS,
        ),
        (
            "nord",
            "Nord",
            "Arctic Ice Studio",
            "An arctic, north-bluish colour palette.",
            "dark",
            NORD_CSS,
        ),
        (
            "skyrim",
            "Skyrim",
            "Mantle Team",
            "Nordic dark theme inspired by The Elder Scrolls V.",
            "dark",
            SKYRIM_CSS,
        ),
        (
            "fallout",
            "Fallout",
            "Mantle Team",
            "Retro terminal green-on-black inspired by Fallout.",
            "dark",
            FALLOUT_CSS,
        ),
    ];

    for &(id, name, author, description, color_scheme, css) in builtins {
        let is_active = builtin_id_to_theme(id).is_some_and(|t| t == *active_theme);
        themes.push(ThemeEntry {
            id: id.to_string(),
            name: name.to_string(),
            author: author.to_string(),
            description: description.to_string(),
            color_scheme: color_scheme.to_string(),
            css: css.to_string(),
            active: is_active,
            builtin: true,
        });
    }

    // ── User-installed themes ─────────────────────────────────────────────────
    let themes_dir = mantle_core::theme::themes_data_dir(&mantle_core::config::data_dir());
    let active_custom_id: Option<&str> = if let Theme::Custom(ref id) = active_theme {
        Some(id.as_str())
    } else {
        None
    };

    for t in mantle_core::theme::scan_themes_dir(&themes_dir) {
        let is_active = active_custom_id.is_some_and(|aid| aid == t.id);
        themes.push(ThemeEntry {
            id: t.id,
            name: t.name,
            author: t.author,
            description: t.description,
            color_scheme: t.color_scheme,
            css: t.css,
            active: is_active,
            builtin: false,
        });
    }

    themes
}

/// Scan `{data_dir}/plugins/`, load every `.so` and `.rhai` plugin, and return
/// UI-ready [`PluginEntry`] snapshots.
///
/// Non-fatal: per-plugin load failures are logged as warnings; a missing
/// plugins directory is silently treated as "no plugins installed."
///
/// # Parameters
/// - `db`: Open database used to load the active profile's mod list for each
///   [`PluginContext`].
/// - `active_profile_id`: Active profile primary key (`None` → empty mod list).
/// - `active_profile`: Human-readable name of the active profile.
/// - `profile_names`: Names of all profiles (passed to each context).
/// - `game`: Detected game, if any.
///
/// # Returns
/// `(plugin_entries, plugin_count)` — count reflects successfully loaded plugins.
fn load_plugins(
    db: &mantle_core::data::Database,
    active_profile_id: Option<i64>,
    active_profile: &str,
    profile_names: &[String],
    game: Option<&mantle_core::game::GameInfo>,
) -> (Vec<crate::state::PluginEntry>, usize) {
    use mantle_core::{
        config::data_dir,
        plugin::{
            context::SettingValue,
            event::{EventBus, ModInfo},
            registry::PluginRegistry,
        },
    };
    use std::sync::Arc;

    // Build mod list snapshot for each PluginContext.
    let mod_infos: Vec<ModInfo> = active_profile_id
        .and_then(|pid| db.with_conn(|conn| mod_list::list_profile_mods(conn, pid)).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|m| ModInfo {
            id: m.mod_id,
            slug: m.mod_slug,
            name: m.mod_name,
            // version is now populated from mods.version; author is not stored
            // in profile_mods and remains empty until a metadata pass is added.
            version: m.version.unwrap_or_default(),
            author: String::new(),
            priority: m.priority,
            is_enabled: m.is_enabled,
            install_dir: m.install_dir,
        })
        .collect();

    let base_data = data_dir();
    let plugins_dir = base_data.join("plugins");
    let event_bus = Arc::new(EventBus::new());
    let mut registry = PluginRegistry::new(Arc::clone(&event_bus), &base_data);

    for err in registry.load_dir(&plugins_dir, &mod_infos, active_profile, profile_names, game) {
        tracing::warn!("plugin load error: {err:?}");
    }

    let count = registry.plugin_count();
    // Collect IDs to owned strings first so `registry` is free to be
    // re-borrowed for `get()` lookup without a live iterator borrow.
    let ids: Vec<String> = registry.plugin_ids().map(str::to_owned).collect();
    let entries = ids
        .iter()
        .filter_map(|id| registry.get(id))
        .map(|p| {
            let settings = p
                .settings()
                .into_iter()
                .map(|s| crate::state::PluginSettingEntry {
                    key: s.key.to_string(),
                    label: s.label.to_string(),
                    description: s.description.map(str::to_string),
                    value: match &s.default {
                        SettingValue::Bool(b) => b.to_string(),
                        SettingValue::String(sv) => sv.clone(),
                        SettingValue::Int(i) => i.to_string(),
                        SettingValue::Float(f) => f.to_string(),
                    },
                })
                .collect();
            crate::state::PluginEntry {
                id: p.id().to_string(),
                name: p.name().to_string(),
                version: p.version().to_string(),
                author: p.author().to_string(),
                description: p.description().to_string(),
                // All loaded plugins are treated as enabled;
                // per-plugin enable/disable is a future feature.
                enabled: true,
                settings,
            }
        })
        .collect();

    (entries, count)
}
