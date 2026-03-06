//! Profile service — typed wrapper around `data::profiles` CRUD operations.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{
    data::{
        profiles::{self, InsertProfile, ProfileRecord},
        Database,
    },
    error::MantleError,
    mod_list,
};

/// Service handle for profile operations.
///
/// Obtained from [`AppServices::profile`](super::AppServices::profile).
/// Borrows the shared database for its lifetime.
pub struct ProfileService<'a> {
    db: &'a Arc<Database>,
}

impl<'a> ProfileService<'a> {
    pub(super) fn new(db: &'a Arc<Database>) -> Self {
        Self { db }
    }

    /// Return all profiles ordered by creation time.
    pub fn list(&self) -> Result<Vec<ProfileRecord>, MantleError> {
        self.db.with_conn(profiles::list_profiles)
    }

    /// Return the currently active profile, or `None` if none is set.
    pub fn active(&self) -> Result<Option<ProfileRecord>, MantleError> {
        self.db.with_conn(profiles::get_active_profile)
    }

    /// Insert a new profile and return its primary key.
    pub fn insert(&self, rec: &InsertProfile<'_>) -> Result<i64, MantleError> {
        self.db.with_conn(|conn| profiles::insert_profile(conn, rec))
    }

    /// Make `profile_id` the active profile (deactivates all others).
    pub fn activate(&self, profile_id: i64) -> Result<(), MantleError> {
        self.db.with_conn(|conn| profiles::set_active_profile(conn, profile_id))
    }

    /// Delete `profile_id`.  Returns `true` if a row was removed.
    pub fn delete(&self, profile_id: i64) -> Result<bool, MantleError> {
        self.db.with_conn(|conn| profiles::delete_profile(conn, profile_id))
    }

    /// Return a map from profile ID to the number of mods in that profile.
    pub fn mod_counts(&self) -> Result<HashMap<i64, usize>, MantleError> {
        self.db.with_conn(mod_list::mod_counts_per_profile)
    }

    /// Create a "Default" profile and activate it if no profiles exist.
    ///
    /// This is idempotent — a no-op when profiles are already present.
    pub fn ensure_default(&self) -> Result<(), MantleError> {
        let is_empty = self.list()?.is_empty();
        if is_empty {
            let id = self.insert(&InsertProfile { name: "Default", game_slug: None })?;
            self.activate(id)?;
        }
        Ok(())
    }
}
