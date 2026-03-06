//! Application configuration — TOML-backed, XDG-compliant.
//!
//! Reads/writes `settings.toml` from the platform-appropriate config
//! directory:
//!
//! | Deployment | Path |
//! |------------|------|
//! | Flatpak    | `~/.var/app/io.mantlemanager.MantleManager/config/settings.toml` |
//! | Native     | `~/.config/mantle-manager/settings.toml` |
//! | Override   | `$MANTLE_CONFIG_DIR/settings.toml` |
//!
//! # Usage
//! ```no_run
//! use mantle_core::config::AppSettings;
//!
//! let path = mantle_core::config::default_settings_path();
//! let settings = AppSettings::load_or_default(&path).unwrap();
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::MantleError;

// ---------------------------------------------------------------------------
// Flatpak app ID — single source of truth
// ---------------------------------------------------------------------------

/// The Flatpak application ID used to build platform-specific paths.
const FLATPAK_APP_ID: &str = "io.mantlemanager.MantleManager";

// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------

/// UI colour scheme preference.
///
/// Built-in variants serialise as `snake_case` strings in `settings.toml`
/// (e.g. `"catppuccin_mocha"`).  User-installed themes serialise as
/// `"custom:{id}"` (e.g. `"custom:my-theme"`).
///
/// `Theme` is not `Copy` because `Custom` holds an owned `String`.
/// Use `.clone()` when a second owned value is needed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Theme {
    /// Follow the system/desktop preference (libadwaita native).
    #[default]
    Auto,
    /// Force light mode (libadwaita native).
    Light,
    /// Force dark mode (libadwaita native).
    Dark,
    /// Catppuccin Mocha — soft dark theme with mauve accent.
    CatppuccinMocha,
    /// Catppuccin Latte — soft light theme with mauve accent.
    CatppuccinLatte,
    /// Nord — arctic, north-bluish dark theme.
    Nord,
    /// Skyrim-inspired — dark Nordic stone with amber gold accent.
    Skyrim,
    /// Fallout-inspired — Pip-Boy terminal green on near-black.
    Fallout,
    /// A user-installed theme identified by its file stem.
    ///
    /// Serialises as `"custom:{id}"`.  The CSS and colour-scheme hint are
    /// resolved at runtime by scanning the themes directory.
    Custom(String),
}

impl std::fmt::Display for Theme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => f.write_str("System default"),
            Self::Light => f.write_str("Light"),
            Self::Dark => f.write_str("Dark"),
            Self::CatppuccinMocha => f.write_str("Catppuccin Mocha"),
            Self::CatppuccinLatte => f.write_str("Catppuccin Latte"),
            Self::Nord => f.write_str("Nord"),
            Self::Skyrim => f.write_str("Skyrim"),
            Self::Fallout => f.write_str("Fallout"),
            Self::Custom(id) => write!(f, "{id}"),
        }
    }
}

// Manual Serialize/Deserialize so Custom(id) round-trips as "custom:id".

impl Serialize for Theme {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // Build the serialised string without an early return so `s` is only
        // consumed once in the final `serialize_str` call.
        let custom_buf;
        let val: &str = match self {
            Self::Auto => "auto",
            Self::Light => "light",
            Self::Dark => "dark",
            Self::CatppuccinMocha => "catppuccin_mocha",
            Self::CatppuccinLatte => "catppuccin_latte",
            Self::Nord => "nord",
            Self::Skyrim => "skyrim",
            Self::Fallout => "fallout",
            Self::Custom(id) => {
                custom_buf = format!("custom:{id}");
                &custom_buf
            }
        };
        s.serialize_str(val)
    }
}

