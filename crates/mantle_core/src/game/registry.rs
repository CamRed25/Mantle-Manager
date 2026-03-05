//! Wine/Proton registry reader — parses `system.reg` and `user.reg`.
//!
//! Wine stores the Windows registry as text files in the `pfx/` directory of
//! each Proton compatibility prefix. These hive files let us read game-relevant
//! registry values without spawning a Wine process or linking to a Windows API.
//!
//! # File format (WINE REGISTRY Version 2)
//!
//! ```text
//! WINE REGISTRY Version 2
//! ;; All keys relative to \\HKEY_LOCAL_MACHINE
//!
//! [Software\\Valve\\Steam]
//! 1651855572
//! "InstallPath"="C:\\Program Files (x86)\\Steam"
//! "SteamExe"="C:\\Program Files (x86)\\Steam\\steam.exe"
//!
//! [Software\\Microsoft\\Windows NT\\CurrentVersion]
//! 1637533143
//! "CurrentVersion"="6.1"
//! "ProductName"="Wine"
//! ```
//!
//! - Section headers use `\\` as the path separator (one level of escaping).
//! - String values use `\\` for a literal backslash and `\"` for a literal quote.
//! - DWORD values are written as `dword:XXXXXXXX` (8 lowercase hex digits).
//! - Binary / multi-string values (`hex:`, `hex(2):`, …) can span multiple
//!   lines using a trailing `\` continuation marker; this parser joins them
//!   before processing.
//! - Standalone integer lines after a section header are Unix timestamps — skip.
//!
//! # Usage
//!
//! ```ignore
//! use mantle_core::game::registry;
//!
//! // Load the system hive from a Proton prefix.
//! let hive = registry::load_system_reg(&pfx_path)?;
//!
//! // Query a string value — key path and name are case-insensitive.
//! if let Some(path) = hive.get_str(r"Software\Valve\Steam", "InstallPath") {
//!     println!("Steam (Wine C:) install: {path}");
//! }
//! ```
//!
//! # References
//! - `PLATFORM_COMPAT.md` §6 — Proton and Wine integration
//! - Wine source: `tools/regedit/regformat.c` — `.reg` file format spec

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::error::MantleError;

// ─── Public types ─────────────────────────────────────────────────────────────

/// A value stored in the Wine registry.
#[derive(Debug, Clone, PartialEq)]
pub enum RegistryValue {
    /// `REG_SZ` or `REG_EXPAND_SZ` — a UTF-8 decoded string.
    ///
    /// Backslash sequences (`\\` → `\`, `\"` → `"`) have already been
    /// resolved; the string is ready to use as-is.
    String(String),

    /// `REG_DWORD` — a 32-bit unsigned integer.
    Dword(u32),

    /// `REG_BINARY` or other binary types (`hex:`, `hex(N):`).
    ///
    /// Decoded from the comma-separated hex representation in the file.
    Binary(Vec<u8>),
}

impl RegistryValue {
    /// Return the inner string if this is a [`RegistryValue::String`].
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Return the inner integer if this is a [`RegistryValue::Dword`].
    #[must_use]
    pub fn as_dword(&self) -> Option<u32> {
        match self {
            Self::Dword(n) => Some(*n),
            _ => None,
        }
    }
}

/// A parsed Wine registry hive (`system.reg` or `user.reg`).
///
/// Produced by [`load_system_reg`] or [`load_user_reg`].
///
/// All lookups are **case-insensitive**: both key paths and value names are
/// normalised to lowercase before storage.
#[derive(Debug, Clone)]
pub struct RegistryHive {
    /// Hive root read from the file comment (e.g. `"HKEY_LOCAL_MACHINE"`).
    /// Empty string when the comment is absent (unusual but tolerated).
    pub hive_root: String,

    /// Inner map: normalised key path → (normalised value name → value).
    ///
    /// Key paths use a single `\` as the separator (the `\\` escaping from the
    /// file is resolved during parsing).
    data: HashMap<String, HashMap<String, RegistryValue>>,
}

impl RegistryHive {
    /// Parse a Wine registry hive from its text content.
    ///
    /// # Parameters
    /// - `content`: The full UTF-8 text of a `.reg` file.
    ///
    /// # Returns
    /// A `RegistryHive` — never fails; unrecognised or malformed lines are
    /// silently skipped.
    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn from_str(content: &str) -> Self {
        parse_hive(content)
    }

    /// Load and parse a Wine registry hive from a file on disk.
    ///
    /// # Errors
    /// Returns [`MantleError::Io`] if the file cannot be read.
    pub fn open(path: &Path) -> Result<Self, MantleError> {
        let content = std::fs::read_to_string(path).map_err(MantleError::Io)?;
        Ok(Self::from_str(&content))
    }

