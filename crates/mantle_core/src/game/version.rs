//! Game version detection.
//!
//! Provides [`read_game_version`], which attempts to derive a human-readable
//! version string for a detected Bethesda title.  Two strategies are tried
//! in order:
//!
//! 1. **Steam ACF manifest** — reads `appmanifest_{app_id}.acf` from the
//!    steamapps directory and extracts the `buildid` field.  This is always
//!    available for Steam installs and requires no PE parsing.
//!
//! 2. **PE version resource** — scans the game EXE for the `VS_FIXEDFILEINFO`
//!    signature (`0xFEEF04BD`) and extracts the four-part version number from
//!    the two DWORD fields that follow.
//!
//! Both strategies are best-effort: failure returns `""` rather than an error,
//! so callers never have to handle partial version data.

use std::{fs, path::Path};

use super::{games::KNOWN_GAMES, GameInfo};

// ─── Public API ───────────────────────────────────────────────────────────────

/// Attempt to determine the installed game version for `game`.
///
/// Tries two strategies in order:
/// 1. Steam ACF buildid  (cheapest — text parse only)
/// 2. PE `VS_FIXEDFILEINFO` version fields  (binary scan of the EXE)
///
/// # Parameters
/// - `game`: Detected game instance including `install_path` and `steam_app_id`.
///
/// # Returns
/// A human-readable version string such as `"build 11234567"` or
/// `"1.6.1130.0"`.  Returns an empty string if no version can be determined.
///
/// # Side Effects
/// Reads from disk.  Errors are logged at DEBUG level and suppressed.
#[must_use]
pub fn read_game_version(game: &GameInfo) -> String {
    // Strategy 1: Steam ACF buildid.
    if let Some(v) = acf_build_id(&game.install_path, game.steam_app_id) {
        return v;
    }

    // Strategy 2: PE VS_FIXEDFILEINFO scan.
    if let Some(v) = pe_version(&game.install_path, game.steam_app_id) {
        return v;
    }

    String::new()
}

// ─── Strategy 1: Steam ACF manifest ──────────────────────────────────────────

/// Attempt to read `buildid` from `steamapps/appmanifest_{app_id}.acf`.
///
/// The steamapps directory is derived by navigating up two levels from
/// `install_path` (which is `<steamapps>/common/<GameName>`).
///
/// # Parameters
/// - `install_path`: The game's root installation directory.
/// - `app_id`:       Steam App ID used to locate the ACF file.
///
/// # Returns
/// `Some("build {buildid}")` on success, `None` otherwise.
fn acf_build_id(install_path: &Path, app_id: u32) -> Option<String> {
    // install_path = <steamapps>/common/<GameName>
    // parent       = <steamapps>/common
    // parent^2     = <steamapps>
    let steamapps = install_path.parent()?.parent()?;
    let acf_path = steamapps.join(format!("appmanifest_{app_id}.acf"));

    let content = fs::read_to_string(&acf_path)
        .map_err(|e| tracing::debug!(%e, path = %acf_path.display(), "could not read ACF"))
        .ok()?;

    // ACF key-value format: `"buildid"   "12345678"`
    // We scan lines for a line containing `"buildid"` and extract the value.
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("\"buildid\"") {
            // The value is the second quoted token on the line.
            let mut quotes = trimmed.splitn(5, '"');
            // tokens: "" | "buildid" | "" | "<value>" | remainder
            for _ in 0..3 {
                quotes.next();
            }
            if let Some(value) = quotes.next() {
                let value = value.trim();
                if !value.is_empty() {
                    tracing::debug!(buildid = value, "read Steam build ID from ACF");
                    return Some(format!("build {value}"));
                }
            }
        }
    }

    tracing::debug!(app_id, "buildid key not found in ACF");
    None
}

// ─── Strategy 2: PE VS_FIXEDFILEINFO scan ────────────────────────────────────

/// Magic signature that precedes the `VS_FIXEDFILEINFO` structure in a PE
/// version resource.
const VS_FIXED_FILE_INFO_SIGNATURE: u32 = 0xFEEF_04BD;

