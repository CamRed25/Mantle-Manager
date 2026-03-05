//! Rhai scripted plugin loader and runtime.
//!
//! Runs `.rhai` scripts as [`MantlePlugin`] instances. Scripts interact with
//! the manager through a sandboxed [`RhaiPluginContext`] API registered with
//! the engine. Native Rust types are never exposed directly.
//!
//! # Script lifecycle
//! 1. `load_scripted_plugin(path)` compiles the script and extracts metadata.
//! 2. `MantlePlugin::init(ctx)` calls the script-defined `init(ctx)` function
//!    and registers any subscriptions the script requested.
//! 3. Event handlers receive a Rhai map describing the event.
//! 4. `MantlePlugin::shutdown()` clears all subscriptions, then calls the
//!    script-defined `shutdown()` function.
//!
//! # Subscription deferred-drain pattern
//! During `init(ctx)`, the script calls `ctx.subscribe(filter, fn_ptr)`. This
//! stores the `(filter_str, FnPtr)` pair in `sub_requests` without re-entering
//! the engine. Once `call_fn("init")` returns (engine lock released), Rust
//! drains the buffer and creates real [`SubscriptionHandle`]s. This avoids a
//! deadlock from calling back into the engine inside a running eval.
//!
//! # References
//! - `standards/PLUGIN_API.md` §4.3 — Rhai restrictions
//! - `standards/PLUGIN_API.md` §6   — plugin discovery and metadata
//! - `standards/PLUGIN_API.md` §9.2 — minimal valid script example

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use rhai::{Dynamic, Engine, FnPtr, Scope, AST};
use semver::Version;

use super::sandbox::{build_sandboxed_engine, SandboxConfig};
use super::{
    EventFilter, MantlePlugin, ModInfo, ModManagerEvent, NotifyLevel, PluginContext, PluginError,
    PluginSetting, SettingValue, SubscriptionHandle, PLUGIN_API_VERSION,
};
use crate::error::MantleError;

// ─── Rhai context wrapper ────────────────────────────────────────────────────

/// Rhai-side API handle passed to `init(ctx)`.
///
/// Wraps an [`Arc<PluginContext>`] and a deferred subscription buffer.
/// Methods are registered on the Rhai engine so scripts can call them as
/// `ctx.subscribe(...)`, `ctx.notify(...)`, etc.
///
/// The struct is [`Clone`] so Rhai can pass it by value across function calls.
#[derive(Clone)]
struct RhaiPluginContext {
    inner: Arc<PluginContext>,
    /// Subscriptions requested during `init` but not yet converted to handles.
    /// Drained after `init` returns, outside the engine lock.
    sub_requests: Arc<Mutex<Vec<(String, FnPtr)>>>,
}

// ─── Engine builder ──────────────────────────────────────────────────────────

/// Convert a [`ModInfo`] snapshot to a Rhai map.
///
/// Produces a map with keys: `"id"`, `"slug"`, `"name"`, `"version"`,
/// `"author"`, `"priority"`, `"is_enabled"`, `"install_dir"`.
fn mod_info_to_dynamic(m: &ModInfo) -> Dynamic {
    let mut map = rhai::Map::new();
    map.insert("id".into(), Dynamic::from(m.id));
    map.insert("slug".into(), Dynamic::from(m.slug.clone()));
    map.insert("name".into(), Dynamic::from(m.name.clone()));
    map.insert("version".into(), Dynamic::from(m.version.clone()));
    map.insert("author".into(), Dynamic::from(m.author.clone()));
    map.insert("priority".into(), Dynamic::from(m.priority));
    map.insert("is_enabled".into(), Dynamic::from(m.is_enabled));
    map.insert("install_dir".into(), Dynamic::from(m.install_dir.clone()));
    Dynamic::from(map)
}

