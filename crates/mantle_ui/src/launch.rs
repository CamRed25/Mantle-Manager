//! VFS mount and Steam launch logic, extracted from `window.rs`.
//!
//! Provides the launch handler closure shared between the header-bar button
//! and the overview hero card button, plus the helper that builds
//! [`mantle_core::vfs::MountParams`] from the active profile's mod list.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::prelude::ButtonExt;
use gtk4::Button;
use mantle_core::vfs::MountHandle;

/// Build a launch handler closure from the three shared Rc refs.
///
/// Returns a `'static` `Fn()` that unmounts any previous VFS overlay, builds
/// fresh [`MountParams`], mounts, then opens `steam://run/<app_id>` via
/// `xdg-open`.  Shared between the header bar button ([`wire_launch_button`])
/// and the overview hero card button (passed as `on_launch` through the
/// widget hierarchy).
///
/// # Parameters
/// - `app_id`: Shared cell containing the detected Steam App ID.
/// - `game_data_path`: Shared cell containing the game's Data directory path.
/// - `mount_handle`: Shared cell persisting the live VFS mount between clicks.
pub(crate) fn make_launch_handler(
    app_id: Rc<Cell<Option<u32>>>,
    game_data_path: Rc<RefCell<Option<PathBuf>>>,
    mount_handle: Rc<RefCell<Option<MountHandle>>>,
) -> impl Fn() + 'static {
    move || {
        let Some(id) = app_id.get() else {
            tracing::warn!("launch_btn clicked but no steam_app_id is set");
            return;
        };

        // Release any previous VFS mount before creating a fresh one.
        if let Some(previous) = mount_handle.borrow_mut().take() {
            if let Err(e) = previous.unmount() {
                tracing::warn!("VFS unmount of previous handle failed: {e}");
            }
        }

        // Mount all enabled mods for the active profile over the game's Data dir.
        let data_dir = game_data_path.borrow().clone();
        if let Some(data_dir) = data_dir {
            match build_mount_params(&data_dir) {
                Ok(params) if params.lower_dirs.is_empty() => {
                    tracing::info!("active profile has no enabled mods — skipping VFS mount");
                }
                Ok(params) => {
                    // Tear down any orphaned overlay left by a previous crash before
                    // mounting so stale mounts don't stack up.
                    if let Err(e) = mantle_core::vfs::teardown_stale(&data_dir) {
                        tracing::warn!("teardown_stale failed (continuing anyway): {e}");
                    }
                    let backend = mantle_core::vfs::select_backend();
                    match mantle_core::vfs::mount_with(backend, params) {
                        Ok(handle) => {
                            tracing::info!("VFS mounted successfully for launch");
                            *mount_handle.borrow_mut() = Some(handle);
                        }
                        Err(e) => {
                            tracing::warn!("VFS mount failed — launching without overlay: {e}");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "failed to build VFS MountParams — launching without overlay: {e}"
                    );
                }
            }
        } else {
            tracing::info!("no game_data_path available — skipping VFS mount");
        }

        let url = format!("steam://run/{id}");
        tracing::info!("launching game: xdg-open {url}");
        if let Err(e) = std::process::Command::new("xdg-open").arg(&url).spawn() {
            tracing::error!("failed to open Steam launch URL '{url}': {e}");
        }
    }
}

/// Wire the header-bar launch button.
///
/// Delegates to [`make_launch_handler`] so the overview hero card and the
/// header bar button share identical VFS + launch logic.
///
/// # Parameters
/// - `btn`: The launch [`gtk4::Button`] to wire.
/// - `app_id`: Shared cell containing the detected Steam App ID.
/// - `game_data_path`: Shared cell containing the game's Data directory path.
/// - `mount_handle`: Shared cell that persists the live VFS mount handle.
pub(crate) fn wire_launch_button(
    btn: &Button,
    app_id: &Rc<Cell<Option<u32>>>,
    game_data_path: &Rc<RefCell<Option<PathBuf>>>,
    mount_handle: &Rc<RefCell<Option<MountHandle>>>,
) {
    let handler =
        make_launch_handler(Rc::clone(app_id), Rc::clone(game_data_path), Rc::clone(mount_handle));
    btn.connect_clicked(move |_| handler());
}

/// Build a [`mantle_core::vfs::MountParams`] from the active profile's
/// enabled mods stored in the database.
///
/// Opens a fresh DB connection each call so it always sees the latest
/// `profile_mods` state (row order from the query provides priority order).
///
/// # Parameters
/// - `data_dir`: The game's Data directory; written as the VFS `merge_dir`.
///
/// # Returns
/// `MountParams` with `lower_dirs` set to each enabled mod's `install_dir`
/// in priority order.  Returns empty `lower_dirs` if no active profile
/// exists or no mods are enabled.
///
/// # Errors
/// Returns a display string if the database cannot be opened or a query fails.
pub(crate) fn build_mount_params(
    data_dir: &std::path::Path,
) -> Result<mantle_core::vfs::MountParams, String> {
    use mantle_core::{
        config::default_db_path,
        data::{profiles::get_active_profile, Database},
        mod_list,
    };

    let db = Database::open(&default_db_path()).map_err(|e| e.to_string())?;
    let profile = db.with_conn(get_active_profile).map_err(|e| e.to_string())?;

    let Some(profile) = profile else {
        return Ok(mantle_core::vfs::MountParams {
            lower_dirs: vec![],
            merge_dir: data_dir.to_path_buf(),
        });
    };

    let mods = db
        .with_conn(|conn| mod_list::list_profile_mods(conn, profile.id))
        .map_err(|e| e.to_string())?;

    let lower_dirs = mods
        .into_iter()
        .filter(|m| m.is_enabled)
        .map(|m| PathBuf::from(m.install_dir))
        .collect();

    Ok(mantle_core::vfs::MountParams {
        lower_dirs,
        merge_dir: data_dir.to_path_buf(),
    })
}