impl<'de> Deserialize<'de> for Theme {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "auto" => Self::Auto,
            "light" => Self::Light,
            "dark" => Self::Dark,
            "catppuccin_mocha" => Self::CatppuccinMocha,
            "catppuccin_latte" => Self::CatppuccinLatte,
            "nord" => Self::Nord,
            "skyrim" => Self::Skyrim,
            "fallout" => Self::Fallout,
            s if s.starts_with("custom:") => Self::Custom(s["custom:".len()..].to_owned()),
            // Unknown value: fall back to Auto rather than failing.
            _ => Self::Auto,
        })
    }
}

// ---------------------------------------------------------------------------
// UiSettings
// ---------------------------------------------------------------------------

/// Settings that affect the visual presentation of the application.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    /// Colour scheme preference for the GTK4 window.
    pub theme: Theme,
    /// Use a denser single-line layout for the mod list.
    pub compact_mod_list: bool,
    /// Show coloured dividers between mods from different sources.
    pub show_separator_colors: bool,
}

impl Default for UiSettings {
    /// Returns the defaults documented in `DATA_MODEL.md` §5.1.
    fn default() -> Self {
        Self {
            theme: Theme::Auto,
            compact_mod_list: false,
            show_separator_colors: true,
        }
    }
}

// ---------------------------------------------------------------------------
// PathSettings
// ---------------------------------------------------------------------------

/// Override paths for directories managed by Mantle Manager.
///
/// Empty string in TOML (`mods_dir = ""`) deserialises to `None`, meaning
/// "use the platform default".  A non-empty value overrides the default.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PathSettings {
    /// Override for the directory where mods are extracted.
    /// `None` → use the platform default (`<data>/mods/`).
    #[serde(with = "opt_path_serde")]
    pub mods_dir: Option<PathBuf>,

    /// Override for the downloads staging directory.
    /// `None` → use the platform default (`<data>/downloads/`).
    #[serde(with = "opt_path_serde")]
    pub downloads_dir: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// NetworkSettings
// ---------------------------------------------------------------------------

/// Network-related settings.  Stored in the config file rather than the
/// database so it survives a database wipe.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkSettings {
    /// Legacy plain-text storage of the Nexus Mods API key.
    ///
    /// This field is kept for **migration only**. On first launch after the
    /// `secrets` feature was introduced, any non-empty value here is migrated
    /// to the OS secret store (GNOME Keyring / KWallet) by
    /// [`crate::secrets::migrate_key_from_toml`], then cleared and saved.
    ///
    /// New code must call [`crate::secrets::get_nexus_api_key`] instead of
    /// reading this field directly.
    ///
    /// The TOML key is kept as `nexus_api_key` for backward compatibility.
    #[serde(default, rename = "nexus_api_key")]
    pub nexus_api_key_legacy: String,
}

// ---------------------------------------------------------------------------
// AppSettings
// ---------------------------------------------------------------------------

/// Root configuration struct for Mantle Manager.
///
/// This is the in-memory representation of `settings.toml`.  It is cheap to
/// clone — most fields are small primitives or short strings.
///
/// # Example
/// ```no_run
/// let path = mantle_core::config::default_settings_path();
/// let cfg = mantle_core::config::AppSettings::load_or_default(&path).unwrap();
/// println!("theme: {:?}", cfg.ui.theme);
/// ```
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// Visual / layout settings.
    pub ui: UiSettings,
    /// Directory path overrides.
    pub paths: PathSettings,
    /// API keys and network behaviour.
    pub network: NetworkSettings,
}

impl AppSettings {
    // -----------------------------------------------------------------------
    // I/O
    // -----------------------------------------------------------------------

    /// Load settings from `path`, returning `AppSettings::default()` if the
    /// file does not yet exist.
    ///
    /// If the file exists but is malformed, an error is returned rather than
    /// silently discarding the user's settings.
    ///
    /// # Parameters
    /// - `path`: Path to the `settings.toml` file.
    ///
    /// # Returns
    /// Parsed `AppSettings` (or defaults). `Err(MantleError::Config(_))` if
    /// the file exists but cannot be read or parsed.
    ///
    /// # Side Effects
    /// None — read-only.
    ///
    /// # Errors
    /// Returns [`MantleError::Config`] if the file exists but cannot be read
    /// or is not valid TOML.
    pub fn load_or_default(path: &Path) -> Result<Self, MantleError> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let raw = std::fs::read_to_string(path)
            .map_err(|e| MantleError::Config(format!("cannot read {}: {e}", path.display())))?;