/// Convert a [`ModManagerEvent`] to a Rhai map suitable for script handlers.
///
/// Every variant produces a map with at least a `"type"` key for dispatch.
/// The `#[non_exhaustive]` wildcard arm produces `{"type": "Unknown"}` so
/// existing scripts remain valid when new variants are added to the core.
fn event_to_dynamic(event: &ModManagerEvent) -> Dynamic {
    let mut map = rhai::Map::new();
    match event {
        ModManagerEvent::GameLaunching(game) => {
            map.insert("type".into(), Dynamic::from("GameLaunching".to_string()));
            map.insert("game_slug".into(), Dynamic::from(game.slug.clone()));
            map.insert("game_name".into(), Dynamic::from(game.name.clone()));
        }
        ModManagerEvent::GameExited { game, exit_code } => {
            map.insert("type".into(), Dynamic::from("GameExited".to_string()));
            map.insert("game_slug".into(), Dynamic::from(game.slug.clone()));
            map.insert("game_name".into(), Dynamic::from(game.name.clone()));
            map.insert("exit_code".into(), Dynamic::from(i64::from(*exit_code)));
        }
        ModManagerEvent::ModInstalled(m) => {
            map.insert("type".into(), Dynamic::from("ModInstalled".to_string()));
            map.insert("mod".into(), mod_info_to_dynamic(m));
        }
        ModManagerEvent::ModEnabled(m) => {
            map.insert("type".into(), Dynamic::from("ModEnabled".to_string()));
            map.insert("mod".into(), mod_info_to_dynamic(m));
        }
        ModManagerEvent::ModDisabled(m) => {
            map.insert("type".into(), Dynamic::from("ModDisabled".to_string()));
            map.insert("mod".into(), mod_info_to_dynamic(m));
        }
        ModManagerEvent::ProfileChanged { old, new } => {
            map.insert("type".into(), Dynamic::from("ProfileChanged".to_string()));
            map.insert("old".into(), Dynamic::from(old.clone()));
            map.insert("new".into(), Dynamic::from(new.clone()));
        }
        ModManagerEvent::OverlayMounted {
            layer_count,
            merged_path,
            ..
        } => {
            map.insert("type".into(), Dynamic::from("OverlayMounted".to_string()));
            map.insert(
                "layer_count".into(),
                Dynamic::from(i64::try_from(*layer_count).unwrap_or(i64::MAX)),
            );
            map.insert(
                "merged_path".into(),
                Dynamic::from(merged_path.to_string_lossy().into_owned()),
            );
        }
        ModManagerEvent::OverlayUnmounted {
            session_duration_secs,
            merged_path,
        } => {
            map.insert("type".into(), Dynamic::from("OverlayUnmounted".to_string()));
            map.insert("duration_secs".into(), Dynamic::from(*session_duration_secs));
            map.insert(
                "merged_path".into(),
                Dynamic::from(merged_path.to_string_lossy().into_owned()),
            );
        }
        ModManagerEvent::ConflictMapUpdated {
            total_conflicts,
            affected_mods,
        } => {
            map.insert("type".into(), Dynamic::from("ConflictMapUpdated".to_string()));
            map.insert(
                "total_conflicts".into(),
                Dynamic::from(i64::try_from(*total_conflicts).unwrap_or(i64::MAX)),
            );
            let mods: rhai::Array =
                affected_mods.iter().map(|s| Dynamic::from(s.clone())).collect();
            map.insert("affected_mods".into(), Dynamic::from(mods));
        }
        ModManagerEvent::DownloadStarted { url, .. } => {
            map.insert("type".into(), Dynamic::from("DownloadStarted".to_string()));
            map.insert("url".into(), Dynamic::from(url.clone()));
        }
        ModManagerEvent::DownloadCompleted { url, result, .. } => {
            map.insert("type".into(), Dynamic::from("DownloadCompleted".to_string()));
            map.insert("url".into(), Dynamic::from(url.clone()));
            map.insert("success".into(), Dynamic::from(result.is_ok()));
        }
    }
    Dynamic::from(map)
}

/// Parse an event filter name string into an [`EventFilter`] variant.
///
/// Returns `None` for unrecognised strings.
fn parse_filter_str(s: &str) -> Option<EventFilter> {
    match s {
        "All" => Some(EventFilter::All),
        "GameLaunching" => Some(EventFilter::GameLaunching),
        "GameExited" => Some(EventFilter::GameExited),
        "ModInstalled" => Some(EventFilter::ModInstalled),
        "ModEnabled" => Some(EventFilter::ModEnabled),
        "ModDisabled" => Some(EventFilter::ModDisabled),
        "ProfileChanged" => Some(EventFilter::ProfileChanged),
        "OverlayMounted" => Some(EventFilter::OverlayMounted),
        "OverlayUnmounted" => Some(EventFilter::OverlayUnmounted),
        "DownloadStarted" => Some(EventFilter::DownloadStarted),
        "DownloadCompleted" => Some(EventFilter::DownloadCompleted),
        "ConflictMapUpdated" => Some(EventFilter::ConflictMapUpdated),
        _ => None,
    }
}

