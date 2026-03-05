//! User-installable theme discovery.
//!
//! Themes live in `{data_dir}/themes/` as `.css` files containing
//! `@define-color` declarations.  An optional `theme.toml` manifest placed
//! alongside the CSS provides display metadata.
//!
//! # File format
//! ```text
//! ~/.local/share/mantle-manager/themes/
//!   my-theme.css      ← required (palette overrides)
//!   my-theme.toml     ← optional metadata
//! ```
//!
//! `theme.toml`:
//! ```toml
//! name        = "My Theme"
//! author      = "You"
//! description = "A short description"
//! color_scheme = "dark"   # "dark" | "light" | "auto"
//! ```
//!
//! The theme ID is the CSS filename stem (e.g. `my-theme`).
//!
//! # Discovery
//! [`scan_themes_dir`] scans a directory for `.css` files, reads the CSS
//! content, and merges any adjacent `.toml` manifest.  A missing directory
//! returns an empty `Vec` without error — identical to the plugin registry
//! behaviour.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::{info, warn};

// ─── ThemeManifest ────────────────────────────────────────────────────────────

/// Deserialised representation of an optional `theme.toml` file placed
/// alongside a `.css` theme file.
///
/// All fields are optional; absent fields fall back to values derived from
/// the filename.
///
/// # Example
/// ```toml
/// name         = "Gruvbox Dark"
/// author       = "Community"
/// description  = "Warm retro-groove colour scheme."
/// color_scheme = "dark"
/// ```
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ThemeManifest {
    /// Display name shown in the UI. Falls back to the theme ID if absent.
    pub name: Option<String>,
    /// Author / maintainer name.
    pub author: Option<String>,
    /// Short one-line description shown as a subtitle.
    pub description: Option<String>,
    /// Hint for the libadwaita colour scheme base.
    ///
    /// Accepted values: `"dark"`, `"light"`, `"auto"` (default when absent).
    pub color_scheme: Option<String>,
}

// ─── UserTheme ────────────────────────────────────────────────────────────────

/// A successfully loaded user-installed theme.
#[derive(Debug, Clone)]
pub struct UserTheme {
    /// Stable ID derived from the CSS filename stem (e.g. `"my-theme"`).
    pub id: String,
    /// Human-readable name (from manifest or falls back to `id`).
    pub name: String,
    /// Author string (empty when not specified in manifest).
    pub author: String,
    /// Short description (empty when not specified in manifest).
    pub description: String,
    /// Colour scheme hint: `"dark"`, `"light"`, or `"auto"`.
    pub color_scheme: String,
    /// Full CSS content of the theme file.
    pub css: String,
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Return `{data_dir}/themes/` — the canonical user themes directory.
///
/// The returned path is not guaranteed to exist; create it with
/// `std::fs::create_dir_all` before writing theme files.
#[must_use]
pub fn themes_data_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("themes")
}

/// Scan `themes_dir` and return all valid `.css` theme files found.
///
/// Files are processed in alphabetical order by filename.  Each `.css` file
/// is read for its palette CSS; an adjacent `.toml` manifest (same stem,
/// `.toml` extension) is read for metadata.  Missing or malformed manifests
/// fall back to defaults silently.
///
/// A missing `themes_dir` returns an empty `Vec` without logging a warning —
/// this is the normal first-launch state.
///
/// # Parameters
/// - `themes_dir`: Directory to scan (usually `{data_dir}/themes/`).
///
/// # Returns
/// `Vec<UserTheme>` in alphabetical filename order.  Empty when the directory
/// is absent or contains no `.css` files.
pub fn scan_themes_dir(themes_dir: &Path) -> Vec<UserTheme> {
    if !themes_dir.exists() {
        info!(path = %themes_dir.display(), "themes directory not found — no user themes loaded");
        return Vec::new();
    }

    let mut entries: Vec<PathBuf> = match std::fs::read_dir(themes_dir) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("css"))
            .collect(),
        Err(err) => {
            warn!(
                path = %themes_dir.display(),
                error = %err,
                "failed to read themes directory"
            );
            return Vec::new();
        }
    };

    // Alphabetical load order — consistent and predictable.
    entries
        .sort_by(|a, b| a.file_name().unwrap_or_default().cmp(b.file_name().unwrap_or_default()));

    entries.iter().filter_map(|path| load_theme(path)).collect()
}

// ─── Private helpers ──────────────────────────────────────────────────────────

