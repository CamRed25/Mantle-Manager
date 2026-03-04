//! `PluginContext`, `MantlePlugin` trait, and supporting plugin API types.
//!
//! Plugins (native `.so` and Rhai scripts) receive an `Arc<PluginContext>`
//! on initialization and interact with Mantle Manager exclusively through
//! this surface. Direct access to `mantle_core` internals is not permitted.
//!
//! # API versioning
//! [`PLUGIN_API_VERSION`] is a `semver::Version` constant exposed so plugins
//! can assert compatibility at load time. The host increments this version
//! according to semantic versioning rules when the API surface changes.
//!
//! # References
//! - `standards/PLUGIN_API.md` §2–§4, §7–§8

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};

use once_cell::sync::Lazy;
use semver::Version;

use super::event::{EventBus, EventFilter, ModInfo, ModManagerEvent, SubscriptionHandle};
use crate::game::GameInfo;

// ─── API version constants ─────────────────────────────────────────────────────

/// Current plugin API version.
///
/// Plugins call `required_api_version()` and compare against this constant to
/// determine whether they are compatible with the loaded host.
///
/// Follows semantic versioning: minor bumps add new methods; major bumps remove
/// or change existing methods.
pub static PLUGIN_API_VERSION: Lazy<Version> = Lazy::new(|| Version::new(1, 0, 0));

/// Rustc version string baked in at compile time by `build.rs`.
///
/// Used for ABI enforcement when loading native `.so` plugins: a plugin's
/// recorded toolchain version must match this string.
pub static RUSTC_TOOLCHAIN_VERSION: &str = env!("RUSTC_VERSION_STRING");

// ─── PluginError ──────────────────────────────────────────────────────────────

/// Errors returned by [`PluginContext`] methods and from [`MantlePlugin::init`].
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    /// Plugin failed to initialize.
    #[error("plugin init failed: {0}")]
    InitFailed(String),

    /// Plugin requested a capability it was not granted.
    #[error("capability not granted: {0}")]
    CapabilityNotGranted(&'static str),

    /// Requested setting key does not exist.
    #[error("setting not found: {0}")]
    SettingNotFound(String),

    /// Setting exists but the type does not match the expected variant.
    #[error("setting type mismatch for key: {0}")]
    SettingTypeMismatch(String),

    /// Plugin requires an API version the host cannot satisfy.
    #[error("API version mismatch: plugin requires {required}, host provides {loaded}")]
    ApiVersionMismatch {
        required: Version,
        loaded:   Version,
    },

    /// Plugin attempted a network operation but the `net` feature is disabled.
    #[error("network feature is not enabled in this build")]
    NetFeatureDisabled,

    /// Catch-all wrapper for other errors.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

// ─── SettingValue / PluginSetting ─────────────────────────────────────────────

/// A typed plugin setting value.
///
/// Returned by [`PluginContext::get_setting`] and accepted by
/// [`PluginContext::set_setting`].
#[derive(Debug, Clone, PartialEq)]
pub enum SettingValue {
    Bool(bool),
    String(String),
    Int(i64),
    Float(f64),
}

/// Declares a single configurable plugin setting.
///
/// Returned by [`MantlePlugin::settings`] so the UI can render a settings
/// panel without knowing the plugin implementation.
#[derive(Debug, Clone)]
pub struct PluginSetting {
    /// Unique key used to read/write the setting via
    /// [`PluginContext::get_setting`] / [`PluginContext::set_setting`].
    pub key:         &'static str,
    /// Short, user-visible label for the setting.
    pub label:       &'static str,
    /// Optional longer description shown as a tooltip or help text.
    pub description: Option<&'static str>,
    /// Default value — used when the setting has not been configured yet.
    pub default:     SettingValue,
}

// ─── Auxiliary enums ─────────────────────────────────────────────────────────

/// Operational state of a mod in the active profile, returned by
/// [`PluginContext::mod_state`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModState {
    Enabled,
    Disabled,
}

/// Severity level for notifications posted via [`PluginContext::notify`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyLevel {
    Info,
    Warning,
    Error,
}

/// Optional capabilities a plugin may hold.
///
/// Granted at plugin load time by the host. Plugins must hold the appropriate
/// capability before calling gated API methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Access to [`PluginContext::queue_download`] and related download events.
    Downloads,
    /// Access to [`PluginContext::notify`] with elevated notification levels.
    Notifications,
}

// ─── DownloadHandle ───────────────────────────────────────────────────────────