/// Build a sandboxed engine and register the [`RhaiPluginContext`] API.
///
/// Calls [`build_sandboxed_engine`] to get resource limits, then adds all
/// `PluginContext` methods as Rhai functions on the `"PluginContext"` type.
///
/// # Parameters
/// - `config`: Sandbox resource limits.
///
/// # Returns
/// A fully configured engine ready to compile plugin scripts.
#[must_use]
fn build_scripted_engine(config: &SandboxConfig) -> Engine {
    let mut engine = build_sandboxed_engine(config);

    engine.register_type_with_name::<RhaiPluginContext>("PluginContext");

    // ctx.subscribe("FilterName", Fn("handler"))
    engine.register_fn("subscribe", |ctx: &mut RhaiPluginContext, filter: String, fp: FnPtr| {
        ctx.sub_requests.lock().expect("sub_requests lock poisoned").push((filter, fp));
    });

    // ctx.notify("Info"|"Warning"|"Error", "message")
    engine.register_fn("notify", |ctx: &mut RhaiPluginContext, level: String, msg: String| {
        let lvl = match level.as_str() {
            "Warning" => NotifyLevel::Warning,
            "Error" => NotifyLevel::Error,
            _ => NotifyLevel::Info,
        };
        ctx.inner.notify(lvl, &msg);
    });

    // ctx.active_profile() -> String
    engine.register_fn("active_profile", |ctx: &mut RhaiPluginContext| -> String {
        ctx.inner.active_profile()
    });

    // ctx.profiles() -> Array of String
    engine.register_fn("profiles", |ctx: &mut RhaiPluginContext| -> rhai::Array {
        ctx.inner.profiles().into_iter().map(Dynamic::from).collect()
    });

    // ctx.mod_list() -> Array of maps
    engine.register_fn("mod_list", |ctx: &mut RhaiPluginContext| -> rhai::Array {
        ctx.inner.mod_list().iter().map(mod_info_to_dynamic).collect()
    });

    // ctx.get_setting("key") -> Dynamic (UNIT if absent)
    engine.register_fn("get_setting", |ctx: &mut RhaiPluginContext, key: String| -> Dynamic {
        match ctx.inner.get_setting(&key) {
            None => Dynamic::UNIT,
            Some(SettingValue::Bool(b)) => Dynamic::from(b),
            Some(SettingValue::Int(i)) => Dynamic::from(i),
            Some(SettingValue::Float(f)) => Dynamic::from(f),
            Some(SettingValue::String(s)) => Dynamic::from(s),
        }
    });

    // ctx.set_setting("key", bool|i64|f64|String) — four type overloads
    engine.register_fn("set_setting", |ctx: &mut RhaiPluginContext, key: String, val: bool| {
        let _ = ctx.inner.set_setting(key, SettingValue::Bool(val));
    });
    engine.register_fn("set_setting", |ctx: &mut RhaiPluginContext, key: String, val: i64| {
        let _ = ctx.inner.set_setting(key, SettingValue::Int(val));
    });
    engine.register_fn(
        "set_setting",
        |ctx: &mut RhaiPluginContext, key: String, val: rhai::FLOAT| {
            let _ = ctx.inner.set_setting(key, SettingValue::Float(val));
        },
    );
    engine.register_fn("set_setting", |ctx: &mut RhaiPluginContext, key: String, val: String| {
        let _ = ctx.inner.set_setting(key, SettingValue::String(val));
    });

    engine
}

/// Returns `true` if the Rhai error is a "function not found" variant.
///
/// Used to treat missing `init` / `shutdown` functions as a no-op rather than
/// an error, per the plugin spec.
fn is_fn_not_found(e: &rhai::EvalAltResult) -> bool {
    matches!(e, rhai::EvalAltResult::ErrorFunctionNotFound(_, _))
}

