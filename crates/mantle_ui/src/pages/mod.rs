//! Page modules for the main content area of the application window.
//!
//! Each sub-module exposes a single `build(state: &AppState) -> GtkBox`
//! (or equivalent) entry point wired into `window::build_main_content`.
pub mod downloads;
pub mod mods;
#[cfg(feature = "net")]
pub mod nexus_search;
pub mod overview;
pub mod plugins;
pub mod profiles;
pub(crate) mod shared;
