//! `mantle_net` — HTTP networking for Mantle Manager.
//!
//! Provides two main subsystems:
//!
//! - **[`download`]**: stream a remote URL to a local file with byte-count
//!   progress callbacks.
//! - **[`nexus`]**: typed client for the Nexus Mods API v1, including mod
//!   search, file listing, and CDN download-link resolution.
//!
//! # Feature gating
//! This crate is listed in the workspace but is only pulled in when the
//! `net` feature is enabled in `mantle_ui`.  All UI code that uses it should
//! be wrapped in `#[cfg(feature = "net")]` guards.
//!
//! # Error handling
//! All public functions return [`error::NetError`] which is re-exported as
//! [`NetError`] at the crate root.

pub mod download;
pub mod error;
pub mod nexus;

/// Crate-level error type — re-exported for convenience.
pub use error::NetError;

/// NXM URL parsing and CDN resolution — re-exported for convenience.
pub use nexus::{parse_nxm_url, resolve_nxm, NxmParams};
