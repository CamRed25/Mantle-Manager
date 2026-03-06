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
use std::sync::Arc;

use adw::prelude::*;
use gtk4::{glib, Box as GtkBox};
use libadwaita as adw;

use mantle_core::{
    config::{default_settings_path, AppSettings},
    plugin::EventBus,
};

#[cfg(feature = "net")]
use crate::pages::nexus_search;
#[cfg(feature = "net")]
use crate::skse;
use crate::{
    downloads::{DownloadProgress, DownloadQueue},
    install_dialog, launch,
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
// Top-level GTK4 application window builder; all widgets are created and wired
// here in sequence. Sub-function extraction would require passing all widget
// handles as parameters, increasing coupling without improving readability.
//
// `nxm_queue`: shared queue of pending `nxm://` URLs pushed by the
// `connect_open` handler in `main.rs`.  Drained by the NXM idle loop (net
// feature only).  Ignored when the net feature is disabled.
#[allow(clippy::too_many_lines)]
pub fn build_ui(app: &adw::Application, nxm_queue: &std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
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

    // ── Shared event bus ──────────────────────────────────────────────────
    // Created once here and threaded through to every subsystem that either
    // publishes events (mods enable/disable, profile activate) or consumes
    // them (state_worker, which re-sends AppState on any change).
    let event_bus = Arc::new(EventBus::new());

    // ── Download progress channel (future background tasks → GTK thread) ──
    // Background download workers will clone `progress_tx` and push
    // DownloadProgress messages; the second idle loop drains `progress_rx`
    // and calls apply_progress() so status is reflected without a full reload.
    let (progress_tx, progress_rx) = std::sync::mpsc::channel::<DownloadProgress>();
    let queue_rc: Rc<RefCell<DownloadQueue>> = Rc::new(RefCell::new(DownloadQueue::new_with_db(
        progress_tx,
        mantle_core::config::default_db_path(),
    )));

    // Shared steam_app_id: set by the idle callback when live state arrives;
    // read by the launch button callback.  Rc<Cell<…>> is safe for the GTK
    // main thread (single-threaded access only).
    let app_id_shared: Rc<Cell<Option<u32>>> = Rc::new(Cell::new(None));

    // Shared game data directory: updated from live state so wire_launch_button
    // can access it at click time without re-reading the DB.
    let game_data_path_shared: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));

    // Shared game kind — updated alongside app_id.  Available unconditionally
    // for game-specific UI decisions; SKSE sensitivity check stays gated on `net`.
    let game_kind_shared: Rc<Cell<Option<mantle_core::game::GameKind>>> = Rc::new(Cell::new(None));

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
    let refresh_fn: Rc<dyn Fn()> = Rc::new(move || {
        state_worker::trigger_reload(refresh_sender.clone());
    });

    // ── Initial page content ──────────────────────────────────────────────
    // ToastOverlay created here so it can be passed to page builders that
    // need to show feedback (e.g. the mods page "Add Mod" button).
    let toast_overlay = adw::ToastOverlay::new();
    // ── Shared launch handler ─────────────────────────────────────────────
    // Built once from the three shared Rc cells so both the header bar button
    // and the overview hero card button fire identical VFS + xdg-open logic.
    let on_launch: Rc<dyn Fn()> = Rc::new(launch::make_launch_handler(
        Rc::clone(&app_id_shared),
        Rc::clone(&game_data_path_shared),
        Rc::clone(&mount_handle_shared),
    ));

    let (stack, init_sidebar_nav, page_handles) = build_main_content(
        &placeholder,
        &window,
        &refresh_fn,
        &toast_overlay,
        &queue_rc,
        &event_bus,
        &on_launch,
    );
    switcher.set_stack(Some(&stack));

    // Store page handles so the idle loop can do in-place updates.
    let page_handles_rc = Rc::new(RefCell::new(page_handles));

    // NavigationSplitView: sidebar = summary panel (left), content = ViewStack (right).
    // Auto-collapses to single-panel navigation on narrow displays,
    // meeting the Steam Deck 1280×800 requirement (UI_GUIDE.md §4.2).
    let split_view = adw::NavigationSplitView::new();
    split_view.set_min_sidebar_width(220.0);
    split_view.set_max_sidebar_width(360.0);
    split_view.set_sidebar_width_fraction(0.25);

    let content_nav = adw::NavigationPage::builder().title("Mantle Manager").child(&stack).build();
    split_view.set_sidebar(Some(&init_sidebar_nav));
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
    launch::wire_launch_button(
        &launch_btn,
        &app_id_shared,
        &game_data_path_shared,
        &mount_handle_shared,
    );

    // ── Wire install button ───────────────────────────────────────────────
    install_dialog::wire_install_button(&install_btn, &window, &toast_overlay);

    // ── Wire SKSE button (net feature only) ───────────────────────────────
    #[cfg(feature = "net")]
    skse::wire_skse_button(&skse_btn, &game_kind_shared, &game_data_path_shared, &toast_overlay);

    // ── Ensure process exits when the window is closed ───────────────────
    // The background state-worker thread blocks indefinitely on EventBus
    // notifications and has no cancellation channel, so it would keep
    // the process alive after the window closes.  Calling app.quit() here
    // exits the GTK main loop immediately; std::process::exit in main()
    // then terminates all threads.
    window.connect_close_request(glib::clone!(
        @weak app =>
        @default-return glib::Propagation::Proceed,
        move |_| {
            app.quit();
            glib::Propagation::Stop
        }
    ));

    window.present();

    // ── Background state loader ───────────────────────────────────────────
    state_worker::spawn(sender, Arc::clone(&event_bus));

    // ── Idle poll: replace content whenever a new AppState is delivered ───
    //
    // Remains registered (`ControlFlow::Continue`) for the lifetime of the
    // window so it picks up both the initial startup load and any subsequent
    // user-triggered reloads queued via `refresh_fn()`.
    let ctx = WindowContext {
        launch_btn: launch_btn.clone(),
        app_id: Rc::clone(&app_id_shared),
        window: window.clone(),
        refresh: Rc::clone(&refresh_fn),
        toast_overlay: toast_overlay.clone(),
        queue: Rc::clone(&queue_rc),
        event_bus: Arc::clone(&event_bus),
        on_launch: Rc::clone(&on_launch),
    };
    let game_data_path_c = Rc::clone(&game_data_path_shared);
    let game_kind_c = Rc::clone(&game_kind_shared);
    let handles_idle = Rc::clone(&page_handles_rc);
    #[cfg(feature = "net")]
    let skse_btn_c = skse_btn.clone();
    glib::idle_add_local(move || {
        use std::sync::mpsc::TryRecvError;
        match receiver.try_recv() {
            Ok(state) => {
                // Propagate game data path to the launch button's shared cell.
                (*game_data_path_c.borrow_mut()).clone_from(&state.game_data_path);
                apply_state_update(&state, &handles_idle.borrow(), &ctx);
                // Update game kind (always); SKSE sensitivity only with `net`.
                {
                    use mantle_core::game::games::KNOWN_GAMES;
                    let kind = state
                        .steam_app_id
                        .and_then(|id| KNOWN_GAMES.iter().find(|g| g.steam_app_id == id))
                        .map(|g| g.kind);
                    game_kind_c.set(kind);
                    #[cfg(feature = "net")]
                    {
                        let supported =
                            kind.is_some_and(|k| mantle_core::skse::config_for_game(k).is_some());
                        skse_btn_c.set_sensitive(supported);
                    }
                }
                glib::ControlFlow::Continue
            }
            Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });

    // ── Idle poll: drain download progress channel ────────────────────────
    //
    // Drains all pending DownloadProgress messages from background tasks in
    // one pass, applies each to the in-memory queue, then — if anything
    // changed — rebuilds the Downloads page widget tree so the user sees live
    // progress bars, status badges, and completion markers immediately.
    //
    // This runs continuously on the GTK main-thread idle queue.  The cost per
    // frame when the queue is empty is a single non-blocking try_recv() call.
    let queue_prog = Rc::clone(&queue_rc);
    let handles_prog = Rc::clone(&page_handles_rc);
    let refresh_prog = Rc::clone(&refresh_fn);
    glib::idle_add_local(move || {
        use std::sync::mpsc::TryRecvError;
        let mut had_updates = false;
        loop {
            match progress_rx.try_recv() {
                Ok(prog) => {
                    queue_prog.borrow_mut().apply_progress(&prog);
                    had_updates = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return glib::ControlFlow::Break,
            }
        }
        // Rebuild the downloads page widget tree whenever progress arrives so
        // progress bars and status badges reflect the latest queue snapshot.
        if had_updates {
            swap_wrap_child(
                &handles_prog.borrow().downloads_wrap,
                &downloads::build(&queue_prog.borrow().snapshot(), &queue_prog, &refresh_prog),
            );
        }
        glib::ControlFlow::Continue
    });

    // ── Idle poll: NXM download queue drain (net feature only) ──────────
    //
    // The `connect_open` GApplication handler (main.rs) pushes incoming
    // `nxm://` URIs into `nxm_queue`.  This idle loop drains that queue,
    // spawns an OS thread per URI to resolve the CDN link via the Nexus API,
    // then feeds the HTTPS URL to `DownloadQueue::enqueue` once resolved.
    //
    // Flow:  nxm_queue (Arc<Mutex<…>>) → OS thread (resolve_nxm) →
    //        nxm_result_rx → DownloadQueue::enqueue → spawn_download
    #[cfg(feature = "net")]
    {
        // Clone the Arc here so the move closure below can take ownership.
        let nxm_queue = std::sync::Arc::clone(nxm_queue);
        // Read the API key from the OS secret store; fall back to the legacy
        // plaintext field in case the `secrets` feature is disabled or the
        // keyring is unavailable.
        let api_key_nxm = mantle_core::secrets::get_nexus_api_key()
            .unwrap_or_else(|| initial_settings.network.nexus_api_key_legacy.clone());
        let downloads_dir_nxm = initial_settings.paths.downloads_dir.clone().unwrap_or_else(|| {
            let d = mantle_core::config::data_dir().join("downloads");
            let _ = std::fs::create_dir_all(&d);
            d
        });
        let queue_nxm = Rc::clone(&queue_rc);
        let (nxm_result_tx, nxm_result_rx) =
            std::sync::mpsc::channel::<(String, String, std::path::PathBuf)>();

        glib::idle_add_local(move || {
            use std::sync::mpsc::TryRecvError;

            // Drain pending NXM URLs and spawn resolution OS threads.
            let pending: Vec<String> = {
                let mut lock = nxm_queue.lock().expect("nxm_queue lock poisoned");
                std::mem::take(&mut *lock)
            };
            for nxm_url in pending {
                let tx = nxm_result_tx.clone();
                let api_key = api_key_nxm.clone();
                let dest_dir = downloads_dir_nxm.clone();
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("tokio rt for NXM resolution");
                    match rt.block_on(mantle_net::nexus::resolve_nxm(&nxm_url, &api_key)) {
                        Ok(cdn_url) => {
                            // Derive filename from CDN URL path; strip query params.
                            let filename = cdn_url
                                .rsplit('/')
                                .next()
                                .and_then(|s| s.split('?').next())
                                .unwrap_or("mod_download.bin")
                                .to_string();
                            let dest = dest_dir.join(&filename);
                            let _ = tx.send((cdn_url, filename, dest));
                        }
                        Err(e) => {
                            tracing::error!(%e, "NXM resolution failed");
                        }
                    }
                });
            }

            // Drain resolved URLs and enqueue real downloads (GTK thread).
            loop {
                match nxm_result_rx.try_recv() {
                    Ok((cdn_url, filename, dest)) => {
                        queue_nxm.borrow_mut().enqueue(cdn_url, filename, dest);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        return glib::ControlFlow::Break;
                    }
                }
            }

            glib::ControlFlow::Continue
        });
    }

    // Consume `nxm_queue` without the net feature so the compiler does not
    // emit an unused-variable warning.
    #[cfg(not(feature = "net"))]
    let _ = nxm_queue;
}

