//! Plugin registry — load/unload lifecycle for native and scripted plugins.
//!
//! # Responsibilities
//! - Scan the `plugins/` directory for `.so` (native) and `.rhai` (scripted)
//!   files at application startup.
//! - Load each file in alphabetical order, calling `MantlePlugin::init` with
//!   a freshly constructed [`PluginContext`].
//! - Enforce unique plugin IDs; reject duplicates with a warning.
//! - Allow graceful shutdown via [`PluginRegistry::unload_all`].
//! - Collect per-plugin load failures without interrupting other plugins.
//!
//! # Thread safety
//! [`PluginRegistry`] is not `Sync`. The caller is responsible for placing it
//! behind a `Mutex` or similar guard when shared across threads.
//!
//! # References
//! - `standards/PLUGIN_API.md` §6 — plugin discovery and load ordering
//! - `standards/PLUGIN_API.md` §7 — capability system
//! - `standards/PLUGIN_API.md` §8 — error handling contract

use std::{
    collections::{HashMap, HashSet},
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde::Deserialize;
use tracing::{info, warn};

use crate::{error::MantleError, game::GameInfo};

use super::{
    native::load_native_plugin, scripted::load_scripted_plugin, Capability, EventBus,
    EventFilter, MantlePlugin, ModInfo, ModManagerEvent, PluginContext, PluginError, SettingValue,
    SubscriptionHandle,
};

// ─── plugin.toml manifest types ───────────────────────────────────────────────

/// Deserialised representation of an optional `plugin.toml` file placed
/// alongside a `.so` or `.rhai` plugin file.
///
/// All fields are optional; absent fields fall back to values returned by
/// the loaded `MantlePlugin` trait methods.
///
/// # Example
/// ```toml
/// id = "skse-installer"
/// name = "SKSE Installer"
/// version = "1.0.0"
/// author = "MO2 Linux"
/// description = "Installs SKSE into the game directory."
/// required_api_version = "1.0.0"
///
/// [capabilities]
/// required = ["downloads"]
/// optional = ["notifications"]
/// ```
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PluginManifest {
    /// Override plugin ID. If absent, uses `MantlePlugin::id()`.
    pub id: Option<String>,
    /// Override display name. If absent, uses `MantlePlugin::name()`.
    pub name: Option<String>,
    /// Override version string. If absent, uses `MantlePlugin::version()`.
    pub version: Option<String>,
    /// Override author string.
    pub author: Option<String>,
    /// Override description string.
    pub description: Option<String>,
    /// Minimum required plugin API version.
    pub required_api_version: Option<String>,
    /// Capability declarations shown to the user before enabling the plugin.
    #[serde(default)]
    pub capabilities: ManifestCapabilities,
}

/// Capability declarations inside `plugin.toml`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ManifestCapabilities {
    /// Capabilities the plugin *must* have to function.
    #[serde(default)]
    pub required: Vec<String>,
    /// Capabilities the plugin requests but can work without.
    #[serde(default)]
    pub optional: Vec<String>,
}

// ─── PluginLoadError ──────────────────────────────────────────────────────────

/// A non-fatal error encountered while loading a single plugin.
///
/// Load failures never prevent other plugins from loading and are never
/// fatal to the application (see `standards/PLUGIN_API.md` §8.2).
#[derive(Debug, thiserror::Error)]
pub enum PluginLoadError {
    /// The plugin file or its shared symbols could not be loaded.
    #[error("failed to load '{}': {source}", path.display())]
    LoadFailed {
        /// Path of the file that failed to load.
        path: PathBuf,
        /// Underlying engine error.
        source: MantleError,
    },

    /// `MantlePlugin::init` returned an error.
    #[error("'{}' init() failed: {source}", path.display())]
    InitFailed {
        /// Path of the plugin whose init failed.
        path: PathBuf,
        /// Error returned or caught from `init`.
        source: PluginError,
    },

    /// `MantlePlugin::init` panicked; the panic message is captured.
    #[error("'{}' init() panicked: {message}", path.display())]
    InitPanicked {
        /// Path of the plugin whose init panicked.
        path: PathBuf,
        /// Human-readable panic message (best-effort extraction).
        message: String,
    },

    /// A second plugin with the same ID was found; the later one is rejected.
    #[error("duplicate plugin ID '{id}' from '{}' — already loaded from '{}'", rejected.display(), existing.display())]
    DuplicateId {
        /// The conflicting plugin ID.
        id: String,
        /// Path of the plugin that was already accepted.
        existing: PathBuf,
        /// Path of the plugin that was rejected because of the conflict.
        rejected: PathBuf,
    },
}

