//! Integration tests for `mantle_core::archive`.
//!
//! These tests exercise the full public API surface:
//! - Magic-byte format detection (`detect_format`)
//! - Synchronous and async listing and extraction via the dispatch layer
//! - Error handling for unsupported or malformed archives
//!
//! Because real Bethesda BSA/BA2 fixtures are not included in the repository,
//! those back-ends are tested with negative cases only (invalid input → error).
//! ZIP is tested with a minimal hand-crafted stored archive.

use std::io::Write;
use tempfile::{NamedTempFile, TempDir};

use mantle_core::archive::{
    detect::{detect_format, detect_format_from_bytes, ArchiveFormat},
    extract_archive, list_files,
};

// ── detect_format ─────────────────────────────────────────────────────────────

#[test]
fn detect_tes3_bsa_magic() {
    let fmt = detect_format_from_bytes(&[0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    assert_eq!(fmt, ArchiveFormat::Tes3Bsa);
}

#[test]
fn detect_tes4_bsa_magic() {
    let fmt = detect_format_from_bytes(b"BSA\x00EXTRA");
    assert_eq!(fmt, ArchiveFormat::Tes4Bsa);
}

#[test]
fn detect_fo4_ba2_magic() {
    let fmt = detect_format_from_bytes(b"BTDXextra");
    assert_eq!(fmt, ArchiveFormat::Fo4Ba2);
}

#[test]
fn detect_zip_magic() {
    let fmt = detect_format_from_bytes(b"PK\x03\x04extra");
    assert_eq!(fmt, ArchiveFormat::Zip);
}

#[test]
fn detect_7z_magic() {
    let fmt = detect_format_from_bytes(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, 0x00, 0x00]);
    assert_eq!(fmt, ArchiveFormat::SevenZip);
}

#[test]
fn detect_rar_magic() {
    let fmt = detect_format_from_bytes(b"Rar!extra");
    assert_eq!(fmt, ArchiveFormat::Rar);
}

#[test]
fn detect_unknown_magic() {
    let fmt = detect_format_from_bytes(b"\xDE\xAD\xBE\xEF\x00\x00\x00\x00");
    assert_eq!(fmt, ArchiveFormat::Unknown);
}

#[test]
fn detect_from_real_file_tes4_bsa() {
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(b"BSA\x00PADDING").unwrap();
    let fmt = detect_format(tmp.path());
    assert_eq!(fmt, ArchiveFormat::Tes4Bsa);
}

#[test]
fn detect_from_real_file_zip() {
    let zip = build_minimal_stored_zip("x.txt", b"x");
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(&zip).unwrap();
    let fmt = detect_format(tmp.path());
    assert_eq!(fmt, ArchiveFormat::Zip);
}

// ── list_files (async) ────────────────────────────────────────────────────────

#[tokio::test]
async fn list_files_zip_roundtrip() {
    let zip = build_minimal_stored_zip("readme.txt", b"content");
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(&zip).unwrap();

    let files = list_files(tmp.path()).await.expect("list should succeed");
    assert!(
        files.iter().any(|f| f.contains("readme.txt")),
        "expected readme.txt in listing; got: {files:?}"
    );
}

#[tokio::test]
async fn list_files_unknown_format_is_error() {
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(b"\x00\x00\x00\x00\x00\x00").unwrap();
    let err = list_files(tmp.path()).await.unwrap_err();
    assert!(
        err.to_string().contains("unsupported"),
        "expected 'unsupported' in error; got: {err}"
    );
}

// ── extract_archive (async) ───────────────────────────────────────────────────

#[tokio::test]
async fn extract_archive_zip_creates_files() {
    let zip = build_minimal_stored_zip("data/notes.txt", b"hello mod");
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(&zip).unwrap();
    let dest = TempDir::new().unwrap();

    extract_archive(tmp.path(), dest.path())
        .await
        .expect("extraction should succeed");

    let out = dest.path().join("data/notes.txt");
    assert!(out.exists(), "data/notes.txt should have been extracted");
    let content = std::fs::read_to_string(&out).unwrap();
    assert_eq!(content, "hello mod");
}

#[tokio::test]
async fn extract_archive_unknown_format_is_error() {
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(b"\xFF\xFF\xFF\xFF magic unknown").unwrap();
    let dest = TempDir::new().unwrap();
    let err = extract_archive(tmp.path(), dest.path()).await.unwrap_err();
    assert!(
        err.to_string().contains("unsupported"),
        "expected 'unsupported' in error; got: {err}"
    );
}

// ── BSA / BA2 negative tests (no real fixture available) ──────────────────────

#[test]
fn list_bsa_files_garbage_is_error() {
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(b"BSA\x00garbage_not_a_real_bsa").unwrap();
    let result = mantle_core::archive::bsa::list_bsa_files(tmp.path());
    assert!(result.is_err(), "expected error for invalid BSA payload");
}

#[test]
fn list_ba2_files_garbage_is_error() {
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(b"BTDXgarbage_not_a_real_ba2").unwrap();
    let result = mantle_core::archive::bsa::list_ba2_files(tmp.path());
    assert!(result.is_err(), "expected error for invalid BA2 payload");
}

// ── ZIP negative tests ────────────────────────────────────────────────────────

#[test]
fn list_zip_with_invalid_data_does_not_panic() {
    // libarchive is permissive: PK magic followed by garbage may return Ok([])
    // rather than an error.  Assert we get a well-typed Result without panicking.
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(b"PK\x03\x04 but this is not a real zip").unwrap();
    let _result = mantle_core::archive::zip::list_zip_files(tmp.path());
    // Ok([]) or Err — both acceptable; the function must not panic
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Builds a minimal stored ZIP containing a single file.  No external crate
/// required; follows the ZIP specification byte for byte.
///
/// # Parameters
/// - `name`:    Entry name (may contain `/` for directories).
/// - `content`: File bytes.
///
/// # Returns
/// Raw ZIP bytes.
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
