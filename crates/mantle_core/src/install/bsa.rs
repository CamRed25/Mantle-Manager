//! BSA / BA2 extraction for installed mod directories.
//!
//! After a mod archive (zip / 7z / etc.) is extracted into the mods directory,
//! it may contain `.bsa` or `.ba2` Bethesda archives that Wine/Proton can
//! struggle to load through the VFS overlay.  Extracting them to loose files
//! improves compatibility and avoids redundant decompression at game runtime.
//!
//! # Workflow
//! 1. [`find_bsa_archives`] — scan a mod directory for `.bsa` / `.ba2` files.
//! 2. [`extract_mod_archives`] — extract each one into its containing folder,
//!    optionally deleting the original archive on success.
//!
//! The actual archive parsing delegates to [`crate::archive::bsa`], which uses
//! the `ba2` crate and supports all Bethesda archive generations (TES3 BSA,
//! TES4/SSE BSA, FO4/Starfield BA2).
//!
//! # Game ↔ format mapping
//! | Game(s)                                      | Extension |
//! |----------------------------------------------|-----------|
//! | Morrowind, Oblivion, FO3, NV, Skyrim, SSE/AE | `.bsa`    |
//! | Fallout 4, Fallout 4 VR, Starfield           | `.ba2`    |
//!
//! # Example
//! ```no_run
//! use mantle_core::install::bsa::extract_mod_archives;
//! use std::path::Path;
//!
//! let result = extract_mod_archives(Path::new("/mods/SkyrimTextures"), false);
//! println!("{} extracted, {} failed", result.extracted.len(), result.failed.len());
//! ```

use std::path::{Path, PathBuf};

use crate::error::MantleError;

// ── Public types ──────────────────────────────────────────────────────────────

/// Outcome of an [`extract_mod_archives`] call.
#[derive(Debug, Default)]
pub struct BsaExtractResult {
    /// Archives that were extracted successfully.
    pub extracted: Vec<PathBuf>,
    /// Archives that failed to extract: `(archive_path, error_message)`.
    pub failed: Vec<(PathBuf, String)>,
    /// Archives that were deleted after successful extraction
    /// (only populated when `delete_after` is `true`).
    pub deleted: Vec<PathBuf>,
}