    /// Look up a registry value.
    ///
    /// Both `key_path` and `value_name` are matched case-insensitively.
    ///
    /// `key_path` may use either single (`\`) or doubled (`\\`) backslashes;
    /// both forms are normalised before lookup.
    ///
    /// # Parameters
    /// - `key_path`: Registry key path relative to the hive root,
    ///   e.g. `r"Software\Valve\Steam"`.
    /// - `value_name`: Value name, e.g. `"InstallPath"`. Use `"@"` for the
    ///   default (unnamed) value.
    #[must_use]
    pub fn get_value(&self, key_path: &str, value_name: &str) -> Option<&RegistryValue> {
        let norm_key = normalize_key(key_path);
        let norm_name = value_name.to_lowercase();
        self.data.get(&norm_key)?.get(&norm_name)
    }

    /// Convenience: look up a string value and return it as `&str`.
    ///
    /// Returns `None` if the key or value is absent, or if the value is not a
    /// string type.
    #[must_use]
    pub fn get_str(&self, key_path: &str, value_name: &str) -> Option<&str> {
        self.get_value(key_path, value_name)?.as_str()
    }

    /// Convenience: look up a DWORD value.
    ///
    /// Returns `None` if the key or value is absent, or if the value is not a
    /// DWORD type.
    #[must_use]
    pub fn get_dword(&self, key_path: &str, value_name: &str) -> Option<u32> {
        self.get_value(key_path, value_name)?.as_dword()
    }

    /// Return `true` if the hive contains at least one entry for `key_path`.
    #[must_use]
    pub fn has_key(&self, key_path: &str) -> bool {
        self.data.contains_key(&normalize_key(key_path))
    }

    /// Iterator over all key paths present in this hive (normalised, lowercase).
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.data.keys().map(String::as_str)
    }
}

// ─── Convenience loaders ──────────────────────────────────────────────────────

/// Load the **system** registry hive (`pfx/system.reg`).
///
/// The system hive contains `HKEY_LOCAL_MACHINE` values, including the
/// `Software\Valve\Steam` key used to locate Steam's (Wine) install path.
///
/// # Parameters
/// - `pfx_path`: Absolute path to the Wine prefix root (`pfx/` directory).
///
/// # Errors
/// Returns [`MantleError::Io`] if `pfx/system.reg` cannot be read.
pub fn load_system_reg(pfx_path: &Path) -> Result<RegistryHive, MantleError> {
    RegistryHive::open(&pfx_path.join("system.reg"))
}

/// Load the **user** registry hive (`pfx/user.reg`).
///
/// The user hive contains `HKEY_CURRENT_USER` values, including user-specific
/// game settings and the Wine user profile paths.
///
/// # Parameters
/// - `pfx_path`: Absolute path to the Wine prefix root (`pfx/` directory).
///
/// # Errors
/// Returns [`MantleError::Io`] if `pfx/user.reg` cannot be read.
pub fn load_user_reg(pfx_path: &Path) -> Result<RegistryHive, MantleError> {
    RegistryHive::open(&pfx_path.join("user.reg"))
}

/// Return the Wine `C:` drive root path inside the given prefix.
///
/// Equivalent to `pfx_path/drive_c`.
#[must_use]
pub fn wine_c_drive(pfx_path: &Path) -> PathBuf {
    pfx_path.join("drive_c")
}

// ─── Parser ───────────────────────────────────────────────────────────────────

