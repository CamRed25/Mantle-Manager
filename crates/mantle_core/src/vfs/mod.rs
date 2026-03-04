//! VFS — virtual filesystem backend for mod staging.
//!
//! Provides a three-tier overlay stack that presents a merged view of the
//! active mod set over the game's `Data/` directory without modifying any
//! real files on disk.
//!
//! # Tier priority (`ARCHITECTURE.md` §4.2)
//! 1. **[`BackendKind::KernelOverlayfs`]** — native `fsopen`/`fsconfig` API; kernel 6.6+, non-Flatpak
//! 2. **[`BackendKind::FuseOverlayfs`]** — `fuse-overlayfs` binary; always used in Flatpak
//! 3. **[`BackendKind::SymlinkFarm`]** — last resort; always available
//!
//! # Usage
//! ```ignore
//! use mantle_core::vfs::{self, MountParams};
//!
//! // Select the backend once at session start and log it.
//! let kind = vfs::select_backend();
//! tracing::info!(backend = %kind, "VFS backend ready");
//!
//! // Build mount parameters from the active mod list.
//! let params = MountParams {
//!     lower_dirs: mod_dirs,   // index 0 = highest-priority mod
//!     merge_dir: game_data,
//! };
//!
//! // Enter an isolated mount namespace (recommended — auto-cleanup on crash).
//! if vfs::is_namespace_available() {
//!     vfs::enter_mount_namespace()?;
//! }
//!
//! // Mount when the user launches the game.
//! let handle = vfs::mount_with(kind, params)?;
//!
//! // … game runs …
//!
//! // Tear down cleanly when the game exits.
//! handle.unmount()?;
//! ```
//!
//! # Startup cleanup
//! Call [`teardown_stale`] on the known merge path before mounting to remove
//! any overlay left behind by a previous crash.
//!
//! # Large mod lists
//! For more than [`STACK_TRIGGER`] mods, use [`mount_stacked`] instead of
//! [`mount_with`].
//!
//! # Module layout
//! - [`mount`]     — public mount lifecycle ([`mount`], [`mount_with`], [`MountHandle`])
//! - [`cleanup`]   — stale mount detection and teardown ([`teardown_stale`])
//! - [`namespace`] — mount namespace isolation ([`enter_mount_namespace`])
//! - [`stacking`]  — nested overlay stacking ([`mount_stacked`], [`STACK_TRIGGER`])
//! - [`detect`]    — environment probes (Flatpak, kernel version, binary availability)
//! - [`backend`]   — backend selection logic and [`BackendKind`] enum
//! - [`types`]     — shared parameter types ([`MountParams`])

pub mod backend;
pub mod cleanup;
pub mod detect;
pub mod mount;
pub mod namespace;
pub mod stacking;
pub mod types;

// Re-export the primary public API at the `vfs` level.
pub use backend::{select_backend, BackendKind, SymlinkFarm};
pub use cleanup::teardown_stale;
pub use detect::{
    fuse_overlayfs_available, has_new_mount_api, is_flatpak, kernel_version, parse_kernel_version,
};
pub use mount::{mount, mount_with, MountHandle};
pub use namespace::{enter_mount_namespace, is_namespace_available};
pub use stacking::{mount_stacked, StackedMount, STACK_TRIGGER};
pub use types::MountParams;

