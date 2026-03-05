//! Rhai sandbox -- capability restrictions and resource limits for scripted plugins.
//!
//! Builds a hardened [`rhai::Engine`] with:
//! - Operation counts, call stack depth, and collection size limits
//! - Module imports disabled (no filesystem access from scripts)
//! - `print` and `debug` routed to `tracing` (not stdout)
//!
//! # Allowed in scripts
//! - Querying the mod list (read-only)
//! - Subscribing to `ModManagerEvent` events
//! - Reading/writing own plugin settings
//! - Emitting notifications via `notify()`
//!
//! # Prohibited in scripts
//! - Any filesystem I/O (`import` is blocked at the module resolver level)
//! - Network access (no API surface exposed)
//! - Process spawning (no API surface exposed)
//! - `eval` (disabled symbol)
//! - Unbounded loops or recursion (operation + depth limits)
//!
//! # References
//! - `standards/PLUGIN_API.md` §4.3 -- Rhai script restrictions
//! - `standards/PLUGIN_API.md` §7 -- sandbox policy

use rhai::{module_resolvers::StaticModuleResolver, Engine};

// Configuration ----------------------------------------------------------------

/// Configuration for the Rhai sandbox applied to every scripted plugin.
///
/// Passed to [`build_sandboxed_engine`] to produce a restricted [`Engine`].
/// All `Option` fields fall back to their stated defaults when `None`.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Maximum total Rhai operations per script call. Prevents infinite loops.
    ///
    /// Default: `1_000_000`. Set to `0` to disable (not recommended).
    pub max_operations: Option<u64>,

    /// Maximum call stack depth. Prevents stack overflow in deeply recursive scripts.
    ///
    /// Default: `64`.
    pub max_call_stack_depth: Option<usize>,

    /// Maximum string length in bytes. Prevents runaway string concatenation.
    ///
    /// Default: `65_536` (64 KiB).
    pub max_string_size: Option<usize>,

    /// Maximum array length. Prevents unbounded array growth.
    ///
    /// Default: `1_024`.
    pub max_array_size: Option<usize>,

    /// Maximum object-map entry count. Prevents unbounded map growth.
    ///
    /// Default: `1_024`.
    pub max_map_size: Option<usize>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            max_operations: Some(1_000_000),
            max_call_stack_depth: Some(64),
            max_string_size: Some(65_536),
            max_array_size: Some(1_024),
            max_map_size: Some(1_024),
        }
    }
}

// Engine builder ---------------------------------------------------------------

/// Build a sandboxed [`rhai::Engine`] from `config`.
///
/// The returned engine has the standard package loaded (arithmetic, strings,
/// collections) but:
/// - All module imports are blocked via a no-op [`StaticModuleResolver`]
/// - `eval` is disabled
/// - Operation count, call depth, and collection-size limits are applied
/// - `print` and `debug` are routed to `tracing::info!` / `tracing::debug!`
///
/// # Parameters
/// - `config`: Resource limits to apply.
///
/// # Returns
/// A configured [`rhai::Engine`] ready to compile and execute scripts.
#[must_use]
pub fn build_sandboxed_engine(config: &SandboxConfig) -> Engine {
    let mut engine = Engine::new();

    // Resource limits
    if let Some(n) = config.max_operations {
        engine.set_max_operations(n);
    }
    if let Some(n) = config.max_call_stack_depth {
        engine.set_max_call_levels(n);
    }
    if let Some(n) = config.max_string_size {
        engine.set_max_string_size(n);
    }
    if let Some(n) = config.max_array_size {
        engine.set_max_array_size(n);
    }
    if let Some(n) = config.max_map_size {
        engine.set_max_map_size(n);
    }

    // Block all module imports. StaticModuleResolver serves only explicitly
    // pre-registered modules; since we register none, all `import` statements
    // fail -- preventing filesystem and network access via module loading.
    engine.set_module_resolver(StaticModuleResolver::new());

    // Disable eval
    engine.disable_symbol("eval");

    // Route print / debug to tracing
    engine.on_print(|s| tracing::info!(target: "rhai_script", "{}", s));
    engine.on_debug(|s, src, pos| {
        tracing::debug!(
            target: "rhai_script",
            src = ?src,
            line = pos.line(),
            "{}",
            s
        );
    });

    engine
}

// Unit tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn limited_engine(max_ops: u64, max_depth: usize) -> Engine {
        build_sandboxed_engine(&SandboxConfig {
            max_operations: Some(max_ops),
            max_call_stack_depth: Some(max_depth),
            ..SandboxConfig::default()
        })
    }

    /// A script that loops more than `max_operations` times must be terminated.
    #[test]
    fn max_operations_blocks_infinite_loop() {
        let engine = limited_engine(100, 64);
        let result = engine.eval::<i64>("let i = 0; loop { i += 1; }");
        assert!(result.is_err(), "expected operation limit error");
    }

    /// A recursively-called function must be stopped at the configured depth.
    #[test]
    fn max_call_depth_blocks_deep_recursion() {
        let engine = limited_engine(1_000_000, 10);
        let script = r#"
            fn recurse(n) { recurse(n + 1) }
            recurse(0)
        "#;
        let result = engine.eval::<i64>(script);
        assert!(result.is_err(), "expected call stack overflow");
    }

    /// `import` must always fail -- the StaticModuleResolver has no modules.
    #[test]
    fn module_import_is_blocked() {
        let engine = build_sandboxed_engine(&SandboxConfig::default());
        let result = engine.eval::<i64>(r#"import "anything" as m; 1"#);
        assert!(result.is_err(), "expected module import to be blocked");
    }

    /// A well-behaved, short script must evaluate successfully.
    #[test]
    fn normal_script_runs_successfully() {
        let engine = build_sandboxed_engine(&SandboxConfig::default());
        let result = engine.eval::<i64>("let x = 1 + 2; x * 3");
        assert_eq!(result.unwrap(), 9);
    }
}
