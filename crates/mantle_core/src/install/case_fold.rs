//! Case-folding normalizer for mod directory trees.
//!
//! Renames all file and directory entries to lowercase so that Windows-authored
//! mods (where `Textures\` and `textures\` are the same path on NTFS) work
//! correctly on Linux's case-sensitive filesystem.
//!
//! # Algorithm
//!
//! 1. Walk the directory tree **bottom-up** (deepest entries first) so that
//!    child paths are renamed before their parent directory names change.
//! 2. Within each directory, group entries by their lowercase name.  If two
//!    entries share a lowercase name (collision), both are reported and neither
//!    is renamed.
//! 3. Use a **two-step rename** via an intermediate `.__case_tmp__` suffix to
//!    safely handle case-only renames even on case-insensitive filesystems
//!    (e.g. NTFS volumes mounted on Linux).
//! 4. **Exclusion patterns**: any directory whose path relative to the root
//!    contains one of the given substrings is excluded entirely — it is not
//!    recursed into and its name is not normalised in the parent.
//!
//! # Example
//! ```no_run
//! use mantle_core::install::case_fold::{normalize_dir, NormalizeResult};
//! use std::path::Path;
//!
//! let result = normalize_dir(
//!     Path::new("/path/to/MyMod"),
//!     false,                        // dry_run
//!     &["SKSE/Plugins"],            // exclusions
//! );
//! println!("{} renamed", result.total_renamed());
//! ```

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

// ── Public types ──────────────────────────────────────────────────────────────

/// Outcome of a [`normalize_dir`] pass.
#[derive(Debug, Default)]
pub struct NormalizeResult {
    /// Number of directory entries renamed to lowercase.
    pub renamed_dirs: u32,
    /// Number of file entries renamed to lowercase.
    pub renamed_files: u32,
    /// Pairs `(path, conflicting_path)` where two entries map to the same
    /// lowercase name.  Neither entry is renamed.
    pub collisions: Vec<(PathBuf, PathBuf)>,
    /// Pairs `(path, error_message)` for rename operations that failed.
    pub errors: Vec<(PathBuf, String)>,
    /// Number of entries already lowercase — no rename needed.
    pub skipped: u32,
    /// Total entries examined (renamed + skipped + collisions + collision peers).
    pub total_scanned: u32,
}

impl NormalizeResult {
    /// Total entries renamed (`renamed_dirs + renamed_files`).
    ///
    /// In dry-run mode this reflects what *would* have been renamed.
    #[must_use]
    pub fn total_renamed(&self) -> u32 {
        self.renamed_dirs + self.renamed_files
    }

    /// `true` if there are any collisions or rename errors.
    #[must_use]
    pub fn has_issues(&self) -> bool {
        !self.collisions.is_empty() || !self.errors.is_empty()
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Normalize all file and directory names under `root` to lowercase.
///
/// # Parameters
/// - `root`: Root directory to normalize (typically a mod folder).
/// - `dry_run`: If `true`, compute renames but do **not** execute them.
///   Counters in the result still reflect what *would* have been renamed.
/// - `exclusions`: Path-fragment substrings.  Any directory whose path
///   relative to `root` contains one of these strings is skipped entirely
///   (not recursed into and not renamed in the parent).
///
/// # Returns
/// A [`NormalizeResult`] with full statistics.
///
/// # Side Effects
/// When `dry_run` is `false`, renames entries under `root` to lowercase.
/// Temporarily creates `<name>.__case_tmp__` during each rename.
#[must_use]
pub fn normalize_dir(root: &Path, dry_run: bool, exclusions: &[&str]) -> NormalizeResult {
    let mut result = NormalizeResult::default();
    if !root.is_dir() {
        result
            .errors
            .push((root.to_path_buf(), "not a directory".to_string()));
        return result;
    }
    normalize_recursive(root, root, dry_run, exclusions, &mut result);
    result
}

// ── Internal ──────────────────────────────────────────────────────────────────

/// Recursive bottom-up normalizer.
///
/// Reads entries in `dir`, recurses into non-excluded subdirectories first
/// (so their contents are renamed before their parent names change), then
/// normalises the file names and the non-excluded subdirectory names within
/// `dir`.
fn normalize_recursive(
    dir: &Path,
    root: &Path,
    dry_run: bool,
    exclusions: &[&str],
    result: &mut NormalizeResult,
) {
    let entries: Vec<fs::DirEntry> = match fs::read_dir(dir) {
        Ok(rd) => rd.flatten().collect(),
        Err(e) => {
            result.errors.push((dir.to_path_buf(), e.to_string()));
            return;
        }
    };

    let mut files = Vec::new();
    let mut subdirs_active = Vec::new();
    let mut subdirs_excluded: u32 = 0;

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let rel_str = rel.to_string_lossy();
            if exclusions.iter().any(|ex| rel_str.contains(*ex)) {
                subdirs_excluded += 1;
            } else {
                subdirs_active.push(entry);
            }
        } else {
            files.push(entry);
        }
    }

