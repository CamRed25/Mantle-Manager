//! BSA and BA2 archive operations via the `ba2` crate.
//!
//! Supports three Bethesda archive generations:
//! - **TES3** (`BSA\x00` magic, version `0x100`) — Morrowind
//! - **TES4** (`BSA\x00` magic, versions v103/v104/v105) — Oblivion through Skyrim SE
//! - **FO4**  (`BTDX` magic) — Fallout 4, Starfield, and next-gen variants
//!
//! All public functions are synchronous and intended to be called from inside
//! `tokio::task::spawn_blocking` by the async wrappers in [`super`].

use std::{
    fs,
    path::{Path, PathBuf},
};

use ba2::{
    fo4,
    prelude::*,
    tes3,
    tes4::{self, FileCompressionOptions as Tes4CompressionOptions},
};

use crate::error::MantleError;

// ── Public API ────────────────────────────────────────────────────────────────

/// Lists all file paths contained in a BSA (tes3 or tes4) archive.
///
/// Paths use forward-slash separators regardless of what is stored on disk.
///
/// # Parameters
/// - `path`: Path to the `.bsa` file.
///
/// # Returns
/// An ordered `Vec<String>` of all relative file paths, or a
/// [`MantleError::Archive`] if the file cannot be read or parsed.
///
/// # Errors
/// Returns [`MantleError::Archive`] if the file cannot be opened, memory-mapped,
/// or parsed as a valid BSA archive.
///
/// # Side Effects
/// Opens and memory-maps the BSA file (via `ba2`).
pub fn list_bsa_files(path: &Path) -> Result<Vec<String>, MantleError> {
    // The tes3 first-byte check: tes3 magic = [0x00, 0x01, 0x00, 0x00]
    // tes4 magic = b"BSA\x00".  detect_format already gave us the hint, but
    // we optimistically try tes3 first; on failure fall back to tes4.
    if let Ok(archive) = tes3_list(path) {
        return Ok(archive);
    }
    tes4_list(path)
}

/// Lists all file paths contained in a BA2 (`BTDX`) archive.
///
/// # Parameters
/// - `path`: Path to the `.ba2` file.
///
/// # Returns
/// An ordered `Vec<String>` of all relative file paths, or a
/// [`MantleError::Archive`] if the file cannot be read or parsed.
///
/// # Errors
/// Returns [`MantleError::Archive`] if the file cannot be opened, memory-mapped,
/// or parsed as a valid BA2 archive.
pub fn list_ba2_files(path: &Path) -> Result<Vec<String>, MantleError> {
    fo4_list(path)
}

/// Extracts all files from a BSA (tes3 or tes4) archive to `dest`.
///
/// Missing parent directories in `dest` are created automatically.
///
/// # Parameters
/// - `path`: Path to the `.bsa` file.
/// - `dest`: Destination directory for extracted files.
///
/// # Returns
/// `Ok(())` on success, or a [`MantleError::Archive`] on failure.
///
/// # Errors
/// Returns [`MantleError::Archive`] if the archive cannot be opened, parsed,
/// or if writing extracted files to `dest` fails.
///
/// # Side Effects
/// Creates files and directories under `dest`. Opens and memory-maps the BSA.
pub fn extract_bsa(path: &Path, dest: &Path) -> Result<(), MantleError> {
    if tes3_extract(path, dest).is_ok() {
        return Ok(());
    }
    tes4_extract(path, dest)
}

/// Extracts all files from a BA2 (`BTDX`) archive to `dest`.
///
/// # Parameters
/// - `path`: Path to the `.ba2` file.
/// - `dest`: Destination directory for extracted files.
///
/// # Returns
/// `Ok(())` on success, or a [`MantleError::Archive`] on failure.
///
/// # Errors
/// Returns [`MantleError::Archive`] if the archive cannot be opened, parsed,
/// or if writing extracted files to `dest` fails.
pub fn extract_ba2(path: &Path, dest: &Path) -> Result<(), MantleError> {
    fo4_extract(path, dest)
}

// ── Internal: TES3 (Morrowind) ────────────────────────────────────────────────

/// Lists files in a tes3 BSA archive.
///
/// # Parameters
/// - `path`: Path to the `.bsa` file.
///
/// # Returns
/// `Vec<String>` of file paths, or an error.
fn tes3_list(path: &Path) -> Result<Vec<String>, MantleError> {
    let archive = tes3::Archive::read(path)
        .map_err(|e| MantleError::Archive(format!("tes3 read error for {}: {e}", path.display())))?;

    let mut files = Vec::new();
    for (key, _file) in &archive {
        let name = bstr_to_string(key.name());
        files.push(normalise_path(&name));
    }
    Ok(files)
}

