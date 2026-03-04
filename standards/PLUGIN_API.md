# Mantle Manager — Plugin API

> **Scope:** Plugin contract, `PluginContext` boundaries, event bus API, versioning policy, and sandboxing rules.
> **Last Updated:** Mar 3, 2026

---

## 1. Overview

The Mantle Manager plugin system allows third-party code to extend the application without modifying core. Plugins interact with the application exclusively through two sanctioned interfaces:

1. **`PluginContext`** — a capability-gated handle to application state
2. **`EventBus`** — a subscribe/publish channel for application lifecycle events

There is no other interface. A plugin that reaches past these two surfaces is exploiting an implementation detail and will break without warning.

**Two plugin types are supported:**

| Type | Language | Sandbox | Use Case |
|------|----------|---------|---------|
| **Rhai script** | Rhai | Full sandbox — no unsafe possible | Simple automations, event reactions, UI extensions |
| **Native plugin** | Rust (compiled `.so`) | API boundary only | Performance-critical work, system access, archive tools |

Both types use `PluginContext` and `EventBus`. The distinction is which capabilities are available through `PluginContext` — scripts receive a restricted subset, native plugins receive an extended set.

---

## 2. Plugin API Version

```rust
// mantle_core::plugin
// semver::Version::new is not const — use Lazy for runtime initialization.
use once_cell::sync::Lazy;

pub static PLUGIN_API_VERSION: Lazy<semver::Version> =
    Lazy::new(|| semver::Version::new(1, 0, 0));
```

**Versioning policy:**

| Change Type | Version Bump | Example |
|-------------|-------------|---------|
| New event added to `ModManagerEvent` | Minor bump | `1.0.0` → `1.1.0` |
| New capability added to `PluginContext` | Minor bump | `1.1.0` → `1.2.0` |
| Event payload fields added (non-breaking) | Minor bump | `1.2.0` → `1.3.0` |
| Event removed or renamed | Major bump | `1.3.0` → `2.0.0` |
| `PluginContext` method removed or signature changed | Major bump | `2.0.0` → `3.0.0` |
| Plugin trait method added (breaking ABI) | Major bump | `3.0.0` → `4.0.0` |
| Bug fix with no API surface change | Patch bump | `1.0.0` → `1.0.1` |

**Rule (from RULE_OF_LAW §3.6):** Any change to `PluginContext`, the event bus API, or the plugin trait definitions requires:
1. A `PLUGIN_API.md` update in the same commit
2. A `PLUGIN_API_VERSION` bump
3. A note in `futures.md` if the change affects planned plugins

---

## 3. Plugin Trait

### 3.1 Core Trait

All plugins — native and Rhai-backed — implement the `MantlePlugin` trait:

```rust
/// Core trait all Mantle Manager plugins must implement.
pub trait MantlePlugin: Send + Sync {
    /// Plugin's unique identifier. Must be stable across versions.
    /// Used for persistent settings storage and conflict detection.
    /// Convention: reverse-domain or short-slug, e.g. "skse-installer"
    fn id(&self) -> &str;

    /// Human-readable display name.
    fn name(&self) -> &str;

    /// Plugin version. Independent of the plugin API version.
    /// Returns owned — callers do not hold a reference into the plugin.
    fn version(&self) -> semver::Version;

    /// Author name or organization.
    fn author(&self) -> &str;

    /// One-line description shown in the plugin manager UI.
    fn description(&self) -> &str;

    /// Minimum plugin API version this plugin requires.
    /// Mantle Manager will refuse to load plugins that require
    /// a higher API version than PLUGIN_API_VERSION.
    /// Returns owned — callers do not hold a reference into the plugin.
    fn required_api_version(&self) -> semver::Version;

    /// Called once after the plugin is loaded and PluginContext is ready.
    /// Subscribe to events here. Return Err to abort loading.
    fn init(&mut self, ctx: Arc<PluginContext>) -> Result<(), PluginError>;

    /// Called before the plugin is unloaded (app shutdown or manual disable).
    /// Clean up resources, unsubscribe from events.
    fn shutdown(&mut self);

    /// Plugin settings definitions. Returned settings are shown in the
    /// plugin manager UI and persisted automatically.
    fn settings(&self) -> Vec<PluginSetting> {
        vec![]
    }
}
```