/// Attempt to load a single `.css` theme file and its optional manifest.
///
/// Returns `None` if the CSS file cannot be read (logged as a warning).
fn load_theme(css_path: &Path) -> Option<UserTheme> {
    // Theme ID = filename stem.
    let id = css_path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown").to_owned();

    // Read CSS content.
    let css = match std::fs::read_to_string(css_path) {
        Ok(c) => c,
        Err(e) => {
            warn!(path = %css_path.display(), error = %e, "failed to read theme CSS — skipping");
            return None;
        }
    };

    // Read optional manifest.
    let manifest = read_manifest(css_path);

    info!(id = %id, path = %css_path.display(), "loaded user theme");

    Some(UserTheme {
        name: manifest.name.clone().unwrap_or_else(|| id.clone()),
        author: manifest.author.unwrap_or_default(),
        description: manifest.description.unwrap_or_default(),
        color_scheme: manifest.color_scheme.unwrap_or_else(|| "auto".to_owned()),
        id,
        css,
    })
}

/// Read and parse `{css_stem}.toml` adjacent to `css_path`.
///
/// Returns [`ThemeManifest::default`] silently if the file is absent or
/// cannot be parsed.
fn read_manifest(css_path: &Path) -> ThemeManifest {
    let manifest_path = css_path.with_extension("toml");

    match std::fs::read_to_string(&manifest_path) {
        Ok(contents) => match toml::from_str::<ThemeManifest>(&contents) {
            Ok(m) => {
                info!(path = %manifest_path.display(), "loaded theme.toml manifest");
                m
            }
            Err(e) => {
                warn!(
                    path = %manifest_path.display(),
                    error = %e,
                    "failed to parse theme.toml — using defaults"
                );
                ThemeManifest::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ThemeManifest::default(),
        Err(e) => {
            warn!(
                path = %manifest_path.display(),
                error = %e,
                "could not read theme.toml — using defaults"
            );
            ThemeManifest::default()
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Missing themes directory returns empty Vec without panic.
    #[test]
    fn scan_missing_dir_returns_empty() {
        let themes = scan_themes_dir(Path::new("/does/not/exist/themes"));
        assert!(themes.is_empty());
    }

    /// Empty themes directory returns empty Vec.
    #[test]
    fn scan_empty_dir_returns_empty() {
        let temp = tempfile::tempdir().unwrap();
        let themes = scan_themes_dir(temp.path());
        assert!(themes.is_empty());
    }

    /// Non-.css files are silently ignored.
    #[test]
    fn scan_ignores_non_css_files() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("readme.txt"), b"hello").unwrap();
        std::fs::write(temp.path().join("config.toml"), b"[x]").unwrap();
        let themes = scan_themes_dir(temp.path());
        assert!(themes.is_empty());
    }

    /// A .css file with no manifest gets defaults.
    #[test]
    fn scan_css_no_manifest_uses_defaults() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("gruvbox-dark.css"),
            b"@define-color accent_color #d79921;",
        )
        .unwrap();

        let themes = scan_themes_dir(temp.path());
        assert_eq!(themes.len(), 1);
        let t = &themes[0];
        assert_eq!(t.id, "gruvbox-dark");
        assert_eq!(t.name, "gruvbox-dark"); // falls back to id
        assert_eq!(t.color_scheme, "auto");
        assert!(t.css.contains("d79921"));
    }

    /// A .css file with a valid manifest uses manifest values.
    #[test]
    fn scan_css_with_manifest() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("gruvbox-dark.css"),
            b"@define-color accent_color #d79921;",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("gruvbox-dark.toml"),
            b"name = \"Gruvbox Dark\"\nauthor = \"Community\"\ncolor_scheme = \"dark\"",
        )
        .unwrap();

        let themes = scan_themes_dir(temp.path());
        assert_eq!(themes.len(), 1);
        let t = &themes[0];
        assert_eq!(t.name, "Gruvbox Dark");
        assert_eq!(t.author, "Community");
        assert_eq!(t.color_scheme, "dark");
    }

    /// Malformed manifest falls back to defaults without failing the load.
    #[test]
    fn scan_malformed_manifest_falls_back() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("broken.css"), b"@define-color x #fff;").unwrap();
        std::fs::write(temp.path().join("broken.toml"), b"NOT VALID TOML !!!").unwrap();

        let themes = scan_themes_dir(temp.path());
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].name, "broken"); // fallback to id
    }

    /// Multiple themes are returned in alphabetical order.
    #[test]
    fn scan_alphabetical_order() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("zzz.css"), b"").unwrap();
        std::fs::write(temp.path().join("aaa.css"), b"").unwrap();
        std::fs::write(temp.path().join("mmm.css"), b"").unwrap();

        let themes = scan_themes_dir(temp.path());
        assert_eq!(themes.len(), 3);
        assert_eq!(themes[0].id, "aaa");
        assert_eq!(themes[1].id, "mmm");
        assert_eq!(themes[2].id, "zzz");
    }

    /// themes_data_dir appends "themes" to the data dir.
    #[test]
    fn themes_data_dir_suffix() {
        let dir = themes_data_dir(Path::new("/home/user/.local/share/mantle-manager"));
        assert_eq!(dir, PathBuf::from("/home/user/.local/share/mantle-manager/themes"));
    }
}
