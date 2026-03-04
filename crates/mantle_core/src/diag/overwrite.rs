//! Overwrite / upper-directory auto-categorizer.
//!
//! After a game session ends, the VFS overlay's upper directory may contain
//! newly generated files (SKSE plugin data, `DynDOLOD` output, xEdit backups,
//! crash logs, ENB shader caches, etc.).  This module classifies those files
//! so they can be moved to named mod folders rather than cluttering a shared
//! overwrite directory.
//!
//! # Workflow
//! 1. Call [`scan_overwrite`] with the path to the upper / overwrite directory.
//! 2. Inspect the returned [`OverwriteScanResult`] for non-empty categories.
//! 3. Move files from each category to the appropriate named mod folder.
//!
//! # Category matching
//! Each [`FileCategory`] is tried in order; the **first** match wins.  Matching
//! checks (all case-insensitive on the normalised path):
//!
//! 1. **Directory markers**: if the file's parent path contains any marker string.
//! 2. **Prefix patterns**: if the file's normalised relative path starts with the
//!    given string.
//! 3. **Suffix patterns**: if the normalised path ends with the given string.
//! 4. **Contains patterns**: if the normalised path contains the given string.
//! 5. **Exact matches**: if the normalised path equals the given string exactly.
//!
//! Files that match none of the above are placed in the `"Uncategorized"`
//! bucket and should be triaged manually.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

// ── Public types ──────────────────────────────────────────────────────────────

/// One category of generated files.
///
/// Categories are checked in declaration order; the first match wins.
#[derive(Debug, Clone, Copy)]
pub struct FileCategory {
    /// Human-readable category name (used as key in [`OverwriteScanResult`]).
    pub name: &'static str,
    /// Suggested mod folder name for auto-move (e.g. `"[Generated] DynDOLOD Output"`).
    pub mod_target: &'static str,
    /// One-line description shown in the UI triage view.
    pub description: &'static str,
    /// Substrings that, if found anywhere in the **parent directory** path
    /// (case-insensitive), immediately classify the file into this category.
    pub dir_markers: &'static [&'static str],
    /// Normalised-path prefixes (lowercase, forward-slash).  A file matches if
    /// its path starts with any of these.
    pub prefix_patterns: &'static [&'static str],
    /// Normalised-path suffixes (lowercase).  A file matches if its path ends
    /// with any of these.
    pub suffix_patterns: &'static [&'static str],
    /// Substrings (lowercase).  A file matches if its normalised path contains
    /// any of these.
    pub contains_patterns: &'static [&'static str],
    /// Exact normalised paths.  A file matches if its normalised path equals
    /// one of these exactly.
    pub exact_matches: &'static [&'static str],
}

impl FileCategory {
    /// Return `true` if `norm_path` (lowercase, forward-slash) or `abs_dir`
    /// (the parent directory as returned by the OS) matches this category.
    #[must_use]
    pub fn matches(&self, norm_path: &str, abs_dir: &str) -> bool {
        let abs_lower = abs_dir.to_lowercase();

        // 1. Directory markers (fast path — avoids walking the pattern lists)
        for marker in self.dir_markers {
            let m = marker.to_lowercase();
            // Match either as a path component in the absolute dir or as a
            // prefix segment in the normalised relative path.
            if abs_lower.contains(&m)
                || norm_path.starts_with(&format!("{m}/"))
                || norm_path == m
            {
                return true;
            }
        }

        // 2. Prefix patterns
        for prefix in self.prefix_patterns {
            if norm_path.starts_with(prefix) {
                return true;
            }
        }

        // 3. Suffix patterns
        for suffix in self.suffix_patterns {
            if norm_path.ends_with(suffix) {
                return true;
            }
        }

        // 4. Contains patterns
        for contains in self.contains_patterns {
            if norm_path.contains(contains) {
                return true;
            }
        }

        // 5. Exact matches
        for exact in self.exact_matches {
            if norm_path == *exact {
                return true;
            }
        }

        false
    }
}

/// Result of [`scan_overwrite`].
#[derive(Debug, Default)]
pub struct OverwriteScanResult {
    /// Files grouped by category name.
    ///
    /// Each value is a sorted list of paths **relative to the overwrite root**.
    /// An `"Uncategorized"` key is always present (may be empty).
    pub by_category: HashMap<String, Vec<PathBuf>>,
}