// ─── LoadedPlugin (private) ───────────────────────────────────────────────────

/// A successfully loaded and initialised plugin, held by the registry.
///
/// The registry owns both the plugin object and its context. Dropping this
/// struct does **not** call `shutdown()` — callers must invoke
/// [`PluginRegistry::unload_all`] explicitly before dropping the registry.
struct LoadedPlugin {
    /// The live plugin object. For `NativePlugin` the inner `Box<dyn …>` also
    /// keeps the shared library mapped, so drop order is correct.
    plugin: Box<dyn MantlePlugin>,
    /// The context that was handed to `init`. Kept alive so the plugin can
    /// continue using it after init returns.
    context: Arc<PluginContext>,
    /// Filesystem path from which this plugin was loaded (for diagnostics).
    path: PathBuf,
}

// ─── PluginRegistry ───────────────────────────────────────────────────────────

/// Central store for all loaded Mantle plugins.
///
/// # Lifecycle
/// ```text
/// let mut reg = PluginRegistry::new(event_bus, data_dir);
/// let errors = reg.load_dir(&plugins_dir, mod_list, profile, profiles, game);
/// // application runs …
/// reg.unload_all();   // calls shutdown() on every plugin in reverse load order
/// ```
///
/// # References
/// - `standards/PLUGIN_API.md` §6   — plugin discovery
/// - `standards/PLUGIN_API.md` §7   — capability system
pub struct PluginRegistry {
    /// All successfully loaded and initialised plugins, in load order
    /// (alphabetical by filename).
    plugins: Vec<LoadedPlugin>,
    /// Shared event bus passed to every [`PluginContext`].
    event_bus: Arc<EventBus>,
    /// Application data directory (e.g. `~/.local/share/mantle-manager/`).
    ///
    /// Per-plugin writable data dirs live at
    /// `{base_data_dir}/plugin-data/{plugin_id}/`.
    base_data_dir: PathBuf,
    /// Shared list of all loaded plugin contexts.
    ///
    /// Captured by the lifecycle subscription closures so they can update
    /// every context snapshot when `ProfileChanged` or `GameLaunching`
    /// fires.  Also cleared by [`unload_all`][Self::unload_all].
    contexts: Arc<Mutex<Vec<Arc<PluginContext>>>>,
    /// Subscription handles for the host-side lifecycle hooks.
    ///
    /// Kept alive for as long as the registry is live. Dropped (and
    /// unsubscribed) by [`unload_all`][Self::unload_all].
    lifecycle_handles: Vec<SubscriptionHandle>,
}

impl PluginRegistry {
    /// Create a new, empty registry.
    ///
    /// No plugins are loaded until [`load_dir`](Self::load_dir) is called.
    ///
    /// # Parameters
    /// - `event_bus`: Shared event bus that all plugin contexts will subscribe
    ///   to. Typically the same `Arc<EventBus>` used by the rest of the app.
    /// - `base_data_dir`: Application-level data directory. Per-plugin
    ///   sub-directories are created inside here automatically.
    ///
    /// # Returns
    /// An empty `PluginRegistry` ready for `load_dir`.
    #[must_use]
    pub fn new(event_bus: Arc<EventBus>, base_data_dir: impl Into<PathBuf>) -> Self {
        Self {
            plugins: Vec::new(),
            event_bus,
            base_data_dir: base_data_dir.into(),
            contexts: Arc::new(Mutex::new(Vec::new())),
            lifecycle_handles: Vec::new(),
        }
    }

