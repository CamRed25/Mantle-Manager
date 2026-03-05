//! Plugin system — extension loading, `PluginContext` API, and event bus.
//!
//! Provides a sandboxed runtime for both native `.so` plugins (via libloading)
//! and Rhai scripting plugins. Exposes a single `PluginContext` surface so
//! plugins cannot reach internal core state directly.
//!
//! # Referenced API contract
//! - ARCHITECTURE.md §4.6 — plugin/ module layout
//! - `standards/PLUGIN_API.md` — full plugin contract

pub mod context;
pub mod event;
pub mod native;
pub mod registry;
pub mod sandbox;
pub mod scripted;

// ─── Public re-exports ────────────────────────────────────────────────────────

// Event system
pub use event::{EventBus, EventFilter, ModInfo, ModManagerEvent, SubscriptionHandle, VfsBackend};

// Plugin trait + context
pub use context::{
    Capability, DownloadHandle, MantlePlugin, ModState, NotifyLevel, PluginContext, PluginError,
    PluginSetting, SettingValue, PLUGIN_API_VERSION, RUSTC_TOOLCHAIN_VERSION,
};

// Plugin registry
pub use registry::{ManifestCapabilities, PluginLoadError, PluginManifest, PluginRegistry};
