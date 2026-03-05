//! Native plugin loader — `.so` shared-library loading via `libloading`.
//!
//! Loads Rust (or C-compatible) native plugins from `.so` files. Each plugin
//! must export two C-ABI symbols:
//!
//! - `create_plugin() -> *mut dyn MantlePlugin`\
//!   Allocates and returns the plugin trait object.
//! - `create_plugin_rustc_version() -> *const c_char`\
//!   Returns a NUL-terminated string of the Rust toolchain version used to
//!   compile the plugin (compared against [`RUSTC_TOOLCHAIN_VERSION`]).
//!
//! # Drop ordering
//! [`NativePlugin`] declares `plugin` *before* `_lib` so that Rust drops the
//! trait object (and runs its vtable calls) **before** unloading the library
//! that holds that code. Reversing the order would be unsound.
//!
//! # Safety
//! Loading arbitrary native code is inherently unsafe. Plugins are loaded from
//! the user data directory only, not from network sources.
//!
//! # References
//! - `standards/PLUGIN_API.md` §3.2 — native plugin contract
//! - `standards/PLUGIN_API.md` §5   — ABI version guard

use std::{ffi::CStr, path::Path, sync::Arc};

use libloading::{Library, Symbol};
use semver::Version;

use super::{
    MantlePlugin, PluginContext, PluginError, PluginSetting, PLUGIN_API_VERSION,
    RUSTC_TOOLCHAIN_VERSION,
};
use crate::error::MantleError;

// C-ABI function pointer types ------------------------------------------------

/// Type of the `create_plugin` export: allocates and returns the plugin object.
///
/// # Safety
/// The caller takes ownership of the returned pointer. It **must** be
/// converted to a `Box<dyn MantlePlugin>` via `Box::from_raw`.
///
/// # Note
/// Fat-pointer return types are not FFI-safe in the strict C sense, but this
/// is an intentional Rust-to-Rust ABI. Both sides must share the same
/// `dyn MantlePlugin` vtable layout, which is enforced by the Rust toolchain
/// version check in `load_native_plugin`.
#[allow(improper_ctypes_definitions)]
type CreatePluginFn = unsafe extern "C" fn() -> *mut dyn MantlePlugin;

/// Type of the `create_plugin_rustc_version` export.
///
/// # Safety
/// Returns a pointer to a NUL-terminated static string in the plugin's data
/// segment. The pointer is valid for the lifetime of the loaded library.
type RustcVersionFn = unsafe extern "C" fn() -> *const std::ffi::c_char;

// NativePlugin ----------------------------------------------------------------

/// A loaded native plugin.
///
/// Wraps both the boxed trait object and the `Library` handle that keeps the
/// shared object mapped in memory. Field declaration order is **significant**:
/// `plugin` is dropped first, then `_lib`, so the plugin's destructor runs
/// before the library is unmapped.
pub struct NativePlugin {
    /// The resolved `MantlePlugin` trait object from the shared library.
    /// Dropped **first** — vtable calls complete before the library unloads.
    pub plugin: Box<dyn MantlePlugin>,
    /// The open library handle. Dropped **second** after the plugin is gone.
    _lib: Library,
}

impl std::fmt::Debug for NativePlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativePlugin")
            .field("id", &self.plugin.id())
            .field("version", &self.plugin.version().to_string())
            .finish_non_exhaustive()
    }
}

impl MantlePlugin for NativePlugin {
    fn id(&self) -> &str {
        self.plugin.id()
    }
    fn name(&self) -> &str {
        self.plugin.name()
    }
    fn version(&self) -> Version {
        self.plugin.version()
    }
    fn author(&self) -> &str {
        self.plugin.author()
    }
    fn description(&self) -> &str {
        self.plugin.description()
    }
    fn required_api_version(&self) -> Version {
        self.plugin.required_api_version()
    }

    fn init(&mut self, ctx: Arc<PluginContext>) -> Result<(), PluginError> {
        self.plugin.init(ctx)
    }

    fn shutdown(&mut self) {
        self.plugin.shutdown();
    }

    fn settings(&self) -> Vec<PluginSetting> {
        self.plugin.settings()
    }
}

// Version helpers -------------------------------------------------------------

/// Check that `required` is compatible with `loaded` using semver rules.
///
/// Compatibility requires:
/// 1. Major versions match exactly (breaking change boundary).
/// 2. `required <= loaded` (the host provides at least the requested version).
///
/// # Parameters
/// - `required`: The version the plugin declares it needs.
/// - `loaded`: The version the host actually provides.
///
/// # Returns
/// `Ok(())` if compatible.
///
/// # Errors
/// Returns [`MantleError::Plugin`] if major versions differ or the plugin
/// requires a newer version than the host provides.
pub fn check_api_compat(required: &Version, loaded: &Version) -> Result<(), MantleError> {
    if required.major != loaded.major {
        return Err(MantleError::Plugin(format!(
            "API major version mismatch: plugin requires {required}, host provides {loaded}"
        )));
    }
    if required > loaded {
        return Err(MantleError::Plugin(format!(
            "API version too new: plugin requires {required}, host provides {loaded}"
        )));
    }
    Ok(())
}