### 3.2 Native Plugin Entry Point

Native `.so` plugins must export a `create_plugin` C-ABI function:

```rust
/// Every native plugin .so must export this symbol.
/// Called by the plugin loader after dlopen().
#[no_mangle]
pub extern "C" fn create_plugin() -> *mut dyn MantlePlugin {
    Box::into_raw(Box::new(MyPlugin::new()))
}
```

The loader calls `create_plugin()`, wraps the returned pointer in a `Box`, and calls `init()`. If `init()` returns `Err`, the plugin is unloaded immediately.

**⚠ ABI Stability Constraint:**

`*mut dyn MantlePlugin` is a Rust fat pointer containing a vtable whose layout is not guaranteed stable across compiler versions. A plugin compiled with a different `rustc` version than the host may silently misalign the vtable, producing undefined behavior with no diagnostic.

**For v1.0, the enforced constraint is: plugins must be compiled with the same rustc version as the host.**

The `PLUGIN_API_VERSION` carries a companion `RUSTC_VERSION` constant for enforcement:

```rust
// mantle_core::plugin — populated at compile time by build.rs
pub static RUSTC_TOOLCHAIN_VERSION: &str = env!("RUSTC_VERSION_STRING");
```

`RUSTC_VERSION_STRING` is not a standard environment variable — it must be emitted by a `build.rs` in every native plugin crate:

```rust
// build.rs — required in every native plugin crate
fn main() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = std::process::Command::new(&rustc)
        .arg("--version")
        .output()
        .expect("failed to run rustc --version");
    let version = String::from_utf8(out.stdout).expect("rustc output is utf8");
    println!("cargo:rustc-env=RUSTC_VERSION_STRING={}", version.trim());
}
```

The plugin loader checks `create_plugin_rustc_version()` — a second required export — against the host's `RUSTC_TOOLCHAIN_VERSION` and rejects mismatched plugins with a clear error:

```rust
/// Every native plugin .so must also export this symbol.
/// Returns the rustc version string the plugin was compiled with.
/// Used by the loader to enforce ABI compatibility.
#[no_mangle]
pub extern "C" fn create_plugin_rustc_version() -> *const std::ffi::c_char {
    concat!(env!("RUSTC_VERSION_STRING"), "\0").as_ptr() as *const std::ffi::c_char
}
```

**Future path:** The `abi_stable` crate provides stable vtable layouts and removes this constraint. Migration to `abi_stable` is tracked in `futures.md`.

### 3.3 Plugin Settings

```rust
pub struct PluginSetting {
    /// Stable key used for persistence. Never change this after shipping.
    pub key: &'static str,
    /// Human-readable label shown in the UI.
    pub label: &'static str,
    /// Optional description shown as a subtitle.
    pub description: Option<&'static str>,
    /// Default value and type discriminant.
    pub default: SettingValue,
}

pub enum SettingValue {
    Bool(bool),
    String(String),
    Int(i64),
    Float(f64),
}
```

---

## 4. PluginContext

`PluginContext` is the only sanctioned interface between plugins and core. It is passed to plugins during `init()` and held for the plugin's lifetime.

### 4.1 Full Interface