    // Recurse depth-first before touching any names in this directory.
    for subdir_entry in &subdirs_active {
        normalize_recursive(&subdir_entry.path(), root, dry_run, exclusions, result);
    }

    // Normalize file names in this directory.
    normalize_entries(dir, &files, false, dry_run, result);

    // Normalize non-excluded subdirectory names.
    normalize_entries(dir, &subdirs_active, true, dry_run, result);

    // Excluded subdirs: count each as one scanned entry but don't touch them.
    result.total_scanned = result.total_scanned.saturating_add(subdirs_excluded);
}

/// Rename a list of directory entries within `parent` to their lowercase names.
///
/// Groups entries by their lowercase name to detect collisions (two entries
/// that would map to the same name after folding).  Collisions are reported
/// but not renamed.
fn normalize_entries(
    parent: &Path,
    entries: &[fs::DirEntry],
    is_dir: bool,
    dry_run: bool,
    result: &mut NormalizeResult,
) {
    // Group by lowercase name → list of originals that map there.
    let mut lower_map: HashMap<String, Vec<String>> = HashMap::new();
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_lowercase();
        lower_map.entry(lower).or_default().push(name);
    }

    for (lower_name, originals) in lower_map {
        let count = u32::try_from(originals.len()).unwrap_or(u32::MAX);
        result.total_scanned = result.total_scanned.saturating_add(count);

        if originals.len() > 1 {
            // Collision: two or more entries share a lowercase name.
            // Record each pair; rename none.
            for (i, orig) in originals.iter().enumerate() {
                let peer_idx = usize::from(i == 0);
                result
                    .collisions
                    .push((parent.join(orig), parent.join(&originals[peer_idx])));
            }
            continue;
        }

        let orig = &originals[0];
        if *orig == lower_name {
            result.skipped += 1;
            continue;
        }

        let src = parent.join(orig);
        let dst = parent.join(&lower_name);

        if !dry_run {
            if let Err(e) = rename_two_step(&src, &dst) {
                result.errors.push((src, e.to_string()));
                continue;
            }
        }

        if is_dir {
            result.renamed_dirs += 1;
        } else {
            result.renamed_files += 1;
        }
    }
}

