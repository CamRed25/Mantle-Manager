//! Conflict Notifier — example native Mantle Manager plugin.
//!
//! Subscribes to [`ModManagerEvent::ConflictMapUpdated`] and posts a warning
//! notification to the UI whenever conflicts are detected or change.
//!
//! # What this demonstrates
//! - Implementing the [`MantlePlugin`] trait
//! - Subscribing to a specific event via [`PluginContext::subscribe`]
//! - Reading plugin settings with [`PluginContext::get_setting`]
//! - Declaring user-configurable settings via [`MantlePlugin::settings`]
//! - The two required C exports every native plugin must provide

use std::ffi::CString;
use std::sync::Arc;

use mantle_core::plugin::{
    context::{
        Capability, MantlePlugin, NotifyLevel, PluginContext, PluginError, PluginSetting,
        SettingValue, RUSTC_TOOLCHAIN_VERSION,
    },
    event::{EventFilter, ModManagerEvent, SubscriptionHandle},
};
use semver::Version;

// ─── Plugin struct ────────────────────────────────────────────────────────────

/// The plugin state persisted for the lifetime of the plugin.
///
/// `handle` keeps the subscription alive. Dropping it unsubscribes automatically.
pub struct ConflictNotifier {
    handle: Option<SubscriptionHandle>,
}

impl ConflictNotifier {
    /// Allocate a new, uninitialised instance.
    ///
    /// `init` must be called before the plugin receives any events.
    fn new() -> Self {
        Self { handle: None }
    }
}

// ─── MantlePlugin implementation ─────────────────────────────────────────────

impl MantlePlugin for ConflictNotifier {
    /// Returns the plugin's stable identifier.
    fn name(&self) -> &str {
        "Conflict Notifier"
    }

    /// Semantic version of the plugin binary.
    fn version(&self) -> Version {
        Version::new(0, 1, 0)
    }

    /// Plugin author name.
    fn author(&self) -> &str {
        "Your Name"
    }

    /// Short description shown in the plugin list UI.
    fn description(&self) -> &str {
        "Posts a warning when the conflict map changes, listing every affected mod."
    }

    /// No elevated capabilities are needed — this plugin only reads events.
    fn capabilities(&self) -> Vec<Capability> {
        vec![]
    }

    /// Declare the settings this plugin exposes to the user.
    ///
    /// These appear in the plugin settings panel inside Mantle Manager.
    fn settings(&self) -> Vec<PluginSetting> {
        vec![
            PluginSetting {
                key: "min_conflicts_to_notify".into(),
                label: "Minimum conflicts before notifying".into(),
                description: "Only show a notification when the total conflict count is at or \
                               above this number. Set to 0 to always notify."
                    .into(),
                default: SettingValue::Int(1),
            },
            PluginSetting {
                key: "show_affected_mods".into(),
                label: "List affected mods in notification".into(),
                description: "Include the names of affected mods in the notification message."
                    .into(),
                default: SettingValue::Bool(true),
            },
        ]
    }

    /// Called once after the plugin is loaded.
    ///
    /// Reads settings and subscribes to [`ModManagerEvent::ConflictMapUpdated`].
    ///
    /// # Parameters
    /// - `ctx`: Shared plugin context — holds the event bus, mod list, settings, etc.
    ///
    /// # Errors
    /// Returns [`PluginError`] if subscription fails (currently infallible, but
    /// callers should propagate the error).
    fn init(&mut self, ctx: Arc<PluginContext>) -> Result<(), PluginError> {
        // Clone Arc for the move closure below.
        let ctx_clone = Arc::clone(&ctx);

        // Subscribe to conflict map updates. The returned handle is stored on
        // self so the subscription stays alive; dropping it unsubscribes.
        self.handle = Some(ctx.subscribe(
            EventFilter::ConflictMapUpdated,
            move |event| {
                if let ModManagerEvent::ConflictMapUpdated {
                    affected_mods,
                    total_conflicts,
                } = event
                {
                    // Read user-configured threshold.
                    let threshold = match ctx_clone.get_setting("min_conflicts_to_notify") {
                        Some(SettingValue::Int(n)) => n,
                        _ => 1,
                    };

                    if (*total_conflicts as i64) < threshold {
                        return;
                    }

                    let show_mods = matches!(
                        ctx_clone.get_setting("show_affected_mods"),
                        Some(SettingValue::Bool(true))
                    );

                    let message = if show_mods && !affected_mods.is_empty() {
                        format!(
                            "{total_conflicts} file conflict(s) detected. Affected mods: {}",
                            affected_mods.join(", ")
                        )
                    } else {
                        format!("{total_conflicts} file conflict(s) detected.")
                    };

                    ctx_clone.notify(NotifyLevel::Warning, &message);
                }
            },
        ));

        tracing::info!("[conflict_notifier] initialised — watching for conflict map updates");
        Ok(())
    }

    /// Called when the plugin is unloaded or the application exits.
    ///
    /// Drops the subscription handle, which unregisters the event handler.
    fn shutdown(&mut self) {
        self.handle = None;
        tracing::info!("[conflict_notifier] shut down");
    }
}

// ─── Required C exports ───────────────────────────────────────────────────────
//
// Every native Mantle Manager plugin must export exactly these two symbols.
// The host resolves them by name via libloading.

/// Allocate a new plugin instance and return a raw fat pointer.
///
/// The host takes ownership of this pointer and calls `shutdown()` + drops it
/// before unloading the library.
///
/// # Safety
/// The returned pointer is valid for `'static`. The host must not call this
/// function more than once per library load.
#[no_mangle]
pub extern "C" fn create_plugin() -> *mut dyn MantlePlugin {
    let plugin = Box::new(ConflictNotifier::new());
    Box::into_raw(plugin)
}

/// Return the Rust toolchain version this plugin was compiled with.
///
/// The host compares this against its own toolchain to detect ABI mismatches.
/// Mismatch causes the load to be rejected.
///
/// # Safety
/// Returns a pointer to a `'static` nul-terminated C string. Valid for the
/// lifetime of the process.
#[no_mangle]
pub extern "C" fn create_plugin_rustc_version() -> *const std::ffi::c_char {
    // SAFETY: RUSTC_TOOLCHAIN_VERSION is a &'static str that contains no
    // interior nul bytes. CString::new will only fail on interior nuls.
    let s = CString::new(RUSTC_TOOLCHAIN_VERSION).expect("RUSTC_TOOLCHAIN_VERSION contains nul");
    s.into_raw()
}