    /// Scan `plugins_dir` and load every `.so` and `.rhai` file found.
    ///
    /// Files are processed in alphabetical order. Each file is loaded,
    /// duplicate IDs are rejected, and `init()` is called exactly once.
    /// Any per-file failure is collected and returned — it never prevents
    /// other plugins from loading.
    ///
    /// # Parameters
    /// - `plugins_dir`: Directory to scan (usually `{data_dir}/plugins/`).
    ///   If the directory does not exist the scan silently returns an empty
    ///   error list.
    /// - `mod_list`: Current mod list snapshot given to each `PluginContext`.
    /// - `active_profile`: Name of the currently active profile.
    /// - `profiles`: All known profile names.
    /// - `game`: Currently detected game, if any.
    ///
    /// # Returns
    /// A `Vec` of non-fatal [`PluginLoadError`]s, one per plugin that failed.
    /// An empty vector means all plugins loaded successfully.
    ///
    /// # Side Effects
    /// Creates per-plugin data directories under
    /// `{base_data_dir}/plugin-data/{plugin_id}/` as needed.
    pub fn load_dir(
        &mut self,
        plugins_dir: &Path,
        mod_list: &[ModInfo],
        active_profile: &str,
        profiles: &[String],
        game: Option<&GameInfo>,
    ) -> Vec<PluginLoadError> {
        let mut errors: Vec<PluginLoadError> = Vec::new();

        // Silently skip a missing plugins directory — not an error condition.
        if !plugins_dir.exists() {
            info!(path = %plugins_dir.display(), "plugins directory not found — no plugins loaded");
            return errors;
        }

        // Collect .so and .rhai entries, skip anything else.
        let mut entries: Vec<PathBuf> = match std::fs::read_dir(plugins_dir) {
            Ok(rd) => rd
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && matches!(p.extension().and_then(|e| e.to_str()), Some("so" | "rhai"))
                })
                .collect(),
            Err(err) => {
                warn!(
                    path = %plugins_dir.display(),
                    error = %err,
                    "failed to read plugins directory"
                );
                return errors;
            }
        };

        // §6.1: alphabetical load order
        entries.sort_by(|a, b| {
            a.file_name().unwrap_or_default().cmp(b.file_name().unwrap_or_default())
        });

        // Track IDs already accepted → detect duplicates (§6.3).
        // Maps plugin_id → the path that successfully loaded with that ID.
        let mut seen_ids: HashMap<String, PathBuf> = HashMap::new();

        for path in entries {
            if let Some(loaded) = self.load_one(
                &path,
                mod_list,
                active_profile,
                profiles,
                game,
                &mut seen_ids,
                &mut errors,
            ) {
                // Capture the context reference for lifecycle hook dispatch
                // before moving the LoadedPlugin into self.plugins.
                self.contexts
                    .lock()
                    .expect("PluginRegistry: context list lock poisoned")
                    .push(Arc::clone(&loaded.context));
                self.plugins.push(loaded);
            }
        }

        // Wire lifecycle hooks now that all contexts are registered.
        self.subscribe_lifecycle_hooks();

        errors
    }

    /// Shut down and remove all loaded plugins.
    ///
    /// 1. Drops all [`SubscriptionHandle`]s created by
    ///    [`subscribe_lifecycle_hooks`][Self::subscribe_lifecycle_hooks],
    ///    unsubscribing the host-side lifecycle hooks from the event bus.
    /// 2. Calls [`MantlePlugin::shutdown`] on every plugin in **reverse** load
    ///    order (last loaded → first unloaded).
    /// 3. Clears the shared context list.
    ///
    /// # Side Effects
    /// After this call `self.plugins` is empty and `plugin_count()` returns 0.
    pub fn unload_all(&mut self) {
        // 1. Unsubscribe host lifecycle hooks before invoking plugin shutdowns.
        self.lifecycle_handles.clear();

        // 2. Reverse order shutdown mirrors typical LIFO stack semantics.
        for loaded in self.plugins.drain(..).rev() {
            let mut plugin = loaded.plugin;
            info!(
                plugin.id = plugin.id(),
                path = %loaded.path.display(),
                "unloading plugin"
            );
            plugin.shutdown();
        }

        // 3. Release context references held by the registry.
        self.contexts
            .lock()
            .expect("PluginRegistry: context list lock poisoned")
            .clear();
    }

    /// Subscribe host-side lifecycle hooks to the shared event bus.
    ///
    /// Called automatically by [`load_dir`][Self::load_dir] after all plugins
    /// are initialised.  Wires two subscriptions:
    ///
    /// - **`ProfileChanged`** → calls
    ///   [`PluginContext::update_active_profile`] on every loaded context,
    ///   keeping the `active_profile` snapshot current.
    /// - **`GameLaunching`** → calls [`PluginContext::update_game`] on every
    ///   loaded context, so plugins see the current game.
    ///
    /// The returned [`SubscriptionHandle`]s are stored in `lifecycle_handles`
    /// and dropped (unsubscribed) by [`unload_all`][Self::unload_all].
    ///
    /// Calling this method again removes the old handles first to avoid
    /// duplicate subscriptions (e.g. if `load_dir` is called more than once).
    pub fn subscribe_lifecycle_hooks(&mut self) {
        // Remove any previously registered handles to avoid duplicate firings.
        self.lifecycle_handles.clear();

        // ── ProfileChanged → update active_profile snapshot ──────────────────
        let ctxs = Arc::clone(&self.contexts);
        let h_profile = self.event_bus.subscribe(
            EventFilter::ProfileChanged,
            move |event| {
                if let ModManagerEvent::ProfileChanged { new, .. } = event {
                    let list = ctxs.lock().expect("lifecycle hook: context list poisoned");
                    for ctx in list.iter() {
                        ctx.update_active_profile(new.clone());
                    }
                }
            },
        );

        // ── GameLaunching → update game snapshot ──────────────────────────────
        let ctxs = Arc::clone(&self.contexts);
        let h_game = self.event_bus.subscribe(
            EventFilter::GameLaunching,
            move |event| {
                if let ModManagerEvent::GameLaunching(game) = event {
                    let list = ctxs.lock().expect("lifecycle hook: context list poisoned");
                    for ctx in list.iter() {
                        ctx.update_game(Some(game.clone()));
                    }
                }
            },
        );

        self.lifecycle_handles.push(h_profile);
        self.lifecycle_handles.push(h_game);
    }

    /// Number of successfully loaded plugins.
    #[must_use]
    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    /// Iterator over the IDs of all loaded plugins, in load order.
    ///
    /// # Returns
    /// An iterator yielding `&str` IDs.
    pub fn plugin_ids(&self) -> impl Iterator<Item = &str> {
        self.plugins.iter().map(|p| p.plugin.id())
    }

    /// Look up a loaded plugin by its stable ID.
    ///
    /// # Parameters
    /// - `id`: The plugin's ID string as returned by `MantlePlugin::id()`.
    ///
    /// # Returns
    /// `Some(&dyn MantlePlugin)` if a plugin with that ID is loaded;
    /// `None` otherwise.
    #[must_use]
    pub fn get<'a>(&'a self, id: &str) -> Option<&'a (dyn MantlePlugin + 'a)> {
        for p in &self.plugins {
            if p.plugin.id() == id {
                return Some(p.plugin.as_ref());
            }
        }
        None
    }

    /// Look up a loaded plugin by ID, returning a mutable reference.
    ///
    /// # Parameters
    /// - `id`: Plugin ID.
    ///
    /// # Returns
    /// `Some(&mut dyn MantlePlugin)` if found; `None` otherwise.
    pub fn get_mut<'a>(&'a mut self, id: &str) -> Option<&'a mut (dyn MantlePlugin + 'a)> {
        for p in &mut self.plugins {
            if p.plugin.id() == id {
                return Some(p.plugin.as_mut());
            }
        }
        None
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Attempt to load a single plugin file, init it, and push it onto
    /// `self.plugins`. Any failure is pushed onto `errors` and the function
    /// returns without adding the plugin.
    ///
    /// # Parameters
    /// - `path`: Full path to the `.so` or `.rhai` file.
    /// - `mod_list` / `active_profile` / `profiles` / `game`: Runtime context
    ///   data forwarded to [`PluginContext::new`].
    /// - `seen_ids`: Mutable map of ID → source path for duplicate detection.
    /// - `errors`: Accumulator for non-fatal errors.
    // clippy::too_many_arguments: load_one is an internal dispatch function that
    // must forward all runtime context to PluginContext::new; splitting it would
    // scatter the validation logic without improving readability.
    // clippy::too_many_lines: the function is long due to sequential validation
    // steps (load, version-check, ID dedup, context build); each step is one
    // logical phase and extracting sub-functions would make the flow harder to follow.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_lines)]
    fn load_one(
        &self,
        path: &Path,
        mod_list: &[ModInfo],
        active_profile: &str,
        profiles: &[String],
        game: Option<&GameInfo>,
        seen_ids: &mut HashMap<String, PathBuf>,
        errors: &mut Vec<PluginLoadError>,
    ) -> Option<LoadedPlugin> {
        // ── 1. Load the binary/script ─────────────────────────────────────────
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        let mut plugin: Box<dyn MantlePlugin> = match ext {
            "so" => match load_native_plugin(path) {
                Ok(p) => Box::new(p),
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "native plugin load failed");
                    errors.push(PluginLoadError::LoadFailed {
                        path: path.to_owned(),
                        source: e,
                    });
                    return None;
                }
            },
            "rhai" => match load_scripted_plugin(path) {
                Ok(p) => Box::new(p),
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "scripted plugin load failed");
                    errors.push(PluginLoadError::LoadFailed {
                        path: path.to_owned(),
                        source: e,
                    });
                    return None;
                }
            },
            _ => return None, // filtered upstream; can't happen
        };

        // ── 2. Duplicate ID check (§6.3) ──────────────────────────────────────
        let plugin_id = plugin.id().to_owned();

        if let Some(existing_path) = seen_ids.get(&plugin_id) {
            warn!(
                id = %plugin_id,
                existing = %existing_path.display(),
                rejected = %path.display(),
                "duplicate plugin ID — rejecting second occurrence"
            );
            errors.push(PluginLoadError::DuplicateId {
                id: plugin_id,
                existing: existing_path.clone(),
                rejected: path.to_owned(),
            });
            return None;
        }

        // ── 3. Read optional plugin.toml manifest ─────────────────────────────
        let manifest = read_manifest(path);

        // ── 4. Resolve capabilities from manifest (§7) ────────────────────────
        let capabilities = resolve_capabilities(&manifest, ext == "rhai");

        // ── 5. Initialise default settings from the plugin's declarations ──────
        let settings: HashMap<String, SettingValue> =
            plugin.settings().into_iter().map(|s| (s.key.to_owned(), s.default)).collect();

        // ── 6. Create per-plugin data directory ───────────────────────────────
        let plugin_data_dir = self.base_data_dir.join("plugin-data").join(&plugin_id);

        if let Err(e) = std::fs::create_dir_all(&plugin_data_dir) {
            warn!(
                path = %plugin_data_dir.display(),
                error = %e,
                "could not create plugin data directory — plugin will have limited persistence"
            );
            // Non-fatal: continue with the dir path even if it wasn't created.
        }

        // ── 7. Construct PluginContext ─────────────────────────────────────────
        let ctx = PluginContext::new(
            plugin_id.clone(),
            mod_list.to_owned(),
            active_profile.to_string(),
            profiles.to_owned(),
            game.cloned(),
            Arc::clone(&self.event_bus),
            settings,
            plugin_data_dir,
            capabilities,
        );

        // ── 8. Call init() — catch panics per §8.2 ────────────────────────────
        let init_result = panic::catch_unwind(AssertUnwindSafe(|| plugin.init(Arc::clone(&ctx))));

        match init_result {
            // init panicked
            Err(panic_val) => {
                let message = extract_panic_message(&panic_val);
                warn!(
                    id = %plugin_id,
                    path = %path.display(),
                    panic = %message,
                    "plugin init() panicked — unloading"
                );
                errors.push(PluginLoadError::InitPanicked {
                    path: path.to_owned(),
                    message,
                });
                None
            }

            // init returned an Err
            Ok(Err(plugin_err)) => {
                warn!(
                    id = %plugin_id,
                    path = %path.display(),
                    error = %plugin_err,
                    "plugin init() returned error — unloading"
                );
                errors.push(PluginLoadError::InitFailed {
                    path: path.to_owned(),
                    source: plugin_err,
                });
                None
            }

            // init succeeded
            Ok(Ok(())) => {
                info!(
                    id = %plugin_id,
                    path = %path.display(),
                    "plugin loaded and initialised"
                );
                seen_ids.insert(plugin_id, path.to_owned());
                Some(LoadedPlugin {
                    plugin,
                    context: ctx,
                    path: path.to_owned(),
                })
            }
        }
    }
}