/// Call a no-arg script function that returns a `String` metadata value.
///
/// # Parameters
/// - `engine`: The compiled engine instance.
/// - `ast`: Compiled AST of the script.
/// - `fn_name`: Name of the function to call.
///
/// # Returns
/// The string returned by the function.
///
/// # Errors
/// Returns [`MantleError::Plugin`] if the function is not found or returns
/// the wrong type.
fn extract_str(engine: &Engine, ast: &AST, fn_name: &str) -> Result<String, MantleError> {
    let mut scope = Scope::new();
    engine
        .call_fn::<String>(&mut scope, ast, fn_name, ())
        .map_err(|e| MantleError::Plugin(format!("script missing '{fn_name}': {e}")))
}

// ─── ScriptedPlugin ----------------------------------------------------------

/// A compiled Rhai script running as a plugin.
///
/// Implements [`MantlePlugin`] by:
/// - Sourcing metadata from script-defined constant functions.
/// - Calling `init(ctx)` and `shutdown()` on the script lifecycle.
/// - Registering event handlers by translating Rhai `FnPtr`s to synchronous
///   Rust closures that call back into the script via the engine.
pub struct ScriptedPlugin {
    id: String,
    name: String,
    version: Version,
    author: String,
    description: String,
    required_api: Version,
    /// Sandboxed engine. `Arc` (not Mutex) because `Engine: Sync` with the
    /// `sync` rhai feature, and `call_fn` takes `&self`.
    engine: Arc<Engine>,
    /// Compiled AST shared between lifecycle calls and event handlers.
    ast: Arc<AST>,
    /// Active subscription handles. Cleared in `shutdown()`.
    handles: Vec<SubscriptionHandle>,
}

impl std::fmt::Debug for ScriptedPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptedPlugin")
            .field("id", &self.id)
            .field("version", &self.version.to_string())
            .finish_non_exhaustive()
    }
}

