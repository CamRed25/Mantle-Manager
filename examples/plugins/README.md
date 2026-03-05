# Mantle Manager — Plugin Examples

Two types of plugins are supported: **native Rust plugins** compiled to `.so` shared libraries, and **Rhai scripting plugins** loaded at runtime without compilation.

See [standards/PLUGIN_API.md](../../standards/PLUGIN_API.md) for the complete contract.

---

## Examples in this directory

| Directory | Type | What it shows |
|---|---|---|
| [rust/conflict_notifier](rust/conflict_notifier/) | Native Rust | Single-event subscription, settings declaration, UI notifications |
| [rust/session_logger](rust/session_logger/) | Native Rust | Multi-event subscription, writing to `data_dir()`, shared Mutex state |
| [rhai/profile_greeter](rhai/profile_greeter/) | Rhai script | Closures that capture `ctx`, reading/writing settings |
| [rhai/mod_watcher](rhai/mod_watcher/) | Rhai script | Multiple subscriptions, event map field access, counters across calls |

---

## Native Rust plugins

### How they work

A native plugin is a Rust `cdylib` crate that exposes two required C symbols. Mantle Manager loads it with `libloading`, calls `create_plugin()` to get a trait object, then calls `init()` / `shutdown()` on it via the `MantlePlugin` trait.

The plugin communicates with the host exclusively through the `PluginContext` it receives in `init()`.

### Required exports

Every native plugin must export exactly these two functions:

```rust
/// Allocate the plugin and return a raw fat pointer owned by the host.
#[no_mangle]
pub extern "C" fn create_plugin() -> *mut dyn MantlePlugin { ... }

/// Return the Rust toolchain version this binary was compiled with.
/// The host rejects the plugin if this doesn't match its own toolchain.
#[no_mangle]
pub extern "C" fn create_plugin_rustc_version() -> *const std::ffi::c_char { ... }
```

### `Cargo.toml` requirements

```toml
[lib]
crate-type = ["cdylib"]   # required — produces a .so

[dependencies]
mantle_core = { path = "path/to/mantle_core" }
```

### `MantlePlugin` trait

```rust
impl MantlePlugin for MyPlugin {
    fn name(&self) -> &str { "My Plugin" }
    fn version(&self) -> Version { Version::new(0, 1, 0) }
    fn author(&self) -> &str { "Your Name" }
    fn description(&self) -> &str { "What the plugin does." }
    fn capabilities(&self) -> Vec<Capability> { vec![] }
    fn settings(&self) -> Vec<PluginSetting> { vec![] }

    fn init(&mut self, ctx: Arc<PluginContext>) -> Result<(), PluginError> {
        // subscribe, read settings, etc.
        Ok(())
    }

    fn shutdown(&mut self) {
        // drop subscription handles
    }
}
```

### Building a native plugin

```bash
# From the plugin's directory
cargo build --release

# The output is target/release/lib<name>.so
# Copy it to Mantle Manager's plugin directory alongside manifest.toml
```

> **ABI note:** Native plugins must be compiled with the **same Rust toolchain version** as the Mantle Manager binary they are loaded into. `create_plugin_rustc_version()` is checked on load; a mismatch is a hard error.

---

## Rhai scripting plugins

### How they work

A Rhai plugin is a plain `.rhai` text file. Mantle Manager compiles it in a sandboxed engine at startup — no compilation step required. The script interacts with the host through a `PluginContext` handle passed to `init(ctx)`.

### Required functions

```rhai
fn init(ctx) {
    // Subscribe to events, read settings, etc.
    ctx.notify("Info", "Plugin loaded.");
}

fn shutdown() {
    // Subscriptions drop automatically — add cleanup here if needed.
}
```

### `PluginContext` API available in scripts

| Method | Signature | Description |
|---|---|---|
| `ctx.subscribe(filter, closure)` | `(String, Fn)` | Subscribe to events matching `filter` (see filter names below) |
| `ctx.notify(level, message)` | `(String, String)` | Post a UI notification (`"Info"`, `"Warning"`, `"Error"`) |
| `ctx.active_profile()` | `() -> String` | Name of the currently active profile |
| `ctx.profiles()` | `() -> Array` | All known profile names |
| `ctx.mod_list()` | `() -> Array` | Mods in the active profile (maps with `name`, `slug`, `priority`, etc.) |
| `ctx.get_setting(key)` | `(String) -> Dynamic` | Read a plugin setting value (`()` if not set) |
| `ctx.set_setting(key, value)` | `(String, bool\|i64\|String)` | Write a plugin setting |

### Event filter names

Pass these as the first argument to `ctx.subscribe()`:

`"All"` `"GameLaunching"` `"GameExited"` `"ModInstalled"` `"ModEnabled"` `"ModDisabled"` `"ProfileChanged"` `"OverlayMounted"` `"OverlayUnmounted"` `"DownloadStarted"` `"DownloadCompleted"` `"ConflictMapUpdated"`

### Event map fields

Each handler closure receives one argument — a map with a `"type"` key and variant-specific fields:

| Event type | Fields |
|---|---|
| `GameLaunching` | `game_slug`, `game_name` |
| `GameExited` | `game_slug`, `game_name`, `exit_code` |
| `ModInstalled` / `ModEnabled` / `ModDisabled` | `mod` → `{ id, slug, name, version, author, priority, is_enabled, install_dir }` |
| `ProfileChanged` | `old`, `new` |
| `OverlayMounted` | `layer_count`, `merged_path` |
| `OverlayUnmounted` | `duration_secs`, `merged_path` |
| `ConflictMapUpdated` | `total_conflicts`, `affected_mods` (array of slugs) |
| `DownloadStarted` | `url` |
| `DownloadCompleted` | `url`, `success` |

### Example

```rhai
fn init(ctx) {
    ctx.subscribe("ModInstalled", |event| {
        let name = event["mod"]["name"];
        ctx.notify("Info", `Installed: ${name}`);
    });
}

fn shutdown() {}
```

---

## `manifest.toml`

Both plugin types require a `manifest.toml` alongside the plugin file (`.so` or `.rhai`):

```toml
id          = "my_plugin"           # stable unique identifier
name        = "My Plugin"           # display name
version     = "0.1.0"
author      = "Your Name"
description = "What this plugin does."

[capabilities]
required = []    # e.g. ["Downloads"] — host will reject load if not grantable
optional = []    # e.g. ["ModifyModList"] — gracefully degraded if absent
```

---

## Plugin installation

Place the plugin files in the Mantle Manager plugin directory:

```
~/.local/share/mantle-manager/plugins/
    my_plugin/
        manifest.toml
        plugin.rhai          ← Rhai plugin
        # — or —
        libmy_plugin.so      ← Native plugin
```

Mantle Manager scans this directory on startup and loads any valid plugin it finds.