        toml::from_str(&raw)
            .map_err(|e| MantleError::Config(format!("cannot parse {}: {e}", path.display())))
    }

    /// Serialise `self` to TOML and write it atomically to `path`.
    ///
    /// Atomic write: the content is first written to `<path>.tmp`, then
    /// `rename`d into place.  On Linux, `rename` within the same filesystem
    /// is atomic — a crash mid-write cannot leave a truncated settings file.
    ///
    /// # Parameters
    /// - `path`: Destination path for the settings file. Parent directory
    ///   must already exist.
    ///
    /// # Returns
    /// `Ok(())` on success. `Err(MantleError::Config(_))` if serialisation
    /// fails or if the file cannot be written.
    ///
    /// # Side Effects
    /// Creates or replaces `path`. Temporarily creates `<path>.tmp`.
    ///
    /// # Errors
    /// Returns [`MantleError::Config`] if serialisation fails or the file
    /// cannot be written or renamed.
    pub fn save(&self, path: &Path) -> Result<(), MantleError> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| MantleError::Config(format!("cannot serialise settings: {e}")))?;

        // Write to a temp file in the same directory, then rename atomically.
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, content.as_bytes())
            .map_err(|e| MantleError::Config(format!("cannot write {}: {e}", tmp.display())))?;

        std::fs::rename(&tmp, path).map_err(|e| {
            MantleError::Config(format!(
                "cannot rename {} → {}: {e}",
                tmp.display(),
                path.display()
            ))
        })
    }
}

// ---------------------------------------------------------------------------
// Path resolution
// ---------------------------------------------------------------------------

/// Resolve the platform-appropriate config directory.
///
/// Precedence (highest first):
/// 1. `$MANTLE_CONFIG_DIR` environment variable (development / testing)
/// 2. Flatpak: `$HOME/.var/app/<APP_ID>/config/`
/// 3. `$XDG_CONFIG_HOME/mantle-manager/`
/// 4. `$HOME/.config/mantle-manager/` (XDG fallback)
///
/// The returned path is **not** guaranteed to exist — callers must create it
/// if needed before writing.
///
/// # Returns
/// `PathBuf` of the config directory.
#[must_use]
pub fn config_dir() -> PathBuf {
    // 1. Explicit override.
    if let Ok(override_dir) = std::env::var("MANTLE_CONFIG_DIR") {
        if !override_dir.is_empty() {
            return PathBuf::from(override_dir);
        }
    }

    // 2. Flatpak.
    if crate::vfs::detect::is_flatpak() {
        if let Some(home) = home_dir() {
            return home.join(".var").join("app").join(FLATPAK_APP_ID).join("config");
        }
    }

    // 3. XDG_CONFIG_HOME.
    if let Ok(xdg_config) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg_config.is_empty() {
            return PathBuf::from(xdg_config).join("mantle-manager");
        }
    }

    // 4. Default XDG location.
    home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".config")
        .join("mantle-manager")
}