impl BsaExtractResult {
    /// `true` if every archive extracted without error.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.failed.is_empty()
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Find all `.bsa` and `.ba2` archives inside `mod_dir` (recursive).
///
/// The search is case-insensitive on the extension (`.BSA`, `.Ba2`, etc. are
/// all included) so that Windows-authored mods with mixed-case names are
/// handled correctly.
///
/// # Parameters
/// - `mod_dir`: Root of the mod directory to search.
///
/// # Returns
/// A sorted `Vec<PathBuf>` of matching archive paths.  Returns an empty
/// `Vec` if `mod_dir` does not exist or cannot be read.
#[must_use]
pub fn find_bsa_archives(mod_dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    find_recursive(mod_dir, &mut found);
    found.sort();
    found
}

/// Extract all BSA / BA2 archives in `mod_dir` into their containing folders.
///
/// Each archive is extracted to the directory that contains it (i.e. the mod
/// root for top-level archives, a sub-folder otherwise).  Missing parent
/// directories are created as needed by the underlying extractor.
///
/// # Parameters
/// - `mod_dir`: Root of the mod directory to scan.
/// - `delete_after`: If `true`, each archive that extracts without error is
///   removed from disk after extraction.
///
/// # Returns
/// A [`BsaExtractResult`] with per-archive outcomes.  This function never
/// fails at the top level — per-archive errors are captured in
/// [`BsaExtractResult::failed`].
pub fn extract_mod_archives(mod_dir: &Path, delete_after: bool) -> BsaExtractResult {
    let mut result = BsaExtractResult::default();

    for archive in find_bsa_archives(mod_dir) {
        let dest = archive.parent().map_or_else(|| mod_dir.to_path_buf(), Path::to_path_buf);

        match extract_one(&archive, &dest) {
            Ok(()) => {
                tracing::info!("bsa_install: extracted {}", archive.display());
                if delete_after {
                    match std::fs::remove_file(&archive) {
                        Ok(()) => {
                            tracing::debug!("bsa_install: deleted {}", archive.display());
                            result.deleted.push(archive.clone());
                        }
                        Err(e) => {
                            tracing::warn!(
                                "bsa_install: could not delete {}: {e}",
                                archive.display()
                            );
                        }
                    }
                }
                result.extracted.push(archive);
            }
            Err(e) => {
                tracing::warn!("bsa_install: extraction failed for {}: {e}", archive.display());
                result.failed.push((archive, e.to_string()));
            }
        }
    }

    result
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Dispatch to the correct extractor based on file extension.
///
/// # Errors
/// Returns [`MantleError::Archive`] if the extension is unrecognised or if
/// the underlying extractor fails.
fn extract_one(archive: &Path, dest: &Path) -> Result<(), MantleError> {
    match archive
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("bsa") => crate::archive::bsa::extract_bsa(archive, dest),
        Some("ba2") => crate::archive::bsa::extract_ba2(archive, dest),
        _ => Err(MantleError::Archive(format!(
            "unrecognised archive extension: {}",
            archive.display()
        ))),
    }
}

/// Recursively collect `.bsa` / `.ba2` paths under `dir`.
fn find_recursive(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            find_recursive(&path, found);
        } else {
            let is_bsa = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "bsa" | "ba2"));
            if is_bsa {
                found.push(path);
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_dir() -> TempDir {
        TempDir::new().unwrap()
    }

    // ── find_bsa_archives ─────────────────────────────────────────────────

    #[test]
    fn finds_bsa_and_ba2_at_top_level() {
        let dir = make_dir();
        fs::write(dir.path().join("mod.bsa"), b"fake").unwrap();
        fs::write(dir.path().join("textures.ba2"), b"fake").unwrap();
        fs::write(dir.path().join("readme.txt"), b"ignore me").unwrap();

        let found = find_bsa_archives(dir.path());
        assert_eq!(found.len(), 2);
        assert!(found.iter().any(|p| p.ends_with("mod.bsa")));
        assert!(found.iter().any(|p| p.ends_with("textures.ba2")));
    }

    #[test]
    fn finds_archives_in_subdirectories() {
        let dir = make_dir();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/deep.bsa"), b"fake").unwrap();

        let found = find_bsa_archives(dir.path());
        assert_eq!(found.len(), 1);
        assert!(found[0].ends_with("deep.bsa"));
    }

    #[test]
    fn ignores_non_archive_extensions() {
        let dir = make_dir();
        fs::write(dir.path().join("mod.esp"), b"plugin").unwrap();
        fs::write(dir.path().join("readme.txt"), b"text").unwrap();
        fs::write(dir.path().join("archive.zip"), b"zip").unwrap();

        let found = find_bsa_archives(dir.path());
        assert!(found.is_empty());
    }

    #[test]
    fn result_is_sorted() {
        let dir = make_dir();
        fs::write(dir.path().join("z_last.bsa"), b"").unwrap();
        fs::write(dir.path().join("a_first.bsa"), b"").unwrap();
        fs::write(dir.path().join("m_middle.ba2"), b"").unwrap();

        let found = find_bsa_archives(dir.path());
        assert_eq!(found.len(), 3);
        let names: Vec<&str> =
            found.iter().map(|p| p.file_name().unwrap().to_str().unwrap()).collect();
        assert!(names[0] < names[1], "result must be sorted");
        assert!(names[1] < names[2], "result must be sorted");
    }

    #[test]
    fn empty_directory_returns_empty() {
        let dir = make_dir();
        assert!(find_bsa_archives(dir.path()).is_empty());
    }

    #[test]
    fn nonexistent_directory_returns_empty() {
        let found = find_bsa_archives(Path::new("/no/such/dir/xyz"));
        assert!(found.is_empty());
    }

    // ── extract_mod_archives ──────────────────────────────────────────────

    #[test]
    fn invalid_archive_reported_in_failed() {
        let dir = make_dir();
        // Write a file with .bsa extension but garbage content — extraction
        // must fail gracefully and be reported in `failed`.
        fs::write(dir.path().join("garbage.bsa"), b"not a real bsa!!!").unwrap();

        let result = extract_mod_archives(dir.path(), false);
        assert_eq!(result.extracted.len(), 0);
        assert_eq!(result.failed.len(), 1, "invalid archive must appear in failed");
        assert!(!result.is_ok());
        // Archive must still exist (we didn't delete it)
        assert!(dir.path().join("garbage.bsa").exists());
    }

    #[test]
    fn delete_after_false_leaves_archive_on_disk() {
        let dir = make_dir();
        fs::write(dir.path().join("bad.bsa"), b"garbage").unwrap();
        extract_mod_archives(dir.path(), false);
        // Whether extraction fails or not, delete_after=false must not remove it
        assert!(dir.path().join("bad.bsa").exists());
    }

    #[test]
    fn no_archives_gives_empty_result() {
        let dir = make_dir();
        fs::write(dir.path().join("mod.esp"), b"plugin").unwrap();

        let result = extract_mod_archives(dir.path(), false);
        assert!(result.extracted.is_empty());
        assert!(result.failed.is_empty());
        assert!(result.deleted.is_empty());
        assert!(result.is_ok());
    }

    // ── BsaExtractResult::is_ok ───────────────────────────────────────────

    #[test]
    fn is_ok_true_when_no_failures() {
        let r = BsaExtractResult::default();
        assert!(r.is_ok());
    }

    #[test]
    fn is_ok_false_when_failures_present() {
        let mut r = BsaExtractResult::default();
        r.failed.push((PathBuf::from("x.bsa"), "oops".into()));
        assert!(!r.is_ok());
    }
}
