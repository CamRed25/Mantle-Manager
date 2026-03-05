//! ZIP archive operations via `compress-tools` (libarchive).
//!
//! Handles all ZIP variants (Deflate, Store, ZIP64) transparently through
//! libarchive.  All public functions are synchronous and should be called from
//! inside `tokio::task::spawn_blocking` by the async wrappers in [`super`].

use std::{fs, path::Path};

use compress_tools::{list_archive_files, uncompress_archive, Ownership};

use crate::error::MantleError;

// ── Public API ────────────────────────────────────────────────────────────────

/// Lists all file paths contained in a ZIP archive.
///
/// # Parameters
/// - `path`: Path to the `.zip` file.
///
/// # Returns
/// An ordered `Vec<String>` of all entries (files and directories), or a
/// [`MantleError::Archive`] if the file cannot be opened or parsed.
///
/// # Side Effects
/// Opens the ZIP file for reading.
///
/// # Errors
/// Returns [`MantleError::Archive`] if the file cannot be opened or parsed.
pub fn list_zip_files(path: &Path) -> Result<Vec<String>, MantleError> {
    let mut source = fs::File::open(path)
        .map_err(|e| MantleError::Archive(format!("cannot open zip {}: {e}", path.display())))?;
    list_archive_files(&mut source)
        .map_err(|e| MantleError::Archive(format!("zip list error for {}: {e}", path.display())))
}