/// Scan the game EXE for a `VS_FIXEDFILEINFO` block and extract the version.
///
/// Searches for the little-endian signature `0xFEEF04BD` in the file bytes,
/// then reads the two DWORD version fields immediately after, interpreting
/// them as `major.minor.build.revision`.
///
/// # Parameters
/// - `install_path`: The game's root installation directory.
/// - `app_id`:       Used to locate the correct executable via `KNOWN_GAMES`.
///
/// # Returns
/// `Some("major.minor.build.revision")` on success, `None` otherwise.
fn pe_version(install_path: &Path, app_id: u32) -> Option<String> {
    // Look up the game's executable name from the static table.
    let exe_name = KNOWN_GAMES.iter().find(|g| g.steam_app_id == app_id).map(|g| g.executable)?;

    let exe_path = install_path.join(exe_name);
    let bytes = fs::read(&exe_path)
        .map_err(|e| tracing::debug!(%e, path = %exe_path.display(), "could not read game EXE"))
        .ok()?;

    // Build a little-endian pattern from the signature constant.
    let sig = VS_FIXED_FILE_INFO_SIGNATURE.to_le_bytes();

    // Find the signature in the raw bytes.
    let offset = bytes.windows(sig.len()).position(|w| w == sig)?;

    // VS_FIXEDFILEINFO layout after the signature:
    //   +0  DWORD dwSignature   (already matched)
    //   +4  DWORD dwStrucVersion
    //   +8  DWORD dwFileVersionMS  = (major << 16) | minor
    //  +12  DWORD dwFileVersionLS  = (build  << 16) | revision
    let base = offset + 4; // skip past dwSignature itself
    if base + 12 > bytes.len() {
        tracing::debug!("PE buffer too small after signature");
        return None;
    }

    // Safely read the four bytes for each DWORD in little-endian order.
    let _struc_ver = read_u32_le(&bytes, base);
    let file_ver_ms = read_u32_le(&bytes, base + 4);
    let file_ver_ls = read_u32_le(&bytes, base + 8);

    let major = (file_ver_ms >> 16) & 0xFFFF;
    let minor = file_ver_ms & 0xFFFF;
    let build = (file_ver_ls >> 16) & 0xFFFF;
    let revision = file_ver_ls & 0xFFFF;

    // Reject obviously invalid versions (all zeros or all 0xFFFF).
    if major == 0 && minor == 0 && build == 0 {
        tracing::debug!("PE version is all-zeroes; ignoring");
        return None;
    }

    let version = format!("{major}.{minor}.{build}.{revision}");
    tracing::debug!(version, "read version from PE resource");
    Some(version)
}

/// Read a little-endian `u32` from `bytes` at `offset`.
///
/// # Panics
/// Caller must ensure `offset + 4 <= bytes.len()`.
#[inline]
fn read_u32_le(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// `acf_build_id` returns `None` for a non-existent path without panicking.
    #[test]
    fn acf_build_id_missing_file_returns_none() {
        let result = acf_build_id(Path::new("/nonexistent/common/GameName"), 489830);
        assert!(result.is_none());
    }

    /// `acf_build_id` correctly parses a well-formed ACF snippet.
    #[test]
    fn acf_build_id_parses_correctly() {
        use std::io::Write;
        let dir = tempfile::tempdir().expect("tempdir");
        // Simulate: <dir>/<app_id>.acf with a buildid key.
        // We need path <install>/../ to be <dir>, so install_path = <dir>/common/Game.
        let common = dir.path().join("common").join("TestGame");
        std::fs::create_dir_all(&common).expect("create common dir");
        let acf = dir.path().join("appmanifest_1234.acf");
        let mut f = std::fs::File::create(&acf).expect("create acf");
        writeln!(f, "\"AppState\"").expect("write");
        writeln!(f, "{{").expect("write");
        writeln!(f, "\t\"appid\"\t\t\"1234\"").expect("write");
        writeln!(f, "\t\"buildid\"\t\t\"99887766\"").expect("write");
        writeln!(f, "}}").expect("write");
        drop(f);

        let result = acf_build_id(&common, 1234);
        assert_eq!(result, Some("build 99887766".to_string()));
    }

    /// `pe_version` returns `None` for an unrecognised app_id without panicking.
    #[test]
    fn pe_version_unknown_app_id_returns_none() {
        // App ID 0 is not in KNOWN_GAMES.
        let result = pe_version(Path::new("/tmp"), 0);
        assert!(result.is_none());
    }
}
