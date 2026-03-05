//! INI file management for per-profile game configuration overrides.
//!
//! Bethesda games read their user settings from `*.ini` files inside the
//! Proton prefix, under:
//! `drive_c/users/steamuser/My Documents/My Games/<game>/`
//!
//! This module provides a hand-rolled parser and writer that:
//! - Preserves comments, blank lines, and key insertion order (via
//!   [`IndexMap`]) for faithful round-trips.
//! - Exposes case-insensitive section and key lookups, because Bethesda INI
//!   keys are conventionally inconsistent about capitalisation.
//! - Provides [`apply_profile_ini`] and [`snapshot_profile_ini`] for
//!   bidirectional synchronisation between the per-profile snapshot directory
//!   and the live game INI directory inside the Proton prefix.
//!
//! # References
//! - `DATA_MODEL.md` §4 — profile directory layout

use std::{
    fs,
    path::{Path, PathBuf},
};

use indexmap::IndexMap;

use crate::error::MantleError;

// ─── Raw line ─────────────────────────────────────────────────────────────────

/// A single line from an INI file, categorised for faithful serialisation.
///
/// We keep the *original* capitalisation of each key and value so that
/// `save()` / `save_to()` never silently alter a pre-existing file.
#[derive(Debug, Clone)]
enum RawLine {
    /// A `[Section]` header.  Contains the exact section name (without the
    /// surrounding brackets) as it appeared in the source.
    Section(String),

    /// A `key = value` line.
    ///
    /// `norm_section` is the lowercase-normalised name of the section this
    /// line belongs to — used by [`GameIni::set`] to find the right line
    /// without an extra pass.
    KeyValue {
        norm_section: String,
        key: String,
        value: String,
    },

    /// A comment (`; …`, `# …`), blank line, or any other content that must
    /// be preserved verbatim.
    Verbatim(String),
}

// ─── GameIni ──────────────────────────────────────────────────────────────────

/// A parsed INI file that preserves comment / blank-line structure.
///
/// Sections and keys are stored in an [`IndexMap`] to maintain insertion
/// order; all lookups normalise to lowercase for case-insensitive comparison.
///
/// # Example
/// ```ignore
/// use mantle_core::game::ini::GameIni;
/// use std::path::Path;
///
/// let mut ini = GameIni::load(Path::new("/path/to/Skyrim.ini"))?;
/// ini.set("Display", "bFull Screen", "1");
/// ini.save()?;
/// ```
pub struct GameIni {
    /// Normalised (lowercase) section name → (normalised key → value).
    sections: IndexMap<String, IndexMap<String, String>>,

    /// The original raw lines from the file.  This is the authoritative
    /// representation used by [`save`] / [`save_to`] to reproduce the file.
    raw_lines: Vec<RawLine>,

    /// The file path this INI was loaded from (used by [`save`]).
    path: PathBuf,
}

impl GameIni {
    // ── Construction ──────────────────────────────────────────────────────────

    /// Load and parse an INI file at `path`.
    ///
    /// Returns an empty `GameIni` (with no sections) if the file does not
    /// exist, so callers can use this without special-casing first-run.
    ///
    /// # Errors
    /// Returns [`MantleError::Io`] if the file exists but cannot be read.
    /// Returns [`MantleError::Config`] if the file contains invalid UTF-8.
    pub fn load(path: &Path) -> Result<Self, MantleError> {
        if !path.exists() {
            return Ok(Self {
                sections: IndexMap::new(),
                raw_lines: Vec::new(),
                path: path.to_path_buf(),
            });
        }

        let raw = fs::read_to_string(path).map_err(MantleError::Io)?;

        Ok(Self::parse(&raw, path.to_path_buf()))
    }