/// Extracts all files from a tes3 BSA archive to `dest`.
///
/// # Parameters
/// - `path`: Path to the `.bsa` file.
/// - `dest`: Destination directory.
///
/// # Returns
/// `Ok(())` on success, or an error.
fn tes3_extract(path: &Path, dest: &Path) -> Result<(), MantleError> {
    let archive = tes3::Archive::read(path)
        .map_err(|e| MantleError::Archive(format!("tes3 read error for {}: {e}", path.display())))?;

    for (key, file) in &archive {
        let rel = normalise_path(&bstr_to_string(key.name()));
        let out_path = dest.join(&rel);
        write_bytes(&out_path, file.as_bytes())?;
    }
    Ok(())
}

// ── Internal: TES4 (Oblivion / Skyrim) ───────────────────────────────────────

/// Lists files in a tes4 BSA archive.
///
/// # Parameters
/// - `path`: Path to the `.bsa` file.
///
/// # Returns
/// `Vec<String>` of forward-slash relative paths, or an error.
fn tes4_list(path: &Path) -> Result<Vec<String>, MantleError> {
    let (archive, _options) = tes4::Archive::read(path)
        .map_err(|e| MantleError::Archive(format!("tes4 read error for {}: {e}", path.display())))?;

    let mut files = Vec::new();
    for (dir_key, dir) in &archive {
        let dir_name = bstr_to_string(dir_key.name());
        let dir_name = normalise_path(&dir_name);
        for (file_key, _file) in dir {
            let file_name = bstr_to_string(file_key.name());
            let rel = join_rel_path(&dir_name, &file_name);
            files.push(rel);
        }
    }
    Ok(files)
}

/// Extracts all files from a tes4 BSA archive to `dest`.
///
/// Compressed files are decompressed before writing.
///
/// # Parameters
/// - `path`: Path to the `.bsa` file.
/// - `dest`: Destination directory.
///
/// # Returns
/// `Ok(())` on success, or an error.
fn tes4_extract(path: &Path, dest: &Path) -> Result<(), MantleError> {
    let (archive, options) = tes4::Archive::read(path)
        .map_err(|e| MantleError::Archive(format!("tes4 read error for {}: {e}", path.display())))?;

    let compression_opts = Tes4CompressionOptions::builder().version(options.version()).build();

    for (dir_key, dir) in &archive {
        let dir_name = normalise_path(&bstr_to_string(dir_key.name()));
        for (file_key, file) in dir {
            let file_name = bstr_to_string(file_key.name());
            let rel = join_rel_path(&dir_name, &file_name);
            let out_path = dest.join(&rel);

            // Decompress if the file is stored compressed; otherwise use raw bytes.
            if file.is_compressed() {
                let mut buf = Vec::new();
                file.decompress_into(&mut buf, &compression_opts).map_err(|e| {
                    MantleError::Archive(format!("tes4 decompress error for {rel}: {e}"))
                })?;
                write_bytes(&out_path, &buf)?;
            } else {
                write_bytes(&out_path, file.as_bytes())?;
            }
        }
    }
    Ok(())
}

// ── Internal: FO4 (Fallout 4 / Starfield) ────────────────────────────────────

/// Lists files in a fo4 BA2 archive.
///
/// # Parameters
/// - `path`: Path to the `.ba2` file.
///
/// # Returns
/// `Vec<String>` of file paths, or an error.
fn fo4_list(path: &Path) -> Result<Vec<String>, MantleError> {
    let (archive, _options) = fo4::Archive::read(path)
        .map_err(|e| MantleError::Archive(format!("fo4 read error for {}: {e}", path.display())))?;

    let mut files = Vec::new();
    for (key, _file) in &archive {
        let name = bstr_to_string(key.name());
        files.push(normalise_path(&name));
    }
    Ok(files)
}

/// Extracts all GNRL files from a fo4 BA2 archive to `dest`.
///
/// DX10 (texture) files are extracted as raw DDS data by concatenating all
/// decompressed chunk bytes in order.
///
/// # Parameters
/// - `path`: Path to the `.ba2` file.
/// - `dest`: Destination directory.
///
/// # Returns
/// `Ok(())` on success, or an error.
fn fo4_extract(path: &Path, dest: &Path) -> Result<(), MantleError> {
    let (archive, _options) = fo4::Archive::read(path)
        .map_err(|e| MantleError::Archive(format!("fo4 read error for {}: {e}", path.display())))?;

    for (key, file) in &archive {
        let rel = normalise_path(&bstr_to_string(key.name()));
        let out_path = dest.join(&rel);

        // Collect all chunk data into a single buffer.
        let mut combined: Vec<u8> = Vec::new();
        for chunk in file {
            let data = if chunk.is_compressed() {
                let decompressed = chunk.decompress(&fo4::ChunkCompressionOptions::default()).map_err(|e| {
                    MantleError::Archive(format!("fo4 chunk decompress error for {rel}: {e}"))
                })?;
                decompressed.as_bytes().to_vec()
            } else {
                chunk.as_bytes().to_vec()
            };
            combined.extend_from_slice(&data);
        }
        write_bytes(&out_path, &combined)?;
    }
    Ok(())
}

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Writes `data` to `path`, creating all parent directories as needed.
///
/// # Parameters
/// - `path`: Destination file path.
/// - `data`: Bytes to write.
///
/// # Returns
/// `Ok(())` on success, or a [`MantleError::Archive`] wrapping any I/O error.
fn write_bytes(path: &Path, data: &[u8]) -> Result<(), MantleError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| MantleError::Archive(format!("failed to create dir {}: {e}", parent.display())))?;
    }
    fs::write(path, data)
        .map_err(|e| MantleError::Archive(format!("failed to write {}: {e}", path.display())))
}

