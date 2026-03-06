//! Mod service — typed wrapper around `mod_list` CRUD operations.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{
    data::Database,
    error::MantleError,
    mod_list::{self, ProfileModEntry},
};

/// Service handle for mod-list operations.
///
/// Obtained from [`AppServices::mods`](super::AppServices::mods).
/// Borrows the shared database for its lifetime.
pub struct ModService<'a> {
    db: &'a Arc<Database>,
}

impl<'a> ModService<'a> {
    pub(super) fn new(db: &'a Arc<Database>) -> Self {
        Self { db }
    }

    /// Return all mods in `profile_id`, ordered by priority ascending.
    ///
    /// # Errors
    /// Returns [`MantleError`] on database failure.
    pub fn list_for_profile(&self, profile_id: i64) -> Result<Vec<ProfileModEntry>, MantleError> {
        self.db.with_conn(|conn| mod_list::list_profile_mods(conn, profile_id))
    }

    /// Enable or disable a mod within a profile.
    ///
    /// Returns `true` if the row was updated, `false` if the mod is not in
    /// the profile.
    ///
    /// # Errors
    /// Returns [`MantleError`] on database failure.
    pub fn set_enabled(
        &self,
        profile_id: i64,
        mod_id: i64,
        enabled: bool,
    ) -> Result<bool, MantleError> {
        self.db
            .with_conn(|conn| mod_list::set_mod_enabled(conn, profile_id, mod_id, enabled))
    }

    /// Return a map from profile ID to the number of mods in that profile.
    ///
    /// # Errors
    /// Returns [`MantleError`] on database failure.
    pub fn counts_per_profile(&self) -> Result<HashMap<i64, usize>, MantleError> {
        self.db.with_conn(mod_list::mod_counts_per_profile)
    }

    /// Add `mod_id` to `profile_id`.
    ///
    /// Returns `true` if inserted, `false` if the mod was already in the
    /// profile (idempotent).
    ///
    /// # Errors
    /// Returns [`MantleError`] on database failure.
    pub fn add_to_profile(&self, profile_id: i64, mod_id: i64) -> Result<bool, MantleError> {
        self.db.with_conn(|conn| mod_list::add_mod_to_profile(conn, profile_id, mod_id))
    }
}