// Loader ----------------------------------------------------------------------

/// Load a native `.so` plugin from `path`, verify its ABI, and return it.
///
/// # Steps
/// 1. Open the shared library with `dlopen`.
/// 2. Resolve `create_plugin_rustc_version` and compare with
///    [`RUSTC_TOOLCHAIN_VERSION`] to catch ABI mismatches.
/// 3. Resolve `create_plugin`, call it, and take ownership of the pointer.
/// 4. Call `check_api_compat` to ensure the plugin's required API version is
///    satisfied by the host.
///
/// # Parameters
/// - `path`: Filesystem path to the `.so` plugin file.
///
/// # Returns
/// A [`NativePlugin`] ready for [`MantlePlugin::init`].
///
/// # Errors
/// Returns [`MantleError::Plugin`] if:
/// - The file cannot be opened or does not export the required symbols.
/// - The Rust toolchain versions differ.
/// - The API versions are incompatible.
///
/// # Safety
/// This function executes arbitrary machine code from the loaded library.
/// Only call it with plugins from trusted sources.
pub fn load_native_plugin(path: &Path) -> Result<NativePlugin, MantleError> {
    // Open the shared library.
    // SAFETY: dlopen is inherently unsafe — we trust the caller.
    let lib = unsafe {
        Library::new(path)
            .map_err(|e| MantleError::Plugin(format!("cannot load '{}': {e}", path.display())))?
    };

    // -- Rustc toolchain version guard ----------------------------------------
    // Both the host and the plugin must be compiled with the same Rust toolchain
    // to guarantee a matching vtable layout for `dyn MantlePlugin`.
    //
    // SAFETY: `lib` loaded without error above. `Library::get` retrieves the symbol
    // with the declared C signature; the returned pointer is valid until `lib` is
    // dropped. `CStr::from_ptr` is safe because `create_plugin_rustc_version`
    // is required by contract to return a null-terminated, statically-allocated
    // UTF-8 string that outlives the library.
    let plugin_rustc: &str = unsafe {
        let sym: Symbol<RustcVersionFn> =
            lib.get(b"create_plugin_rustc_version\0").map_err(|e| {
                MantleError::Plugin(format!(
                    "'{}' is missing 'create_plugin_rustc_version': {e}",
                    path.display()
                ))
            })?;
        let ptr = sym();
        CStr::from_ptr(ptr).to_str().map_err(|_| {
            MantleError::Plugin(format!(
                "'{}': rustc version string is not valid UTF-8",
                path.display()
            ))
        })?
    };

    if plugin_rustc != RUSTC_TOOLCHAIN_VERSION {
        return Err(MantleError::Plugin(format!(
            "'{}': Rust toolchain mismatch — plugin {plugin_rustc}, host {RUSTC_TOOLCHAIN_VERSION}",
            path.display()
        )));
    }

    // -- Plugin object construction -------------------------------------------
    //
    // SAFETY: `lib` loaded without error above. `create_plugin` is required by
    // contract to return an owned, non-null heap-allocated `*mut dyn MantlePlugin`.
    // We check for null explicitly and then take ownership via `Box::from_raw`,
    // which is safe given the non-null check and the ownership contract.
    let plugin: Box<dyn MantlePlugin> = unsafe {
        let sym: Symbol<CreatePluginFn> = lib.get(b"create_plugin\0").map_err(|e| {
            MantleError::Plugin(format!("'{}' is missing 'create_plugin': {e}", path.display()))
        })?;
        let raw = sym();
        if raw.is_null() {
            return Err(MantleError::Plugin(format!(
                "'{}': create_plugin returned null",
                path.display()
            )));
        }
        Box::from_raw(raw)
    };

    // -- API version compatibility check -------------------------------------
    check_api_compat(&plugin.required_api_version(), &PLUGIN_API_VERSION)?;

    Ok(NativePlugin { plugin, _lib: lib })
}

// Unit tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn ver(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    /// Exact version match must succeed.
    #[test]
    fn check_api_compat_exact_match() {
        assert!(check_api_compat(&ver("1.0.0"), &ver("1.0.0")).is_ok());
    }

    /// Plugin requiring older patch is fine — host is newer.
    #[test]
    fn check_api_compat_older_required_is_ok() {
        assert!(check_api_compat(&ver("1.0.0"), &ver("1.2.3")).is_ok());
    }

    /// Plugin requiring newer minor than host provides must fail.
    #[test]
    fn check_api_compat_newer_required_fails() {
        assert!(check_api_compat(&ver("1.5.0"), &ver("1.2.0")).is_err());
    }

    /// Major version mismatch must always fail regardless of minor/patch.
    #[test]
    fn check_api_compat_major_mismatch_fails() {
        assert!(check_api_compat(&ver("2.0.0"), &ver("1.9.9")).is_err());
    }

    /// Loading a nonexistent path must return an error.
    #[test]
    fn load_nonexistent_path_returns_error() {
        let result = load_native_plugin(Path::new("/nonexistent/plugin.so"));
        assert!(result.is_err(), "expected an error for nonexistent path");
    }
}