impl MantlePlugin for ScriptedPlugin {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn version(&self) -> Version {
        self.version.clone()
    }
    fn author(&self) -> &str {
        &self.author
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn required_api_version(&self) -> Version {
        self.required_api.clone()
    }

    /// Call the script's `init(ctx)` function and register any subscriptions.
    ///
    /// # Errors
    /// Returns [`PluginError::InitFailed`] if the script's `init` function
    /// returns an error. A missing `init` function is treated as success.
    fn init(&mut self, ctx: Arc<PluginContext>) -> Result<(), PluginError> {
        // Create the Rhai context wrapper with a fresh subscription buffer.
        let rhai_ctx = RhaiPluginContext {
            inner: Arc::clone(&ctx),
            sub_requests: Arc::new(Mutex::new(vec![])),
        };

        // Call the script's init function. Missing function = no-op (OK).
        {
            let mut scope = Scope::new();
            match self.engine.call_fn::<()>(&mut scope, &self.ast, "init", (rhai_ctx.clone(),)) {
                Ok(()) => {}
                Err(e) if is_fn_not_found(&e) => {} // init not defined -- allowed
                Err(e) => return Err(PluginError::InitFailed(e.to_string())),
            }
        }

        // Drain subscription requests accumulated during init().
        // The engine is NOT locked here — no deadlock risk.
        let sub_reqs: Vec<(String, FnPtr)> = rhai_ctx
            .sub_requests
            .lock()
            .expect("sub_requests lock poisoned")
            .drain(..)
            .collect();

        for (filter_str, fp) in sub_reqs {
            let Some(filter) = parse_filter_str(&filter_str) else {
                tracing::warn!(
                    "scripted plugin '{}': unknown event filter '{}', skipping",
                    self.id,
                    filter_str
                );
                continue;
            };
            let engine_arc = Arc::clone(&self.engine);
            let ast_arc = Arc::clone(&self.ast);
            let handle = ctx.subscribe(filter, move |event| {
                let event_dyn = event_to_dynamic(event);
                if let Err(e) = fp.call::<()>(&engine_arc, &ast_arc, (event_dyn,)) {
                    tracing::warn!("scripted plugin event handler error: {e}");
                }
            });
            self.handles.push(handle);
        }
        Ok(())
    }

    /// Drop all subscription handles, then call `shutdown()` in the script.
    fn shutdown(&mut self) {
        // Unsubscribe all event handlers before calling script shutdown.
        self.handles.clear();
        // Call the script's shutdown function (best-effort; errors are logged).
        let mut scope = Scope::new();
        if let Err(e) = self.engine.call_fn::<()>(&mut scope, &self.ast, "shutdown", ()) {
            if !is_fn_not_found(&e) {
                tracing::warn!("scripted plugin '{}': shutdown error: {e}", self.id);
            }
        }
    }

    fn settings(&self) -> Vec<PluginSetting> {
        vec![]
    }
}

// ─── Loader ------------------------------------------------------------------

/// Load, sandbox, and compile a Rhai script plugin from `path`.
///
/// # Steps
/// 1. Read the source file.
/// 2. Build a sandboxed engine with [`build_scripted_engine`].
/// 3. Compile the script into an AST.
/// 4. Extract metadata by calling the required metadata functions.
/// 5. Verify `plugin_required_api_version` is compatible with
///    [`PLUGIN_API_VERSION`].
///
/// # Parameters
/// - `path`: Filesystem path to the `.rhai` script file.
///
/// # Returns
/// A [`ScriptedPlugin`] ready for [`MantlePlugin::init`].
///
/// # Errors
/// [`MantleError::Plugin`] if the file cannot be read, compilation fails,
/// required metadata functions are missing, or API version is incompatible.
pub fn load_scripted_plugin(path: &Path) -> Result<ScriptedPlugin, MantleError> {
    load_scripted_plugin_with_config(path, &SandboxConfig::default())
}

/// Load with a custom [`SandboxConfig`] — useful in tests.
///
/// # Errors
/// Same as [`load_scripted_plugin`].
pub fn load_scripted_plugin_with_config(
    path: &Path,
    config: &SandboxConfig,
) -> Result<ScriptedPlugin, MantleError> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| MantleError::Plugin(format!("cannot read '{}': {e}", path.display())))?;

    let engine = build_scripted_engine(config);

    let ast = engine
        .compile(&source)
        .map_err(|e| MantleError::Plugin(format!("'{}' compile error: {e}", path.display())))?;

    // Extract required metadata
    let id = extract_str(&engine, &ast, "plugin_id")?;
    let name = extract_str(&engine, &ast, "plugin_name")?;
    let version_str = extract_str(&engine, &ast, "plugin_version")?;
    let author = extract_str(&engine, &ast, "plugin_author")?;
    let description = extract_str(&engine, &ast, "plugin_description")?;
    let api_str = extract_str(&engine, &ast, "plugin_required_api_version")?;

    let version = Version::parse(&version_str).map_err(|e| {
        MantleError::Plugin(format!(
            "'{}': invalid plugin_version '{version_str}': {e}",
            path.display()
        ))
    })?;

    let required_api = Version::parse(&api_str).map_err(|e| {
        MantleError::Plugin(format!(
            "'{}': invalid plugin_required_api_version '{api_str}': {e}",
            path.display()
        ))
    })?;

    // API version compatibility check
    let loaded = PLUGIN_API_VERSION.clone();
    if required_api.major != loaded.major || required_api > loaded {
        return Err(MantleError::Plugin(format!(
            "'{}': API version mismatch: script requires {required_api}, host provides {loaded}",
            path.display()
        )));
    }

    Ok(ScriptedPlugin {
        id,
        name,
        version,
        author,
        description,
        required_api,
        engine: Arc::new(engine),
        ast: Arc::new(ast),
        handles: vec![],
    })
}