/// Extracts all files from a ZIP archive to `dest`.
///
/// Missing parent directories in `dest` are created automatically.
/// Ownership information from the archive is ignored (safe for sandboxed use).
///
/// # Parameters
/// - `path`: Path to the `.zip` file.
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
pub fn extract_zip(path: &Path, dest: &Path) -> Result<(), MantleError> {
    let mut source = fs::File::open(path)
        .map_err(|e| MantleError::Archive(format!("cannot open zip {}: {e}", path.display())))?;
    fs::create_dir_all(dest).map_err(|e| {
        MantleError::Archive(format!("cannot create dest dir {}: {e}", dest.display()))
    })?;
    uncompress_archive(&mut source, dest, Ownership::Ignore)
        .map_err(|e| MantleError::Archive(format!("zip extract error for {}: {e}", path.display())))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::{NamedTempFile, TempDir};

    #[test]
    fn list_zip_garbage_does_not_panic() {
        // libarchive is permissive: arbitrary bytes may return Ok([]) rather than Err.
        // Ensure the function does not panic and returns a well-typed Result.
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"this is not a zip").unwrap();
        let _result = list_zip_files(tmp.path()); // Ok([]) or Err — both acceptable
    }

    #[test]
    fn extract_zip_garbage_does_not_panic() {
        // libarchive may extract 0 files or return an error for arbitrary data.
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"not a zip").unwrap();
        let dest = TempDir::new().unwrap();
        let _result = extract_zip(tmp.path(), dest.path()); // Ok(()) or Err — both acceptable
    }

    #[test]
    fn list_zip_error_on_missing_file() {
        let result = list_zip_files(Path::new("/nonexistent/archive.zip"));
        assert!(result.is_err());
    }

    #[test]
    fn extract_zip_roundtrip() {
        // Build a minimal valid ZIP in memory using: PK\x03\x04 local file header
        // + central directory + end-of-central-directory record.
        //
        // We construct the smallest possible ZIP with a single stored file
        // ("hello.txt" → b"Hello, world!") so the test does not depend on any
        // zip crate.
        let zip_bytes = build_minimal_zip("hello.txt", b"Hello, world!");
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(&zip_bytes).unwrap();
        let dest = TempDir::new().unwrap();

        extract_zip(tmp.path(), dest.path()).expect("extract should succeed");

        let out = dest.path().join("hello.txt");
        assert!(out.exists(), "hello.txt should be extracted");
        let contents = std::fs::read_to_string(&out).unwrap();
        assert_eq!(contents, "Hello, world!");
    }

    #[test]
    fn list_zip_roundtrip() {
        let zip_bytes = build_minimal_zip("greeting.txt", b"Hi!");
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(&zip_bytes).unwrap();

        let files = list_zip_files(tmp.path()).expect("list should succeed");
        assert!(
            files.iter().any(|f| f.contains("greeting.txt")),
            "expected greeting.txt in listing, got {files:?}"
        );
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Builds a minimal ZIP archive containing a single stored (uncompressed)
    /// file.  No external crate required — byte layout follows the ZIP spec.
    ///
    /// # Parameters
    /// - `name`:    File name to embed.
    /// - `content`: Raw file bytes.
    ///
    /// # Returns
    /// Raw ZIP bytes.
    fn build_minimal_zip(name: &str, content: &[u8]) -> Vec<u8> {
        let name_bytes = name.as_bytes();
        let name_len = name_bytes.len() as u16;
        let data_len = content.len() as u32;

        // CRC-32 of the content.
        let crc = crc32_simple(content);

        let mut buf: Vec<u8> = Vec::new();

        // ── Local file header (offset 0) ──────────────────────────────────
        let local_header_offset: u32 = 0;
        write_u32(&mut buf, 0x04034b50); // Local file signature
        write_u16(&mut buf, 20); // Version needed
        write_u16(&mut buf, 0); // Flags
        write_u16(&mut buf, 0); // Compression method: STORED
        write_u16(&mut buf, 0); // Last mod time
        write_u16(&mut buf, 0); // Last mod date
        write_u32(&mut buf, crc); // CRC-32
        write_u32(&mut buf, data_len); // Compressed size
        write_u32(&mut buf, data_len); // Uncompressed size
        write_u16(&mut buf, name_len); // File name length
        write_u16(&mut buf, 0); // Extra field length
        buf.extend_from_slice(name_bytes);
        buf.extend_from_slice(content);

        // ── Central directory header ─────────────────────────────────────
        let cd_offset = buf.len() as u32;
        write_u32(&mut buf, 0x02014b50); // Central directory signature
        write_u16(&mut buf, 20); // Version made by
        write_u16(&mut buf, 20); // Version needed
        write_u16(&mut buf, 0); // Flags
        write_u16(&mut buf, 0); // Compression: STORED
        write_u16(&mut buf, 0); // Last mod time
        write_u16(&mut buf, 0); // Last mod date
        write_u32(&mut buf, crc); // CRC-32
        write_u32(&mut buf, data_len); // Compressed size
        write_u32(&mut buf, data_len); // Uncompressed size
        write_u16(&mut buf, name_len); // File name length
        write_u16(&mut buf, 0); // Extra field length
        write_u16(&mut buf, 0); // File comment length
        write_u16(&mut buf, 0); // Disk number start
        write_u16(&mut buf, 0); // Internal attrs
        write_u32(&mut buf, 0); // External attrs
        write_u32(&mut buf, local_header_offset);
        buf.extend_from_slice(name_bytes);

        // ── End of central directory ─────────────────────────────────────
        let cd_size = (buf.len() as u32) - cd_offset;
        write_u32(&mut buf, 0x06054b50); // EOCD signature
        write_u16(&mut buf, 0); // Disk number
        write_u16(&mut buf, 0); // Disk with CD
        write_u16(&mut buf, 1); // Entries on disk
        write_u16(&mut buf, 1); // Total entries
        write_u32(&mut buf, cd_size); // Central directory size
        write_u32(&mut buf, cd_offset); // CD offset
        write_u16(&mut buf, 0); // Comment length

        buf
    }

    fn write_u16(buf: &mut Vec<u8>, v: u16) {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    fn write_u32(buf: &mut Vec<u8>, v: u32) {
        buf.extend_from_slice(&v.to_le_bytes());
    }

    /// Simple CRC-32 (polynomial 0xEDB88320) for test use.
    fn crc32_simple(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
}