// ─── Stand-alone helpers ─────────────────────────────────────────────────────

/// Attempt to read and parse a `plugin.toml` manifest adjacent to `plugin_path`.
///
/// If the manifest file does not exist or cannot be parsed, return
/// [`PluginManifest::default`] silently.
///
/// # Parameters
/// - `plugin_path`: Path to the `.so` or `.rhai` file.
///
/// # Returns
/// Parsed `PluginManifest` if the `.toml` is present and valid; otherwise a
/// default (all-`None`) manifest.
fn read_manifest(plugin_path: &Path) -> PluginManifest {
    let manifest_path = plugin_path.with_extension("toml");
    match std::fs::read_to_string(&manifest_path) {
        Ok(contents) => match toml::from_str::<PluginManifest>(&contents) {
            Ok(m) => {
                info!(
                    path = %manifest_path.display(),
                    "loaded plugin.toml manifest"
                );
                m
            }
            Err(e) => {
                warn!(
                    path = %manifest_path.display(),
                    error = %e,
                    "failed to parse plugin.toml — using defaults"
                );
                PluginManifest::default()
            }
        },
        // File absent is the common case; only warn on real I/O errors.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => PluginManifest::default(),
        Err(e) => {
            warn!(
                path = %manifest_path.display(),
                error = %e,
                "could not read plugin.toml — using defaults"
            );
            PluginManifest::default()
        }
    }
}