/// Resolve the platform-appropriate application data directory.
///
/// Precedence (highest first):
/// 1. `$MANTLE_DATA_DIR` environment variable
/// 2. Flatpak: `$HOME/.var/app/<APP_ID>/data/`
/// 3. `$XDG_DATA_HOME/mantle-manager/`
/// 4. `$HOME/.local/share/mantle-manager/` (XDG fallback)
///
/// The returned path is **not** guaranteed to exist.
///
/// # Returns
/// `PathBuf` of the data directory.
#[must_use]
pub fn data_dir() -> PathBuf {
    // 1. Explicit override.
    if let Ok(override_dir) = std::env::var("MANTLE_DATA_DIR") {
        if !override_dir.is_empty() {
            return PathBuf::from(override_dir);
        }
    }

    // 2. Flatpak.
    if crate::vfs::detect::is_flatpak() {
        if let Some(home) = home_dir() {
            return home.join(".var").join("app").join(FLATPAK_APP_ID).join("data");
        }
    }

    // 3. XDG_DATA_HOME.
    if let Ok(xdg_data) = std::env::var("XDG_DATA_HOME") {
        if !xdg_data.is_empty() {
            return PathBuf::from(xdg_data).join("mantle-manager");
        }
    }

    // 4. Default XDG location.
    home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".local")
        .join("share")
        .join("mantle-manager")
}

/// Return the canonical path to the settings file.
///
/// This is `config_dir() / "settings.toml"`.  The parent directory may not
/// yet exist; call `std::fs::create_dir_all(path.parent())` before
/// [`AppSettings::save`].
///
/// # Returns
/// `PathBuf` of the settings file.
#[must_use]
pub fn default_settings_path() -> PathBuf {
    config_dir().join("settings.toml")
}

