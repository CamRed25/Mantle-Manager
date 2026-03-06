//! `mantle_ui` — GTK4 / libadwaita application entry point.
//!
//! Initialises tracing, builds the `adw::Application`, and hands control
//! to the GTK main loop. All logic delegates to `mantle_core`; no business
//! logic lives in this crate per `UI_GUIDE.md` §1.4.

use std::sync::{Arc, Mutex};

use adw::prelude::*;
use libadwaita as adw;

mod downloads;
mod pages;
mod settings;
mod sidebar;
mod state;
mod state_worker;
mod window;

/// Application ID registered with the D-Bus session bus.
/// Must match the Flatpak bundle ID in the manifest.
const APP_ID: &str = "io.github.mantle_manager.MantleManager";

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // ── NXM URI queue ─────────────────────────────────────────────────────
    // `connect_open` fills this with incoming `nxm://` URIs; the window's
    // idle loop drains it, resolves each URL to a CDN link, and enqueues the
    // download.  Arc<Mutex<…>> allows `connect_open` (which must be Send) to
    // push safely while the GTK main thread pops.
    let nxm_queue: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let app = adw::Application::builder()
        .application_id(APP_ID)
        // HANDLES_OPEN: enables connect_open to receive nxm:// URIs forwarded
        // by the desktop (xdg-open / D-Bus activation).
        .flags(adw::gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    // ── NXM deep-link handler ─────────────────────────────────────────────
    // Called by GTK when the OS asks us to "open" a URI (e.g. the browser
    // clicked "Download with Mod Manager" on Nexus).  We push any nxm://
    // URIs into `nxm_queue`; the window idle loop resolves and downloads them.
    {
        let nxm_queue_open = Arc::clone(&nxm_queue);
        app.connect_open(move |app, files, _hint| {
            for file in files {
                let uri = file.uri();
                if uri.starts_with("nxm://") {
                    if let Ok(mut q) = nxm_queue_open.lock() {
                        q.push(uri.to_string());
                    }
                }
            }
            // Bring the window to the foreground (activates if not yet shown).
            app.activate();
        });
    }

    // ── Build the main window on activate ────────────────────────────────
    {
        let nxm_queue_act = Arc::clone(&nxm_queue);
        app.connect_activate(move |app| {
            window::build_ui(app, Arc::clone(&nxm_queue_act));
        });
    }

    let exit_code = app.run();
    std::process::exit(exit_code.into());
}