// ─── Private helpers ──────────────────────────────────────────────────────────

/// Apply a freshly loaded [`AppState`] snapshot to the live UI.
///
/// Updates every page and the header launch button using stable widget
/// handles (no `ViewStack` rebuild).  Called from the idle loop on each
/// state delivery.
///
/// # Parameters
/// - `state`: The new application state to render.
/// - `handles`: Stable per-page widget handles created once at startup.
/// - `ctx`: Window-level context (launch button, queues, callbacks, etc.).
fn apply_state_update(state: &AppState, handles: &PageHandles, ctx: &WindowContext) {
    // Mods: in-place update (search text preserved across refreshes).
    mods::update(&handles.mods_handle, state, &ctx.refresh, &ctx.event_bus);

    // Overview: swap child (no persistent input state).
    swap_wrap_child(
        &handles.overview_wrap,
        &overview::build(state, Rc::clone(&handles.navigate_to_mods), &ctx.refresh, &ctx.on_launch),
    );

    // Plugins: swap child.
    swap_wrap_child(&handles.plugins_wrap, &plugins::build(state, &ctx.window, &ctx.refresh));

    // Downloads: swap child.
    swap_wrap_child(
        &handles.downloads_wrap,
        &downloads::build(&ctx.queue.borrow().snapshot(), &ctx.queue, &ctx.refresh),
    );

    // Profiles: swap child.
    swap_wrap_child(
        &handles.profiles_wrap,
        &profiles::build(state, &ctx.window, &ctx.refresh, &ctx.event_bus),
    );

    // Sidebar: replace nav page child.
    handles.sidebar_nav.set_child(Some(&sidebar::build(state)));

    // Header bar launch button.
    ctx.app_id.set(state.steam_app_id);
    ctx.launch_btn.set_label(&launch_button_label(state));
    ctx.launch_btn
        .set_tooltip_text(Some(&format!("Launch {}", state.launch_target)));
    ctx.launch_btn.set_sensitive(state.steam_app_id.is_some());
}

