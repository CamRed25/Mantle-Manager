//! Archive extraction and inspection — BSA, BA2, ZIP, 7z, RAR.
//!
//! # Architecture
//!
//! The module is split into format-specific back-ends:
//!
//! | Sub-module   | Format(s)                           | Back-end           |
//! |--------------|-------------------------------------|--------------------|
//! | [`bsa`]      | Morrowind BSA, Oblivion/Skyrim BSA, | `ba2` crate        |
//! |              | Fallout 4 / Starfield BA2           |                    |
//! | [`zip`]      | ZIP (all variants)                  | `compress-tools`   |
//! | [`sevenz`]   | 7-Zip (all LZMA/LZMA2 variants)     | `compress-tools`   |
//! | [`rar`]      | RAR4 / RAR5 (extract-only)          | `compress-tools`   |
//! | [`detect`]   | Magic-byte format identification    | (built-in)         |
//!
//! ## Async safety
//!
//! All back-end functions are **synchronous** (they may block on I/O).  The
//! public functions exposed by this module wrap them with
//! `tokio::task::spawn_blocking` so callers never block the async executor.

pub mod bsa;
pub mod detect;
pub mod rar;
pub mod sevenz;
pub mod zip;

pub use detect::{detect_format, detect_format_from_bytes, ArchiveFormat};

use std::path::Path;
use tokio::task::spawn_blocking;

use crate::error::MantleError;

// ── Public async API ──────────────────────────────────────────────────────────

/// Lists all file paths contained in an archive at `path`.
///
/// The archive format is detected automatically from the file's magic bytes.
///
/// # Parameters
/// - `path`: Path to the archive file.
///
/// # Returns
/// An ordered `Vec<String>` of all relative file paths, or a
/// [`MantleError::Archive`] if the format is unsupported or the file is
/// malformed.
///
/// # Async
/// Runs the blocking I/O on a dedicated thread pool via
/// `tokio::task::spawn_blocking`.
///
/// # Errors
/// Returns [`MantleError::Archive`] if the archive format is unsupported,
/// the file is malformed, or the `spawn_blocking` task fails.
pub async fn list_files(path: &Path) -> Result<Vec<String>, MantleError> {
    let path = path.to_path_buf();
    spawn_blocking(move || list_files_sync(&path))
        .await
        .map_err(|e| MantleError::Archive(format!("spawn_blocking error: {e}")))?
}

/// Extracts all files from an archive at `path` into `dest`.
///
/// The archive format is detected automatically from the file's magic bytes.
/// Missing parent directories inside `dest` are created as needed.
///
/// # Parameters
/// - `path`: Path to the archive file.
/// - `dest`: Destination directory. Will be created if it does not exist.
///
/// # Returns
/// `Ok(())` on success, or a [`MantleError::Archive`] if the format is
/// unsupported or extraction fails.
///
/// # Async
/// Runs the blocking I/O on a dedicated thread pool via
/// `tokio::task::spawn_blocking`.
///
/// # Errors
/// Returns [`MantleError::Archive`] if the archive format is unsupported,
/// extraction fails, or the `spawn_blocking` task fails.
pub async fn extract_archive(path: &Path, dest: &Path) -> Result<(), MantleError> {
    let path = path.to_path_buf();
    let dest = dest.to_path_buf();
    spawn_blocking(move || extract_archive_sync(&path, &dest))
        .await
        .map_err(|e| MantleError::Archive(format!("spawn_blocking error: {e}")))?
}

// ── Internal sync dispatch ─────────────────────────────────────────────────

/// Synchronous listing dispatch — called from the blocking thread pool.
///
/// # Parameters
/// - `path`: Path to the archive file.
///
/// # Returns
/// `Vec<String>` of relative file paths, or an error.
fn list_files_sync(path: &Path) -> Result<Vec<String>, MantleError> {
    match detect_format(path) {
        ArchiveFormat::Tes3Bsa | ArchiveFormat::Tes4Bsa => bsa::list_bsa_files(path),
        ArchiveFormat::Fo4Ba2 => bsa::list_ba2_files(path),
        ArchiveFormat::Zip => zip::list_zip_files(path),
        ArchiveFormat::SevenZip => sevenz::list_sevenz_files(path),
        ArchiveFormat::Rar => rar::list_rar_files(path),
        ArchiveFormat::Unknown => {
            Err(MantleError::Archive(format!("unknown or unsupported archive format: {}", path.display())))
        }
    }
}