impl OverwriteScanResult {
    /// Total number of files found across all categories.
    #[must_use]
    pub fn total_files(&self) -> usize {
        self.by_category.values().map(Vec::len).sum()
    }

    /// `true` if the overwrite directory contained no files.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total_files() == 0
    }

    /// Sorted list of category names that have at least one file.
    #[must_use]
    pub fn non_empty_categories(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .by_category
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, _)| k.as_str())
            .collect();
        names.sort_unstable();
        names
    }
}

// ── Category table ────────────────────────────────────────────────────────────

/// All file categories, checked in order (first match wins).
pub static CATEGORIES: &[FileCategory] = &[
    FileCategory {
        name: "Creation Club",
        mod_target: "[Generated] Creation Club Content",
        description: "Bethesda Creation Club DLC content",
        dir_markers: &[],
        prefix_patterns: &[
            "textures/cc",
            "meshes/cc",
            "sound/cc",
            "scripts/cc",
            "strings/cc",
        ],
        suffix_patterns: &[],
        contains_patterns: &[],
        exact_matches: &[],
    },
    FileCategory {
        name: "DynDOLOD Output",
        mod_target: "[Generated] DynDOLOD Output",
        description: "DynDOLOD / TexGen LOD generation output",
        dir_markers: &["DynDOLOD", "DynDOLOD_Output"],
        prefix_patterns: &[
            "dyndolod/",
            "textures/terrain/",
            "meshes/terrain/",
        ],
        suffix_patterns: &[
            "dyndolod.esp",
            "dyndolod.esm",
            "dyndolod.esl",
        ],
        contains_patterns: &["dyndolod"],
        exact_matches: &[],
    },
    FileCategory {
        name: "TexGen Output",
        mod_target: "[Generated] TexGen Output",
        description: "TexGen texture generation output",
        dir_markers: &["TexGen_Output"],
        prefix_patterns: &["texgen_output/"],
        suffix_patterns: &[],
        contains_patterns: &[],
        exact_matches: &[],
    },
    FileCategory {
        name: "Nemesis Output",
        mod_target: "[Generated] Nemesis Output",
        description: "Nemesis animation engine output",
        dir_markers: &["Nemesis_Engine"],
        prefix_patterns: &[
            "nemesis_engine/",
            "meshes/actors/character/animations/dynamicanimationreplacer",
            "meshes/actors/character/animations/openanimationreplacer",
            "meshes/actors/character/_1stperson/animations",
        ],
        suffix_patterns: &[],
        contains_patterns: &[],
        exact_matches: &[],
    },
    FileCategory {
        name: "FNIS Output",
        mod_target: "[Generated] FNIS Output",
        description: "FNIS animation output",
        dir_markers: &["GenerateFNISforUsers"],
        prefix_patterns: &[
            "tools/generatefnis",
            "meshes/actors/character/animations/fnis",
        ],
        suffix_patterns: &[],
        contains_patterns: &[],
        exact_matches: &[],
    },
    FileCategory {
        name: "BodySlide Output",
        mod_target: "[Generated] BodySlide Output",
        description: "BodySlide / Outfit Studio output",
        dir_markers: &["CalienteTools"],
        prefix_patterns: &[
            "calientetools/",
            "meshes/actors/character/bodygendata",
            "shapedata/",
        ],
        suffix_patterns: &[],
        contains_patterns: &[],
        exact_matches: &[],
    },
    FileCategory {
        name: "xEdit Backups",
        mod_target: "[Generated] xEdit Backups",
        description: "xEdit session backup files",
        dir_markers: &[
            "TES5Edit Backups",
            "SSEEdit Backups",
            "FO4Edit Backups",
            "FNVEdit Backups",
            "TES4Edit Backups",
            "xEdit Backups",
        ],
        prefix_patterns: &[
            "tes5edit backups/",
            "sseedit backups/",
            "fo4edit backups/",
            "fnvedit backups/",
            "tes4edit backups/",
            "xedit backups/",
        ],
        suffix_patterns: &[],
        contains_patterns: &[
            ".esp.backup.",
            ".esm.backup.",
            ".esl.backup.",
        ],
        exact_matches: &[],
    },
    FileCategory {
        name: "SKSE Data",
        mod_target: "[Generated] SKSE Cosaves",
        description: "Script extender plugin data and per-save cosave data",
        dir_markers: &[],
        prefix_patterns: &[
            "skse/plugins/",
            "skse/",
            "nvse/",
            "f4se/",
            "obse/",
            "mwse/",
            "sfse/",
        ],
        suffix_patterns: &[
            ".skse",
            ".f4se",
            ".nvse",
            ".fose",
            ".obse",
            ".sfse",
        ],
        contains_patterns: &[],
        exact_matches: &[],
    },
    FileCategory {
        name: "ENB / ReShade",
        mod_target: "[Generated] ENB Config",
        description: "ENB / ReShade configuration and shader cache",
        dir_markers: &["enbseries", "reshade-shaders"],
        prefix_patterns: &[
            "enbseries/",
            "reshade-shaders/",
        ],
        suffix_patterns: &[],
        contains_patterns: &[],
        exact_matches: &[
            "enblocal.ini",
            "enbseries.ini",
            "reshade.ini",
        ],
    },
    FileCategory {
        name: "Crash Logs",
        mod_target: "[Generated] Crash Logs",
        description: "Game crash logs and diagnostic files",
        dir_markers: &["NetScriptFramework", "Trainwreck"],
        prefix_patterns: &[
            "netscriptframework/",
            "trainwreck/",
            "crash-",
        ],
        suffix_patterns: &[],
        contains_patterns: &["crashlog"],
        exact_matches: &["crash.txt"],
    },
    FileCategory {
        name: "Synthesis Output",
        mod_target: "[Generated] Synthesis Output",
        description: "Synthesis patcher output",
        dir_markers: &["SynthesisOutput"],
        prefix_patterns: &["synthesisoutput/"],
        suffix_patterns: &[],
        contains_patterns: &[],
        exact_matches: &["synthesis.esp"],
    },
    FileCategory {
        name: "Bashed Patch",
        mod_target: "[Generated] Bashed Patch",
        description: "Wrye Bash bashed / merged patch",
        dir_markers: &[],
        prefix_patterns: &["bashed patch"],
        suffix_patterns: &[],
        contains_patterns: &[],
        exact_matches: &[],
    },
    FileCategory {
        name: "Smashed Patch",
        mod_target: "[Generated] Smashed Patch",
        description: "Mator Smash merged patch",
        dir_markers: &[],
        prefix_patterns: &["smashed patch"],
        suffix_patterns: &[],
        contains_patterns: &[],
        exact_matches: &[],
    },
];

