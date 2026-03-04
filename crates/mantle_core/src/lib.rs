//! `mantle_core` — Core library for Mantle Manager.
//!
//! All VFS operations, archive I/O, mod metadata, conflict detection,
//! game detection, profile management, and plugin execution live here.
//! The UI crate treats this as a pure logic library with no GTK4 dependency.
//!
//! # Module layout
//! ```text
//! mantle_core
//! ├── config   — Application configuration (TOML-backed)
//! ├── data     — SQLite layer (mod metadata, profile, load order)
//! ├── archive  — BSA / BA2 / zip archive extraction
//! ├── conflict — File ownership and conflict graph
//! ├── diag     — Post-session diagnostics (cosave check, overwrite classify)
//! ├── game     — Game detection and Proton prefix helpers
//! ├── install  — Post-extraction pipeline (case-fold, BSA/BA2 extract)
//! ├── mod_list — Mod list state and ordering
//! ├── plugin   — Extension/scripting system (PluginContext, EventBus, Rhai)
//! ├── profile  — Profile CRUD and activation
//! ├── vfs      — Virtual filesystem backend (overlayfs / fuse / symlink)
//! └── error    — Root error type (MantleError)
//! ```

pub mod archive;
pub mod config;
pub mod conflict;
pub mod data;
pub mod diag;
pub mod error;
pub mod game;
pub mod install;
pub mod mod_list;
pub mod plugin;
pub mod profile;
pub mod vfs;

/// Re-export the root error type at the crate top level so callers
/// can use `mantle_core::Error` directly.
pub use error::MantleError as Error;