/// Parse the full content of a Wine `.reg` file into a [`RegistryHive`].
fn parse_hive(content: &str) -> RegistryHive {
    // ── Pass 1: join continuation lines ──────────────────────────────────────
    // Multi-line hex values end their intermediate lines with a bare `\`.
    // String values always end with `"`, so joining bare-`\`-terminated lines
    // is safe.
    let mut lines: Vec<String> = Vec::with_capacity(content.lines().count());
    let mut acc = String::new();

    for raw in content.lines() {
        if raw.ends_with('\\') && !raw.ends_with("\\\\") {
            // Continuation: strip trailing `\` and keep accumulating.
            acc.push_str(&raw[..raw.len() - 1]);
        } else {
            acc.push_str(raw);
            lines.push(std::mem::take(&mut acc));
        }
    }
    if !acc.is_empty() {
        lines.push(acc);
    }

    // ── Pass 2: parse ─────────────────────────────────────────────────────────
    let mut hive_root = String::new();
    let mut data: HashMap<String, HashMap<String, RegistryValue>> = HashMap::new();
    let mut current_key = String::new();

    for line in &lines {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }

        // Comments — also carry the hive-root hint on the first comment line.
        if trimmed.starts_with(';') {
            if let Some(rest) = trimmed.strip_prefix(";; All keys relative to \\\\") {
                rest.trim().clone_into(&mut hive_root);
            }
            continue;
        }

        // Version header.
        if trimmed.starts_with("WINE REGISTRY") {
            continue;
        }

        // Key deletion marker: `[-Key\\Path]` — skip the whole key.
        if trimmed.starts_with("[-") {
            current_key.clear();
            continue;
        }

        // Section header: `[Key\\Path]`
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let raw_key = &trimmed[1..trimmed.len() - 1];
            current_key = normalize_key(raw_key);
            data.entry(current_key.clone()).or_default();
            continue;
        }

        // Standalone integer lines are Unix timestamps after section headers.
        if trimmed.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        // Value lines (only valid inside a section).
        if current_key.is_empty() {
            continue;
        }

        if let Some((name, value)) = parse_value_line(trimmed) {
            data.entry(current_key.clone()).or_default().insert(name, value);
        }
    }

    RegistryHive { hive_root, data }
}

/// Parse one value line, e.g.:
/// - `"InstallPath"="C:\\Program Files (x86)\\Steam"`
/// - `"MajorVersion"=dword:00000006`
/// - `@="default value"`
/// - `"data"=hex:de,ad,be,ef`
fn parse_value_line(line: &str) -> Option<(String, RegistryValue)> {
    if line.starts_with('@') {
        // Default (unnamed) value: `@=...`
        let rest = line.strip_prefix('@')?.strip_prefix('=')?;
        let value = parse_value_data(rest)?;
        return Some(("@".to_owned(), value));
    }

    if !line.starts_with('"') {
        return None;
    }

    let after_open_quote = &line[1..];
    let (name_raw, rest_after_name) = parse_quoted_string(after_open_quote)?;
    let rest = rest_after_name.strip_prefix('=')?;
    let value = parse_value_data(rest)?;

    Some((name_raw.to_lowercase(), value))
}

/// Parse the right-hand side of `name=<here>`.
fn parse_value_data(s: &str) -> Option<RegistryValue> {
    if let Some(s) = s.strip_prefix('"') {
        // REG_SZ string.
        let (val, _) = parse_quoted_string(s)?;
        Some(RegistryValue::String(val))
    } else if let Some(hex) = s.strip_prefix("dword:") {
        // REG_DWORD: 8 hex digits.
        let n = u32::from_str_radix(hex.trim(), 16).ok()?;
        Some(RegistryValue::Dword(n))
    } else if let Some(hex_body) = s.strip_prefix("hex:") {
        // REG_BINARY.
        Some(RegistryValue::Binary(parse_hex_bytes(hex_body)))
    } else if let Some(rest) = s.strip_prefix("hex(") {
        // REG_EXPAND_SZ (hex(2):), REG_MULTI_SZ (hex(7):), etc.
        let colon_pos = rest.find(':')?;
        let hex_body = &rest[colon_pos + 1..];
        Some(RegistryValue::Binary(parse_hex_bytes(hex_body)))
    } else {
        None
    }
}

/// Parse a `\"`-terminated quoted string, returning `(unescaped_content, remaining_slice)`.
///
/// The caller must have already stripped the opening `"`.
fn parse_quoted_string(s: &str) -> Option<(String, &str)> {
    let mut result = String::new();
    let mut chars = s.char_indices().peekable();

    loop {
        let (i, c) = chars.next()?;
        match c {
            '"' => return Some((result, &s[i + 1..])),
            '\\' => match chars.next()?.1 {
                '\\' => result.push('\\'),
                '"' => result.push('"'),
                'n' => result.push('\n'),
                'r' => result.push('\r'),
                '0' => result.push('\0'),
                other => {
                    result.push('\\');
                    result.push(other);
                }
            },
            _ => result.push(c),
        }
    }
}

/// Decode a comma-separated hex byte sequence (possibly with embedded whitespace).
fn parse_hex_bytes(s: &str) -> Vec<u8> {
    s.split(',').filter_map(|tok| u8::from_str_radix(tok.trim(), 16).ok()).collect()
}

/// Normalise a registry key path for case-insensitive lookup.
///
/// Resolves the `\\` escape (one level) to a single `\`, then lowercases.
fn normalize_key(key: &str) -> String {
    key.replace("\\\\", "\\").to_lowercase()
}

// ─── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // Minimal real-world-shaped registry excerpt used across tests.
    const SYSTEM_REG: &str = r#"WINE REGISTRY Version 2