/// Convert manifest capability strings to a [`HashSet<Capability>`].
///
/// Unknown capability names are logged and ignored. Rhai plugins may not
/// request `downloads` (§7.3) — any such declaration is silently dropped.
///
/// # Parameters
/// - `manifest`: Parsed manifest (or default if absent).
/// - `is_rhai`: Whether the plugin is a Rhai script (disables download cap).
///
/// # Returns
/// `HashSet<Capability>` to pass to [`PluginContext::new`].
fn resolve_capabilities(manifest: &PluginManifest, is_rhai: bool) -> HashSet<Capability> {
    let mut caps = HashSet::new();

    for name in manifest
        .capabilities
        .required
        .iter()
        .chain(manifest.capabilities.optional.iter())
    {
        match name.to_lowercase().as_str() {
            "downloads" => {
                if is_rhai {
                    // §7.3: Rhai may not request the downloads capability.
                    warn!(
                        capability = "downloads",
                        "Rhai plugin requested 'downloads' capability — ignored (§7.3)"
                    );
                } else {
                    caps.insert(Capability::Downloads);
                }
            }
            "notifications" => {
                caps.insert(Capability::Notifications);
            }
            unknown => {
                warn!(capability = %unknown, "unknown capability declared in plugin.toml — ignored");
            }
        }
    }

    caps
}

