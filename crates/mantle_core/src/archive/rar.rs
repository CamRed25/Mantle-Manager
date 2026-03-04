//! RAR archive operations via `compress-tools` (libarchive).
//!
//! RAR support is **extraction-only** — libarchive can read RAR4/RAR5 archives
//! but cannot create them.  All public functions are synchronous and should be
//! called from inside `tokio::task::spawn_blocking` by the async wrappers in
//! [`super`].
//!
//! # Note
//! RAR5 support requires libarchive 3.3.0 or newer.

use std::{fs, path::Path};

use compress_tools::{list_archive_files, uncompress_archive, Ownership};

use crate::error::MantleError;

// ── Public API ────────────────────────────────────────────────────────────────

/// Lists all entry paths contained in a RAR archive.
///
/// # Parameters
/// - `path`: Path to the `.rar` file.
///
/// # Returns
/// An ordered `Vec<String>` of all entries (files and directories), or a
/// [`MantleError::Archive`] if the file cannot be opened or parsed.
///
/// # Side Effects
/// Opens the RAR file for reading.
///
/// # Errors
/// Returns [`MantleError::Archive`] if the file cannot be opened or parsed.
pub fn list_rar_files(path: &Path) -> Result<Vec<String>, MantleError> {
    let mut source = fs::File::open(path)
        .map_err(|e| MantleError::Archive(format!("cannot open rar {}: {e}", path.display())))?;
    list_archive_files(&mut source)
        .map_err(|e| MantleError::Archive(format!("rar list error for {}: {e}", path.display())))
}

/// Extracts all files from a RAR archive to `dest`.
///
/// Missing parent directories in `dest` are created automatically.
/// Ownership information from the archive is ignored (safe for sandboxed use).
///
/// # Parameters
/// - `path`: Path to the `.rar` file.
/// - `dest`: Destination directory for extracted files.
///
/// # Returns
/// `Ok(())` on success, or a [`MantleError::Archive`] on failure.
///
/// # Side Effects
/// Creates files and directories under `dest`.
///
/// # Errors
/// Returns [`MantleError::Archive`] if the file cannot be opened, `dest` cannot
/// be created, or extraction fails.
pub fn extract_rar(path: &Path, dest: &Path) -> Result<(), MantleError> {
    let mut source = fs::File::open(path)
        .map_err(|e| MantleError::Archive(format!("cannot open rar {}: {e}", path.display())))?;
    fs::create_dir_all(dest)
        .map_err(|e| MantleError::Archive(format!("cannot create dest dir {}: {e}", dest.display())))?;
    uncompress_archive(&mut source, dest, Ownership::Ignore)
        .map_err(|e| MantleError::Archive(format!("rar extract error for {}: {e}", path.display())))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::{NamedTempFile, TempDir};

    #[test]
    fn list_rar_garbage_does_not_panic() {
        // libarchive is permissive: arbitrary bytes may succeed with an empty list.
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"not a rar archive").unwrap();
        let _result = list_rar_files(tmp.path()); // Ok([]) or Err — both acceptable
    }

    #[test]
    fn extract_rar_garbage_does_not_panic() {
        // libarchive may extract 0 files or return an error for arbitrary data.
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"garbage").unwrap();
        let dest = TempDir::new().unwrap();
        let _result = extract_rar(tmp.path(), dest.path()); // Ok(()) or Err — both acceptable
    }

    #[test]
    fn list_rar_error_on_missing_file() {
        let result = list_rar_files(Path::new("/nonexistent/archive.rar"));
        assert!(result.is_err());
    }
}
