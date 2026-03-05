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

use crate::{
    downloads::{DownloadProgress, DownloadQueue},
    pages::{downloads, mods, overview, plugins, profiles},
    settings, sidebar,
    state::AppState,
    state_worker,
};
#[cfg(feature = "net")]
use crate::pages::nexus_search;

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
    let queue_rc: Rc<RefCell<DownloadQueue>> =
        Rc::new(RefCell::new(DownloadQueue::new(progress_tx)));

    // Shared steam_app_id: set by the idle callback when live state arrives;
    // read by the launch button callback.  Rc<Cell<…>> is safe for the GTK
    // main thread (single-threaded access only).
    let app_id_shared: Rc<Cell<Option<u32>>> = Rc::new(Cell::new(None));

    // Shared game data directory: updated from live state so wire_launch_button
    // can access it at click time without re-reading the DB.
    let game_data_path_shared: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));

    // Shared game kind — updated alongside app_id.  Available unconditionally
    // for game-specific UI decisions; SKSE sensitivity check stays gated on `net`.
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
    let on_launch: Rc<dyn Fn()> = Rc::new(make_launch_handler(
        Rc::clone(&app_id_shared),
        Rc::clone(&game_data_path_shared),
        Rc::clone(&mount_handle_shared),
    ));

    let (stack, init_sidebar_nav, page_handles) = build_main_content(
        &placeholder, &window, &refresh_fn, &toast_overlay, &queue_rc, &event_bus, &on_launch,
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
                    queue_prog.borrow_mut().apply_progress(prog);
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
                &downloads::build(
                    &queue_prog.borrow().snapshot(),
                    &queue_prog,
                    &refresh_prog,
                ),
            );
        }
        glib::ControlFlow::Continue
    });
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
        &overview::build(
            state,
            Rc::clone(&handles.navigate_to_mods),
            &ctx.refresh,
            &ctx.on_launch,
        ),
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
fn make_launch_handler(
    app_id: Rc<Cell<Option<u32>>>,
    game_data_path: Rc<RefCell<Option<PathBuf>>>,
    mount_handle: Rc<RefCell<Option<mantle_core::vfs::MountHandle>>>,
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
fn wire_launch_button(
    btn: &gtk4::Button,
    app_id: &Rc<Cell<Option<u32>>>,
    game_data_path: &Rc<RefCell<Option<PathBuf>>>,
    mount_handle: &Rc<RefCell<Option<mantle_core::vfs::MountHandle>>>,
) {
    let handler = make_launch_handler(
        Rc::clone(app_id),
        Rc::clone(game_data_path),
        Rc::clone(mount_handle),
    );
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

/// Typed messages sent from the SKSE installer background thread to the idle
/// poll loop.
///
/// Using an enum instead of `Result<String, String>` eliminates the fragile
/// `msg.starts_with("Script extender")` sentinel check.
#[cfg(feature = "net")]
enum SkseMsg {
    /// Intermediate progress update — update the pending toast label.
    Progress(String),
    /// Installation completed successfully — show final toast and re-enable button.
    Done(String),
    /// Installation failed — show error toast and re-enable button.
    Err(String),
}

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

    let (tx, rx) = mpsc::channel::<SkseMsg>();

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
                toast_overlay.add_toast(
                    adw::Toast::builder().title(msg).timeout(4).build(),
                );
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