    /// Parse INI text into a `GameIni`.  Used internally and in tests.
    fn parse(text: &str, path: PathBuf) -> Self {
        let mut sections: IndexMap<String, IndexMap<String, String>> = IndexMap::new();
        let mut raw_lines: Vec<RawLine> = Vec::new();
        let mut current_section = String::new();

        for line in text.lines() {
            let trimmed = line.trim();

            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                // Section header — strip brackets to get the name.
                let sec_name = &trimmed[1..trimmed.len() - 1];
                current_section = sec_name.to_lowercase();
                sections.entry(current_section.clone()).or_default();
                raw_lines.push(RawLine::Section(sec_name.to_string()));
            } else if trimmed.starts_with(';') || trimmed.starts_with('#') || trimmed.is_empty() {
                // Comment or blank line — preserve verbatim.
                raw_lines.push(RawLine::Verbatim(line.to_string()));
            } else if let Some(eq_pos) = trimmed.find('=') {
                // key = value pair.
                let key = trimmed[..eq_pos].trim().to_string();
                let value = trimmed[eq_pos + 1..].trim().to_string();
                let norm_key = key.to_lowercase();

                sections
                    .entry(current_section.clone())
                    .or_default()
                    .insert(norm_key, value.clone());

                raw_lines.push(RawLine::KeyValue {
                    norm_section: current_section.clone(),
                    key,
                    value,
                });
            } else {
                // Unknown / unsupported line — preserve verbatim.
                raw_lines.push(RawLine::Verbatim(line.to_string()));
            }
        }

        Self {
            sections,
            raw_lines,
            path,
        }
    }

    // ── Accessors ─────────────────────────────────────────────────────────────

    /// Get the value for a key (case-insensitive section and key matching).
    ///
    /// Returns `None` if the section or key does not exist.
    ///
    /// # Parameters
    /// - `section`: Section name, e.g. `"Display"`.
    /// - `key`: Key name, e.g. `"bFull Screen"`.
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.sections
            .get(&section.to_lowercase())
            .and_then(|s| s.get(&key.to_lowercase()))
            .map(String::as_str)
    }

    // ── Mutations ─────────────────────────────────────────────────────────────

    /// Set (or insert) a value.
    ///
    /// - If the section and key already exist, the value is updated in place
    ///   (both in the in-memory index and in the raw line that will be
    ///   serialised).
    /// - If the section exists but the key is absent, a new line is appended
    ///   after the last key/value line in that section.
    /// - If the section is absent, a new `[Section]` header and key/value line
    ///   are appended to the end of the file.
    ///
    /// # Parameters
    /// - `section`: Target section name (original casing preserved in output).
    /// - `key`: Target key name (original casing preserved in output).
    /// - `value`: New value string.
    pub fn set(&mut self, section: &str, key: &str, value: impl Into<String>) {
        let value = value.into();
        let norm_sec = section.to_lowercase();
        let norm_key = key.to_lowercase();

        // Update the index unconditionally first.
        self.sections
            .entry(norm_sec.clone())
            .or_default()
            .insert(norm_key.clone(), value.clone());

        // Try to update an existing KeyValue line in-place.
        let mut updated = false;
        for line in &mut self.raw_lines {
            if let RawLine::KeyValue {
                norm_section,
                key: k,
                value: v,
            } = line
            {
                if norm_section == &norm_sec && k.to_lowercase() == norm_key {
                    v.clone_from(&value);
                    updated = true;
                    break;
                }
            }
        }

        if updated {
            return;
        }

        // Key not found. Check whether the section header exists.
        let section_exists = self
            .raw_lines
            .iter()
            .any(|l| matches!(l, RawLine::Section(s) if s.to_lowercase() == norm_sec));

        if section_exists {
            // Append the new key/value after the last line belonging to this
            // section (i.e. before the next Section header or end-of-file).
            let insert_pos = self.last_line_of_section(&norm_sec);
            self.raw_lines.insert(
                insert_pos + 1,
                RawLine::KeyValue {
                    norm_section: norm_sec,
                    key: key.to_string(),
                    value,
                },
            );
        } else {
            // New section — append header + key/value at the end.
            self.raw_lines.push(RawLine::Section(section.to_string()));
            self.raw_lines.push(RawLine::KeyValue {
                norm_section: norm_sec,
                key: key.to_string(),
                value,
            });
        }
    }

    /// Return the index of the last `raw_lines` entry that belongs to
    /// `norm_section` (case-insensitive section comparison).
    ///
    /// This is the insertion point for new keys inside an existing section.
    fn last_line_of_section(&self, norm_section: &str) -> usize {
        let mut in_section = false;
        let mut last_pos = 0usize;

        for (i, line) in self.raw_lines.iter().enumerate() {
            match line {
                RawLine::Section(s) => {
                    if s.to_lowercase() == norm_section {
                        in_section = true;
                        last_pos = i;
                    } else if in_section {
                        // Hit the next section — stop.
                        break;
                    }
                }
                RawLine::KeyValue {
                    norm_section: ns, ..
                } if in_section => {
                    if ns == norm_section {
                        last_pos = i;
                    } else {
                        // Guard against orphaned lines.
                        break;
                    }
                }
                RawLine::Verbatim(_) if in_section => {
                    last_pos = i;
                }
                _ => {}
            }
        }

        last_pos
    }

    // ── Serialisation ─────────────────────────────────────────────────────────

    /// Write the INI back to the path it was loaded from.
    ///
    /// # Errors
    /// Returns [`MantleError::Io`] if the file cannot be written.
    pub fn save(&self) -> Result<(), MantleError> {
        self.save_to(&self.path.clone())
    }

    /// Write the INI to `path` (used to create per-profile snapshots).
    ///
    /// Creates the parent directory if it does not exist.
    ///
    /// # Errors
    /// Returns [`MantleError::Io`] if the directory cannot be created or the
    /// file cannot be written.
    pub fn save_to(&self, path: &Path) -> Result<(), MantleError> {
        if let Some(dir) = path.parent() {
            if !dir.exists() {
                fs::create_dir_all(dir).map_err(MantleError::Io)?;
            }
        }

        let mut out = String::new();
        for line in &self.raw_lines {
            match line {
                RawLine::Section(name) => {
                    out.push('[');
                    out.push_str(name);
                    out.push_str("]\n");
                }
                RawLine::KeyValue { key, value, .. } => {
                    out.push_str(key);
                    out.push_str(" = ");
                    out.push_str(value);
                    out.push('\n');
                }
                RawLine::Verbatim(s) => {
                    out.push_str(s);
                    out.push('\n');
                }
            }
        }

        fs::write(path, out).map_err(MantleError::Io)
    }
}