;; All keys relative to \\HKEY_LOCAL_MACHINE

[Software\\Valve\\Steam]
1651855572
"InstallPath"="C:\\Program Files (x86)\\Steam"
"SteamExe"="C:\\Program Files (x86)\\Steam\\steam.exe"
"Language"="english"

[Software\\Microsoft\\Windows NT\\CurrentVersion]
1637533143
"CurrentVersion"="6.1"
"CurrentBuildNumber"="7601"
"ProductName"="Wine"
"RegisteredOwner"="User"

[Software\\Microsoft\\Windows\\CurrentVersion]
1637533143
"ProgramFilesDir"="C:\\Program Files"
"ProgramFilesDir (x86)"="C:\\Program Files (x86)"

[System\\ControlSet001\\Control\\ComputerName\\ComputerName]
1637533143
"ComputerName"="DESKTOP-ABC123"
"@"="default-value"
"MajorVersion"=dword:00000006
"MinorVersion"=dword:00000001
"BinaryData"=hex:de,ad,be,ef
"#;

    fn hive() -> RegistryHive {
        RegistryHive::from_str(SYSTEM_REG)
    }

    // ── hive_root ─────────────────────────────────────────────────────────────

    #[test]
    fn hive_root_is_parsed() {
        let h = hive();
        assert_eq!(h.hive_root, "HKEY_LOCAL_MACHINE");
    }

    // ── get_str ───────────────────────────────────────────────────────────────

    #[test]
    fn get_str_existing_value() {
        let h = hive();
        let val = h.get_str(r"Software\Valve\Steam", "InstallPath").unwrap();
        assert_eq!(val, r"C:\Program Files (x86)\Steam");
    }

    #[test]
    fn get_str_case_insensitive_key() {
        let h = hive();
        // Key path in uppercase — must still match.
        let val = h.get_str(r"SOFTWARE\VALVE\STEAM", "InstallPath").unwrap();
        assert_eq!(val, r"C:\Program Files (x86)\Steam");
    }

    #[test]
    fn get_str_case_insensitive_value_name() {
        let h = hive();
        let val = h.get_str(r"Software\Valve\Steam", "installpath").unwrap();
        assert_eq!(val, r"C:\Program Files (x86)\Steam");
    }

    #[test]
    fn get_str_doubled_backslash_key() {
        // Caller passes doubled backslashes as they appear in the file.
        let h = hive();
        let val = h.get_str(r"Software\\Valve\\Steam", "InstallPath").unwrap();
        assert_eq!(val, r"C:\Program Files (x86)\Steam");
    }

    #[test]
    fn get_str_missing_key_returns_none() {
        let h = hive();
        assert!(h.get_str(r"Software\Nonexistent\Key", "Name").is_none());
    }

    #[test]
    fn get_str_missing_value_returns_none() {
        let h = hive();
        assert!(h.get_str(r"Software\Valve\Steam", "NoSuchValue").is_none());
    }

    #[test]
    fn get_str_with_spaces_in_path() {
        let h = hive();
        let val = h
            .get_str(r"Software\Microsoft\Windows NT\CurrentVersion", "ProductName")
            .unwrap();
        assert_eq!(val, "Wine");
    }

    #[test]
    fn get_str_value_name_with_parentheses() {
        let h = hive();
        let val = h
            .get_str(r"Software\Microsoft\Windows\CurrentVersion", "ProgramFilesDir (x86)")
            .unwrap();
        assert_eq!(val, r"C:\Program Files (x86)");
    }

    #[test]
    fn string_value_with_backslash_in_path() {
        let h = hive();
        let exe = h.get_str(r"Software\Valve\Steam", "SteamExe").unwrap();
        assert!(exe.contains('\\'), "backslash must survive unescaping");
        assert_eq!(exe, r"C:\Program Files (x86)\Steam\steam.exe");
    }

    // ── get_dword ─────────────────────────────────────────────────────────────

    #[test]
    fn get_dword_existing_value() {
        let h = hive();
        let major = h
            .get_dword(r"System\ControlSet001\Control\ComputerName\ComputerName", "MajorVersion")
            .unwrap();
        assert_eq!(major, 6);
    }

    #[test]
    fn get_dword_minor_version() {
        let h = hive();
        let minor = h
            .get_dword(r"System\ControlSet001\Control\ComputerName\ComputerName", "MinorVersion")
            .unwrap();
        assert_eq!(minor, 1);
    }

    #[test]
    fn get_dword_returns_none_for_string_value() {
        let h = hive();
        // InstallPath is a string — as_dword must return None.
        assert!(h.get_dword(r"Software\Valve\Steam", "InstallPath").is_none());
    }

    // ── binary values ─────────────────────────────────────────────────────────

    #[test]
    fn binary_value_decoded() {
        let h = hive();
        let val = h
            .get_value(r"System\ControlSet001\Control\ComputerName\ComputerName", "BinaryData")
            .unwrap();
        assert_eq!(val, &RegistryValue::Binary(vec![0xde, 0xad, 0xbe, 0xef]));
    }

    // ── default value (@) ─────────────────────────────────────────────────────

    #[test]
    fn default_value_read_as_at_sign() {
        let h = hive();
        let val = h
            .get_str(r"System\ControlSet001\Control\ComputerName\ComputerName", "@")
            .unwrap();
        assert_eq!(val, "default-value");
    }

    // ── has_key ───────────────────────────────────────────────────────────────

    #[test]
    fn has_key_true_for_present_key() {
        assert!(hive().has_key(r"Software\Valve\Steam"));
    }

    #[test]
    fn has_key_false_for_absent_key() {
        assert!(!hive().has_key(r"Software\Missing\Key"));
    }

    // ── keys iterator ─────────────────────────────────────────────────────────

    #[test]
    fn keys_contains_all_parsed_sections() {
        let h = hive();
        let keys: Vec<&str> = h.keys().collect();
        assert!(keys.iter().any(|k| *k == r"software\valve\steam"));
        assert!(keys
            .iter()
            .any(|k| { k.contains("windows nt") && k.contains("currentversion") }));
    }

    // ── continuation lines ────────────────────────────────────────────────────

    #[test]
    fn multiline_hex_value_is_joined() {
        let content = "WINE REGISTRY Version 2\n\
                       [TestKey\\Sub]\n\
                       1651855572\n\
                       \"data\"=hex:de,ad,\\\n\
                       be,ef\n";
        let h = RegistryHive::from_str(content);
        let val = h.get_value(r"TestKey\Sub", "data").unwrap();
        assert_eq!(val, &RegistryValue::Binary(vec![0xde, 0xad, 0xbe, 0xef]));
    }

    // ── load_system_reg / load_user_reg ───────────────────────────────────────

    #[test]
    fn load_system_reg_reads_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("system.reg"), SYSTEM_REG.as_bytes()).unwrap();
        let h = load_system_reg(dir.path()).unwrap();
        assert_eq!(h.hive_root, "HKEY_LOCAL_MACHINE");
        assert!(h.has_key(r"Software\Valve\Steam"));
    }

    #[test]
    fn load_user_reg_reads_file() {
        let dir = TempDir::new().unwrap();
        let user_reg = "WINE REGISTRY Version 2\n\
                        ;; All keys relative to \\\\HKEY_CURRENT_USER\n\
                        \n\
                        [Software\\Wine\\DllOverrides]\n\
                        1651855572\n\
                        \"winemenubuilder.exe\"=\"\"\n";
        std::fs::write(dir.path().join("user.reg"), user_reg.as_bytes()).unwrap();
        let h = load_user_reg(dir.path()).unwrap();
        assert_eq!(h.hive_root, "HKEY_CURRENT_USER");
        assert!(h.has_key(r"Software\Wine\DllOverrides"));
    }

    #[test]
    fn load_system_reg_missing_file_returns_io_error() {
        let dir = TempDir::new().unwrap();
        let result = load_system_reg(dir.path());
        assert!(
            matches!(result, Err(MantleError::Io(_))),
            "missing file must return MantleError::Io"
        );
    }

    // ── wine_c_drive ──────────────────────────────────────────────────────────

    #[test]
    fn wine_c_drive_returns_drive_c_subpath() {
        let pfx = Path::new("/home/user/.steam/steamapps/compatdata/489830/pfx");
        let c = wine_c_drive(pfx);
        assert_eq!(c.file_name().unwrap(), "drive_c");
        assert!(c.starts_with(pfx));
    }

    // ── edge cases ────────────────────────────────────────────────────────────

    #[test]
    fn empty_content_returns_empty_hive() {
        let h = RegistryHive::from_str("");
        assert_eq!(h.hive_root, "");
        assert!(h.keys().next().is_none());
    }

    #[test]
    fn malformed_lines_are_silently_skipped() {
        let content = "WINE REGISTRY Version 2\n\
                       [Good\\Key]\n\
                       not_a_value_line\n\
                       \"valid\"=\"yes\"\n\
                       ====garbage====\n";
        let h = RegistryHive::from_str(content);
        assert!(h.get_str(r"Good\Key", "valid").is_some());
    }
}