/// Extract a human-readable string from a `Box<dyn Any>` panic value.
///
/// Tries `&str` first, then `String`, then falls back to `"<non-string panic>"`.
///
/// # Parameters
/// - `panic_val`: The value caught by `catch_unwind`.
///
/// # Returns
/// A human-readable string describing the panic.
fn extract_panic_message(panic_val: &dyn std::any::Any) -> String {
    if let Some(s) = panic_val.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = panic_val.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic>".to_owned()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // ── helpers ───────────────────────────────────────────────────────────────

    fn make_registry() -> PluginRegistry {
        let bus = Arc::new(EventBus::new());
        PluginRegistry::new(bus, std::env::temp_dir().join("mantle_registry_test"))
    }

    // ── manifest capability resolution ────────────────────────────────────────

    /// Native plugin with no manifest gets an empty capability set.
    #[test]
    fn resolve_capabilities_empty_manifest_native() {
        let m = PluginManifest::default();
        let caps = resolve_capabilities(&m, false);
        assert!(caps.is_empty());
    }

    /// Rhai plugin requesting downloads has the cap silently dropped.
    #[test]
    fn resolve_capabilities_rhai_downloads_ignored() {
        let mut m = PluginManifest::default();
        m.capabilities.required = vec!["downloads".to_owned()];
        let caps = resolve_capabilities(&m, true);
        assert!(!caps.contains(&Capability::Downloads));
    }

    /// Native plugin requesting downloads gets the capability.
    #[test]
    fn resolve_capabilities_native_downloads_granted() {
        let mut m = PluginManifest::default();
        m.capabilities.required = vec!["downloads".to_owned()];
        let caps = resolve_capabilities(&m, false);
        assert!(caps.contains(&Capability::Downloads));
    }

    /// Unknown capability names are silently ignored.
    #[test]
    fn resolve_capabilities_unknown_ignored() {
        let mut m = PluginManifest::default();
        m.capabilities.required = vec!["teleportation".to_owned()];
        let caps = resolve_capabilities(&m, false);
        assert!(caps.is_empty());
    }

    /// Both required and optional capabilities are resolved.
    #[test]
    fn resolve_capabilities_merges_required_and_optional() {
        let mut m = PluginManifest::default();
        m.capabilities.required = vec!["downloads".to_owned()];
        m.capabilities.optional = vec!["notifications".to_owned()];
        let caps = resolve_capabilities(&m, false);
        assert!(caps.contains(&Capability::Downloads));
        assert!(caps.contains(&Capability::Notifications));
    }

    // ── panic message extraction ───────────────────────────────────────────────

    #[test]
    fn extract_panic_message_str_slice() {
        let result = panic::catch_unwind(|| panic!("oops")).unwrap_err();
        let msg = extract_panic_message(result.as_ref());
        assert!(msg.contains("oops"), "expected 'oops' in '{msg}'");
    }

    #[test]
    fn extract_panic_message_string() {
        let result = panic::catch_unwind(|| panic!("{}", "owned string".to_owned())).unwrap_err();
        let msg = extract_panic_message(result.as_ref());
        assert!(msg.contains("owned string"), "expected message in '{msg}'");
    }

    // ── registry creation ─────────────────────────────────────────────────────

    #[test]
    fn new_registry_is_empty() {
        let reg = make_registry();
        assert_eq!(reg.plugin_count(), 0);
        assert!(reg.get("anything").is_none());
    }

    // ── load_dir on nonexistent directory ─────────────────────────────────────

    /// A missing plugins directory produces no errors and loads zero plugins.
    #[test]
    fn load_dir_missing_dir_returns_no_errors() {
        let mut reg = make_registry();
        let errors = reg.load_dir(
            Path::new("/does/not/exist/plugins"),
            &[],
            "default",
            &["default".to_owned()],
            None,
        );
        assert!(errors.is_empty());
        assert_eq!(reg.plugin_count(), 0);
    }

    /// An empty (existing) plugins directory produces no errors and loads zero
    /// plugins.
    #[test]
    fn load_dir_empty_dir_returns_no_errors() {
        let temp = tempfile::tempdir().unwrap();
        let plugins_dir = temp.path().join("plugins");
        std::fs::create_dir_all(&plugins_dir).unwrap();

        let mut reg = make_registry();
        let errors = reg.load_dir(&plugins_dir, &[], "default", &["default".to_owned()], None);
        assert!(errors.is_empty());
        assert_eq!(reg.plugin_count(), 0);
    }

    // ── unload_all on empty registry ─────────────────────────────────────────

    #[test]
    fn unload_all_empty_registry_is_safe() {
        let mut reg = make_registry();
        reg.unload_all(); // must not panic
        assert_eq!(reg.plugin_count(), 0);
    }

    // ── manifest parsing ─────────────────────────────────────────────────────

    /// A well-formed plugin.toml is parsed without error.
    #[test]
    fn read_manifest_valid_toml() {
        let temp = tempfile::tempdir().unwrap();
        let so_path = temp.path().join("myplugin.so");
        let toml_path = temp.path().join("myplugin.toml");
        std::fs::write(&so_path, b"").unwrap();
        std::fs::write(
            &toml_path,
            br#"
id = "myplugin"
name = "My Plugin"
version = "1.0.0"
author = "Author"
description = "A test plugin."

[capabilities]
required = ["notifications"]
"#,
        )
        .unwrap();
        let m = read_manifest(&so_path);
        assert_eq!(m.id.as_deref(), Some("myplugin"));
        assert_eq!(m.name.as_deref(), Some("My Plugin"));
        assert!(m.capabilities.required.contains(&"notifications".to_owned()));
    }

    /// A missing plugin.toml returns a default manifest (no errors).
    #[test]
    fn read_manifest_missing_toml_returns_default() {
        let temp = tempfile::tempdir().unwrap();
        let so_path = temp.path().join("nomanifest.so");
        std::fs::write(&so_path, b"").unwrap();
        let m = read_manifest(&so_path);
        assert!(m.id.is_none());
        assert!(m.capabilities.required.is_empty());
    }

    /// A malformed plugin.toml falls back to defaults.
    #[test]
    fn read_manifest_malformed_toml_returns_default() {
        let temp = tempfile::tempdir().unwrap();
        let so_path = temp.path().join("bad.so");
        let toml_path = temp.path().join("bad.toml");
        std::fs::write(&so_path, b"").unwrap();
        std::fs::write(&toml_path, b"this is not valid toml !!!").unwrap();
        let m = read_manifest(&so_path);
        assert!(m.id.is_none());
    }

    // ── load_dir filters non-plugin files ────────────────────────────────────

    /// Non-.so/.rhai files in the plugins dir are silently ignored.
    #[test]
    fn load_dir_ignores_non_plugin_files() {
        let temp = tempfile::tempdir().unwrap();
        let plugins_dir = temp.path().join("plugins");
        std::fs::create_dir_all(&plugins_dir).unwrap();
        // Write a .toml and a .txt — neither should be loaded.
        std::fs::write(plugins_dir.join("readme.txt"), b"").unwrap();
        std::fs::write(plugins_dir.join("config.toml"), b"").unwrap();

        let mut reg = make_registry();
        let errors = reg.load_dir(&plugins_dir, &[], "default", &["default".to_owned()], None);
        assert!(errors.is_empty());
        assert_eq!(reg.plugin_count(), 0);
    }

    // ── lifecycle hooks ───────────────────────────────────────────────────────

    /// Helper: inject a pre-built context into the registry's context list and
    /// subscribe lifecycle hooks. Used when we don't have a real plugin to load.
    fn inject_ctx_and_subscribe(
        reg: &mut PluginRegistry,
        ctx: Arc<crate::plugin::PluginContext>,
    ) {
        reg.contexts
            .lock()
            .expect("test: context list poisoned")
            .push(ctx);
        reg.subscribe_lifecycle_hooks();
    }

    /// `subscribe_lifecycle_hooks` updates `active_profile` on all plugin
    /// contexts when a `ProfileChanged` event fires on the shared bus.
    #[test]
    fn lifecycle_profile_changed_updates_all_contexts() {
        use crate::plugin::{ModManagerEvent, PluginContext};

        let bus = Arc::new(EventBus::new());
        let mut reg = PluginRegistry::new(Arc::clone(&bus), std::env::temp_dir());

        let ctx = PluginContext::for_tests();
        assert_eq!(ctx.active_profile(), "Default");

        inject_ctx_and_subscribe(&mut reg, Arc::clone(&ctx));

        bus.publish(&ModManagerEvent::ProfileChanged {
            old: "Default".into(),
            new: "Survival Run".into(),
        });

        assert_eq!(
            ctx.active_profile(),
            "Survival Run",
            "context snapshot must reflect the new profile name"
        );
    }

    /// `subscribe_lifecycle_hooks` updates the `game` snapshot on all plugin
    /// contexts when a `GameLaunching` event fires on the shared bus.
    #[test]
    fn lifecycle_game_launching_updates_all_contexts() {
        use crate::game::GameKind;
        use crate::plugin::{ModManagerEvent, PluginContext};

        let bus = Arc::new(EventBus::new());
        let mut reg = PluginRegistry::new(Arc::clone(&bus), std::env::temp_dir());

        let ctx = PluginContext::for_tests();
        assert!(ctx.game().is_none(), "game should be None before any event");

        inject_ctx_and_subscribe(&mut reg, Arc::clone(&ctx));

        bus.publish(&ModManagerEvent::GameLaunching(crate::game::GameInfo {
            slug: "skyrim_se".into(),
            name: "Skyrim SE".into(),
            kind: GameKind::SkyrimSE,
            steam_app_id: 489830,
            install_path: "/game".into(),
            data_path: "/game/Data".into(),
            proton_prefix: None,
        }));

        assert!(
            ctx.game().is_some(),
            "context game snapshot must be populated after GameLaunching"
        );
        assert_eq!(ctx.game().unwrap().slug, "skyrim_se");
    }

    /// `unload_all` drops lifecycle handles and releases the context list.
    #[test]
    fn unload_all_clears_lifecycle_handles_and_contexts() {
        use crate::plugin::{ModManagerEvent, PluginContext};

        let bus = Arc::new(EventBus::new());
        let mut reg = PluginRegistry::new(Arc::clone(&bus), std::env::temp_dir());

        let ctx = PluginContext::for_tests();
        inject_ctx_and_subscribe(&mut reg, Arc::clone(&ctx));

        // Two lifecycle handles were registered (ProfileChanged + GameLaunching).
        assert_eq!(reg.lifecycle_handles.len(), 2);

        reg.unload_all();

        // After unload, the context list is empty and handlers are gone.
        assert!(reg.contexts.lock().unwrap().is_empty());
        assert!(reg.lifecycle_handles.is_empty());

        // Publishing an event after unload must not update the now-gone context.
        let profile_before = ctx.active_profile();
        bus.publish(&ModManagerEvent::ProfileChanged {
            old: "Default".into(),
            new: "Should Not Update".into(),
        });
        assert_eq!(
            ctx.active_profile(),
            profile_before,
            "unloaded context must not be updated by post-unload events"
        );
    }
}
