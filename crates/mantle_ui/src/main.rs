//! `mantle_ui` — GTK4 / libadwaita application entry point.
//!
//! Initialises tracing, builds the `adw::Application`, and hands control
//! to the GTK main loop. All logic delegates to `mantle_core`; no business
//! logic lives in this crate per `UI_GUIDE.md` §1.4.

use adw::prelude::*;
use libadwaita as adw;

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

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(window::build_ui);

    let exit_code = app.run();
    std::process::exit(exit_code.into());
}