/// Rename `src` → `dst` via an intermediate temp name.
///
/// The two-step rename (`src` → `src.__case_tmp__` → `dst`) ensures that
/// case-only renames work even on case-insensitive filesystems where a direct
/// `rename("Foo", "foo")` would be a no-op.
fn rename_two_step(src: &Path, dst: &Path) -> std::io::Result<()> {
    let orig_name = src
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let tmp = src.with_file_name(format!("{orig_name}.__case_tmp__"));
    fs::rename(src, &tmp)?;
    fs::rename(&tmp, dst)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ── Helpers ───────────────────────────────────────────────────────────

    /// Create a temp directory pre-populated with `(relative_path, content)` pairs.
    fn build_tree(files: &[(&str, &[u8])]) -> TempDir {
        let dir = TempDir::new().unwrap();
        for (rel, content) in files {
            let p = dir.path().join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(p, content).unwrap();
        }
        dir
    }

    // ── Already lowercase ─────────────────────────────────────────────────

    #[test]
    fn already_lowercase_file_skipped() {
        let dir = build_tree(&[("textures/wood.dds", b"data")]);
        let result = normalize_dir(dir.path(), false, &[]);
        assert_eq!(result.renamed_files, 0, "file was already lowercase");
        assert_eq!(result.renamed_dirs, 0, "dir was already lowercase");
        // 1 dir ("textures") + 1 file ("wood.dds") = 2 skipped
        assert_eq!(result.skipped, 2);
        assert!(result.collisions.is_empty());
        assert!(result.errors.is_empty());
    }

    // ── Uppercase → lowercase ──────────────────────────────────────────────

    #[test]
    fn uppercase_file_is_renamed() {
        let dir = build_tree(&[("textures/Wood.dds", b"pixels")]);
        let result = normalize_dir(dir.path(), false, &[]);
        assert_eq!(result.renamed_files, 1);
        assert!(dir.path().join("textures/wood.dds").exists(), "renamed file must exist");
        assert!(!dir.path().join("textures/Wood.dds").exists(), "original must be gone");
    }

    #[test]
    fn uppercase_dir_is_renamed() {
        let dir = build_tree(&[("Textures/file.dds", b"data")]);
        let result = normalize_dir(dir.path(), false, &[]);
        assert_eq!(result.renamed_dirs, 1);
        assert!(dir.path().join("textures").is_dir(), "renamed dir must exist");
        assert!(!dir.path().join("Textures").exists(), "original dir must be gone");
    }

    #[test]
    fn both_file_and_dir_renamed() {
        let dir = build_tree(&[("Textures/Wood.dds", b"data")]);
        let result = normalize_dir(dir.path(), false, &[]);
        assert_eq!(result.renamed_dirs, 1, "Textures→textures");
        assert_eq!(result.renamed_files, 1, "Wood.dds→wood.dds");
        assert!(dir.path().join("textures/wood.dds").exists());
    }

    // ── Nested directories ────────────────────────────────────────────────

    #[test]
    fn deeply_nested_all_normalized() {
        let dir = build_tree(&[("A/B/C/File.Txt", b"x")]);
        let _ = normalize_dir(dir.path(), false, &[]);
        assert!(dir.path().join("a/b/c/file.txt").exists());
    }

    // ── Dry run ───────────────────────────────────────────────────────────

    #[test]
    fn dry_run_counts_but_does_not_rename() {
        let dir = build_tree(&[("Textures/Wood.dds", b"data")]);
        let result = normalize_dir(dir.path(), true, &[]);
        // Should count what would be renamed
        assert!(result.total_renamed() > 0, "dry run must still count renames");
        // But originals must survive
        assert!(
            dir.path().join("Textures").exists(),
            "Textures dir must survive dry run"
        );
        assert!(
            dir.path().join("Textures/Wood.dds").exists(),
            "Wood.dds must survive dry run"
        );
    }

    // ── Collision detection ───────────────────────────────────────────────

    #[test]
    fn collision_neither_renamed() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("readme.txt"), b"lower").unwrap();
        fs::write(dir.path().join("README.txt"), b"upper").unwrap();

        let result = normalize_dir(dir.path(), false, &[]);
        // Two collisions reported (one per conflicting entry)
        assert_eq!(result.collisions.len(), 2, "one collision entry per file");
        assert_eq!(result.renamed_files, 0, "neither file must be renamed");
        // Both originals still exist
        assert!(dir.path().join("readme.txt").exists());
        assert!(dir.path().join("README.txt").exists());
    }

    // ── Exclusion patterns ────────────────────────────────────────────────

    #[test]
    fn excluded_dir_not_renamed_or_recursed() {
        let dir = build_tree(&[
            ("SKSE/Plugins/MyPlugin.dll", b"dll"),
            ("Textures/Wood.dds", b"tex"),
        ]);
        let result = normalize_dir(dir.path(), false, &["SKSE"]);

        // Non-excluded path is fully normalized
        assert!(dir.path().join("textures/wood.dds").exists());

        // Excluded dir: name not changed, contents not touched
        assert!(dir.path().join("SKSE").exists(), "SKSE must not be renamed");
        assert!(
            dir.path().join("SKSE/Plugins/MyPlugin.dll").exists(),
            "contents must not be renamed"
        );

        // No errors
        assert!(result.errors.is_empty());
    }

    #[test]
    fn exclusion_does_not_affect_sibling_dirs() {
        let dir = build_tree(&[
            ("SKSE/Plugins/Foo.dll", b"dll"),
            ("Meshes/Foo.nif", b"nif"),
        ]);
        let _ = normalize_dir(dir.path(), false, &["SKSE"]);
        assert!(dir.path().join("meshes/foo.nif").exists());
    }

    // ── Error handling ────────────────────────────────────────────────────

    #[test]
    fn non_existent_root_returns_error() {
        let result = normalize_dir(Path::new("/no/such/path/xyz"), false, &[]);
        assert!(!result.errors.is_empty(), "must report error for missing root");
        assert_eq!(result.total_renamed(), 0);
    }

    // ── total_scanned ─────────────────────────────────────────────────────

    #[test]
    fn total_scanned_matches_entry_count() {
        // 1 dir "textures" + 2 files "a.dds", "b.dds"
        let dir = build_tree(&[("textures/a.dds", b""), ("textures/b.dds", b"")]);
        let result = normalize_dir(dir.path(), false, &[]);
        assert_eq!(result.total_scanned, 3, "textures/ + a.dds + b.dds");
        assert_eq!(result.skipped, 3, "all already lowercase");
    }
}
