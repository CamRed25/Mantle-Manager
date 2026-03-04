//! Background state loader — bridges `mantle_core` data into the UI thread.
//!
//! Spawns a `std::thread` that opens the `SQLite` database, reads profile
//! and mod data, and delivers an [`AppState`] snapshot over a
//! [`gtk4::glib::Sender`].
//!
//! # Delivery model
//! Currently delivers a **single snapshot** on application startup.  Live
//! updates (re-send when profile changes, mods are enabled/disabled, etc.)
//! require wiring the plugin [`EventBus`] into this thread and are deferred
//! to item z / the full signal wiring pass.
//!
//! # First launch
//! If neither the database nor the config file exist yet, this worker creates
//! them (the DB is initialised with all schema migrations).  The resulting
//! [`AppState`] reflects an empty setup — no profiles, no mods, no downloads
//! — and the UI will show its empty-state widgets.
//!
//! # Deferred fields
//! The following [`AppState`] fields are still incomplete:
//! - `game_version` — requires reading the game EXE or Steam manifest.
//! - `launch_target` — currently mirrors `game_name`; proper xSE detection deferred.
//! - `downloads` — the download queue lives in-memory; no DB persistence yet.
//!
//! [`EventBus`]: mantle_core::plugin::event::EventBus

use std::sync::mpsc::Sender;

use mantle_core::{
    config::{default_db_path, AppSettings},
    data::{profiles, Database},
    game,
    mod_list,
    vfs,
};

use crate::state::{AppState, ModEntry, ProfileEntry};

// ─── Public API ───────────────────────────────────────────────────────────────

/// Spawn a background OS thread that loads the initial [`AppState`] from
/// `mantle_core` and sends it via `sender`.
///
/// The thread exits after delivering the snapshot.  No live-update loop is
/// started here; that is deferred to item z.
///
/// # Parameters
/// - `sender`: The sending end of a `std::sync::mpsc::channel::<AppState>`.
///   The corresponding receiver must already be registered with
///   `glib::idle_add_local` before this function is called to avoid missing
///   the delivery.
///
/// # Side Effects
/// Spawns one OS thread.  The thread opens (or creates) the `SQLite`
/// database and reads profiles, mods, and config.
pub fn spawn(sender: Sender<AppState>) {
    std::thread::spawn(move || match load_state() {
        Ok(state) => {
            if sender.send(state).is_err() {
                tracing::warn!(
                    "state_worker: receiver dropped before initial state was delivered"
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                "state_worker: failed to load initial state, UI will keep placeholder: {e}"
            );
        }
    });
}

// ─── Private implementation ───────────────────────────────────────────────────

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
    let _settings: AppSettings = {
        use mantle_core::config::default_settings_path;
        AppSettings::load_or_default(&default_settings_path()).unwrap_or_default()
    };

    // ── Profiles ──────────────────────────────────────────────────────────
    let all_profiles = db.with_conn(profiles::list_profiles)?;
    let active = db.with_conn(profiles::get_active_profile)?;

    let active_profile_name = active
        .as_ref()
        .map_or_else(String::new, |p| p.name.clone());
    let active_profile_id = active.as_ref().map(|p| p.id);

    // Batch-query mod counts for all profiles in one round-trip so the
    // sidebar can show "{n} mods" next to each profile without N extra queries.
    let profile_mod_counts = db
        .with_conn(mod_list::mod_counts_per_profile)
        .unwrap_or_default();

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

    // ── Assemble snapshot ─────────────────────────────────────────────────
    // Game detection: run detect_all_steam() and pick the first result.
    // If Steam is not installed or no supported game is found, all game
    // fields are left empty and the launch button is disabled.
    // ── Game detection ─────────────────────────────────────────────────────
    // run detect_all_steam() and pick the first result.  If Steam is not
    // installed or no supported title is found, game fields stay empty and
    // the launch button is disabled.
    let detected = game::detect_all_steam().unwrap_or_default();
    let first_game = detected.into_iter().next();

    let (steam_app_id, game_name, launch_target) = if let Some(ref g) = first_game {
        (
            Some(g.steam_app_id),
            g.name.clone(),
            // Use the game's short display name as the launch target until
            // xSE / custom launch-target detection is added (deferred).
            g.name.clone(),
        )
    } else {
        (None, String::new(), String::new())
    };

    // ── Plugin registry ───────────────────────────────────────────────────
    let profile_names: Vec<String> = all_profiles.iter().map(|p| p.name.clone()).collect();
    let (plugin_entries, plugin_count) = load_plugins(
        &db,
        active_profile_id,
        &active_profile_name,
        &profile_names,
        first_game.as_ref(),
    );

    Ok(AppState {
        steam_app_id,
        game_name,
        // Version string requires reading the game EXE or a manifest;
        // deferred to the game-version detection pass.
        game_version: String::new(),
        launch_target,
        active_profile: active_profile_name,
        mod_count,
        plugin_count,
        conflict_count,
        overlay_backend: vfs::select_backend().to_string(),
        mods: mod_entries,
        profiles: profile_entries,
        // Download queue lives in-memory only; no DB persistence yet.
        downloads: vec![],
        plugins: plugin_entries,
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
    let id_to_ci: HashMap<i64, usize> = enabled_mods
        .iter()
        .enumerate()
        .map(|(i, m)| (m.mod_id, i))
        .collect();

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
            let has = !matches!(
                conflict_map.role_of_mod(&m.mod_slug),
                conflict::ModRole::Clean
            );
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
            // mod_version not stored in profile_mods yet; deferred until
            // the archive extraction layer populates mods.metadata.
            version: None,
            has_conflict: conflict_by_id.get(&m.mod_id).copied().unwrap_or(false),
        })
        .collect();

Ok((entries, count, total_file_conflicts))
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
            context::{SettingValue},
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
            // version and author not stored in profile_mods; left empty.
            version: String::new(),
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