// ─── Profile INI helpers ──────────────────────────────────────────────────────

/// Copy per-profile INI overrides into the active game's Proton prefix.
///
/// Copies every `*.ini` file from `profile_ini_dir` into `game_ini_dir`,
/// creating the destination directory if it does not exist. Existing files
/// are overwritten.
///
/// Returns `Ok(())` immediately (no-op) if `profile_ini_dir` is absent, so
/// newly created profiles that have no saved INIs do not produce an error.
///
/// # Parameters
/// - `profile_ini_dir`: `{data_dir}/profiles/{profile_id}/ini/` — source.
/// - `game_ini_dir`: `drive_c/users/steamuser/My Documents/My Games/{slug}/` — destination.
///
/// # Errors
/// Returns [`MantleError::Io`] if a file copy fails or the destination
/// directory cannot be created.
pub fn apply_profile_ini(profile_ini_dir: &Path, game_ini_dir: &Path) -> Result<(), MantleError> {
    if !profile_ini_dir.exists() {
        return Ok(());
    }

    fs::create_dir_all(game_ini_dir).map_err(MantleError::Io)?;

    for entry in fs::read_dir(profile_ini_dir).map_err(MantleError::Io)? {
        let entry = entry.map_err(MantleError::Io)?;
        let src = entry.path();

        if src
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("ini"))
        {
            let file_name = entry.file_name();
            let dst = game_ini_dir.join(&file_name);
            fs::copy(&src, &dst).map_err(MantleError::Io)?;
            tracing::debug!("apply_profile_ini: {} → {}", src.display(), dst.display());
        }
    }

    Ok(())
}

