//! Magic-byte format detection for archive files.
//!
//! Reads the first 8 bytes of a file to identify its archive format without
//! incurring the cost of a full parse. All detection is infallible — files
//! that cannot be opened or read return [`ArchiveFormat::Unknown`].

use std::{fs::File, io::Read, path::Path};

// ── Magic byte constants ─────────────────────────────────────────────────────

/// Morrowind BSA header magic (little-endian `0x00000100`).
const MAGIC_TES3: [u8; 4] = [0x00, 0x01, 0x00, 0x00];

/// Oblivion / Skyrim BSA header magic ("BSA\0").
const MAGIC_TES4: [u8; 4] = *b"BSA\x00";

/// Fallout 4 / Starfield BA2 header magic ("BTDX").
const MAGIC_BA2: [u8; 4] = *b"BTDX";

/// ZIP local-file header magic ("PK\x03\x04").
const MAGIC_ZIP: [u8; 4] = *b"PK\x03\x04";

/// 7-Zip archive magic (6 bytes: `37 7A BC AF 27 1C`).
const MAGIC_7Z: [u8; 6] = [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C];

/// RAR 4 / RAR 5 archive magic ("Rar!").
const MAGIC_RAR: [u8; 4] = *b"Rar!";

// ── Public types ─────────────────────────────────────────────────────────────

/// The archive format detected from a file's magic bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveFormat {
    /// Morrowind BSA (ba2 tes3 module).
    Tes3Bsa,
    /// Oblivion / Skyrim LE / Skyrim SE BSA (ba2 tes4 module).
    Tes4Bsa,
    /// Fallout 4 or Starfield BA2 (ba2 fo4 module).
    Fo4Ba2,
    /// ZIP archive (via compress-tools / libarchive).
    Zip,
    /// 7-Zip archive (via compress-tools / libarchive).
    SevenZip,
    /// RAR archive (via compress-tools / libarchive, extraction only).
    Rar,
    /// Format not recognised.
    Unknown,
}

// ── Public functions ─────────────────────────────────────────────────────────

/// Detects the archive format of `path` by inspecting its magic bytes.
///
/// # Parameters
/// - `path`: Path to the file whose format should be detected.
///
/// # Returns
/// The detected [`ArchiveFormat`]. Returns [`ArchiveFormat::Unknown`] if the
/// file cannot be opened, is shorter than expected, or matches no known
/// signature.
///
/// # Examples
/// ```rust
/// use std::path::Path;
/// use mantle_core::archive::detect::{detect_format, ArchiveFormat};
///
/// let fmt = detect_format(Path::new("nonexistent.bsa"));
/// assert_eq!(fmt, ArchiveFormat::Unknown);
/// ```
#[must_use]
pub fn detect_format(path: &Path) -> ArchiveFormat {
    let mut buf = [0u8; 8];
    let Ok(n) = File::open(path).and_then(|mut f| f.read(&mut buf)) else {
        return ArchiveFormat::Unknown;
    };
    let header = &buf[..n];
    classify(header)
}

/// Detects the archive format from an already-read byte slice (at least the
/// first 8 bytes of the file are sufficient).
///
/// # Parameters
/// - `header`: A slice containing at least the first few bytes of the file.
///
/// # Returns
/// The detected [`ArchiveFormat`].
#[must_use]
pub fn detect_format_from_bytes(header: &[u8]) -> ArchiveFormat {
    classify(header)
}

// ── Internal helpers ─────────────────────────────────────────────────────────

/// Classifies a header slice against known magic signatures.
///
/// # Parameters
/// - `header`: Byte slice (typically the first 8 bytes of a file).
///
/// # Returns
/// Matching [`ArchiveFormat`] or [`ArchiveFormat::Unknown`].
fn classify(header: &[u8]) -> ArchiveFormat {
    if header.starts_with(&MAGIC_TES3) {
        ArchiveFormat::Tes3Bsa
    } else if header.starts_with(&MAGIC_TES4) {
        ArchiveFormat::Tes4Bsa
    } else if header.starts_with(&MAGIC_BA2) {
        ArchiveFormat::Fo4Ba2
    } else if header.starts_with(&MAGIC_ZIP) {
        ArchiveFormat::Zip
    } else if header.starts_with(&MAGIC_7Z) {
        ArchiveFormat::SevenZip
    } else if header.starts_with(&MAGIC_RAR) {
        ArchiveFormat::Rar
    } else {
        ArchiveFormat::Unknown
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: build a header buffer with the given magic at the front.
    fn hdr(magic: &[u8]) -> Vec<u8> {
        let mut v = vec![0u8; 8];
        let n = magic.len().min(8);
        v[..n].copy_from_slice(&magic[..n]);
        v
    }

    #[test]
    fn tes3_bsa_magic() {
        let fmt = detect_format_from_bytes(&hdr(&MAGIC_TES3));
        assert_eq!(fmt, ArchiveFormat::Tes3Bsa);
    }

    #[test]
    fn tes4_bsa_magic() {
        let fmt = detect_format_from_bytes(&hdr(&MAGIC_TES4));
        assert_eq!(fmt, ArchiveFormat::Tes4Bsa);
    }

    #[test]
    fn fo4_ba2_magic() {
        let fmt = detect_format_from_bytes(&hdr(&MAGIC_BA2));
        assert_eq!(fmt, ArchiveFormat::Fo4Ba2);
    }

    #[test]
    fn zip_magic() {
        let fmt = detect_format_from_bytes(&hdr(&MAGIC_ZIP));
        assert_eq!(fmt, ArchiveFormat::Zip);
    }

    #[test]
    fn sevenz_magic() {
        let fmt = detect_format_from_bytes(&hdr(&MAGIC_7Z));
        assert_eq!(fmt, ArchiveFormat::SevenZip);
    }

    #[test]
    fn rar_magic() {
        let fmt = detect_format_from_bytes(&hdr(&MAGIC_RAR));
        assert_eq!(fmt, ArchiveFormat::Rar);
    }

    #[test]
    fn unknown_magic() {
        let fmt = detect_format_from_bytes(b"\xFF\xFE\x00\x00\x00\x00\x00\x00");
        assert_eq!(fmt, ArchiveFormat::Unknown);
    }

    #[test]
    fn empty_bytes_returns_unknown() {
        let fmt = detect_format_from_bytes(b"");
        assert_eq!(fmt, ArchiveFormat::Unknown);
    }

    #[test]
    fn nonexistent_file_returns_unknown() {
        let fmt = detect_format(Path::new("/nonexistent/surely/does/not/exist.bsa"));
        assert_eq!(fmt, ArchiveFormat::Unknown);
    }

    #[test]
    fn detect_from_temp_file_with_tes4_magic() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().expect("tmp file");
        tmp.write_all(b"BSA\x00EXTRA_BYTES").expect("write");
        let fmt = detect_format(tmp.path());
        assert_eq!(fmt, ArchiveFormat::Tes4Bsa);
    }
}
