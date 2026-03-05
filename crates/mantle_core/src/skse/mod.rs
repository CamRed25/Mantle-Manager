//! SKSE installer — automated script extender download and install.
//!
//! The entry point is [`install_skse`], which runs the full pipeline:
//! version check → download → archive validation → extraction → case-fold
//! normalisation → loader validation → Proton DLL override write → version
//! file write.
//!
//! # Feature gate
//!
//! This entire module requires the `net` feature.  To use it, enable the
//! feature in your crate:
//!
//! ```toml
//! mantle_core = { ..., features = ["net"] }
//! ```
//!
//! # Supported games
//!
//! See [`config::SKSE_GAME_MAP`] for the full list.  [`GameKind::Morrowind`]
//! and [`GameKind::Starfield`] return [`MantleError::Skse`] (no stable
//! download endpoint).
//!
//! # Archive structure
//!
//! SKSE releases typically contain a single versioned top-level directory
//! (e.g. `skse64_2_2_6/`).  [`install_skse`] strips that wrapper so the
//! loader executable ends up directly in `{game_dir}/`.

pub mod config;
pub mod download;
pub mod proton;
pub mod version;

pub use config::{config_for_game, SkseGameConfig, SKSE_GAME_MAP};
pub use download::DownloadConfig;
pub use proton::write_dll_overrides;
pub use version::latest_version;
pub use version::{installed_version, parse_version_str, SkseVersion};

use std::path::{Path, PathBuf};

use crate::{error::MantleError, game::GameKind};

// ── Public types ──────────────────────────────────────────────────────────────

/// Input configuration for a single [`install_skse`] run.
#[derive(Debug, Clone)]
pub struct SkseInstallConfig {
    /// Root game installation directory (e.g. `…/Skyrim Special Edition`).
    /// SKSE files are extracted directly into this directory.
    pub game_dir: PathBuf,
    /// Proton prefix `pfx/` directory.  When `Some`, DLL overrides are written
    /// to `{prefix}/user.reg`.  When `None`, the override step is skipped.
    pub proton_prefix: Option<PathBuf>,
    /// Download tuning (retries, timeout, backoff). Uses [`DownloadConfig::default`]
    /// when not specified.
    pub download: DownloadConfig,
    /// Directory for temporary download files.  Defaults to the system temp
    /// directory (`std::env::temp_dir()`) when `None`.
    pub temp_dir: Option<PathBuf>,
    /// When `true` (the default) the install is skipped if the installed
    /// version already matches the latest available version.
    pub skip_if_current: bool,
}

impl SkseInstallConfig {
    /// Creates a minimal config for `game_dir` with all other fields at their
    /// defaults.
    #[must_use]
    pub fn new(game_dir: PathBuf) -> Self {
        Self {
            game_dir,
            proton_prefix: None,
            download: DownloadConfig::default(),
            temp_dir: None,
            skip_if_current: true,
        }
    }
}

/// Outcome of a successful [`install_skse`] call.
#[derive(Debug, Clone)]
pub struct SkseInstallResult {
    /// The version that is now installed.
    pub version_installed: SkseVersion,
    /// `true` if the install was skipped because the latest version was
    /// already present (requires `skip_if_current = true`).
    pub was_up_to_date: bool,
    /// `true` if Proton DLL overrides were written during this run.
    pub dll_overrides_written: bool,
}

/// Progress events emitted by [`install_skse`] via the `progress` callback.
///
/// The callback is called from the async task — it must not block the executor.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SkseProgress {
    /// Fetching the latest version string from the upstream endpoint.
    CheckingVersion,
    /// Downloading the archive.  `total` is `None` when `Content-Length` is
    /// absent.
    Downloading {
        /// Bytes received so far.
        bytes: u64,
        /// Total expected bytes, if known.
        total: Option<u64>,
    },
    /// Extracting the downloaded archive into `game_dir`.
    Extracting,
    /// Checking that the loader executable is present after extraction.
    Validating,
    /// Writing `native,builtin` overrides to the Proton prefix `user.reg`.
    WritingDllOverrides,
    /// All steps completed successfully.
    Done,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Downloads and installs the script extender for `kind` into the directory