/// Immutable window-level context forwarded to [`apply_state_update`].
///
/// Bundles the eight parameters that would otherwise be threaded
/// individually through every update call.  All fields are inexpensive
/// GTK/`Rc`/`Arc` ref-count clones; cloning the struct is O(1).
#[derive(Clone)]
struct WindowContext {
    /// Header-bar launch button — label and sensitivity updated each cycle.
    launch_btn: gtk4::Button,
    /// Shared Steam app-id cell — written by the idle loop on each delivery.
    app_id: Rc<Cell<Option<u32>>>,
    /// Main application window forwarded to page builders that open dialogs.
    window: adw::ApplicationWindow,
    /// Callback to re-queue a state reload after any DB mutation.
    refresh: Rc<dyn Fn()>,
    /// Toast overlay for non-blocking user notifications.
    #[allow(dead_code)] // reserved for future toast calls
    toast_overlay: adw::ToastOverlay,
    /// Shared download queue forwarded to the Downloads page builder.
    queue: Rc<RefCell<DownloadQueue>>,
    /// Shared event bus forwarded to Mods and Profiles builders.
    event_bus: Arc<EventBus>,
    /// Shared launch handler forwarded to the Overview hero button.
    on_launch: Rc<dyn Fn()>,
}

/// Stable widget handles for per-page in-place updates.
///
/// Created once by [`build_main_content`] and stored in `build_ui`.
/// The outer widget hierarchy (`ViewStack`, `NavigationSplitView`) is never
/// rebuilt; only the data-bearing inner widgets are updated on each state
/// delivery.
struct PageHandles {
    /// Container for the Overview page child — swapped on each update.
    overview_wrap: GtkBox,
    /// In-place update handle for the Mods page (preserves `SearchEntry` state).
    mods_handle: mods::ModsHandle,
    /// Container for the Plugins page child — swapped on each update.
    plugins_wrap: GtkBox,
    /// Container for the Downloads page child — swapped on each update.
    downloads_wrap: GtkBox,
    /// Container for the Profiles page child — swapped on each update.
    profiles_wrap: GtkBox,
    /// Closure to navigate the `ViewStack` to the "mods" tab.
    navigate_to_mods: Rc<dyn Fn()>,
    /// Stable sidebar `NavigationPage` — its child is replaced on each update.
    sidebar_nav: adw::NavigationPage,
}