```rust
pub struct PluginContext {
    // Internal fields — not pub. All access goes through methods below.
}

impl PluginContext {
    // ── Mod List (read-only) ──────────────────────────────────────────

    /// Snapshot of the current mod list in priority order.
    /// Returns a cloned snapshot — callers do not hold a lock.
    pub fn mod_list(&self) -> Vec<ModInfo>;

    /// State of a single mod by name.
    pub fn mod_state(&self, name: &str) -> Option<ModState>;

    // ── Profile (read-only) ───────────────────────────────────────────

    /// Currently active profile name.
    pub fn active_profile(&self) -> String;

    /// List of all profile names.
    pub fn profiles(&self) -> Vec<String>;

    // ── Game Info (read-only) ─────────────────────────────────────────

    /// Currently managed game, if any.
    pub fn game(&self) -> Option<GameInfo>;

    // ── Event Bus ────────────────────────────────────────────────────

    /// Subscribe to an event type. Handler is called on the tokio runtime.
    /// Returns a SubscriptionHandle — drop it to unsubscribe.
    pub fn subscribe<F>(&self, event: EventFilter, handler: F) -> SubscriptionHandle
    where
        F: Fn(&ModManagerEvent) + Send + Sync + 'static;

    // ── Plugin Settings ───────────────────────────────────────────────

    /// Read a persisted setting value for this plugin.
    pub fn get_setting(&self, key: &str) -> Option<SettingValue>;

    /// Write a setting value. Persisted to SQLite immediately.
    pub fn set_setting(&self, key: &str, value: SettingValue) -> Result<(), PluginError>;

    // ── Download Queue (native plugins only — requires Capability::Downloads) ──

    /// Queue a download. Returns a handle for progress tracking.
    /// Requires: net feature enabled + Capability::Downloads granted.
    pub fn queue_download(&self, url: &str, dest: &Path) -> Result<DownloadHandle, PluginError>;

    // ── Plugin Data Directory ─────────────────────────────────────────

    /// Path to this plugin's private data directory.
    /// Plugins may read and write freely within this directory.
    /// No access to paths outside this directory is granted.
    pub fn data_dir(&self) -> PathBuf;

    // ── Notifications ─────────────────────────────────────────────────

    /// Post a notification to the UI notification area.
    pub fn notify(&self, level: NotifyLevel, message: &str);
}
```

### 4.2 What PluginContext Does NOT Expose

The following are explicitly not available to plugins:

| Not Exposed | Reason |
|-------------|--------|
| SQLite handle or connection | Plugins use `get_setting`/`set_setting` for persistence |
| Raw filesystem paths outside `data_dir()` | Prevents plugins from reading/writing arbitrary files |
| VFS internals or mount handles | Overlay lifecycle is core-owned |
| Mod list write access | Plugins observe state, they do not mutate it |
| Other plugins' contexts or settings | No cross-plugin communication via context |
| Application shutdown or restart | Lifecycle is core-owned |

If a plugin needs something not on this list, the correct path is to request a capability extension — file it in `futures.md` with the use case. Do not reach past `PluginContext`.

### 4.3 Rhai Script Restrictions

Rhai scripts receive a further-restricted view of `PluginContext`. Available in Rhai:

- `mod_list()` — read only
- `active_profile()` — read only
- `game()` — read only
- `get_setting()` / `set_setting()` — own plugin settings only
- `notify()` — notifications
- `subscribe()` — event subscription

Not available in Rhai:
- `queue_download()` — network operations require native plugin
- `data_dir()` — filesystem access not available in Rhai sandbox
- Any capability-gated API

---

## 5. Event Bus

### 5.1 Event Types

```rust
#[derive(Debug, Clone)]
pub enum ModManagerEvent {
    /// Game is about to launch. Pre-flight checks run before this fires.
    /// Overlay is not yet mounted when this fires.
    GameLaunching(GameInfo),

    /// Game process has exited. Overlay teardown begins after this fires.
    GameExited {
        game: GameInfo,
        exit_code: i32,
    },

    /// A mod archive was extracted and registered in the mod list.
    ModInstalled(ModInfo),

    /// A mod was enabled in the active profile.
    ModEnabled(ModInfo),

    /// A mod was disabled in the active profile.
    ModDisabled(ModInfo),

    /// The active profile changed.
    ProfileChanged {
        old: String,
        new: String,
    },

    /// The overlay was successfully mounted.
    OverlayMounted {
        backend: VfsBackend,
        layer_count: usize,
        merged_path: PathBuf,
    },

    /// The overlay was unmounted (game exit, profile change, or manual).
    OverlayUnmounted {
        merged_path: PathBuf,
        session_duration_secs: f64,
    },

    /// A download was queued and started.
    /// Only fires when mantle_net feature is enabled.
    DownloadStarted {
        url: String,
        dest: PathBuf,
    },

    /// A download completed successfully or failed.
    /// Only fires when mantle_net feature is enabled.
    DownloadCompleted {
        url: String,
        dest: PathBuf,
        result: Result<u64, String>, // Ok(bytes) or Err(message)
    },

    /// The conflict map was updated after a mod state change.
    /// Fires after ModInstalled, ModEnabled, or ModDisabled once
    /// the conflict rescan completes. Use this rather than ModEnabled
    /// if your plugin needs the post-rescan conflict state.
    ConflictMapUpdated {
        affected_mods: Vec<String>,
        total_conflicts: usize,
    },
}
```