/// Synchronous extraction dispatch — called from the blocking thread pool.
///
/// # Parameters
/// - `path`: Path to the archive file.
/// - `dest`: Destination directory.
///
/// # Returns
/// `Ok(())` on success, or an error.
fn extract_archive_sync(path: &Path, dest: &Path) -> Result<(), MantleError> {
    match detect_format(path) {
        ArchiveFormat::Tes3Bsa | ArchiveFormat::Tes4Bsa => bsa::extract_bsa(path, dest),
        ArchiveFormat::Fo4Ba2 => bsa::extract_ba2(path, dest),
        ArchiveFormat::Zip => zip::extract_zip(path, dest),
        ArchiveFormat::SevenZip => sevenz::extract_sevenz(path, dest),
        ArchiveFormat::Rar => rar::extract_rar(path, dest),
        ArchiveFormat::Unknown => {
            Err(MantleError::Archive(format!("unknown or unsupported archive format: {}", path.display())))
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_files_sync_unknown_format_returns_error() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"\xFF\xFE unknown magic bytes").unwrap();
        let result = list_files_sync(&tmp.path().to_path_buf());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("unsupported"),
            "error message should mention unsupported, got: {msg}"
        );
    }

    #[test]
    fn extract_archive_sync_unknown_format_returns_error() {
        use std::io::Write;
        use tempfile::{NamedTempFile, TempDir};

        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"\xDE\xAD\xBE\xEF unknown magic").unwrap();
        let dest = TempDir::new().unwrap();
        let result = extract_archive_sync(&tmp.path().to_path_buf(), &dest.path().to_path_buf());
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn list_files_async_zip_roundtrip() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Reuse the ZIP builder from zip::tests by duplicating the critical bytes.
        let zip_bytes = build_minimal_stored_zip("async_test.txt", b"async content");
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(&zip_bytes).unwrap();

        let files = list_files(tmp.path()).await.expect("async list should work");
        assert!(files.iter().any(|f| f.contains("async_test.txt")), "got: {files:?}");
    }

    #[tokio::test]
    async fn extract_archive_async_zip_roundtrip() {
        use std::io::Write;
        use tempfile::{NamedTempFile, TempDir};

        let zip_bytes = build_minimal_stored_zip("out.txt", b"extracted async");
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(&zip_bytes).unwrap();
        let dest = TempDir::new().unwrap();

        extract_archive(tmp.path(), dest.path())
            .await
            .expect("async extract should work");

        let out = dest.path().join("out.txt");
        assert!(out.exists(), "out.txt should be extracted");
    }

    // ── Test helpers ─────────────────────────────────────────────────────────

    /// Minimal stored ZIP builder (mirrors zip.rs test helper for self-containment).
    fn build_minimal_stored_zip(name: &str, content: &[u8]) -> Vec<u8> {
        let name_len = name.len() as u16;
        let data_len = content.len() as u32;
        let crc = crc32(content);
        let mut buf = Vec::<u8>::new();

        let local_offset: u32 = 0;
        w32(&mut buf, 0x04034b50);
        w16(&mut buf, 20);
        w16(&mut buf, 0);
        w16(&mut buf, 0);
        w16(&mut buf, 0);
        w16(&mut buf, 0);
        w32(&mut buf, crc);
        w32(&mut buf, data_len);
        w32(&mut buf, data_len);
        w16(&mut buf, name_len);
        w16(&mut buf, 0);
        buf.extend_from_slice(name.as_bytes());
        buf.extend_from_slice(content);

        let cd = buf.len() as u32;
        w32(&mut buf, 0x02014b50);
        w16(&mut buf, 20);
        w16(&mut buf, 20);
        w16(&mut buf, 0);
        w16(&mut buf, 0);
        w16(&mut buf, 0);
        w16(&mut buf, 0);
        w32(&mut buf, crc);
        w32(&mut buf, data_len);
        w32(&mut buf, data_len);
        w16(&mut buf, name_len);
        w16(&mut buf, 0);
        w16(&mut buf, 0);
        w16(&mut buf, 0);
        w16(&mut buf, 0);
        w32(&mut buf, 0);
        w32(&mut buf, local_offset);
        buf.extend_from_slice(name.as_bytes());

        let cd_size = (buf.len() as u32) - cd;
        w32(&mut buf, 0x06054b50);
        w16(&mut buf, 0);
        w16(&mut buf, 0);
        w16(&mut buf, 1);
        w16(&mut buf, 1);
        w32(&mut buf, cd_size);
        w32(&mut buf, cd);
        w16(&mut buf, 0);
        buf
    }

    fn w16(b: &mut Vec<u8>, v: u16) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    fn w32(b: &mut Vec<u8>, v: u32) {
        b.extend_from_slice(&v.to_le_bytes());
    }
    fn crc32(data: &[u8]) -> u32 {
        let mut c: u32 = 0xFFFF_FFFF;
        for &x in data {
            c ^= u32::from(x);
            for _ in 0..8 {
                c = if c & 1 == 1 {
                    (c >> 1) ^ 0xEDB88320
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
}