/// specified by `cfg.game_dir`.
///
/// Progress events are reported via `progress`.  The callback is called from
/// within the async task and must not block the executor.
///
/// # Errors
///
/// Returns [`MantleError::Skse`] if:
/// - the game has no supported script extender ([`config_for_game`] returns `None`),
/// - the version check, download, or extraction fails,
/// - post-extraction validation cannot find the loader executable.
///
/// Returns [`MantleError::Io`] on filesystem errors.
pub async fn install_skse<F>(
    kind: GameKind,
    cfg: SkseInstallConfig,
    progress: F,
) -> Result<SkseInstallResult, MantleError>
where
    F: Fn(SkseProgress),
{
    // ── Step 1: Resolve game config ───────────────────────────────────────────
    let skse_cfg = config_for_game(kind).ok_or_else(|| {
        MantleError::Skse(format!("{kind} does not have a supported script extender"))
    })?;

    // ── Step 2: Fetch latest version ──────────────────────────────────────────
    progress(SkseProgress::CheckingVersion);
    let latest = version::latest_version(skse_cfg, cfg.download.timeout_secs).await?;
    tracing::info!(game = %kind, version = %latest, "Latest {} version", skse_cfg.display_name);

    // ── Step 3: Skip if already current ───────────────────────────────────────
    if cfg.skip_if_current {
        if let Some(installed) = version::installed_version(&cfg.game_dir, skse_cfg) {
            if installed >= latest {
                tracing::info!(
                    version = %installed,
                    "Already up-to-date; skipping install"
                );
                return Ok(SkseInstallResult {
                    version_installed: installed,
                    was_up_to_date: true,
                    dll_overrides_written: false,
                });
            }
        }
    }

    // ── Step 4: Download archive ──────────────────────────────────────────────
    let temp_base = cfg.temp_dir.as_deref().map_or_else(std::env::temp_dir, PathBuf::from);
    std::fs::create_dir_all(&temp_base)?;

    let archive_name = skse_cfg.download_url.rsplit('/').next().unwrap_or("skse-latest");
    let archive_path = temp_base.join(archive_name);

    download::download_file(skse_cfg.download_url, &archive_path, &cfg.download, |bytes, total| {
        progress(SkseProgress::Downloading { bytes, total })
    })
    .await?;

    // ── Step 5: Validate archive magic bytes ──────────────────────────────────
    let fmt = crate::archive::detect_format(&archive_path);
    match fmt {
        crate::archive::ArchiveFormat::SevenZip
        | crate::archive::ArchiveFormat::Zip
        | crate::archive::ArchiveFormat::Rar => {}
        other => {
            let _ = std::fs::remove_file(&archive_path);
            return Err(MantleError::Skse(format!(
                "Downloaded archive has unexpected format ({other:?}); \
                 the download may be corrupt"
            )));
        }
    }

    // ── Step 6: Extract (with top-level directory stripping) ──────────────────
    progress(SkseProgress::Extracting);
    extract_and_flatten(&archive_path, &cfg.game_dir).await?;

    // ── Step 7: Case-fold normalise (preserves Data/SKSE/Plugins) ────────────
    let fold = crate::install::case_fold::normalize_dir(&cfg.game_dir, false, &["SKSE/Plugins"]);
    if fold.has_issues() {
        tracing::warn!(
            collisions = fold.collisions.len(),
            errors = fold.errors.len(),
            "Case-fold normalisation reported issues in game directory"
        );
    }

    // ── Step 8: Validate loader presence ─────────────────────────────────────
    progress(SkseProgress::Validating);
    let loader_found = skse_cfg.loader_names.iter().any(|name| cfg.game_dir.join(name).exists());

    if !loader_found {
        return Err(MantleError::Skse(format!(
            "Installation failed: none of {:?} found in {}",
            skse_cfg.loader_names,
            cfg.game_dir.display()
        )));
    }

    // ── Step 9: Write Proton DLL overrides ────────────────────────────────────
    let mut dll_overrides_written = false;
    if let Some(ref prefix) = cfg.proton_prefix {
        if !skse_cfg.dll_overrides.is_empty() {
            progress(SkseProgress::WritingDllOverrides);
            let user_reg = prefix.join("user.reg");
            proton::write_dll_overrides(&user_reg, skse_cfg.dll_overrides)?;
            dll_overrides_written = true;
        }
    }

    // ── Step 10: Write version marker file ───────────────────────────────────
    let version_path = cfg.game_dir.join(skse_cfg.version_file);
    std::fs::write(&version_path, format!("{} {} {}\n", latest.major, latest.minor, latest.patch))?;

    // ── Step 11: Clean up downloaded archive ──────────────────────────────────
    if let Err(e) = std::fs::remove_file(&archive_path) {
        tracing::warn!(path = %archive_path.display(), "Failed to remove temp archive: {e}");
    }

    tracing::info!(game = %kind, version = %latest, "Script extender installed");
    progress(SkseProgress::Done);

    Ok(SkseInstallResult {
        version_installed: latest,
        was_up_to_date: false,
        dll_overrides_written,
    })
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Extracts `archive` to a temp directory adjacent to `dest`, then moves the
/// contents into `dest`.
///
/// If the archive contains a single top-level directory (the common SKSE
/// packaging pattern, e.g. `skse64_2_2_6/`), that wrapper directory is
/// stripped so files land directly in `dest`.
async fn extract_and_flatten(archive: &Path, dest: &Path) -> Result<(), MantleError> {
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    let tmp = tempfile::TempDir::new_in(parent).map_err(|e| {
        MantleError::Skse(format!(
            "Failed to create extraction temp directory in {}: {e}",
            parent.display()
        ))
    })?;

    crate::archive::extract_archive(archive, tmp.path()).await?;

    // Detect single top-level directory wrapper.
    let top_entries: Vec<_> =
        std::fs::read_dir(tmp.path())?.filter_map(std::result::Result::ok).collect();

    let source = if top_entries.len() == 1
        && top_entries[0].file_type().map(|ft| ft.is_dir()).unwrap_or(false)
    {
        top_entries[0].path()
    } else {
        tmp.path().to_path_buf()
    };

    std::fs::create_dir_all(dest)?;

    for entry in std::fs::read_dir(&source)? {
        let entry = entry?;
        let target = dest.join(entry.file_name());
        // Try atomic rename first; fall back to recursive copy+remove for
        // cross-device moves (unlikely but possible with custom temp_dir).
        if std::fs::rename(entry.path(), &target).is_err() {
            copy_path_recursive(&entry.path(), &target)?;
            if entry.file_type()?.is_dir() {
                std::fs::remove_dir_all(entry.path())?;
            } else {
                std::fs::remove_file(entry.path())?;
            }
        }
    }

    Ok(())
}

/// Recursively copies a file or directory tree from `src` to `dst`.
fn copy_path_recursive(src: &Path, dst: &Path) -> Result<(), MantleError> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_path_recursive(&entry.path(), &dst.join(entry.file_name()))?;
        }
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst)?;
    }
    Ok(())
}
