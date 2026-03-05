//! Main application window — assembles the header bar, `adw::ViewStack`,
//! sidebar, and wires up the settings dialog, live state channel, launch
//! button, and archive install logic.
//!
//! # Architecture
//! The window header bar (including the `adw::ViewSwitcher`, launch button,
//! install button, and settings button) is built **once** and never rebuilt.
//! Page content uses [`adw::NavigationSplitView`] — sidebar (summary) on the
//! left, five-tab [`adw::ViewStack`] on the right — wrapped in an
//! [`adw::ToastOverlay`] for non-blocking notifications (`UI_GUIDE.md` §5.2,
//! §4.2).
//!
//! # State lifecycle
//! 1. `build_ui` is called by GTK on the `activate` signal.
//! 2. A [`std::sync::mpsc`] channel is created.
//! 3. The window is shown immediately with [`AppState::placeholder`] data.
//! 4. [`state_worker::spawn`] reads the database and detects games in a
//!    background thread, sending the live [`AppState`] over the channel.
//! 5. The [`glib::idle_add_local`] callback replaces content on delivery.
//!
//! # Launch button
//! Opens `steam://run/<app_id>` via `xdg-open` once a game is detected.
//! Disabled with label "No Game Detected" until live state confirms a game.
//!
//! # Archive install
//! The "Install Mod" button opens a [`gtk4::FileChooserNative`] for zip,
//! 7z, and rar archives.  Extraction runs on a background OS thread using a
//! per-call [`tokio::runtime::Runtime`].  Results surface as [`adw::Toast`]
//! notifications via the [`adw::ToastOverlay`].

use std::cell::Cell;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk4::glib;
use libadwaita as adw;

use mantle_core::config::{default_settings_path, AppSettings};

use crate::{
    downloads::{DownloadProgress, DownloadQueue},
    pages::{downloads, mods, overview, plugins, profiles},
    settings, sidebar,
    state::AppState,
    state_worker,
};

// ─── Public entry point ───────────────────────────────────────────────────────