### 5.2 Event Ordering Guarantees

| Event | Guarantee |
|-------|-----------|
| `GameLaunching` | Fires before overlay mount begins |
| `OverlayMounted` | Fires after mount is verified, before game process starts |
| `GameExited` | Fires after game process exits, before overlay teardown |
| `OverlayUnmounted` | Fires after overlay teardown completes |
| `ModInstalled` | Fires after mod is registered in mod list |
| `ConflictMapUpdated` | Always fires after `ModInstalled`, `ModEnabled`, `ModDisabled` |
| `ProfileChanged` | Fires after old profile state is saved, before new profile loads |

### 5.3 Subscribing to Events

```rust
fn init(&mut self, ctx: Arc<PluginContext>) -> Result<(), PluginError> {
    // Subscribe to a single event type
    let handle = ctx.subscribe(EventFilter::GameLaunching, |event| {
        if let ModManagerEvent::GameLaunching(game) = event {
            tracing::info!("Game launching: {}", game.name);
        }
    });

    // Store the handle — dropping it unsubscribes
    self.subscription = Some(handle);
    Ok(())
}

fn shutdown(&mut self) {
    // Drop the handle to unsubscribe
    self.subscription = None;
}
```

### 5.4 Event Handler Rules

- Handlers are called on the tokio runtime — they must not block
- Handlers must not call back into `PluginContext` methods that could deadlock
- Handlers that need to do significant work should spawn a task:
  ```rust
  ctx.subscribe(EventFilter::ModInstalled, move |event| {
      let ctx = ctx.clone();
      tokio::spawn(async move {
          // do work here
      });
  });
  ```
- Handler panics are caught by the event bus and logged — they do not crash the application

---

## 6. Plugin Discovery

### 6.1 Directory Scanning

Plugins are discovered by scanning the `plugins/` directory at application startup:

```
~/.var/app/io.mantlemanager.MantleManager/data/plugins/   (Flatpak)
~/.local/share/mantle-manager/plugins/                     (native)
```

**Scan rules:**

- `.so` files → attempt to load as native plugins via `libloading`
- `*.rhai` files → load as Rhai scripts
- Subdirectories are not scanned recursively
- Files that fail to load are logged as warnings and skipped — they do not prevent other plugins from loading
- Load order within the directory is alphabetical by filename

### 6.2 Plugin Manifest (Optional)

A plugin may include a `plugin.toml` alongside its `.so` or `.rhai` file. If present, it is used for display in the plugin manager UI before the plugin is loaded:

```toml
# plugin.toml
id = "skse-installer"
name = "SKSE Installer"
version = "1.0.0"
author = "MO2 Linux"
description = "Automatically downloads and installs SKSE for supported games."
required_api_version = "1.0.0"
```

If `plugin.toml` is absent, the metadata is read from the loaded plugin's `MantlePlugin` implementation at runtime.

### 6.3 Plugin Conflicts

If two plugins share the same `id()`, the second one discovered (alphabetically) is rejected with a warning. Plugin IDs must be unique across the `plugins/` directory.

---

## 7. Capability System

### 7.1 Overview

Some `PluginContext` methods require explicit capability grants. Capabilities are declared in `plugin.toml` and shown to the user in the plugin manager before the plugin is enabled:

```toml
# plugin.toml
[capabilities]
required = ["downloads"]
optional = ["notifications"]
```

### 7.2 Defined Capabilities

| Capability | Grants Access To | Shown to User As |
|-----------|-----------------|-----------------|
| `downloads` | `queue_download()`, `DownloadStarted`, `DownloadCompleted` events | "Can queue downloads" |
| `notifications` | `notify()` | "Can show notifications" |

