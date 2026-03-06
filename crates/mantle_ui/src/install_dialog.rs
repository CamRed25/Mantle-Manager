//! Mod archive install dialog and post-extraction pipeline, extracted from `window.rs`.
//!
//! Provides the file-chooser dialog for picking a mod archive, the extraction
//! logic, and the database registration pipeline that runs after a successful
//! extract.

use adw::prelude::*;
use gtk4::glib;
use libadwaita as adw;

/// Open a [`gtk4::FileChooserNative`] to install a mod archive.
///
/// Shared by the header install button and the mods-page "Add Mod" button.
/// Archives are filtered to `.zip`, `.7z`, and `.rar`.
///
/// # Parameters
/// - `window`: Transient parent for the file chooser dialog.
/// - `toast_overlay`: Toast target for installation feedback.
pub fn open_mod_install_dialog(window: &adw::ApplicationWindow, toast_overlay: &adw::ToastOverlay) {
    let chooser = gtk4::FileChooserNative::builder()
        .title("Install Mod Archive")
        .transient_for(window)
        .action(gtk4::FileChooserAction::Open)
        .accept_label("Install")
        .cancel_label("Cancel")
        .build();

    let filter = gtk4::FileFilter::new();
    filter.set_name(Some("Mod Archives (zip, 7z, rar)"));
    filter.add_pattern("*.zip");
    filter.add_pattern("*.7z");
    filter.add_pattern("*.rar");
    chooser.add_filter(&filter);

    chooser.connect_response(glib::clone!(
        @weak toast_overlay =>
        move |dialog, response| {
            if response == gtk4::ResponseType::Accept {
                if let Some(path) = dialog.file().and_then(|f| f.path()) {
                    install_mod_archive(path, toast_overlay.clone());
                }
            }
        }
    ));

    chooser.show();
}

pub(crate) fn wire_install_button(
    btn: &gtk4::Button,
    window: &adw::ApplicationWindow,
    toast_overlay: &adw::ToastOverlay,
) {
    btn.connect_clicked(glib::clone!(
        @weak window,
        @weak toast_overlay =>
        move |_| {
            open_mod_install_dialog(&window, &toast_overlay);
        }
    ));
}

/// Attempt to register a successfully extracted mod in the database, add it
/// to the active profile, and return its primary key for use by the
/// post-extraction pipeline.
///
/// All errors are silently swallowed so that extraction success is never
/// retroactively marked as failure due to a DB issue.  Failures here will
/// surface as missing mods in the list on next state reload.
///
/// # Parameters
/// - `name`: Human-readable mod name (also used to derive the slug).
/// - `install_dir`: Absolute path to the extracted mod directory.
/// - `archive_path`: Path to the source archive (stored for reference).
///
/// # Returns
/// `Some(mod_id)` on success; `None` if the DB operation failed.
fn register_installed_mod(name: &str, install_dir: &str, archive_path: &str) -> Option<i64> {
    use mantle_core::{
        config::default_db_path,
        data::{
            mods::{get_mod_by_slug, insert_mod, InsertMod},
            profiles::get_active_profile,
            Database,
        },
        mod_list::add_mod_to_profile,
    };

    // Stable slug: lowercase, non-alphanumeric chars replaced with underscores.
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();

    let Ok(db) = Database::open(&default_db_path()) else {
        return None;
    };

    // Resolve or create the mod record.
    let mod_id = db.with_conn(|conn| {
        let existing = get_mod_by_slug(conn, &slug).ok().flatten();
        if let Some(rec) = existing {
            Ok::<i64, mantle_core::Error>(rec.id)
        } else {
            insert_mod(
                conn,
                &InsertMod {
                    slug: &slug,
                    name,
                    version: None,
                    author: None,
                    description: None,
                    nexus_mod_id: None,
                    nexus_file_id: None,
                    source_url: None,
                    archive_path: Some(archive_path),
                    install_dir,
                    archive_hash: None,
                    installed_at: None,
                },
            )
        }
    });

    // Add to the active profile (no-op if already present).
    if let Ok(mid) = mod_id {
        db.with_conn(|conn| {
            if let Ok(Some(profile)) = get_active_profile(conn) {
                if let Err(e) = add_mod_to_profile(conn, profile.id, mid) {
                    tracing::warn!(
                        mod_id = mid,
                        profile_id = profile.id,
                        error = %e,
                        "register_installed_mod: failed to add mod to active profile"
                    );
                }
            }
        });
        Some(mid)
    } else {
        None
    }
}