/// Handle to an in-progress download.
///
/// # Note
/// The `net` feature is not yet implemented. [`PluginContext::queue_download`]
/// always returns [`PluginError::NetFeatureDisabled`]; this type exists to
/// satisfy the API contract described in `standards/PLUGIN_API.md §6`.
#[derive(Debug)]
pub struct DownloadHandle {
    _private: (),
}

// ─── MantlePlugin trait ───────────────────────────────────────────────────────

/// The trait every plugin must implement.
///
/// # Lifecycle
/// 1. Host verifies `required_api_version()` against [`PLUGIN_API_VERSION`].
/// 2. `init` is called once with shared `Arc<PluginContext>`.
/// 3. The plugin registers subscriptions and exposes any commands via `ctx`.
/// 4. `shutdown` is called before unload. Dropping `SubscriptionHandle`s here
///    is the recommended way to unsubscribe.
///
/// # Thread safety
/// The trait requires `Send + Sync` so plugins can be held in `Arc<Mutex<>>`.
pub trait MantlePlugin: Send + Sync {
    /// Short, unique, lowercase identifier for this plugin. Must be stable
    /// across plugin versions (used as a dictionary key in plugin storage).
    fn id(&self) -> &str;

    /// Human-readable display name.
    fn name(&self) -> &str;

    /// Current plugin version.
    fn version(&self) -> Version;

    /// Author or organization name.
    fn author(&self) -> &str;

    /// One-line description shown in the plugin panel.
    fn description(&self) -> &str;

    /// Minimum API version this plugin requires.
    ///
    /// The host checks this against [`PLUGIN_API_VERSION`] using semver
    /// compatible-release semantics before calling `init`.
    fn required_api_version(&self) -> Version;

    /// Called once after the plugin is loaded.
    ///
    /// # Errors
    /// Return [`PluginError::InitFailed`] if a required resource is unavailable.
    /// A failed `init` unloads the plugin immediately.
    fn init(&mut self, ctx: Arc<PluginContext>) -> Result<(), PluginError>;

    /// Called before the plugin is unloaded. Must not fail.
    ///
    /// Drop any `SubscriptionHandle`s stored on `self` here to avoid
    /// reference cycles with the `EventBus`.
    fn shutdown(&mut self);

    /// Declares the settings this plugin exposes to the UI.
    ///
    /// The default implementation returns an empty list (no settings).
    fn settings(&self) -> Vec<PluginSetting> {
        vec![]
    }
}

// ─── PluginContext ────────────────────────────────────────────────────────────

/// The shared interface provided to every loaded plugin.
///
/// Always held behind `Arc<PluginContext>`. Plugins must not bypass this type
/// to access `mantle_core` internals.
///
/// # Thread safety
/// All mutable state is guarded by `RwLock` or `Mutex`. Methods that read
/// shared state return owned clones, avoiding borrow lifetime issues across
/// plugin/host thread boundaries.
#[derive(Debug)]
pub struct PluginContext {
    /// ID of the plugin that owns this context instance.
    plugin_id: String,
    /// Read-only snapshot of the current mod list. Updated by the host on
    /// every profile change or mod install/enable/disable.
    mod_list: RwLock<Vec<ModInfo>>,
    /// Currently active profile name.
    active_profile: RwLock<String>,
    /// All known profile names.
    profiles: RwLock<Vec<String>>,
    /// Currently detected game, if any.
    game: RwLock<Option<GameInfo>>,
    /// Shared event bus. Plugins subscribe here; core publishes here.
    event_bus: Arc<EventBus>,
    /// Plugin-private settings store. Pre-populated with defaults at load time.
    settings: Mutex<HashMap<String, SettingValue>>,
    /// Writable directory for plugin-private persistent data.
    data_dir: PathBuf,
    /// Capability flags granted by the host at load time.
    capabilities: HashSet<Capability>,
}