// ── Public API ────────────────────────────────────────────────────────────────

/// Scan `overwrite_dir` and classify every file into a category.
///
/// All files that do not match any category are placed in `"Uncategorized"`.
/// Paths in the result are relative to `overwrite_dir` and use the platform
/// path separator (not normalised).
///
/// # Parameters
/// - `overwrite_dir`: Root of the VFS upper directory or MO2 Overwrite folder.
///
/// # Returns
/// An [`OverwriteScanResult`] with files grouped by category name.
#[must_use]
pub fn scan_overwrite(overwrite_dir: &Path) -> OverwriteScanResult {
    scan_overwrite_with_categories(overwrite_dir, CATEGORIES)
}

/// Like [`scan_overwrite`] but accepts a custom category slice.
///
/// Useful for tests and for callers that want to extend or replace the default
/// category list.
#[must_use]
pub fn scan_overwrite_with_categories(
    overwrite_dir: &Path,
    categories: &[FileCategory],
) -> OverwriteScanResult {
    let mut result = OverwriteScanResult::default();
    result.by_category.insert("Uncategorized".to_owned(), Vec::new());
    for cat in categories {
        result
            .by_category
            .entry(cat.name.to_owned())
            .or_default();
    }

    if !overwrite_dir.is_dir() {
        return result;
    }

    walk_dir(overwrite_dir, overwrite_dir, categories, &mut result);

    // Sort each category's file list for deterministic output.
    for files in result.by_category.values_mut() {
        files.sort();
    }

    result
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn walk_dir(
    dir: &Path,
    root: &Path,
    categories: &[FileCategory],
    result: &mut OverwriteScanResult,
) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_dir(&path, root, categories, result);
        } else {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let norm = rel.to_string_lossy().replace('\\', "/").to_lowercase();
            let abs_dir_str = dir.to_string_lossy();
            let category = classify(&norm, &abs_dir_str, categories);
            result
                .by_category
                .entry(category.to_owned())
                .or_default()
                .push(rel.to_path_buf());
        }
    }
}