/// Snapshot the current game INI files into the profile's ini directory.
///
/// The reverse of [`apply_profile_ini`]: copies every `*.ini` file from
/// `game_ini_dir` into `profile_ini_dir`. Called when the user manually saves
/// a profile INI snapshot or before switching away from a profile.
///
/// Returns `Ok(())` immediately (no-op) if `game_ini_dir` is absent.
///
/// # Parameters
/// - `game_ini_dir`: `drive_c/users/steamuser/My Documents/My Games/{slug}/` — source.
/// - `profile_ini_dir`: `{data_dir}/profiles/{profile_id}/ini/` — destination.
///
/// # Errors
/// Returns [`MantleError::Io`] if a file copy fails or the destination
/// directory cannot be created.
pub fn snapshot_profile_ini(
    game_ini_dir: &Path,
    profile_ini_dir: &Path,
) -> Result<(), MantleError> {
    if !game_ini_dir.exists() {
        return Ok(());
    }

    fs::create_dir_all(profile_ini_dir).map_err(MantleError::Io)?;

    for entry in fs::read_dir(game_ini_dir).map_err(MantleError::Io)? {
        let entry = entry.map_err(MantleError::Io)?;
        let src = entry.path();

        if src
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("ini"))
        {
            let file_name = entry.file_name();
            let dst = profile_ini_dir.join(&file_name);
            fs::copy(&src, &dst).map_err(MantleError::Io)?;
            tracing::debug!("snapshot_profile_ini: {} → {}", src.display(), dst.display());
        }
    }

    Ok(())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Sample INI text used across multiple tests. Includes comments and
    /// blank lines to verify round-trip fidelity.
    const SAMPLE_INI: &str = "\
; Skyrim.ini — managed by Mantle Manager
[Display]
bFull Screen = 0
iSize H = 1080
iSize W = 1920

