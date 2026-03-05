//! SKSE version detection — installed and latest.
//!
//! [`installed_version`] reads the version file written to the game directory
//! after a successful install.  [`latest_version`] fetches the upstream
//! plain-text version endpoint (requires the `net` feature).

use std::{fmt, path::Path};

use crate::error::MantleError;

use super::config::SkseGameConfig;

// ── Public types ──────────────────────────────────────────────────────────────

/// Three-part version number for a script extender release.
///
/// Parsed from the plain-text version files shipped in SKSE archives
/// and served by the silverlock.org version endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SkseVersion {
    /// Major version component.
    pub major: u32,
    /// Minor version component.
    pub minor: u32,
    /// Patch version component.
    pub patch: u32,
}

impl fmt::Display for SkseVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

// ── Parsing ───────────────────────────────────────────────────────────────────

/// Parses a version string in either space-separated (`"2 2 6 0"`) or
/// dot-separated (`"2.2.6"`) format.  Returns `None` if parsing fails.
///
/// The optional fourth (build) component is silently ignored.
#[must_use]
pub fn parse_version_str(s: &str) -> Option<SkseVersion> {
    let s = s.trim();
    let sep = if s.contains(' ') { ' ' } else { '.' };
    let mut parts = s.splitn(4, sep);

    let major = parts.next()?.trim().parse().ok()?;
    let minor = parts.next()?.trim().parse().ok()?;
    let patch = parts.next()?.trim().parse().ok()?;

    Some(SkseVersion { major, minor, patch })
}

// ── Installed version ─────────────────────────────────────────────────────────

/// Reads the script extender version that is currently installed in
/// `game_dir` by checking `{game_dir}/{config.version_file}`.
///
/// Returns `None` if the version file is absent (not installed) or cannot be
/// parsed.  A missing file is not an error — it simply means the script
/// extender has not been installed yet.
#[must_use]
pub fn installed_version(game_dir: &Path, config: &SkseGameConfig) -> Option<SkseVersion> {
    let path = game_dir.join(config.version_file);
    let content = std::fs::read_to_string(path).ok()?;
    parse_version_str(&content)
}

// ── Latest version (net feature) ─────────────────────────────────────────────

/// Fetches the latest available script extender version from the upstream
/// version endpoint defined in `config.version_url`.
///
/// # Errors
///
/// Returns [`MantleError::Skse`] if the HTTP request fails or the response
/// cannot be parsed as a version string.
#[cfg(feature = "net")]
pub async fn latest_version(
    config: &SkseGameConfig,
    timeout_secs: u64,
) -> Result<SkseVersion, MantleError> {
    use std::time::Duration;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .map_err(|e| MantleError::Skse(format!("Failed to build HTTP client: {e}")))?;

    let text = client
        .get(config.version_url)
        .send()
        .await
        .map_err(|e| MantleError::Skse(format!("Version check failed for {}: {e}", config.display_name)))?
        .error_for_status()
        .map_err(|e| MantleError::Skse(format!("Version endpoint HTTP error: {e}")))?
        .text()
        .await
        .map_err(|e| MantleError::Skse(format!("Failed to read version response: {e}")))?;

    parse_version_str(&text).ok_or_else(|| {
        MantleError::Skse(format!(
            "Could not parse version string from {}: {:?}",
            config.version_url, text
        ))
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_space_separated() {
        let v = parse_version_str("2 2 6 0").unwrap();
        assert_eq!(v, SkseVersion { major: 2, minor: 2, patch: 6 });
    }

    #[test]
    fn parse_dot_separated() {
        let v = parse_version_str("2.2.6").unwrap();
        assert_eq!(v, SkseVersion { major: 2, minor: 2, patch: 6 });
    }

    #[test]
    fn parse_with_trailing_newline() {
        let v = parse_version_str("1 7 3 15\n").unwrap();
        assert_eq!(v, SkseVersion { major: 1, minor: 7, patch: 3 });
    }

    #[test]
    fn parse_empty_returns_none() {
        assert!(parse_version_str("").is_none());
        assert!(parse_version_str("   ").is_none());
    }

    #[test]
    fn parse_garbage_returns_none() {
        assert!(parse_version_str("not a version").is_none());
        assert!(parse_version_str("abc 1 2").is_none());
    }

    #[test]
    fn parse_only_two_parts_returns_none() {
        assert!(parse_version_str("2.6").is_none());
    }

    #[test]
    fn display_format() {
        let v = SkseVersion { major: 2, minor: 2, patch: 6 };
        assert_eq!(v.to_string(), "2.2.6");
    }

    #[test]
    fn version_ordering() {
        let v1 = SkseVersion { major: 2, minor: 2, patch: 3 };
        let v2 = SkseVersion { major: 2, minor: 2, patch: 6 };
        let v3 = SkseVersion { major: 2, minor: 3, patch: 0 };
        assert!(v1 < v2);
        assert!(v2 < v3);
        assert_eq!(v2, SkseVersion { major: 2, minor: 2, patch: 6 });
    }

    #[test]
    fn installed_version_absent_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::skse::config::config_for_game(crate::game::GameKind::SkyrimSE).unwrap();
        assert!(installed_version(dir.path(), cfg).is_none());
    }

    #[test]
    fn installed_version_reads_file() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::skse::config::config_for_game(crate::game::GameKind::SkyrimSE).unwrap();
        std::fs::write(dir.path().join(cfg.version_file), "2 2 6 0\n").unwrap();
        let v = installed_version(dir.path(), cfg).unwrap();
        assert_eq!(v, SkseVersion { major: 2, minor: 2, patch: 6 });
    }
}