fn classify<'a>(
    norm_path: &str,
    abs_dir: &str,
    categories: &'a [FileCategory],
) -> &'a str {
    for cat in categories {
        if cat.matches(norm_path, abs_dir) {
            return cat.name;
        }
    }
    "Uncategorized"
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn build_tree(files: &[&str]) -> TempDir {
        let dir = TempDir::new().unwrap();
        for rel in files {
            let p = dir.path().join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(p, b"").unwrap();
        }
        dir
    }

    fn category_of(dir: &TempDir, rel: &str) -> String {
        let result = scan_overwrite(dir.path());
        for (cat, files) in &result.by_category {
            for f in files {
                if f == &PathBuf::from(rel) {
                    return cat.clone();
                }
            }
        }
        "not found".to_owned()
    }

    // ── Structural tests ──────────────────────────────────────────────────

    #[test]
    fn empty_dir_returns_empty_result() {
        let dir = TempDir::new().unwrap();
        let result = scan_overwrite(dir.path());
        assert!(result.is_empty());
    }

    #[test]
    fn nonexistent_dir_returns_empty() {
        let result = scan_overwrite(Path::new("/no/such/dir/xyz"));
        assert!(result.is_empty());
    }

    #[test]
    fn uncategorized_key_always_present() {
        let dir = TempDir::new().unwrap();
        let result = scan_overwrite(dir.path());
        assert!(result.by_category.contains_key("Uncategorized"));
    }

    // ── Category classification ───────────────────────────────────────────

    #[test]
    fn dyndolod_dir_classified_correctly() {
        let dir = build_tree(&["DynDOLOD/Output/DynDOLOD.esp"]);
        assert_eq!(category_of(&dir, "DynDOLOD/Output/DynDOLOD.esp"), "DynDOLOD Output");
    }

    #[test]
    fn skse_plugin_json_classified_as_skse_data() {
        let dir = build_tree(&["SKSE/Plugins/myplugin.json"]);
        assert_eq!(category_of(&dir, "SKSE/Plugins/myplugin.json"), "SKSE Data");
    }

    #[test]
    fn crash_log_classified_correctly() {
        let dir = build_tree(&["crash-2024-01-01-00-00-00.txt"]);
        assert_eq!(
            category_of(&dir, "crash-2024-01-01-00-00-00.txt"),
            "Crash Logs"
        );
    }

    #[test]
    fn xedit_backup_classified_correctly() {
        let dir = build_tree(&["MyPlugin.esp.backup.2024_01_01"]);
        assert_eq!(
            category_of(&dir, "MyPlugin.esp.backup.2024_01_01"),
            "xEdit Backups"
        );
    }

    #[test]
    fn bodyslide_output_classified_correctly() {
        let dir = build_tree(&["CalienteTools/BodySlide/ShapeData/Outfit.nif"]);
        assert_eq!(
            category_of(&dir, "CalienteTools/BodySlide/ShapeData/Outfit.nif"),
            "BodySlide Output"
        );
    }

    #[test]
    fn unrecognised_file_is_uncategorized() {
        let dir = build_tree(&["some_random_file.dat"]);
        assert_eq!(category_of(&dir, "some_random_file.dat"), "Uncategorized");
    }

    #[test]
    fn enb_ini_classified_correctly() {
        let dir = build_tree(&["enblocal.ini"]);
        assert_eq!(category_of(&dir, "enblocal.ini"), "ENB / ReShade");
    }

    #[test]
    fn total_files_sums_all_categories() {
        let dir = build_tree(&[
            "DynDOLOD/DynDOLOD.esp",
            "SKSE/Plugins/data.json",
            "random.txt",
        ]);
        let result = scan_overwrite(dir.path());
        assert_eq!(result.total_files(), 3);
    }

    #[test]
    fn non_empty_categories_sorted() {
        let dir = build_tree(&[
            "DynDOLOD/x.esp",
            "crash-2024.txt",
            "random.dat",
        ]);
        let result = scan_overwrite(dir.path());
        let cats = result.non_empty_categories();
        assert!(cats.windows(2).all(|w| w[0] <= w[1]), "must be sorted");
    }
}