If a plugin calls a capability-gated method without the capability declared, `PluginContext` returns `Err(PluginError::CapabilityNotGranted)`. It does not panic.

### 7.3 Rhai Capability Restrictions

Rhai scripts may not request capabilities beyond the Rhai-allowed subset defined in §4.3. A Rhai script that declares `capabilities.required = ["downloads"]` in its manifest will have the capability silently ignored — downloads require native plugin.

---

## 8. Error Handling

### 8.1 PluginError

```rust
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("plugin init failed: {0}")]
    InitFailed(String),

    #[error("capability '{0}' not granted — declare it in plugin.toml")]
    CapabilityNotGranted(&'static str),

    #[error("setting key '{0}' not found")]
    SettingNotFound(String),

    #[error("setting type mismatch for key '{0}'")]
    SettingTypeMismatch(String),

    #[error("plugin API version {required} required, but loaded API is {loaded}")]
    ApiVersionMismatch {
        required: semver::Version,
        loaded: semver::Version,
    },

    #[error("download not available: net feature not enabled")]
    NetFeatureDisabled,

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
```

### 8.2 Plugin Load Failures

A plugin that fails to load does not prevent other plugins from loading. Load failures are:

- Logged at `warn` level with the plugin filename and error
- Shown in the plugin manager UI as "Failed to load"
- Never fatal to the application

A plugin that panics during `init()` is treated as a load failure — the panic is caught, logged, and the plugin is unloaded.

---

## 9. Writing a Plugin — Quick Reference

### 9.1 Minimal Native Plugin

```rust
use mantle_plugin::{MantlePlugin, PluginContext, PluginError, PluginSetting};
use semver::Version;
use std::sync::Arc;

pub struct MyPlugin {
    subscription: Option<mantle_plugin::SubscriptionHandle>,
}

impl MantlePlugin for MyPlugin {
    fn id(&self) -> &str { "my-plugin" }
    fn name(&self) -> &str { "My Plugin" }
    fn version(&self) -> semver::Version { semver::Version::new(1, 0, 0) }
    fn author(&self) -> &str { "Author Name" }
    fn description(&self) -> &str { "Does something useful." }
    fn required_api_version(&self) -> semver::Version { semver::Version::new(1, 0, 0) }

    fn init(&mut self, ctx: Arc<PluginContext>) -> Result<(), PluginError> {
        let handle = ctx.subscribe(
            mantle_plugin::EventFilter::GameLaunching,
            |event| {
                tracing::info!("Game launching: {:?}", event);
            },
        );
        self.subscription = Some(handle);
        Ok(())
    }

    fn shutdown(&mut self) {
        self.subscription = None;
    }
}

#[no_mangle]
pub extern "C" fn create_plugin() -> *mut dyn MantlePlugin {
    Box::into_raw(Box::new(MyPlugin { subscription: None }))
}

// Required second export — see §3.2 for build.rs setup
#[no_mangle]
pub extern "C" fn create_plugin_rustc_version() -> *const std::ffi::c_char {
    concat!(env!("RUSTC_VERSION_STRING"), "\0").as_ptr() as *const std::ffi::c_char
}
```

### 9.2 Minimal Rhai Script

```rhai
// my_plugin.rhai
// Fires a notification when any game launches.

fn init(ctx) {
    ctx.subscribe("GameLaunching", |event| {
        ctx.notify("info", `Game launching: ${event.game.name}`);
    });
}

fn shutdown() {
    // nothing to clean up
}
```

---

## 10. Cross-References

| Topic | Standard |
|-------|----------|
| Governance and plugin boundary rules | [RULE_OF_LAW.md §3.6, §5.3](RULE_OF_LAW.md) |
| Plugin module structure in mantle_core | [ARCHITECTURE.md §4.6](ARCHITECTURE.md) |
| Unsafe rules for native plugins | [CODING_STANDARDS.md §6.3](CODING_STANDARDS.md) |
| Plugin crate extraction trigger | [ARCHITECTURE.md §2.2](ARCHITECTURE.md) |
| Test coverage for plugin traits | [TESTING_GUIDE.md](TESTING_GUIDE.md) |
