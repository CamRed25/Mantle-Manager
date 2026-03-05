//! Proton prefix DLL override writer.
//!
//! [`write_dll_overrides`] appends `"<dll>"="native,builtin"` entries to the
//! `[Software\Wine\DllOverrides]` section of the Proton prefix's `user.reg`
//! file.  The operation is **idempotent** — running it twice produces the same
//! result as running it once.
//!
//! # Wine registry format
//!
//! ```text
//! WINE REGISTRY Version 2
//! ...
//! [Software\Wine\DllOverrides]
//! 1738953600
//! "skse64_steam_loader"="native,builtin"
//! "d3d11"="native,builtin"
//!
//! [Software\Wine\...]
//! ...
//! ```
//!
//! Section headers are plain lines starting with `[`.  The timestamp line
//! (Unix epoch decimal) immediately follows the header.  Key/value pairs
//! follow the timestamp until the next `[` line or end of file.
//!
//! # Atomic write
//!
//! The updated file is written to `user.reg.tmp` in the same directory, then
//! atomically renamed over `user.reg` — consistent with the rest of the codebase.

use std::path::Path;

use crate::error::MantleError;

const SECTION: &str = r"[Software\Wine\DllOverrides]";

// ── Public API ────────────────────────────────────────────────────────────────

/// Ensures each DLL in `dlls` has a `"native,builtin"` entry under
/// `[Software\Wine\DllOverrides]` in `user_reg`.
///
/// - If `user_reg` does not exist the function logs a warning and returns `Ok`
///   (the Proton prefix has not been initialised yet; the user should launch
///   the game through Steam at least once first).
/// - If the section is absent it is appended at the end of the file.
/// - Existing entries are never modified or duplicated.
///
/// # Errors
///
/// Returns [`MantleError::Io`] on filesystem errors or
/// [`MantleError::Skse`] if the file cannot be written atomically.
pub fn write_dll_overrides(user_reg: &Path, dlls: &[&str]) -> Result<(), MantleError> {
    if !user_reg.exists() {
        tracing::warn!(
            path = %user_reg.display(),
            "Proton user.reg not found — Proton prefix may not be initialised yet. \
             Skipping DLL overrides."
        );
        return Ok(());
    }

    let content = std::fs::read_to_string(user_reg)?;
    let lines: Vec<&str> = content.lines().collect();

    // ── Locate [Software\Wine\DllOverrides] ──────────────────────────────────
    let section_start = lines.iter().position(|l| *l == SECTION);

    let new_content = if let Some(start) = section_start {
        // Find where the section ends: the next line that begins a new section,
        // or the end of the file.
        let section_end = lines[start + 1..]
            .iter()
            .position(|l| l.starts_with('['))
            .map_or(lines.len(), |offset| start + 1 + offset);

        let section_slice = &lines[start..section_end];

        let mut result: Vec<String> = Vec::with_capacity(lines.len() + dlls.len());

        // Lines up to (not including) section_end
        for line in &lines[..section_end] {
            result.push((*line).to_string());
        }

        // Append any missing dll entries just before the section end
        for dll in dlls {
            let entry_prefix = format!("\"{dll}\"=");
            let already_present = section_slice.iter().any(|l| l.starts_with(&entry_prefix));
            if !already_present {
                result.push(format!("\"{dll}\"=\"native,builtin\""));
            }
        }

        // Remaining lines from section_end onwards
        for line in &lines[section_end..] {
            result.push((*line).to_string());
        }

        result.join("\n")
    } else {
        // Section absent — append it at the end.
        let mut result: Vec<String> = lines.iter().map(|l| (*l).to_string()).collect();

        result.push(String::new());
        result.push(SECTION.to_string());

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        result.push(timestamp.to_string());

        for dll in dlls {
            result.push(format!("\"{dll}\"=\"native,builtin\""));
        }

        result.join("\n")
    };

    // ── Atomic write ─────────────────────────────────────────────────────────
    let tmp_path = user_reg.with_extension("tmp");
    std::fs::write(&tmp_path, &new_content)
        .map_err(|e| MantleError::Skse(format!("Failed to write {}: {e}", tmp_path.display())))?;
    std::fs::rename(&tmp_path, user_reg)
        .map_err(|e| MantleError::Skse(format!("Failed to rename to {}: {e}", user_reg.display())))?;

    tracing::debug!(
        path = %user_reg.display(),
        dlls = ?dlls,
        "DLL overrides written to Proton prefix"
    );

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_reg(content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("user.reg");
        std::fs::write(&path, content).unwrap();
        (dir, path)
    }

    #[test]
    fn absent_file_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("user.reg");
        let result = write_dll_overrides(&path, &["skse64_steam_loader"]);
        assert!(result.is_ok());
        assert!(!path.exists());
    }

    #[test]
    fn appends_entry_to_existing_section() {
        let content = "\
WINE REGISTRY Version 2\n\
\n\
[Software\\Wine\\DllOverrides]\n\
1000000\n\
\n\
[Software\\Wine\\Other]\n\
1000000\n";
        let (_dir, path) = make_reg(content);
        write_dll_overrides(&path, &["skse64_steam_loader"]).unwrap();
        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("\"skse64_steam_loader\"=\"native,builtin\""));
        // The [Other] section should still be present after the new entry.
        assert!(result.contains("[Software\\Wine\\Other]"));
    }

    #[test]
    fn idempotent_double_write() {
        let content = "\
WINE REGISTRY Version 2\n\
\n\
[Software\\Wine\\DllOverrides]\n\
1000000\n";
        let (_dir, path) = make_reg(content);
        write_dll_overrides(&path, &["skse64_steam_loader"]).unwrap();
        write_dll_overrides(&path, &["skse64_steam_loader"]).unwrap();

        let result = std::fs::read_to_string(&path).unwrap();
        let count = result.matches("\"skse64_steam_loader\"=").count();
        assert_eq!(count, 1, "entry should appear exactly once after two writes");
    }

    #[test]
    fn creates_section_when_absent() {
        let content = "WINE REGISTRY Version 2\n";
        let (_dir, path) = make_reg(content);
        write_dll_overrides(&path, &["skse64_steam_loader"]).unwrap();
        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("[Software\\Wine\\DllOverrides]"));
        assert!(result.contains("\"skse64_steam_loader\"=\"native,builtin\""));
    }

    #[test]
    fn multiple_dlls_all_added() {
        let content = "[Software\\Wine\\DllOverrides]\n1000000\n";
        let (_dir, path) = make_reg(content);
        write_dll_overrides(&path, &["dll_a", "dll_b", "dll_c"]).unwrap();
        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("\"dll_a\"=\"native,builtin\""));
        assert!(result.contains("\"dll_b\"=\"native,builtin\""));
        assert!(result.contains("\"dll_c\"=\"native,builtin\""));
    }

    #[test]
    fn existing_entry_not_duplicated() {
        let content = "[Software\\Wine\\DllOverrides]\n1000000\n\
                       \"already_here\"=\"native,builtin\"\n";
        let (_dir, path) = make_reg(content);
        write_dll_overrides(&path, &["already_here", "new_dll"]).unwrap();
        let result = std::fs::read_to_string(&path).unwrap();
        let count = result.matches("\"already_here\"=").count();
        assert_eq!(count, 1);
        assert!(result.contains("\"new_dll\"=\"native,builtin\""));
    }
}