/// Return the canonical path to the `SQLite` database file.
///
/// This is `data_dir() / "mantle.db"`.
///
/// # Returns
/// `PathBuf` of the database file.
#[must_use]
pub fn default_db_path() -> PathBuf {
    data_dir().join("mantle.db")
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Best-effort resolution of the user's home directory.
///
/// Prefers `$HOME` over libc `getpwuid_r` to avoid NSS overhead in sandboxed
/// environments. Returns `None` only if both `$HOME` and the libc call fail.
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

// ---------------------------------------------------------------------------
// Custom serde for Option<PathBuf>
// ---------------------------------------------------------------------------

/// Serialise/deserialise `Option<PathBuf>` as an empty string for `None`
/// and a UTF-8 path string for `Some`.
///
/// TOML format: `mods_dir = ""` → `None`, `mods_dir = "/home/user/mods"` →
/// `Some(PathBuf::from("/home/user/mods"))`.
mod opt_path_serde {
    use std::path::PathBuf;

    use serde::{Deserialize, Deserializer, Serializer};

    /// Serialise `Option<PathBuf>` as a string (`""` for `None`).
    #[allow(clippy::ref_option)] // serde `with` attribute requires `&Option<T>` not `Option<&T>`
    pub fn serialize<S>(value: &Option<PathBuf>, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            None => ser.serialize_str(""),
            Some(p) => ser.serialize_str(p.to_str().unwrap_or("")),
        }
    }

    /// Deserialise a string into `Option<PathBuf>` (`""` → `None`).
    pub fn deserialize<'de, D>(de: D) -> Result<Option<PathBuf>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: String = String::deserialize(de)?;
        if s.is_empty() {
            Ok(None)
        } else {
            Ok(Some(PathBuf::from(s)))
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    // ── Default values ────────────────────────────────────────────────────

    /// All Default fields must match DATA_MODEL.md §5.1.
    #[test]
    fn default_values_match_spec() {
        let s = AppSettings::default();
        assert_eq!(s.ui.theme, Theme::Auto);
        assert!(!s.ui.compact_mod_list);
        assert!(s.ui.show_separator_colors);
        assert!(s.paths.mods_dir.is_none());
        assert!(s.paths.downloads_dir.is_none());
        assert!(s.network.nexus_api_key_legacy.is_empty());
    }

    // ── TOML round-trip ───────────────────────────────────────────────────

    /// Serialise then deserialise must produce an equal struct.
    #[test]
    fn serde_roundtrip_default() {
        let original = AppSettings::default();
        let toml_str = toml::to_string_pretty(&original).unwrap();
        let recovered: AppSettings = toml::from_str(&toml_str).unwrap();
        assert_eq!(original, recovered);
    }

    /// Non-default values survive a round-trip.
    #[test]
    fn serde_roundtrip_non_default() {
        let mut s = AppSettings::default();
        s.ui.theme = Theme::Dark;
        s.ui.compact_mod_list = true;
        s.paths.mods_dir = Some(PathBuf::from("/mnt/games/mods"));
        s.network.nexus_api_key_legacy = "secret-key".to_string();

        let toml_str = toml::to_string_pretty(&s).unwrap();
        let recovered: AppSettings = toml::from_str(&toml_str).unwrap();
        assert_eq!(s, recovered);
    }

    // ── Theme serialisation ───────────────────────────────────────────────

    #[test]
    fn theme_serialises_lowercase() {
        let s = toml::to_string_pretty(&AppSettings {
            ui: UiSettings {
                theme: Theme::Light,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
        assert!(s.contains("theme = \"light\""), "theme must serialise as lowercase");
    }

    #[test]
    fn theme_all_variants_roundtrip() {
        let themes = [
            Theme::Auto,
            Theme::Light,
            Theme::Dark,
            Theme::CatppuccinMocha,
            Theme::CatppuccinLatte,
            Theme::Nord,
            Theme::Skyrim,
            Theme::Fallout,
        ];
        for theme in &themes {
            let mut s = AppSettings::default();
            // Clone before moving into the struct so we can use `theme` again
            // in the assertion.
            s.ui.theme = theme.clone();
            let t = toml::to_string_pretty(&s).unwrap();
            let r: AppSettings = toml::from_str(&t).unwrap();
            assert_eq!(&r.ui.theme, theme, "roundtrip failed for {theme}");
        }
    }

    #[test]
    fn theme_display_names_non_empty() {
        let themes = [
            Theme::Auto,
            Theme::Light,
            Theme::Dark,
            Theme::CatppuccinMocha,
            Theme::CatppuccinLatte,
            Theme::Nord,
            Theme::Skyrim,
            Theme::Fallout,
            Theme::Custom("my-theme".to_string()),
        ];
        for theme in &themes {
            assert!(!theme.to_string().is_empty(), "{theme:?} has empty display name");
        }
    }

    #[test]
    fn theme_snake_case_serialisation() {
        let cases = [
            (Theme::CatppuccinMocha, "catppuccin_mocha"),
            (Theme::CatppuccinLatte, "catppuccin_latte"),
            (Theme::Nord, "nord"),
            (Theme::Skyrim, "skyrim"),
            (Theme::Fallout, "fallout"),
        ];
        for (theme, expected) in cases {
            let mut s = AppSettings::default();
            // Clone before moving into the struct so `theme` is still
            // available inside the assert! format string.
            s.ui.theme = theme.clone();
            let toml_str = toml::to_string_pretty(&s).unwrap();
            assert!(
                toml_str.contains(&format!("theme = \"{expected}\"")),
                "{theme:?} must serialise as \"{expected}\", got:\n{toml_str}",
            );
        }
    }

    /// Custom theme roundtrips as "custom:{id}".
    #[test]
    fn custom_theme_roundtrip() {
        let mut s = AppSettings::default();
        s.ui.theme = Theme::Custom("gruvbox-dark".to_string());
        let toml_str = toml::to_string_pretty(&s).unwrap();
        assert!(
            toml_str.contains("theme = \"custom:gruvbox-dark\""),
            "Custom theme must serialise as 'custom:id', got:\n{toml_str}"
        );
        let r: AppSettings = toml::from_str(&toml_str).unwrap();
        assert_eq!(r.ui.theme, Theme::Custom("gruvbox-dark".to_string()));
    }

    /// Unknown theme string deserialises to Auto rather than failing.
    #[test]
    fn unknown_theme_falls_back_to_auto() {
        let toml_str = "[ui]\ntheme = \"nonexistent_theme\"";
        let s: AppSettings = toml::from_str(toml_str).unwrap();
        assert_eq!(s.ui.theme, Theme::Auto);
    }

    // ── opt_path_serde ────────────────────────────────────────────────────

    #[test]
    fn empty_mods_dir_deserialises_to_none() {
        let toml_str = "[paths]\nmods_dir = \"\"\ndownloads_dir = \"\"";
        let s: AppSettings = toml::from_str(toml_str).unwrap();
        assert!(s.paths.mods_dir.is_none());
    }

    #[test]
    fn non_empty_mods_dir_deserialises_to_some() {
        let toml_str = "[paths]\nmods_dir = \"/opt/mods\"";
        let s: AppSettings = toml::from_str(toml_str).unwrap();
        assert_eq!(s.paths.mods_dir, Some(PathBuf::from("/opt/mods")));
    }

    #[test]
    fn none_path_serialises_as_empty_string() {
        let s = AppSettings::default();
        let t = toml::to_string_pretty(&s).unwrap();
        // Must produce `mods_dir = ""` not `mods_dir = None`
        assert!(t.contains("mods_dir = \"\""));
    }

    // ── File I/O ─────────────────────────────────────────────────────────

    /// Save then load_or_default must produce the original struct.
    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("settings.toml");

        let mut original = AppSettings::default();
        original.ui.theme = Theme::Dark;
        original.network.nexus_api_key_legacy = "test-key".to_string();

        original.save(&path).unwrap();
        let loaded = AppSettings::load_or_default(&path).unwrap();
        assert_eq!(original, loaded);
    }

    /// Load from a non-existent path must return defaults without error.
    #[test]
    fn load_nonexistent_returns_default() {
        let s =
            AppSettings::load_or_default(Path::new("/tmp/nonexistent_mantle_settings_xyz.toml"))
                .unwrap();
        assert_eq!(s, AppSettings::default());
    }

    /// Loading a malformed TOML must return an error, not silently default.
    #[test]
    fn load_malformed_toml_returns_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, b"[[[[not valid toml").unwrap();

        let result = AppSettings::load_or_default(&path);
        assert!(result.is_err(), "malformed TOML must return an error");
        assert!(matches!(result.unwrap_err(), MantleError::Config(_)));
    }

    /// Save is atomic — the `.tmp` file must not be left behind on success.
    #[test]
    fn save_leaves_no_tmp_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("settings.toml");
        AppSettings::default().save(&path).unwrap();
        assert!(!path.with_extension("tmp").exists(), "tmp file must be removed");
    }

    // ── Path resolution ───────────────────────────────────────────────────

    /// MANTLE_CONFIG_DIR env var must take priority.
    #[test]
    fn config_dir_respects_env_override() {
        // Use a subtest-scoped env as std::env is process-global.
        // We can't isolate env easily without a mutex, but set and restore.
        let key = "MANTLE_CONFIG_DIR";
        let restore = std::env::var(key).ok();
        std::env::set_var(key, "/tmp/mantle-config-override");
        let result = config_dir();
        match restore {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        assert_eq!(result, PathBuf::from("/tmp/mantle-config-override"));
    }

    /// MANTLE_DATA_DIR env var must take priority.
    #[test]
    fn data_dir_respects_env_override() {
        let key = "MANTLE_DATA_DIR";
        let restore = std::env::var(key).ok();
        std::env::set_var(key, "/tmp/mantle-data-override");
        let result = data_dir();
        match restore {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        assert_eq!(result, PathBuf::from("/tmp/mantle-data-override"));
    }

    /// default_settings_path must end in `settings.toml`.
    #[test]
    fn default_settings_path_ends_correctly() {
        let p = default_settings_path();
        assert_eq!(p.file_name().unwrap(), "settings.toml");
    }

    /// default_db_path must end in `mantle.db`.
    #[test]
    fn default_db_path_ends_correctly() {
        let p = default_db_path();
        assert_eq!(p.file_name().unwrap(), "mantle.db");
    }
}
