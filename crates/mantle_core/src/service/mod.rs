//! Application service layer — bridges the raw `data` CRUD functions and the UI.
//!
//! `AppServices` owns a shared `Arc<Database>` and hands out lightweight
//! service structs (`ProfileService`, `ModService`) that borrow it.  Call
//! sites in the UI layer construct an `AppServices` once per request and then
//! work through the typed API instead of calling `db.with_conn(...)` directly.
//!
//! # Thread model
//! `AppServices` is `Send + Sync` (because `Arc<Database>` is).  The background
//! state-worker thread holds one long-lived instance; short-lived instances can
//! also be constructed for one-off refreshes.

use std::sync::Arc;

use crate::data::Database;

pub mod mod_service;
pub mod profile;

pub use mod_service::ModService;
pub use profile::ProfileService;

/// Owned handle to all application services.
///
/// Construct once per session (or per refresh cycle) with [`AppServices::new`]
/// and obtain per-domain service handles via [`profile`] and [`mods`].
///
/// [`profile`]: AppServices::profile
/// [`mods`]: AppServices::mods
pub struct AppServices {
    /// Shared database handle.  Stored as `Arc` so `ProfileService` and
    /// `ModService` can borrow it without lifetime complexity.
    pub db: Arc<Database>,
}

impl AppServices {
    /// Create an `AppServices` wrapping the given (already-migrated) database.
    pub fn new(db: Database) -> Self {
        Self { db: Arc::new(db) }
    }

    /// Return a `ProfileService` borrowing this instance's database.
    #[must_use]
    pub fn profile(&self) -> ProfileService<'_> {
        ProfileService::new(&self.db)
    }

    /// Return a `ModService` borrowing this instance's database.
    #[must_use]
    pub fn mods(&self) -> ModService<'_> {
        ModService::new(&self.db)
    }
}