/// Converts a `ba2::BStr` reference to a `String` using lossy UTF-8 decoding.
///
/// # Parameters
/// - `bstr`: The byte-string slice returned by ba2 key name methods.
///
/// # Returns
/// An owned `String`.
fn bstr_to_string(bstr: &ba2::BStr) -> String {
    String::from_utf8_lossy(bstr.as_ref()).into_owned()
}

/// Joins an optional directory prefix with a file name into a relative path.
///
/// Handles the common tes4 case where the directory component can be "." or
/// empty (meaning the file lives at the archive root).
///
/// # Parameters
/// - `dir`:  Directory portion of the path (may be empty or ".").
/// - `file`: File name.
///
/// # Returns
/// A forward-slash-separated relative path string.
fn join_rel_path(dir: &str, file: &str) -> String {
    let dir = dir.trim_matches(&['/', '\\', '.'][..]);
    if dir.is_empty() {
        file.to_owned()
    } else {
        format!("{dir}/{file}")
    }
}

/// Normalises a path string to forward slashes and strips any leading separators.
///
/// # Parameters
/// - `raw`: Raw path string (potentially with backslashes).
///
/// # Returns
/// Normalised path string.
fn normalise_path(raw: &str) -> String {
    raw.replace('\\', "/").trim_start_matches('/').to_owned()
}

// ── Build helper for destination path ─────────────────────────────────────────

/// Extends a `PathBuf` with a relative path component while refusing to escape
/// the destination root (prevents path-traversal attacks).
///
/// Rejects any `rel` that contains a `..` (`ParentDir`) component before joining,
/// since `Path::starts_with` does not canonicalise `..` segments.
///
/// # Parameters
/// - `dest`: Base destination directory.
/// - `rel`:  Relative file path inside the archive.
///
/// # Returns
/// The joined path, or a [`MantleError::Archive`] if `rel` would escape `dest`.
#[allow(dead_code)] // Retained for future use by streaming extractors.
fn safe_join(dest: &Path, rel: &str) -> Result<PathBuf, MantleError> {
    use std::path::Component;
    // Reject paths with any ".." component before joining — std::Path::starts_with
    // does NOT resolve ".." so we must check explicitly.
    if Path::new(rel).components().any(|c| c == Component::ParentDir) {
        return Err(MantleError::Archive(format!(
            "path traversal detected: {rel:?} escapes destination {}",
            dest.display()
        )));
    }
    Ok(dest.join(rel))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_path_converts_backslash() {
        assert_eq!(normalise_path("meshes\\foo\\bar.nif"), "meshes/foo/bar.nif");
    }

    #[test]
    fn normalise_path_strips_leading_slash() {
        assert_eq!(normalise_path("/textures/a.dds"), "textures/a.dds");
    }

    #[test]
    fn join_rel_path_with_dir() {
        assert_eq!(join_rel_path("meshes/foo", "bar.nif"), "meshes/foo/bar.nif");
    }

    #[test]
    fn join_rel_path_root_dir_dot() {
        assert_eq!(join_rel_path(".", "readme.txt"), "readme.txt");
    }

    #[test]
    fn join_rel_path_empty_dir() {
        assert_eq!(join_rel_path("", "readme.txt"), "readme.txt");
    }

    #[test]
    fn safe_join_allows_valid_path() {
        let dest = Path::new("/tmp/dest");
        let result = safe_join(dest, "meshes/foo/bar.nif").unwrap();
        assert_eq!(result, Path::new("/tmp/dest/meshes/foo/bar.nif"));
    }

    #[test]
    fn safe_join_rejects_traversal() {
        let dest = Path::new("/tmp/dest");
        let err = safe_join(dest, "../../etc/passwd").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("path traversal"));
    }

    #[test]
    fn list_bsa_files_returns_error_for_non_bsa() {
        use std::io::Write;
        use tempfile::NamedTempFile;
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"not a bsa file at all").unwrap();
        let result = list_bsa_files(tmp.path());
        assert!(result.is_err(), "expected error for non-BSA file");
    }

    #[test]
    fn list_ba2_files_returns_error_for_non_ba2() {
        use std::io::Write;
        use tempfile::NamedTempFile;
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"not a ba2 file at all").unwrap();
        let result = list_ba2_files(tmp.path());
        assert!(result.is_err(), "expected error for non-BA2 file");
    }

    #[test]
    fn extract_bsa_returns_error_for_non_bsa() {
        use std::io::Write;
        use tempfile::{NamedTempFile, TempDir};
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"garbage").unwrap();
        let dest = TempDir::new().unwrap();
        let result = extract_bsa(tmp.path(), dest.path());
        assert!(result.is_err());
    }
}