impl PluginContext {
    /// Create a fully initialized `PluginContext`.
    ///
    /// Called by the host loader, not by plugins.
    ///
    /// # Parameters
    /// - `plugin_id`: The plugin's stable ID string.
    /// - `mod_list`: Initial mod list snapshot.
    /// - `active_profile`: Currently active profile name.
    /// - `profiles`: All known profile names.
    /// - `game`: Currently detected game, if any.
    /// - `event_bus`: Shared `Arc<EventBus>`.
    /// - `settings`: Pre-populated setting values (typically loaded from disk).
    /// - `data_dir`: Writable directory for plugin-private data.
    /// - `capabilities`: Capability flags granted to this plugin.
    ///
    /// # Returns
    /// `Arc<PluginContext>` ready to pass to [`MantlePlugin::init`].
    // clippy::too_many_arguments: PluginContext::new is the single designated
    // construction site; all nine arguments are distinct, required fields with
    // no sensible grouping that would not obscure the API.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        plugin_id:      impl Into<String>,
        mod_list:       Vec<ModInfo>,
        active_profile: impl Into<String>,
        profiles:       Vec<String>,
        game:           Option<GameInfo>,
        event_bus:      Arc<EventBus>,
        settings:       HashMap<String, SettingValue>,
        data_dir:       impl Into<PathBuf>,
        capabilities:   HashSet<Capability>,
    ) -> Arc<Self> {
        Arc::new(Self {
            plugin_id:      plugin_id.into(),
            mod_list:       RwLock::new(mod_list),
            active_profile: RwLock::new(active_profile.into()),
            profiles:       RwLock::new(profiles),
            game:           RwLock::new(game),
            event_bus,
            settings:       Mutex::new(settings),
            data_dir:       data_dir.into(),
            capabilities,
        })
    }

    // ── Read-only snapshot accessors ──────────────────────────────────────────

    /// Returns a cloned snapshot of the current mod list.
    ///
    /// # Returns
    /// `Vec<ModInfo>` sorted by priority descending.
    ///
    /// # Panics
    /// Panics if the internal `RwLock` is poisoned.
    #[must_use]
    pub fn mod_list(&self) -> Vec<ModInfo> {
        self.mod_list
            .read()
            .expect("PluginContext: mod_list lock poisoned")
            .clone()
    }

    /// Look up a mod's enabled/disabled state by slug.
    ///
    /// # Parameters
    /// - `slug`: The mod's stable slug identifier.
    ///
    /// # Returns
    /// `Some(ModState)` if a mod with that slug exists; `None` otherwise.
    ///
    /// # Panics
    /// Panics if the internal `RwLock` is poisoned.
    #[must_use]
    pub fn mod_state(&self, slug: &str) -> Option<ModState> {
        let list = self
            .mod_list
            .read()
            .expect("PluginContext: mod_list lock poisoned");
        list.iter()
            .find(|m| m.slug == slug)
            .map(|m| if m.is_enabled { ModState::Enabled } else { ModState::Disabled })
    }

    /// Returns the name of the currently active profile.
    ///
    /// # Panics
    /// Panics if the internal `RwLock` is poisoned.
    #[must_use]
    pub fn active_profile(&self) -> String {
        self.active_profile
            .read()
            .expect("PluginContext: active_profile lock poisoned")
            .clone()
    }

    /// Returns a cloned list of all known profile names.
    ///
    /// # Panics
    /// Panics if the internal `RwLock` is poisoned.
    #[must_use]
    pub fn profiles(&self) -> Vec<String> {
        self.profiles
            .read()
            .expect("PluginContext: profiles lock poisoned")
            .clone()
    }

    /// Returns information about the currently active game, if detected.
    ///
    /// # Panics
    /// Panics if the internal `RwLock` is poisoned.
    #[must_use]
    pub fn game(&self) -> Option<GameInfo> {
        self.game
            .read()
            .expect("PluginContext: game lock poisoned")
            .clone()
    }

    // ── Event subscription ────────────────────────────────────────────────────

    /// Subscribe to events matching `filter`.
    ///
    /// Returns a [`SubscriptionHandle`] that unsubscribes on drop. Store the
    /// handle on the plugin struct; drop it in `shutdown()`.
    ///
    /// # Parameters
    /// - `filter`: Determines which event variants are delivered.
    /// - `handler`: Synchronous callback. Avoid blocking; spawn a task for async work.
    ///
    /// # Returns
    /// [`SubscriptionHandle`] — keep alive for as long as events are desired.
    pub fn subscribe<F>(&self, filter: EventFilter, handler: F) -> SubscriptionHandle
    where
        F: Fn(&ModManagerEvent) + Send + Sync + 'static,
    {
        self.event_bus.subscribe(filter, handler)
    }

    // ── Settings ──────────────────────────────────────────────────────────────

    /// Retrieve the current value of a named setting.
    ///
    /// # Parameters
    /// - `key`: The setting key as declared in [`MantlePlugin::settings`].
    ///
    /// # Returns
    /// `Some(SettingValue)` if the key exists; `None` otherwise.
    ///
    /// # Panics
    /// Panics if the internal settings `Mutex` is poisoned.
    #[must_use]
    pub fn get_setting(&self, key: &str) -> Option<SettingValue> {
        self.settings
            .lock()
            .expect("PluginContext: settings lock poisoned")
            .get(key)
            .cloned()
    }

    /// Update the value of a named setting.
    ///
    /// The change is stored in memory only. The host persists settings to disk
    /// when the plugin is unloaded or the application exits.
    ///
    /// # Parameters
    /// - `key`: Setting key to update.
    /// - `value`: New value.
    ///
    /// # Panics
    /// Panics if the internal settings `Mutex` is poisoned.
    ///
    /// # Errors
    /// Currently always returns `Ok(())`. Reserved for future validation.
    pub fn set_setting(&self, key: impl Into<String>, value: SettingValue) -> Result<(), PluginError> {
        self.settings
            .lock()
            .expect("PluginContext: settings lock poisoned")
            .insert(key.into(), value);
        Ok(())
    }

    // ── Network ───────────────────────────────────────────────────────────────

    /// Queue a file download.
    ///
    /// # Note
    /// The `net` feature is not yet implemented. This method always returns
    /// [`PluginError::NetFeatureDisabled`]. The method signature is stable and
    /// will be backed by a real implementation in a future release.
    ///
    /// # Parameters
    /// - `url`: Download URL.
    /// - `dest`: Local destination path.
    ///
    /// # Errors
    /// [`PluginError::CapabilityNotGranted`] if `Capability::Downloads` is absent.
    /// [`PluginError::NetFeatureDisabled`] always (feature not implemented).
    pub fn queue_download(
        &self,
        _url: &str,
        _dest: &std::path::Path,
    ) -> Result<DownloadHandle, PluginError> {
        if !self.capabilities.contains(&Capability::Downloads) {
            return Err(PluginError::CapabilityNotGranted("Downloads"));
        }
        Err(PluginError::NetFeatureDisabled)
    }

    // ── Misc helpers ──────────────────────────────────────────────────────────

    /// Returns the plugin's writable data directory.
    ///
    /// The host creates this directory before calling `init`. Plugins may use
    /// it freely for caches, config files, and other persistent data.
    #[must_use]
    pub fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    /// Post a notification to the Mantle Manager UI.
    ///
    /// Also logs the message via `tracing` at the corresponding severity level.
    ///
    /// # Parameters
    /// - `level`: Severity of the notification.
    /// - `message`: Message text shown to the user.
    pub fn notify(&self, level: NotifyLevel, message: &str) {
        match level {
            NotifyLevel::Info    => tracing::info!("[plugin:{}] {}", self.plugin_id, message),
            NotifyLevel::Warning => tracing::warn!("[plugin:{}] {}", self.plugin_id, message),
            NotifyLevel::Error   => tracing::error!("[plugin:{}] {}", self.plugin_id, message),
        }
    }

    // ── Internal host-facing mutators ─────────────────────────────────────────

    /// Update the internal mod list. Called by the host after a mod state change.
    ///
    /// # Parameters
    /// - `list`: Fresh snapshot of the mod list.
    #[allow(dead_code)]
    pub(crate) fn update_mod_list(&self, list: Vec<ModInfo>) {
        *self
            .mod_list
            .write()
            .expect("PluginContext: mod_list lock poisoned") = list;
    }

    /// Update the active profile. Called by the host on profile switch.
    ///
    /// # Parameters
    /// - `profile`: New active profile name.
    #[allow(dead_code)]
    pub(crate) fn update_active_profile(&self, profile: impl Into<String>) {
        *self
            .active_profile
            .write()
            .expect("PluginContext: active_profile lock poisoned") = profile.into();
    }

    // ── Test helpers ──────────────────────────────────────────────────────────

    /// Construct a minimal `PluginContext` suitable for unit tests.
    ///
    /// All fields are initialised with empty/default values. The returned
    /// `Arc<PluginContext>` can be passed to [`MantlePlugin::init`] in tests.
    ///
    /// # Returns
    /// `Arc<PluginContext>` with no backing services.
    #[cfg(test)]
    pub fn for_tests() -> Arc<Self> {
        Self::new(
            "test-plugin",
            vec![],
            "Default",
            vec!["Default".to_string()],
            None,
            Arc::new(EventBus::new()),
            HashMap::new(),
            "/tmp/mantle_test_plugin",
            HashSet::new(),
        )
    }
}