/// Run the post-extraction pipeline for a newly installed mod:
/// extract embedded BSA/BA2 archives, normalize path case, then scan
/// all loose files and register them in the `mod_files` table.
///
/// All steps are best-effort — failures are logged but do not abort the
/// install.  A missing `mod_files` row simply means that mod is invisible
/// to the conflict scanner until the next full rescan.
///
/// # Parameters
/// - `mod_id`: The mod's primary key in `mods`.
/// - `dest`: Root directory of the extracted mod.
fn register_mod_files(mod_id: i64, dest: &std::path::Path) {
    use mantle_core::{
        config::default_db_path,
        data::{
            mod_files::{hash_file_xxh3, insert_mod_files, InsertModFile},
            Database,
        },
        install::{extract_mod_archives, normalize_dir},
    };

    // 1. Extract embedded BSA/BA2 archives and delete the originals.
    let bsa_result = extract_mod_archives(dest, true);
    if !bsa_result.is_ok() {
        tracing::warn!("register_mod_files: {} BSA(s) failed to extract", bsa_result.failed.len());
    }

    // 2. Normalize all filesystem paths to lowercase.
    let fold_result = normalize_dir(dest, false, &[]);
    if fold_result.has_issues() {
        tracing::warn!(
            "register_mod_files: {} collision(s), {} error(s) during case-fold",
            fold_result.collisions.len(),
            fold_result.errors.len()
        );
    }

    // 3. Walk the mod directory and collect all loose files.
    let paths = collect_mod_files(dest);

    // 4. Compute XXH3 hashes and build insert records.
    //    Owned (path, hash, size) tuples are required so InsertModFile can
    //    borrow them with the same lifetime.
    let mut file_data: Vec<(String, String, i64)> = Vec::with_capacity(paths.len());
    for path in &paths {
        let Ok(rel_path) = path.strip_prefix(dest) else {
            continue;
        };
        let rel = rel_path.to_string_lossy().into_owned();
        let Some(hash) = hash_file_xxh3(path) else {
            tracing::warn!("register_mod_files: could not hash {}", path.display());
            continue;
        };
        let size = i64::try_from(path.metadata().map_or(0, |m| m.len())).unwrap_or(i64::MAX);
        file_data.push((rel, hash, size));
    }

    let records: Vec<InsertModFile<'_>> = file_data
        .iter()
        .map(|(p, h, s)| InsertModFile {
            mod_id,
            rel_path: p,
            file_hash: h,
            file_size: *s,
            archive_name: None,
        })
        .collect();

    let Ok(db) = Database::open(&default_db_path()) else {
        return;
    };
    if let Err(e) = db.with_conn(|conn| insert_mod_files(conn, &records)) {
        tracing::warn!("register_mod_files: DB insert failed: {e}");
    }
}

/// Recursively collect all regular files under `dir`.
fn collect_mod_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(collect_mod_files(&path));
            } else if path.is_file() {
                files.push(path);
            }
        }
    }
    files
}

/// Extract `archive_path` into the configured mods directory and surface the
/// result as [`adw::Toast`] notifications on `toast_overlay`.
///
/// # Flow
/// 1. Derives the mod name from the archive file stem.
/// 2. Shows a persistent "Installing…" toast immediately.
/// 3. Spawns an OS thread that creates a single-use
///    [`tokio::runtime::Runtime`] and calls
///    [`mantle_core::archive::extract_archive`].
/// 4. The result is returned via an `mpsc` channel.
/// 5. A [`glib::idle_add_local`] callback dismisses the pending toast and
///    shows a success or error toast.
///
/// # Parameters
/// - `archive_path`: Absolute path to the archive file to extract.
/// - `toast_overlay`: The [`adw::ToastOverlay`] that will show status toasts.
///
/// # Side Effects
/// - Creates the destination mod directory beneath the configured `mods_dir`.
/// - Spawns one OS thread per invocation.
/// - Registers one `glib::idle_add_local` callback per invocation.
fn install_mod_archive(archive_path: std::path::PathBuf, toast_overlay: adw::ToastOverlay) {
    use mantle_core::config::{default_db_path, default_settings_path, AppSettings};

    // Derive mod folder name from the archive stem (e.g. "SkyUI_5.2SE").
    let mod_name = archive_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown_mod")
        .to_string();

    // Persistent "Installing…" toast shown while extraction runs.
    // timeout = 0 means the toast never auto-dismisses.
    let pending_toast = adw::Toast::builder()
        .title(format!("Installing '{mod_name}'\u{2026}"))
        .timeout(0)
        .build();
    toast_overlay.add_toast(pending_toast.clone());

    // Channel: background thread → GTK idle callback.
    let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();

    std::thread::spawn(move || {
        // Each install creates its own single-use tokio runtime.
        let rt = match tokio::runtime::Runtime::new() {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(Err(format!("tokio runtime error: {e}")));
                return;
            }
        };

        // Resolve mods directory from settings, falling back to <data>/mods/.
        let settings = AppSettings::load_or_default(&default_settings_path()).unwrap_or_default();
        let mods_dir = settings.paths.mods_dir.unwrap_or_else(|| {
            default_db_path()
                .parent()
                .map_or_else(|| std::path::PathBuf::from("mods"), |p| p.join("mods"))
        });

        let dest = mods_dir.join(&mod_name);

        if let Err(e) = std::fs::create_dir_all(&dest) {
            let _ = tx.send(Err(format!("could not create mod directory: {e}")));
            return;
        }

        match rt.block_on(mantle_core::archive::extract_archive(&archive_path, &dest)) {
            Ok(()) => {
                // Register the mod in the database, add to active profile,
                // run post-extraction pipeline, and register file paths for
                // conflict detection.
                let install_dir = dest.to_string_lossy().into_owned();
                let archive_str = archive_path.to_string_lossy().into_owned();
                if let Some(mid) = register_installed_mod(&mod_name, &install_dir, &archive_str) {
                    register_mod_files(mid, &dest);
                }

                let _ = tx.send(Ok(mod_name));
            }
            Err(e) => {
                // Clean up the empty destination directory on extraction failure.
                let _ = std::fs::remove_dir(&dest);
                let _ = tx.send(Err(format!("{e}")));
            }
        }
    });

    // Deliver the result to the GTK main thread via idle polling.
    glib::idle_add_local(move || {
        use std::sync::mpsc::TryRecvError;
        match rx.try_recv() {
            Ok(Ok(name)) => {
                pending_toast.dismiss();
                toast_overlay.add_toast(
                    adw::Toast::builder()
                        .title(format!("'{name}' installed successfully"))
                        .timeout(4)
                        .build(),
                );
                glib::ControlFlow::Break
            }
            Ok(Err(msg)) => {
                pending_toast.dismiss();
                toast_overlay.add_toast(
                    adw::Toast::builder()
                        .title(format!("Install failed: {msg}"))
                        .timeout(6)
                        .build(),
                );
                glib::ControlFlow::Break
            }
            Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}