// ─── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Write `content` to a temporary `.rhai` file and return handles.
    fn temp_script(content: &str) -> (tempfile::NamedTempFile, std::path::PathBuf) {
        let mut f = tempfile::Builder::new().suffix(".rhai").tempfile().expect("create temp file");
        f.write_all(content.as_bytes()).expect("write temp file");
        let path = f.path().to_owned();
        (f, path)
    }

    const MINIMAL_SCRIPT: &str = r#"
        fn plugin_id()                  { "test-plugin" }
        fn plugin_name()                { "Test Plugin" }
        fn plugin_version()             { "1.0.0" }
        fn plugin_author()              { "Test Author" }
        fn plugin_description()         { "A test plugin." }
        fn plugin_required_api_version(){ "1.0.0" }
    "#;

    // ── Loader tests ─────────────────────────────────────────────────────────

    /// Loading from a nonexistent path must return an error.
    #[test]
    fn nonexistent_path_returns_error() {
        let result = load_scripted_plugin(Path::new("/nonexistent/plugin.rhai"));
        assert!(result.is_err(), "expected error for nonexistent path");
    }

    /// A script missing required metadata functions must fail.
    #[test]
    fn missing_metadata_returns_error() {
        let (_f, path) = temp_script("fn unrelated() { 42 }");
        let result = load_scripted_plugin(&path);
        assert!(result.is_err(), "expected error for missing metadata");
    }

    /// A script with a syntax error must fail at load time.
    #[test]
    fn compile_error_returns_error() {
        let (_f, path) = temp_script("this is not valid rhai @@@");
        let result = load_scripted_plugin(&path);
        assert!(result.is_err(), "expected compile error");
    }

    /// A well-formed script must produce correct metadata fields.
    #[test]
    fn metadata_extracted_correctly() {
        let (_f, path) = temp_script(MINIMAL_SCRIPT);
        let plugin = load_scripted_plugin(&path).expect("load should succeed");
        assert_eq!(plugin.id(), "test-plugin");
        assert_eq!(plugin.name(), "Test Plugin");
        assert_eq!(plugin.version(), Version::parse("1.0.0").unwrap());
        assert_eq!(plugin.author(), "Test Author");
        assert_eq!(plugin.description(), "A test plugin.");
    }

    /// A script requiring a future API version must be rejected.
    #[test]
    fn api_version_mismatch_returns_error() {
        let script = r#"
            fn plugin_id()                  { "future-plugin" }
            fn plugin_name()                { "Future Plugin" }
            fn plugin_version()             { "1.0.0" }
            fn plugin_author()              { "Future Author" }
            fn plugin_description()         { "Uses future API." }
            fn plugin_required_api_version(){ "99.0.0" }
        "#;
        let (_f, path) = temp_script(script);
        let result = load_scripted_plugin(&path);
        assert!(result.is_err(), "expected API mismatch error");
    }

    /// A script with no `init` function must still load and init successfully.
    #[test]
    fn init_with_no_init_fn_succeeds() {
        let (_f, path) = temp_script(MINIMAL_SCRIPT);
        let mut plugin = load_scripted_plugin(&path).expect("load should succeed");
        let ctx = PluginContext::for_tests();
        plugin.init(ctx).expect("init should succeed with no init function");
    }

    /// A script with no `shutdown` function must still shutdown cleanly.
    #[test]
    fn shutdown_with_no_shutdown_fn_succeeds() {
        let (_f, path) = temp_script(MINIMAL_SCRIPT);
        let mut plugin = load_scripted_plugin(&path).expect("load should succeed");
        let ctx = PluginContext::for_tests();
        plugin.init(ctx).expect("init ok");
        plugin.shutdown(); // must not panic
    }

    /// A script whose `init` function triggers the operation limit must fail.
    #[test]
    fn sandbox_blocks_infinite_loop_in_init() {
        let script = r#"
            fn plugin_id()                  { "loop-plugin" }
            fn plugin_name()                { "Loop Plugin" }
            fn plugin_version()             { "1.0.0" }
            fn plugin_author()              { "Loop Author" }
            fn plugin_description()         { "Loops forever." }
            fn plugin_required_api_version(){ "1.0.0" }
            fn init(ctx) {
                let i = 0;
                loop { i += 1; }
            }
        "#;
        let config = SandboxConfig {
            max_operations: Some(100),
            ..SandboxConfig::default()
        };
        let (_f, path) = temp_script(script);
        let mut plugin =
            load_scripted_plugin_with_config(&path, &config).expect("load should succeed");
        let ctx = PluginContext::for_tests();
        let result = plugin.init(ctx);
        assert!(result.is_err(), "expected init to fail due to operation limit");
    }
}