[General]
; This is a comment
uGridsToLoad = 5
sStartingCell =
";

    // ── Parse tests ───────────────────────────────────────────────────────────

    #[test]
    fn parse_sections_and_keys() {
        let ini = GameIni::parse(SAMPLE_INI, PathBuf::from("test.ini"));
        // Both sections present.
        assert!(ini.sections.contains_key("display"));
        assert!(ini.sections.contains_key("general"));
        // Keys normalised.
        assert_eq!(ini.get("Display", "bFull Screen"), Some("0"));
        assert_eq!(ini.get("Display", "iSize H"), Some("1080"));
        assert_eq!(ini.get("General", "uGridsToLoad"), Some("5"));
    }

    #[test]
    fn get_case_insensitive_section_and_key() {
        let ini = GameIni::parse(SAMPLE_INI, PathBuf::from("test.ini"));
        // Both section and key lookups must be case-insensitive.
        assert_eq!(ini.get("DISPLAY", "BFULL SCREEN"), Some("0"));
        assert_eq!(ini.get("display", "isize h"), Some("1080"));
        assert_eq!(ini.get("GENERAL", "UGridsToLoad"), Some("5"));
    }

    #[test]
    fn get_missing_section_returns_none() {
        let ini = GameIni::parse(SAMPLE_INI, PathBuf::from("test.ini"));
        assert!(ini.get("NonExistent", "SomeKey").is_none());
    }

    #[test]
    fn get_missing_key_returns_none() {
        let ini = GameIni::parse(SAMPLE_INI, PathBuf::from("test.ini"));
        assert!(ini.get("Display", "NonExistentKey").is_none());
    }

    // ── Set tests ─────────────────────────────────────────────────────────────

    #[test]
    fn set_updates_existing_key() {
        let mut ini = GameIni::parse(SAMPLE_INI, PathBuf::from("test.ini"));
        ini.set("Display", "bFull Screen", "1");
        assert_eq!(ini.get("Display", "bFull Screen"), Some("1"));
    }

    #[test]
    fn set_creates_key_in_existing_section() {
        let mut ini = GameIni::parse(SAMPLE_INI, PathBuf::from("test.ini"));
        ini.set("Display", "iRefreshRate", "144");
        assert_eq!(ini.get("Display", "iRefreshRate"), Some("144"));
    }

    #[test]
    fn set_creates_new_section_and_key() {
        let mut ini = GameIni::parse(SAMPLE_INI, PathBuf::from("test.ini"));
        ini.set("Grass", "iMaxGrassTypesPerTexure", "15");
        assert!(ini.sections.contains_key("grass"));
        assert_eq!(ini.get("Grass", "iMaxGrassTypesPerTexure"), Some("15"));
    }

    #[test]
    fn set_preserves_section_count() {
        let mut ini = GameIni::parse(SAMPLE_INI, PathBuf::from("test.ini"));
        // Updating an existing key should not create a duplicate section.
        ini.set("Display", "bFull Screen", "1");
        assert_eq!(ini.sections.len(), 2, "no new sections should be created");
    }

    // ── Round-trip tests ──────────────────────────────────────────────────────

    #[test]
    fn round_trip_save_load_preserves_values() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("Skyrim.ini");

        // Parse + save.
        let mut ini = GameIni::parse(SAMPLE_INI, path.clone());
        ini.set("Display", "bFull Screen", "1");
        ini.save_to(&path).unwrap();

        // Reload and verify.
        let reloaded = GameIni::load(&path).unwrap();
        assert_eq!(reloaded.get("Display", "bFull Screen"), Some("1"));
        assert_eq!(reloaded.get("Display", "iSize H"), Some("1080"));
        assert_eq!(reloaded.get("General", "uGridsToLoad"), Some("5"));
    }

    #[test]
    fn round_trip_preserves_section_order() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("Skyrim.ini");

        let ini = GameIni::parse(SAMPLE_INI, path.clone());
        ini.save_to(&path).unwrap();

        let reloaded = GameIni::load(&path).unwrap();
        let section_names: Vec<&String> = reloaded.sections.keys().collect();
        assert_eq!(section_names, &["display", "general"]);
    }

    #[test]
    fn load_absent_file_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("absent.ini");
        let ini = GameIni::load(&path).unwrap();
        assert!(ini.sections.is_empty());
        assert!(ini.get("Anything", "Keys").is_none());
    }

    // ── apply_profile_ini / snapshot_profile_ini ──────────────────────────────

    #[test]
    fn apply_profile_ini_copies_files_and_creates_dirs() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("profile/ini");
        let dst = tmp.path().join("game_dir");

        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("Skyrim.ini"), "[Display]\nbFull Screen = 1\n").unwrap();
        fs::write(src.join("SkyrimPrefs.ini"), "[Launcher]\nbShowFloatingQuestMarkers = 1\n")
            .unwrap();

        apply_profile_ini(&src, &dst).unwrap();

        assert!(dst.join("Skyrim.ini").exists());
        assert!(dst.join("SkyrimPrefs.ini").exists());

        let content = fs::read_to_string(dst.join("Skyrim.ini")).unwrap();
        assert!(content.contains("bFull Screen = 1"));
    }

    #[test]
    fn apply_profile_ini_is_noop_when_src_absent() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("nonexistent");
        let dst = tmp.path().join("game_dir");

        // Must not error even though source doesn't exist.
        apply_profile_ini(&src, &dst).unwrap();
        assert!(!dst.exists(), "dst should not have been created");
    }

    #[test]
    fn apply_profile_ini_skips_non_ini_files() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("ini");
        let dst = tmp.path().join("game");

        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("Skyrim.ini"), "data").unwrap();
        fs::write(src.join("readme.txt"), "text").unwrap();

        apply_profile_ini(&src, &dst).unwrap();

        assert!(dst.join("Skyrim.ini").exists());
        assert!(!dst.join("readme.txt").exists());
    }

    #[test]
    fn snapshot_profile_ini_copies_files_from_game_dir() {
        let tmp = TempDir::new().unwrap();
        let game_dir = tmp.path().join("game");
        let profile_dir = tmp.path().join("profile/ini");

        fs::create_dir_all(&game_dir).unwrap();
        fs::write(game_dir.join("Skyrim.ini"), "[Display]\nbFull Screen = 0\n").unwrap();

        snapshot_profile_ini(&game_dir, &profile_dir).unwrap();

        assert!(profile_dir.join("Skyrim.ini").exists());
        let content = fs::read_to_string(profile_dir.join("Skyrim.ini")).unwrap();
        assert!(content.contains("bFull Screen = 0"));
    }

    #[test]
    fn snapshot_profile_ini_is_noop_when_game_dir_absent() {
        let tmp = TempDir::new().unwrap();
        let game_dir = tmp.path().join("nonexistent");
        let profile_dir = tmp.path().join("profile/ini");

        snapshot_profile_ini(&game_dir, &profile_dir).unwrap();
        assert!(!profile_dir.exists());
    }
}
