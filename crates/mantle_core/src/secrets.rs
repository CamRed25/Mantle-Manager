//! OS secret store integration for sensitive credentials.
//!
//! Stores the Nexus Mods API key in the platform secret store (GNOME Keyring /
//! `KWallet` via the D-Bus Secret Service API) instead of plain-text TOML.
//!
//! # Feature gate
//!
//! All real functionality requires the `secrets` feature. When the feature is
//! disabled (e.g., headless CI without a D-Bus session), the public functions
//! compile as no-op stubs that return `None` / `Ok(())`. Callers fall back to
//! reading the `nexus_api_key_legacy` field from `AppSettings`.
//!
//! # Async caution
//!
//! The `keyring` crate v3 spins its own internal tokio runtime on first use.
//! Do **not** call these functions from within an async context — use
//! `tokio::task::spawn_blocking` if you need them from async code.

// ---------------------------------------------------------------------------
// Feature-enabled implementation
// ---------------------------------------------------------------------------

#[cfg(feature = "secrets")]
mod real {
    /// Flatpak app ID — used as the keyring service name.
    const SERVICE: &str = "io.mantlemanager.MantleManager";
    /// Keyring account name for the Nexus Mods API key.
    const NEXUS_KEY_ACCOUNT: &str = "nexus-api-key";
    use crate::error::MantleError;

    fn entry() -> Result<keyring::Entry, MantleError> {
        keyring::Entry::new(SERVICE, NEXUS_KEY_ACCOUNT)
            .map_err(|e| MantleError::Config(format!("keyring entry error: {e}")))
    }

    /// Retrieve the Nexus Mods API key from the OS secret store.
    ///
    /// Returns `None` if no key is stored or the secret service is unavailable.
    /// Errors other than "no entry" are logged as warnings and treated as `None`
    /// so the caller can fall back gracefully.
    pub fn get_nexus_api_key() -> Option<String> {
        match entry().and_then(|e| {
            e.get_password()
                .map_err(|e| MantleError::Config(format!("keyring get error: {e}")))
        }) {
            Ok(key) => Some(key),
            Err(e) => {
                // NoEntry is expected when no key is set yet — log at debug only.
                tracing::debug!("secrets::get_nexus_api_key: {e}");
                None
            }
        }
    }

    /// Store the Nexus Mods API key in the OS secret store.
    ///
    /// # Errors
    /// Returns [`MantleError::Config`] if the keyring entry cannot be created
    /// or the password cannot be stored.
    pub fn set_nexus_api_key(key: &str) -> Result<(), MantleError> {
        entry()?
            .set_password(key)
            .map_err(|e| MantleError::Config(format!("keyring set error: {e}")))
    }

    /// Delete the Nexus Mods API key from the OS secret store.
    ///
    /// Returns `Ok(())` if no entry exists (idempotent).
    ///
    /// # Errors
    /// Returns [`MantleError::Config`] if the keyring entry cannot be created
    /// or the credential cannot be deleted.
    pub fn delete_nexus_api_key() -> Result<(), MantleError> {
        match entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(MantleError::Config(format!("keyring delete error: {e}"))),
        }
    }

    /// Migrate a plain-text key from `settings.toml` into the secret store.
    ///
    /// If `legacy_key` is non-empty:
    /// 1. Write it to the keyring.
    /// 2. Clear `network.nexus_api_key_legacy` in the settings struct.
    /// 3. Save the updated settings to `path`.
    ///
    /// This is a one-time operation — once the key is migrated the legacy
    /// field stays empty and the keyring is the single source of truth.
    ///
    /// # Errors
    /// Returns [`MantleError::Config`] if writing to the keyring fails, or
    /// [`MantleError`] if loading/saving the settings file fails.
    pub fn migrate_key_from_toml(
        legacy_key: &str,
        path: &std::path::Path,
    ) -> Result<(), MantleError> {
        if legacy_key.is_empty() {
            return Ok(());
        }
        set_nexus_api_key(legacy_key)?;

        let mut settings = crate::config::AppSettings::load_or_default(path)?;
        settings.network.nexus_api_key_legacy.clear();
        settings.save(path)
    }
}

// ---------------------------------------------------------------------------
// No-op stubs when the feature is disabled
// ---------------------------------------------------------------------------

#[cfg(not(feature = "secrets"))]
mod real {
    use crate::error::MantleError;

    /// No-op: returns `None` when the `secrets` feature is disabled.
    pub fn get_nexus_api_key() -> Option<String> {
        None
    }

    /// No-op: `Ok(())` when the `secrets` feature is disabled.
    pub fn set_nexus_api_key(_key: &str) -> Result<(), MantleError> {
        Ok(())
    }

    /// No-op: `Ok(())` when the `secrets` feature is disabled.
    pub fn delete_nexus_api_key() -> Result<(), MantleError> {
        Ok(())
    }

    /// No-op: `Ok(())` when the `secrets` feature is disabled.
    pub fn migrate_key_from_toml(
        _legacy_key: &str,
        _path: &std::path::Path,
    ) -> Result<(), MantleError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Public re-exports
// ---------------------------------------------------------------------------

pub use real::{delete_nexus_api_key, get_nexus_api_key, migrate_key_from_toml, set_nexus_api_key};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // These tests always run (no-op path, works without D-Bus in CI).
    // The real keyring path requires a running secret service daemon and is
    // tested manually or in integration environments.

    #[cfg(not(feature = "secrets"))]
    #[test]
    fn get_returns_none_without_secrets_feature() {
        assert!(get_nexus_api_key().is_none());
    }

    #[cfg(not(feature = "secrets"))]
    #[test]
    fn set_is_noop_without_secrets_feature() {
        assert!(set_nexus_api_key("my-key").is_ok());
    }

    #[cfg(not(feature = "secrets"))]
    #[test]
    fn delete_is_noop_without_secrets_feature() {
        assert!(delete_nexus_api_key().is_ok());
    }

    #[cfg(not(feature = "secrets"))]
    #[test]
    fn migrate_is_noop_without_secrets_feature() {
        assert!(migrate_key_from_toml("old-key", std::path::Path::new("/nonexistent.toml")).is_ok());
    }

    /// Round-trip through the real secret store. Requires a running D-Bus
    /// secret service (GNOME Keyring / KWallet). Run with:
    ///   cargo test -p mantle_core --features secrets -- --ignored
    #[cfg(feature = "secrets")]
    #[test]
    #[ignore = "requires a running D-Bus secret service daemon"]
    fn keyring_roundtrip() {
        let key = "mantle-test-key-roundtrip";
        set_nexus_api_key(key).expect("set failed");
        let got = get_nexus_api_key().expect("get returned None");
        assert_eq!(got, key);
        delete_nexus_api_key().expect("delete failed");
        assert!(get_nexus_api_key().is_none());
    }
}