/// Build the stable [`adw::ViewStack`] with all five page wrappers and the
/// sidebar [`adw::NavigationPage`].
///
/// Called **once** from `build_ui`. The returned [`PageHandles`] is used for
/// all subsequent in-place updates; the `ViewStack` and navigation hierarchy are
/// never torn down or rebuilt.
///
/// # Parameters
/// - `state`: Snapshot used to populate every page at startup.
/// - `window`: Main application window, passed to page builders.
/// - `refresh`: Callback to re-queue a state reload after any DB mutation.
/// - `toast_overlay`: Toast overlay forwarded to page builders.
/// - `queue`: Shared download queue forwarded to the Downloads page.
/// - `event_bus`: Shared event bus forwarded to Mods and Profiles pages.
/// - `on_launch`: Shared launch handler forwarded to the Overview hero button.
///
/// # Returns
/// `(stack, sidebar_nav, handles)` where `stack` is placed in the
/// `NavigationSplitView` and `sidebar_nav` is its sidebar; `handles` stores
/// stable widget refs for subsequent [`apply_state_update`] calls.
fn build_main_content(
    state: &AppState,
    window: &adw::ApplicationWindow,
    refresh: &Rc<dyn Fn()>,
    toast_overlay: &adw::ToastOverlay,
    queue: &Rc<RefCell<DownloadQueue>>,
    event_bus: &Arc<EventBus>,
    on_launch: &Rc<dyn Fn()>,
) -> (adw::ViewStack, adw::NavigationPage, PageHandles) {
    let stack = adw::ViewStack::new();

    let navigate_to_mods: Rc<dyn Fn()> = {
        let stack_weak = stack.downgrade();
        Rc::new(move || {
            if let Some(s) = stack_weak.upgrade() {
                s.set_visible_child_name("mods");
            }
        })
    };

    // Overview — stable wrapper, child swapped on update.
    let overview_wrap = GtkBox::builder().orientation(gtk4::Orientation::Vertical).build();
    overview_wrap.append(&overview::build(state, Rc::clone(&navigate_to_mods), refresh, on_launch));
    let ov_page = stack.add_titled(&overview_wrap, Some("overview"), "Overview");
    ov_page.set_icon_name(Some("go-home-symbolic"));

    // Mods — in-place update via ModsHandle (search text persists).
    let (mods_root, mods_handle) = mods::build(state, window, refresh, toast_overlay, event_bus);
    let mods_page = stack.add_titled(&mods_root, Some("mods"), "Mods");
    mods_page.set_icon_name(Some("application-x-addon-symbolic"));

    // Plugins — stable wrapper, child swapped on update.
    let plugins_wrap = GtkBox::builder().orientation(gtk4::Orientation::Vertical).build();
    plugins_wrap.append(&plugins::build(state, window, refresh));
    let plugins_page = stack.add_titled(&plugins_wrap, Some("plugins"), "Plugins");
    plugins_page.set_icon_name(Some("application-x-executable-symbolic"));

    // Downloads — stable wrapper, child swapped on update.
    let downloads_wrap = GtkBox::builder().orientation(gtk4::Orientation::Vertical).build();
    downloads_wrap.append(&downloads::build(&queue.borrow().snapshot(), queue, refresh));
    let downloads_page = stack.add_titled(&downloads_wrap, Some("downloads"), "Downloads");
    downloads_page.set_icon_name(Some("folder-download-symbolic"));

    // Nexus Mod Search — built once, manages its own internal state.
    // Only compiled and shown when the `net` feature is enabled.
    #[cfg(feature = "net")]
    {
        let nexus_root = nexus_search::build(queue, refresh, toast_overlay);
        let nexus_page = stack.add_titled(&nexus_root, Some("nexus"), "Search");
        nexus_page.set_icon_name(Some("system-search-symbolic"));
    }

    // Profiles — stable wrapper, child swapped on update.
    let profiles_wrap = GtkBox::builder().orientation(gtk4::Orientation::Vertical).build();
    profiles_wrap.append(&profiles::build(state, window, refresh, event_bus));
    let profiles_page = stack.add_titled(&profiles_wrap, Some("profiles"), "Profiles");
    profiles_page.set_icon_name(Some("avatar-default-symbolic"));

    // Sidebar — stable NavigationPage, child replaced on update.
    let aside = sidebar::build(state);
    let sidebar_nav = adw::NavigationPage::builder().title("Summary").child(&aside).build();

    let handles = PageHandles {
        overview_wrap,
        mods_handle,
        plugins_wrap,
        downloads_wrap,
        profiles_wrap,
        navigate_to_mods,
        sidebar_nav: sidebar_nav.clone(),
    };

    (stack, sidebar_nav, handles)
}

/// Replace the only child of a stable wrapper [`GtkBox`].
///
/// Used by [`apply_state_update`] to swap page content without touching the
/// wrapper widget or its position in the [`adw::ViewStack`].
fn swap_wrap_child(wrapper: &GtkBox, new_child: &impl gtk4::prelude::IsA<gtk4::Widget>) {
    while let Some(child) = wrapper.first_child() {
        wrapper.remove(&child);
    }
    wrapper.append(new_child);
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

// Install dialog logic moved to crate::install_dialog.
// open_mod_install_dialog is re-exported below for pages/mods.rs compatibility.
pub use install_dialog::open_mod_install_dialog;
