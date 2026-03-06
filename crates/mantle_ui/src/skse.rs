//! SKSE installer (and equivalent script extenders), extracted from `window.rs`.
//!
//! Provides the typed channel message enum and the button/install logic for
//! downloading and installing the script extender for a detected game.
//!
//! Requires the `net` feature.

#![cfg(feature = "net")]

use adw::prelude::*;
use gtk4::glib;
use libadwaita as adw;

/// Typed messages sent from the SKSE installer background thread to the idle
/// poll loop.
///
/// Using an enum instead of `Result<String, String>` eliminates the fragile
/// `msg.starts_with("Script extender")` sentinel check.
pub(crate) enum SkseMsg {
    /// Intermediate progress update — update the pending toast label.
    Progress(String),
    /// Installation completed successfully — show final toast and re-enable button.
    Done(String),
    /// Installation failed — show error toast and re-enable button.
    Err(String),
}

pub(crate) fn wire_skse_button(
    btn: &gtk4::Button,
    game_kind: &std::rc::Rc<std::cell::Cell<Option<mantle_core::game::GameKind>>>,
    game_data_path: &std::rc::Rc<std::cell::RefCell<Option<std::path::PathBuf>>>,
    toast_overlay: &adw::ToastOverlay,
) {
    use mantle_core::skse::config_for_game;

    let btn_c = btn.clone();
    let game_kind_c = game_kind.clone();
    let game_data_c = game_data_path.clone();
    let overlay_c = toast_overlay.clone();

    btn.connect_clicked(move |_| {
        let Some(kind) = game_kind_c.get() else {
            return;
        };
        if config_for_game(kind).is_none() {
            return;
        }
        let Some(game_dir) = game_data_c.borrow().as_ref().cloned() else {
            return;
        };
        btn_c.set_sensitive(false);
        let overlay = overlay_c.clone();
        let btn_restore = btn_c.clone();
        run_skse_install(kind, game_dir, overlay, move || {
            btn_restore.set_sensitive(true);
        });
    });
}

pub(crate) fn run_skse_install(
    kind: mantle_core::game::GameKind,
    game_dir: std::path::PathBuf,
    toast_overlay: adw::ToastOverlay,
    on_done: impl Fn() + 'static,
) {
    use mantle_core::skse::{install_skse, SkseInstallConfig, SkseProgress};
    use std::sync::mpsc;

    let (tx, rx) = mpsc::channel::<SkseMsg>();

    // Progress toast that stays until we dismiss it.
    let pending_toast =
        adw::Toast::builder().title("Installing script extender…").timeout(0).build();
    toast_overlay.add_toast(pending_toast.clone());

    let tx_progress = tx.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio rt");

        let proton_prefix = mantle_core::game::games::KNOWN_GAMES
            .iter()
            .find(|d| d.kind == kind)
            .and_then(|d| mantle_core::game::proton::proton_prefix(d.steam_app_id));

        let cfg = SkseInstallConfig {
            game_dir,
            proton_prefix,
            download: mantle_core::skse::DownloadConfig::default(),
            temp_dir: None,
            skip_if_current: true,
        };

        let result = rt.block_on(install_skse(kind, cfg, {
            let tx = tx_progress;
            move |ev| {
                let msg = match ev {
                    SkseProgress::CheckingVersion => "Checking version…".to_string(),
                    SkseProgress::Downloading { bytes, total } => match total {
                        Some(t) => format!("Downloading… {bytes}/{t} bytes"),
                        None => format!("Downloading… {bytes} bytes"),
                    },
                    SkseProgress::Extracting => "Extracting…".to_string(),
                    SkseProgress::Validating => "Validating…".to_string(),
                    SkseProgress::WritingDllOverrides => "Writing DLL overrides…".to_string(),
                    SkseProgress::Done => "Done".to_string(),
                    _ => return,
                };
                let _ = tx.send(SkseMsg::Progress(msg));
            }
        }));

        match result {
            Ok(res) if res.was_up_to_date => {
                let _ = tx.send(SkseMsg::Done(format!(
                    "Script extender {} is already up to date",
                    res.version_installed
                )));
            }
            Ok(res) => {
                let _ = tx.send(SkseMsg::Done(format!(
                    "Script extender {} installed successfully",
                    res.version_installed
                )));
            }
            Err(e) => {
                let _ = tx.send(SkseMsg::Err(format!("{e}")));
            }
        }
    });

    glib::idle_add_local(move || {
        use std::sync::mpsc::TryRecvError;
        match rx.try_recv() {
            Ok(SkseMsg::Done(msg)) => {
                pending_toast.dismiss();
                toast_overlay.add_toast(adw::Toast::builder().title(msg).timeout(4).build());
                on_done();
                glib::ControlFlow::Break
            }
            Ok(SkseMsg::Err(msg)) => {
                pending_toast.dismiss();
                toast_overlay.add_toast(
                    adw::Toast::builder()
                        .title(format!("Script extender install failed: {msg}"))
                        .timeout(6)
                        .build(),
                );
                on_done();
                glib::ControlFlow::Break
            }
            Ok(SkseMsg::Progress(msg)) => {
                pending_toast.set_title(&msg);
                glib::ControlFlow::Continue
            }
            Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                on_done();
                glib::ControlFlow::Break
            }
        }
    });
}