// ─── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::GameKind;
    use super::super::event::ModManagerEvent;

    fn game_info() -> GameInfo {
        GameInfo {
            slug:          "skyrim_se".into(),
            name:          "Skyrim SE".into(),
            kind:          GameKind::SkyrimSE,
            steam_app_id:  489830,
            install_path:  "/game".into(),
            data_path:     "/game/Data".into(),
            proton_prefix: None,
        }
    }

    fn mod_info(slug: &str, enabled: bool) -> ModInfo {
        ModInfo {
            id:          1,
            slug:        slug.into(),
            name:        slug.to_uppercase(),
            version:     "1.0".into(),
            author:      "Author".into(),
            priority:    1,
            is_enabled:  enabled,
            install_dir: format!("/mods/{slug}"),
        }
    }

    // ── Settings ─────────────────────────────────────────────────────────────

    #[test]
    fn settings_get_returns_none_for_missing_key() {
        let ctx = PluginContext::for_tests();
        assert!(ctx.get_setting("missing").is_none());
    }

    #[test]
    fn settings_set_and_get_round_trip() {
        let ctx = PluginContext::for_tests();
        ctx.set_setting("notify_on_conflict", SettingValue::Bool(true)).unwrap();
        assert_eq!(
            ctx.get_setting("notify_on_conflict"),
            Some(SettingValue::Bool(true)),
        );
    }

    #[test]
    fn settings_overwrite_existing_value() {
        let ctx = PluginContext::for_tests();
        ctx.set_setting("level", SettingValue::Int(1)).unwrap();
        ctx.set_setting("level", SettingValue::Int(2)).unwrap();
        assert_eq!(ctx.get_setting("level"), Some(SettingValue::Int(2)));
    }

    // ── Mod list / mod state ──────────────────────────────────────────────────

    #[test]
    fn mod_list_returns_snapshot() {
        let ctx = PluginContext::for_tests();
        ctx.update_mod_list(vec![mod_info("alpha", true), mod_info("beta", false)]);
        let list = ctx.mod_list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].slug, "alpha");
    }

    #[test]
    fn mod_state_enabled() {
        let ctx = PluginContext::for_tests();
        ctx.update_mod_list(vec![mod_info("alpha", true)]);
        assert_eq!(ctx.mod_state("alpha"), Some(ModState::Enabled));
    }

    #[test]
    fn mod_state_disabled() {
        let ctx = PluginContext::for_tests();
        ctx.update_mod_list(vec![mod_info("alpha", false)]);
        assert_eq!(ctx.mod_state("alpha"), Some(ModState::Disabled));
    }

    #[test]
    fn mod_state_not_found_returns_none() {
        let ctx = PluginContext::for_tests();
        assert!(ctx.mod_state("nonexistent").is_none());
    }

    // ── Profile ───────────────────────────────────────────────────────────────

    #[test]
    fn active_profile_returns_initial_value() {
        let ctx = PluginContext::for_tests();
        assert_eq!(ctx.active_profile(), "Default");
    }

    #[test]
    fn update_active_profile_reflects_immediately() {
        let ctx = PluginContext::for_tests();
        ctx.update_active_profile("Custom");
        assert_eq!(ctx.active_profile(), "Custom");
    }

    // ── Game ─────────────────────────────────────────────────────────────────

    #[test]
    fn game_returns_none_when_unset() {
        let ctx = PluginContext::for_tests();
        assert!(ctx.game().is_none());
    }

    // ── Event subscription ────────────────────────────────────────────────────

    #[test]
    fn subscribe_via_context_fires_handler() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let ctx   = PluginContext::for_tests();
        let count = Arc::new(AtomicUsize::new(0));
        let c     = Arc::clone(&count);
        let _h    = ctx.subscribe(EventFilter::ProfileChanged, move |_| {
            c.fetch_add(1, Ordering::Relaxed);
        });
        ctx.event_bus.publish(&ModManagerEvent::ProfileChanged {
            old: "a".into(),
            new: "b".into(),
        });
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    // ── Downloads ─────────────────────────────────────────────────────────────

    #[test]
    fn queue_download_without_capability_returns_error() {
        let ctx = PluginContext::for_tests();
        let err = ctx.queue_download("https://example.com/mod.zip", std::path::Path::new("/tmp/mod.zip"));
        assert!(matches!(err, Err(PluginError::CapabilityNotGranted(_))));
    }

    #[test]
    fn queue_download_with_capability_returns_not_implemented() {
        let mut caps = HashSet::new();
        caps.insert(Capability::Downloads);
        let ctx = PluginContext::new(
            "test", vec![], "Default", vec![], None,
            Arc::new(EventBus::new()), HashMap::new(), "/tmp", caps,
        );
        let err = ctx.queue_download("https://example.com/mod.zip", std::path::Path::new("/tmp/mod.zip"));
        assert!(matches!(err, Err(PluginError::NetFeatureDisabled)));
    }
}