/// Construct and present the main application window.
///
/// Called by GTK on the first `activate` signal.  Must be called on the GTK
/// main thread only.
///
/// # Parameters
/// - `app`: The running [`adw::Application`]; the window registers itself as
///   a child of this application.
///
/// # Side Effects
/// - Applies the saved color-scheme preference via [`settings::apply_theme`].
/// - Launches background game detection + DB load (see [`state_worker::spawn`]).
/// - Presents the main window.
#[allow(clippy::too_many_lines)]
pub fn build_ui(app: &adw::Application) {
    // Apply saved color scheme before any widgets render so the first frame
    // uses the correct theme.
    let settings_path = default_settings_path();
    let initial_settings = AppSettings::load_or_default(&settings_path).unwrap_or_default();

    // Scan user themes so we can resolve CSS for a Custom theme at startup.
    let themes_dir = mantle_core::theme::themes_data_dir(&mantle_core::config::data_dir());
    let user_themes = mantle_core::theme::scan_themes_dir(&themes_dir);
    let custom_css: Option<(String, bool)> =
        if let mantle_core::config::Theme::Custom(ref id) = initial_settings.ui.theme {
            user_themes
                .iter()
                .find(|t| &t.id == id)
                .map(|t| (t.css.clone(), t.color_scheme != "light"))
        } else {
            None
        };
    settings::apply_theme(
        &initial_settings.ui.theme,
        custom_css.as_ref().map(|(css, dark)| (css.as_str(), *dark)),
    );

    // ── State channel (background loader → GTK thread) ────────────────────
    // std::sync::mpsc is used because glib::MainContext::channel was removed
    // in glib 0.19.  The idle_add_local callback polls try_recv() each cycle.
    let (sender, receiver) = std::sync::mpsc::channel::<AppState>();

    // ── Download progress channel (future background tasks → GTK thread) ──
    // Background download workers will clone `progress_tx` and push
    // DownloadProgress messages; the second idle loop drains `progress_rx`
    // and calls apply_progress() so status is reflected without a full reload.
    let (progress_tx, progress_rx) = std::sync::mpsc::channel::<DownloadProgress>();
    let queue_rc: Rc<RefCell<DownloadQueue>> =
        Rc::new(RefCell::new(DownloadQueue::new(progress_tx)));

    // Shared steam_app_id: set by the idle callback when live state arrives;
    // read by the launch button callback.  Rc<Cell<…>> is safe for the GTK
    // main thread (single-threaded access only).
    let app_id_shared: Rc<Cell<Option<u32>>> = Rc::new(Cell::new(None));

    // Shared game data directory: updated from live state so wire_launch_button
    // can access it at click time without re-reading the DB.
    let game_data_path_shared: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));

    // Shared game kind — updated alongside app_id; drives SKSE button sensitivity.
    // Only needed when the `net` feature is enabled.
    #[cfg(feature = "net")]
    let game_kind_shared: Rc<Cell<Option<mantle_core::game::GameKind>>> =
        Rc::new(Cell::new(None));

    // Persistent VFS mount handle: stored after each successful mount and
    // released (unmounted) on the next launch click.  `None` before the first
    // click or when no mods are active.
    let mount_handle_shared: Rc<RefCell<Option<mantle_core::vfs::MountHandle>>> =
        Rc::new(RefCell::new(None));

    let placeholder = AppState::placeholder();

    // ── Header bar (built once, never rebuilt) ────────────────────────────
    let header = adw::HeaderBar::new();

    // Launch button — disabled until live state confirms a detected game.
    let launch_label = launch_button_label(&placeholder);
    let launch_btn = gtk4::Button::with_label(&launch_label);
    launch_btn.add_css_class("suggested-action");
    launch_btn.set_tooltip_text(Some(&format!("Launch {}", placeholder.launch_target)));
    if placeholder.steam_app_id.is_none() {
        launch_btn.set_sensitive(false);
    }
    header.pack_end(&launch_btn);

    // "Install Mod" button — opens FileChooserNative for archive selection.
    let install_btn = gtk4::Button::from_icon_name("document-save-symbolic");
    install_btn.set_tooltip_text(Some("Install Mod"));
    header.pack_end(&install_btn);

    // "Script Extender" button — downloads and installs SKSE/F4SE/etc.
    // Only compiled when the `net` feature is enabled.
    #[cfg(feature = "net")]
    let skse_btn = {
        let btn = gtk4::Button::from_icon_name("system-software-update-symbolic");
        btn.set_tooltip_text(Some("Install Script Extender (SKSE, F4SE, …)"));
        btn.set_sensitive(false); // enabled once a supported game is detected
        header.pack_end(&btn);
        btn
    };

    // Settings gear — opens the preferences dialog.
    let settings_btn = gtk4::Button::from_icon_name("preferences-system-symbolic");
    settings_btn.set_tooltip_text(Some("Settings"));
    header.pack_end(&settings_btn);

    // ViewSwitcher is permanent; its stack is re-targeted when state changes.
    let switcher = adw::ViewSwitcher::new();
    switcher.set_policy(adw::ViewSwitcherPolicy::Wide);
    header.set_title_widget(Some(&switcher));

    // ── Window (created early for dialog placement) ───────────────────────
    // Content is set after the toolbar_view is assembled below.
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Mantle Manager")
        .default_width(1280)
        .default_height(800)
        .build();

    // ── Live-refresh closure ──────────────────────────────────────────────
    // Cloning `sender` before spawning allows user-action callbacks to queue
    // additional state reloads at any time by calling `refresh_fn()`.
    // The idle callback stays registered as `Continue` so it processes every
    // delivery — both the initial startup load and user-triggered reloads.
    let refresh_sender = sender.clone();
    let refresh_fn: Rc<dyn Fn()> = Rc::new(move || state_worker::spawn(refresh_sender.clone()));

    // ── Initial page content ──────────────────────────────────────────────
    // ToastOverlay created here so it can be passed to page builders that
    // need to show feedback (e.g. the mods page "Add Mod" button).
    let toast_overlay = adw::ToastOverlay::new();
    let (stack, aside) =
        build_main_content(&placeholder, &window, &refresh_fn, &toast_overlay, &queue_rc);
    switcher.set_stack(Some(&stack));

    // NavigationSplitView: sidebar = summary panel (left), content = ViewStack (right).
    // Auto-collapses to single-panel navigation on narrow displays,
    // meeting the Steam Deck 1280×800 requirement (UI_GUIDE.md §4.2).
    let split_view = adw::NavigationSplitView::new();
    split_view.set_min_sidebar_width(220.0);
    split_view.set_max_sidebar_width(360.0);
    split_view.set_sidebar_width_fraction(0.25);

    let sidebar_nav = adw::NavigationPage::builder().title("Summary").child(&aside).build();
    let content_nav = adw::NavigationPage::builder().title("Mantle Manager").child(&stack).build();
    split_view.set_sidebar(Some(&sidebar_nav));
    split_view.set_content(Some(&content_nav));

    // ToastOverlay wraps the split view so toasts render above all content.
    toast_overlay.set_child(Some(&split_view));

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&toast_overlay));

    // Attach the assembled content tree to the window.
    window.set_content(Some(&toolbar_view));

    // ── Wire settings button ──────────────────────────────────────────────
    settings_btn.connect_clicked(glib::clone!(
        @weak window =>
        move |_| {
            let path = default_settings_path();
            let current = AppSettings::load_or_default(&path).unwrap_or_default();
            let dialog = settings::build_dialog(current, path);
            dialog.set_transient_for(Some(&window));
            dialog.present();
        }
    ));

    // ── Wire launch button ────────────────────────────────────────────────
    wire_launch_button(&launch_btn, &app_id_shared, &game_data_path_shared, &mount_handle_shared);

    // ── Wire install button ───────────────────────────────────────────────
    wire_install_button(&install_btn, &window, &toast_overlay);

    // ── Wire SKSE button (net feature only) ───────────────────────────────
    #[cfg(feature = "net")]
    wire_skse_button(
        &skse_btn,
        &game_kind_shared,
        &game_data_path_shared,
        &toast_overlay,
    );

    window.present();

    // ── Background state loader ───────────────────────────────────────────
    state_worker::spawn(sender);

    // ── Idle poll: replace content whenever a new AppState is delivered ───
    //
    // Remains registered (`ControlFlow::Continue`) for the lifetime of the
    // window so it picks up both the initial startup load and any subsequent
    // user-triggered reloads queued via `refresh_fn()`.
    let split_view_c = split_view.clone();
    let switcher_c = switcher.clone();
    let launch_btn_c = launch_btn.clone();
    let app_id_c = Rc::clone(&app_id_shared);
    let game_data_path_c = Rc::clone(&game_data_path_shared);
    let window_c = window.clone();
    let refresh_fn_c = Rc::clone(&refresh_fn);
    let toast_overlay_c = toast_overlay.clone();
    let queue_idle = Rc::clone(&queue_rc);
    #[cfg(feature = "net")]
    let skse_btn_c = skse_btn.clone();
    #[cfg(feature = "net")]
    let game_kind_c = Rc::clone(&game_kind_shared);
    glib::idle_add_local(move || {
        use std::sync::mpsc::TryRecvError;
        match receiver.try_recv() {
            Ok(state) => {
                // Propagate game data path to the launch button’s shared cell.
                (*game_data_path_c.borrow_mut()).clone_from(&state.game_data_path);
                apply_state_update(
                    &state,
                    (&split_view_c, &switcher_c),
                    &launch_btn_c,
                    &app_id_c,
                    &window_c,
                    &refresh_fn_c,
                    &toast_overlay_c,
                    &queue_idle,
                );
                // Update SKSE button sensitivity whenever live state arrives.
                #[cfg(feature = "net")]
                {
                    use mantle_core::game::games::KNOWN_GAMES;
                    let kind = state
                        .steam_app_id
                        .and_then(|id| KNOWN_GAMES.iter().find(|g| g.steam_app_id == id))
                        .map(|g| g.kind);
                    game_kind_c.set(kind);
                    let supported =
                        kind.is_some_and(|k| mantle_core::skse::config_for_game(k).is_some());
                    skse_btn_c.set_sensitive(supported);
                }
                glib::ControlFlow::Continue
            }
            Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });

    // ── Idle poll: drain download progress channel ────────────────────────
    //
    // Each message updates one job's status in the queue.  No full page
    // rebuild is triggered here (scaffolding); the next user interaction or
    // state refresh will naturally repaint the downloads page.
    let queue_prog = Rc::clone(&queue_rc);
    glib::idle_add_local(move || {
        use std::sync::mpsc::TryRecvError;
        loop {
            match progress_rx.try_recv() {
                Ok(prog) => {
                    queue_prog.borrow_mut().apply_progress(prog);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return glib::ControlFlow::Break,
            }
        }
        glib::ControlFlow::Continue
    });
}

// ─── Private helpers ──────────────────────────────────────────────────────────

/// Apply a freshly loaded [`AppState`] to the live UI.
///
/// Rebuilds the page stack and sidebar, then updates the launch button state.
/// Called from the idle loop whenever a new state snapshot is delivered.
///
/// # Parameters
/// - `state`: New application state snapshot.
/// - `nav`: `(split_view, switcher)` — navigation split view and view switcher to update.
/// - `launch_btn`: Launch button to update label/sensitivity on.
/// - `app_id`: Shared cell updated with the new `steam_app_id`.
/// - `window`: Application window passed through to page builders.
/// - `refresh`: Refresh callback passed through to page builders.
/// - `toast_overlay`: Toast overlay passed through to page builders.
/// - `queue`: Shared download queue forwarded to the downloads page.
#[allow(clippy::too_many_arguments)]
fn apply_state_update(
    state: &AppState,
    nav: (&adw::NavigationSplitView, &adw::ViewSwitcher),
    launch_btn: &gtk4::Button,
    app_id: &Rc<Cell<Option<u32>>>,
    window: &adw::ApplicationWindow,
    refresh: &Rc<dyn Fn()>,
    toast_overlay: &adw::ToastOverlay,
    queue: &Rc<RefCell<DownloadQueue>>,
) {
    let (split_view, switcher) = nav;
    let (new_stack, new_aside) =
        build_main_content(state, window, refresh, toast_overlay, queue);
    switcher.set_stack(Some(&new_stack));

    let new_sidebar = adw::NavigationPage::builder().title("Summary").child(&new_aside).build();
    let new_content =
        adw::NavigationPage::builder().title("Mantle Manager").child(&new_stack).build();
    split_view.set_sidebar(Some(&new_sidebar));
    split_view.set_content(Some(&new_content));

    app_id.set(state.steam_app_id);
    launch_btn.set_label(&launch_button_label(state));
    launch_btn.set_tooltip_text(Some(&format!("Launch {}", state.launch_target)));
    launch_btn.set_sensitive(state.steam_app_id.is_some());
}

/// Build the `adw::ViewStack` (five tabs) and the sidebar for `state`.
///
/// Called at startup with the placeholder and again when the live
/// [`AppState`] arrives from the background loader, and after every
/// user-triggered state refresh.
///
/// # Parameters
/// - `state`: Snapshot used to populate every page.
/// - `window`: Main application window, passed to page builders that
///   need to set `transient_for` on dialogs.
/// - `refresh`: Callback to re-queue a state reload. Call after any
///   DB-mutating user action so the UI reflects the new state.
/// - `queue`: Shared download queue forwarded to the downloads page so
///   its action buttons can mutate queue state directly.
///
/// # Returns
/// `(stack, aside)` — the five-tab view stack and the scrollable sidebar.
fn build_main_content(
    state: &AppState,
    window: &adw::ApplicationWindow,
    refresh: &Rc<dyn Fn()>,
    toast_overlay: &adw::ToastOverlay,
    queue: &Rc<RefCell<DownloadQueue>>,
) -> (adw::ViewStack, gtk4::ScrolledWindow) {
    let stack = adw::ViewStack::new();

    // Create the navigate_to_mods closure before building the overview page so
    // it can capture a weak reference to the stack.  Using a weak reference
    // avoids a reference cycle (stack → overview widget → closure → stack).
    let navigate_to_mods: Rc<dyn Fn()> = {
        let stack_weak = stack.downgrade();
        Rc::new(move || {
            if let Some(s) = stack_weak.upgrade() {
                s.set_visible_child_name("mods");
            }
        })
    };

    let ov_page = stack.add_titled(
        &overview::build(state, Rc::clone(&navigate_to_mods), refresh),
        Some("overview"),
        "Overview",
    );
    ov_page.set_icon_name(Some("go-home-symbolic"));

    let mods_page =
        stack.add_titled(&mods::build(state, window, refresh, toast_overlay), Some("mods"), "Mods");
    mods_page.set_icon_name(Some("application-x-addon-symbolic"));

    let plugins_page =
        stack.add_titled(&plugins::build(state, window, refresh), Some("plugins"), "Plugins");
    plugins_page.set_icon_name(Some("application-x-executable-symbolic"));

    let downloads_page = stack.add_titled(
        &downloads::build(
            &queue.borrow().snapshot(),
            queue,
            refresh,
        ),
        Some("downloads"),
        "Downloads",
    );
    downloads_page.set_icon_name(Some("folder-download-symbolic"));

    let profiles_page =
        stack.add_titled(&profiles::build(state, window, refresh), Some("profiles"), "Profiles");
    profiles_page.set_icon_name(Some("avatar-default-symbolic"));

    let aside = sidebar::build(state);

    (stack, aside)
}

/// Returns the formatted launch button label for a given `state`.
///
/// Uses a Unicode "play" triangle followed by the launch target name.
/// Falls back to a "No Game Detected" label when `launch_target` is empty.
fn launch_button_label(state: &AppState) -> String {
    if state.launch_target.is_empty() {
        "\u{25b6}  No Game Detected".to_string()
    } else {
        format!("\u{25b6}  Launch {}", state.launch_target)
    }
}

/// Wire the launch button to mount enabled mods via the VFS layer and then
/// open `steam://run/<app_id>` via `xdg-open`.
///
/// On each click the previous `MountHandle` is released (unmounted) before a
/// new one is created, so re-clicking after changing the active mod list
/// re-mounts cleanly.  If the VFS mount fails the game is launched anyway so
/// the user is never stuck — only the overlay is absent.
///
/// # Parameters
/// - `btn`: The launch [`gtk4::Button`] to wire.
/// - `app_id`: Shared cell containing the detected Steam App ID.
/// - `game_data_path`: Shared cell containing the game's Data directory path;
///   used as the VFS `merge_dir`.
/// - `mount_handle`: Shared cell that persists the live [`mantle_core::vfs::MountHandle`]
///   between clicks.  `None` when no mods are enabled or before first launch.
fn wire_launch_button(
    btn: &gtk4::Button,
    app_id: &Rc<Cell<Option<u32>>>,
    game_data_path: &Rc<RefCell<Option<PathBuf>>>,
    mount_handle: &Rc<RefCell<Option<mantle_core::vfs::MountHandle>>>,
) {
    let app_id = Rc::clone(app_id);
    let game_data_path = Rc::clone(game_data_path);
    let mount_handle = Rc::clone(mount_handle);
    btn.connect_clicked(move |_| {
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
    });
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
fn build_mount_params(data_dir: &std::path::Path) -> Result<mantle_core::vfs::MountParams, String> {
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

fn wire_install_button(
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

// ── SKSE installer (net feature) ─────────────────────────────────────────────

#[cfg(feature = "net")]
fn wire_skse_button(
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
        let Some(kind) = game_kind_c.get() else { return };
        if config_for_game(kind).is_none() {
            return;
        }
        let Some(game_dir) = game_data_c.borrow().as_ref().cloned() else { return };
        btn_c.set_sensitive(false);
        let overlay = overlay_c.clone();
        let btn_restore = btn_c.clone();
        run_skse_install(kind, game_dir, overlay, move || {
            btn_restore.set_sensitive(true);
        });
    });
}

#[cfg(feature = "net")]
fn run_skse_install(
    kind: mantle_core::game::GameKind,
    game_dir: std::path::PathBuf,
    toast_overlay: adw::ToastOverlay,
    on_done: impl Fn() + 'static,
) {
    use mantle_core::skse::{install_skse, SkseInstallConfig, SkseProgress};
    use std::sync::mpsc;

    let (tx, rx) = mpsc::channel::<Result<String, String>>();

    // Progress toast that stays until we dismiss it.
    let pending_toast = adw::Toast::builder()
        .title("Installing script extender…")
        .timeout(0)
        .build();
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
                let _ = tx.send(Ok(msg));
            }
        }));

        match result {
            Ok(res) if res.was_up_to_date => {
                let _ = tx.send(Ok(format!(
                    "Script extender {} is already up to date",
                    res.version_installed
                )));
            }
            Ok(res) => {
                let _ = tx.send(Ok(format!(
                    "Script extender {} installed successfully",
                    res.version_installed
                )));
            }
            Err(e) => {
                let _ = tx.send(Err(format!("{e}")));
            }
        }
    });

    glib::idle_add_local(move || {
        use std::sync::mpsc::TryRecvError;
        match rx.try_recv() {
            Ok(Ok(msg)) => {
                // Progress messages come as Ok(Ok(..)); the final result starts with "Script extender".
                if msg.starts_with("Script extender") {
                    pending_toast.dismiss();
                    toast_overlay.add_toast(
                        adw::Toast::builder()
                            .title(msg)
                            .timeout(4)
                            .build(),
                    );
                    on_done();
                    glib::ControlFlow::Break
                } else {
                    pending_toast.set_title(&msg);
                    glib::ControlFlow::Continue
                }
            }
            Ok(Err(msg)) => {
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
            Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                on_done();
                glib::ControlFlow::Break
            }
        }
    });
}
