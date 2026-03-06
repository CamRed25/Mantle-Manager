//! Nexus Mods search page (requires the `net` feature).
//!
//! Presents a two-panel layout:
//!
//! - **Left**: search entry, game-domain selector, scrollable results list.
//! - **Right**: files panel for the selected mod with per-file Download buttons.
//!
//! All network calls happen on detached OS threads (each with its own
//! single-threaded Tokio runtime) and communicate back to the GTK main thread
//! via an [`std::sync::mpsc`] channel polled by a `glib::idle_add_local` loop.
//!
//! # Wire-up
//! Call [`build`] once from `build_main_content` and add the returned
//! [`gtk4::Box`] to the `adw::ViewStack`.  The page manages its own internal
//! state and does not require an external refresh signal.
//!
//! # References
//! - `standards/UI_GUIDE.md` §3, §5, §8, §9
//! - Tier 3/g item 15

use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::mpsc};

use adw::prelude::*;
use gtk4::{glib, Box as GtkBox, Button, Label, ListBox, Orientation, ScrolledWindow, Spinner};
use libadwaita as adw;

use mantle_core::config::{default_settings_path, AppSettings};
use mantle_net::nexus::{
    models::{DownloadLink, ModFile, SearchResult},
    NexusClient,
};

use crate::downloads::DownloadQueue;

// ─── Supported game list ─────────────────────────────────────────────────────

/// Human-readable names for the game-domain dropdown.
const GAME_LABELS: &[&str] = &[
    "Skyrim SE",
    "Skyrim LE",
    "Fallout 4",
    "New Vegas",
    "Oblivion",
];

/// Nexus Mods game domain slugs corresponding to [`GAME_LABELS`].
const GAME_DOMAINS: &[&str] = &[
    "skyrimspecialedition",
    "skyrim",
    "fallout4",
    "newvegas",
    "oblivion",
];

/// Nexus search-endpoint game IDs corresponding to [`GAME_LABELS`].
const GAME_IDS: &[u32] = &[1704, 110, 1151, 130, 101];

// ─── Async message type ───────────────────────────────────────────────────────

/// Messages sent from background worker threads to the GTK idle loop.
enum SearchMsg {
    /// Search results returned successfully.
    Results(Vec<SearchResult>),
    /// File list for a specific mod returned successfully.
    Files {
        mod_id: u32,
        mod_name: String,
        files: Vec<ModFile>,
    },
    /// Download links for a single file returned successfully.
    DownloadLinks {
        /// Original file name (used to determine the destination path).
        file_name: String,
        /// CDN download links; the first one is used.
        links: Vec<DownloadLink>,
    },
    /// A background operation failed.
    Err(String),
}

// ─── Internal UI state ───────────────────────────────────────────────────────

/// Holds the live data needed to populate and react to the search page.
struct SearchState {
    /// Latest search results (populated on search completion).
    results: Vec<SearchResult>,
    /// The game domain selected when the last search ran.
    current_domain: String,
    /// API key snapshot (read from settings at search time).
    api_key: String,
    /// Context for the mod currently shown in the files panel.
    current_mod: Option<ModContext>,
    /// True while a network request is in flight.
    loading: bool,
}

/// Context for the currently selected mod displayed in the files panel.
struct ModContext {
    mod_id: u32,
    mod_name: String,
}

// ─── Public entry point ───────────────────────────────────────────────────────

/// Build the Nexus Mod Search page widget tree.
///
/// Returns a vertical [`GtkBox`] suitable for insertion into an
/// [`adw::ViewStack`].  The page is built **once** and manages its own
/// internal state; it does not need an external state-refresh callback.
///
/// # Parameters
/// - `queue`         – Shared download queue; mods are enqueued from this page.
/// - `refresh`       – Called after each enqueue so the Downloads page updates.
/// - `toast_overlay` – Used to surface error and confirmation toasts.
// GTK4 builder pattern: all widgets are created inline and wired together;
// sub-function extraction would require threading every handle through parameters,
// increasing coupling without improving readability.
#[allow(clippy::too_many_lines)]
pub fn build(
    queue: &Rc<RefCell<DownloadQueue>>,
    refresh: &Rc<dyn Fn()>,
    toast_overlay: &adw::ToastOverlay,
) -> GtkBox {
    // ── Message channel from worker threads → idle loop ───────────────────
    let (tx, rx) = mpsc::channel::<SearchMsg>();

    // ── Shared search state (GTK main thread only) ────────────────────────
    let state: Rc<RefCell<SearchState>> = Rc::new(RefCell::new(SearchState {
        results: Vec::new(),
        current_domain: GAME_DOMAINS[0].to_string(),
        api_key: String::new(),
        current_mod: None,
        loading: false,
    }));

    // ─────────────────────────────────────────────────────────────────────
    // Widget tree
    // ─────────────────────────────────────────────────────────────────────

    let outer = GtkBox::builder().orientation(Orientation::Vertical).spacing(0).build();

    // ── Search toolbar ────────────────────────────────────────────────────
    let toolbar = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .build();

    // Game-domain dropdown
    let domain_dropdown = gtk4::DropDown::from_strings(GAME_LABELS);
    domain_dropdown.set_tooltip_text(Some("Target game on Nexus Mods"));
    toolbar.append(&domain_dropdown);

    // Search entry
    let search_entry = gtk4::SearchEntry::builder()
        .placeholder_text("Search Nexus Mods…")
        .hexpand(true)
        .build();
    toolbar.append(&search_entry);

    // Loading spinner
    let spinner = Spinner::new();
    spinner.set_visible(false);
    toolbar.append(&spinner);

    // Search button
    let search_btn = Button::builder().label("Search").build();
    search_btn.add_css_class("suggested-action");
    toolbar.append(&search_btn);

    outer.append(&toolbar);
    outer.append(&gtk4::Separator::new(Orientation::Horizontal));

    // ── Main two-panel content area ───────────────────────────────────────
    let content_box = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(0)
        .vexpand(true)
        .build();

    // Left: search results list
    let results_scroll = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .hexpand(true)
        .build();

    let results_list =
        Rc::new(ListBox::builder().selection_mode(gtk4::SelectionMode::Single).build());
    results_list.add_css_class("navigation-sidebar");
    results_scroll.set_child(Some(results_list.as_ref()));
    content_box.append(&results_scroll);

    // Right: files panel (hidden until a mod is selected)
    let files_pane = Rc::new(
        GtkBox::builder()
            .orientation(Orientation::Vertical)
            .spacing(8)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .width_request(300)
            .visible(false)
            .build(),
    );

    let files_header = Label::builder().label("Available Files").halign(gtk4::Align::Start).build();
    files_header.add_css_class("heading");
    files_pane.append(&files_header);

    let files_list_scroll = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .build();

    let files_list = Rc::new(ListBox::builder().selection_mode(gtk4::SelectionMode::None).build());
    files_list.add_css_class("boxed-list");
    files_list_scroll.set_child(Some(files_list.as_ref()));
    files_pane.append(&files_list_scroll);

    content_box.append(&gtk4::Separator::new(Orientation::Vertical));
    content_box.append(files_pane.as_ref());

    outer.append(&content_box);

    // ── Status bar ────────────────────────────────────────────────────────
    let status_label = Rc::new(
        Label::builder()
            .label("Enter a search term above")
            .halign(gtk4::Align::Start)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(12)
            .margin_end(12)
            .build(),
    );
    status_label.add_css_class("caption");
    status_label.add_css_class("dim-label");
    outer.append(status_label.as_ref());

    // ─────────────────────────────────────────────────────────────────────
    // Search action (shared between button click and Enter key)
    // ─────────────────────────────────────────────────────────────────────

    let do_search = {
        let tx = tx.clone();
        let spinner = spinner.clone();
        let results_list = Rc::clone(&results_list);
        let state = Rc::clone(&state);
        let status_label = Rc::clone(&status_label);
        let search_entry = search_entry.clone();
        let domain_dropdown = domain_dropdown.clone();
        let toast_overlay = toast_overlay.clone();

        Rc::new(move || {
            let query = search_entry.text().to_string();
            if query.is_empty() {
                return;
            }

            // Load the API key from the OS secret store; fall back to the
            // legacy plain-text field for when the `secrets` feature is off.
            let settings =
                AppSettings::load_or_default(&default_settings_path()).unwrap_or_default();
            let api_key = mantle_core::secrets::get_nexus_api_key()
                .unwrap_or_else(|| settings.network.nexus_api_key_legacy.clone());
            if api_key.is_empty() {
                toast_overlay
                    .add_toast(adw::Toast::new("Set your Nexus API key in Settings first"));
                return;
            }

            let idx = domain_dropdown.selected() as usize;
            let domain = GAME_DOMAINS[idx.min(GAME_DOMAINS.len() - 1)].to_string();
            let game_id = GAME_IDS[idx.min(GAME_IDS.len() - 1)];

            // Clear previous results.
            while let Some(row) = results_list.first_child() {
                results_list.remove(&row);
            }

            // Update shared state.
            {
                let mut s = state.borrow_mut();
                s.current_domain.clone_from(&domain);
                s.api_key.clone_from(&api_key);
                s.loading = true;
                s.current_mod = None;
            }

            spinner.start();
            spinner.set_visible(true);
            status_label.set_label(&format!("Searching '{query}'…"));

            // Spawn background search thread.
            let tx = tx.clone();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("tokio rt for nexus search");

                let result = rt.block_on(async {
                    let client = NexusClient::new(api_key)?;
                    client.search(&domain, &query, game_id).await
                });

                match result {
                    Ok(resp) => {
                        let _ = tx.send(SearchMsg::Results(resp.results));
                    }
                    Err(e) => {
                        let _ = tx.send(SearchMsg::Err(e.to_string()));
                    }
                }
            });
        })
    };

    // Wire search button.
    {
        let do_search = Rc::clone(&do_search);
        search_btn.connect_clicked(move |_| do_search());
    }
    // Wire Enter key in search entry.
    {
        let do_search = Rc::clone(&do_search);
        search_entry.connect_activate(move |_| do_search());
    }

    // ─────────────────────────────────────────────────────────────────────
    // Result row selection → fetch file list
    // ─────────────────────────────────────────────────────────────────────

    {
        let tx = tx.clone();
        let state = Rc::clone(&state);
        let files_pane = Rc::clone(&files_pane);
        let toast_overlay = toast_overlay.clone();

        results_list.connect_row_selected(move |_, opt_row| {
            let Some(row) = opt_row else { return };
            let idx = usize::try_from(row.index()).unwrap_or(0);

            let (mod_id, mod_name, domain, api_key) = {
                let snap = state.borrow();
                let Some(result) = snap.results.get(idx) else {
                    return;
                };
                (
                    result.mod_id,
                    result.name.clone(),
                    snap.current_domain.clone(),
                    snap.api_key.clone(),
                )
            };

            if api_key.is_empty() {
                toast_overlay
                    .add_toast(adw::Toast::new("Set your Nexus API key in Settings first"));
                return;
            }

            // Hide the files pane while loading the new mod's files.
            files_pane.set_visible(false);

            let tx = tx.clone();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("tokio rt for nexus files");

                let result = rt.block_on(async {
                    let client = NexusClient::new(api_key)?;
                    client.get_files(&domain, mod_id).await
                });

                match result {
                    Ok(resp) => {
                        let _ = tx.send(SearchMsg::Files {
                            mod_id,
                            mod_name,
                            files: resp.files,
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(SearchMsg::Err(e.to_string()));
                    }
                }
            });
        });
    }

    // ─────────────────────────────────────────────────────────────────────
    // Idle loop — pump the channel, update widgets
    // ─────────────────────────────────────────────────────────────────────
    // Clone owned Rc / GObject handles for the 'static idle closure.
    let queue_idle = Rc::clone(queue);
    let refresh_idle = Rc::clone(refresh);
    let toast_idle = toast_overlay.clone();

    glib::idle_add_local(move || {
        use std::sync::mpsc::TryRecvError;
        loop {
            match rx.try_recv() {
                // ── Search results arrived ─────────────────────────────
                Ok(SearchMsg::Results(results)) => {
                    spinner.stop();
                    spinner.set_visible(false);

                    // Clear the list before re-populating.
                    while let Some(row) = results_list.first_child() {
                        results_list.remove(&row);
                    }

                    let count = results.len();
                    for result in &results {
                        results_list.append(&make_result_row(result));
                    }

                    status_label.set_label(&format!("{count} result(s)"));
                    state.borrow_mut().results = results;
                }

                // ── File list for a mod arrived ────────────────────────
                Ok(SearchMsg::Files {
                    mod_id,
                    mod_name,
                    files,
                }) => {
                    // Rebuild the files panel with one row per file.
                    while let Some(row) = files_list.first_child() {
                        files_list.remove(&row);
                    }

                    // Only show Main and Update category files.
                    let visible_files: Vec<&ModFile> = files
                        .iter()
                        .filter(|f| {
                            f.category_name.as_deref().map_or(true, |c| {
                                matches!(c, "MAIN" | "Main" | "UPDATE" | "Update")
                            })
                        })
                        .collect();

                    for file in &visible_files {
                        let row = make_file_row(file, &tx, &state);
                        files_list.append(&row);
                    }

                    files_pane.set_visible(!visible_files.is_empty());
                    state.borrow_mut().current_mod = Some(ModContext { mod_id, mod_name });
                }

                // ── Download links arrived → enqueue ───────────────────
                Ok(SearchMsg::DownloadLinks { file_name, links }) => {
                    if let Some(first_link) = links.into_iter().next() {
                        let mod_name = state
                            .borrow()
                            .current_mod
                            .as_ref()
                            .map(|c| c.mod_name.clone())
                            .unwrap_or_default();

                        let dest = resolve_download_dest(&file_name);

                        queue_idle.borrow_mut().enqueue(first_link.uri, mod_name, dest);
                        refresh_idle();
                        toast_idle.add_toast(adw::Toast::new("Download queued"));
                    }
                }

                // ── Error from any background task ─────────────────────
                Ok(SearchMsg::Err(e)) => {
                    spinner.stop();
                    spinner.set_visible(false);
                    status_label.set_label(&format!("Error: {e}"));
                    toast_idle.add_toast(adw::Toast::new(&format!("Nexus error: {e}")));
                    state.borrow_mut().loading = false;
                }

                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return glib::ControlFlow::Break,
            }
        }
        glib::ControlFlow::Continue
    });

    outer
}

// ─── Row builders ─────────────────────────────────────────────────────────────

/// Build a single search-result row for the results `ListBox`.
///
/// Displays mod name, author name, and endorsement count.
///
/// # Parameters
/// - `result` – The [`SearchResult`] to represent.
fn make_result_row(result: &SearchResult) -> gtk4::ListBoxRow {
    let outer = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(2)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .build();

    let name = Label::new(Some(&result.name));
    name.add_css_class("body");
    name.set_halign(gtk4::Align::Start);
    name.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    outer.append(&name);

    let meta = GtkBox::builder().orientation(Orientation::Horizontal).spacing(8).build();

    let author =
        Label::new(Some(&format!("by {}", result.username.as_deref().unwrap_or("Unknown"))));
    author.add_css_class("caption");
    author.add_css_class("dim-label");
    meta.append(&author);

    let endorsements = Label::new(Some(&format!("♥ {}", result.endorsements.unwrap_or(0))));
    endorsements.add_css_class("caption");
    endorsements.add_css_class("accent");
    meta.append(&endorsements);

    outer.append(&meta);

    let row = gtk4::ListBoxRow::new();
    row.set_child(Some(&outer));
    row
}

/// Build a single file row for the files-panel `ListBox`.
///
/// Displays the file name, version, and approximate size.  The Download
/// button spawns a thread to fetch CDN links then sends a
/// [`SearchMsg::DownloadLinks`] message back to the idle loop.
///
/// # Parameters
/// - `file`  – The [`ModFile`] to represent.
/// - `tx`    – Sender half of the page's message channel.
/// - `state` – Shared search state (provides domain, `mod_id`, `api_key`).
fn make_file_row(
    file: &ModFile,
    tx: &mpsc::Sender<SearchMsg>,
    state: &Rc<RefCell<SearchState>>,
) -> gtk4::ListBoxRow {
    let row_box = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .build();

    // File info column
    let info = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .build();

    let fname = Label::new(Some(&file.file_name));
    fname.set_halign(gtk4::Align::Start);
    fname.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    fname.add_css_class("body");
    info.append(&fname);

    let version_str = file.version.as_deref().unwrap_or("?");
    // KB-to-MB conversion for display; sub-byte precision loss is acceptable.
    #[allow(clippy::cast_precision_loss)]
    let size_mb = file.size_kb.unwrap_or(0) as f64 / 1024.0;
    let meta_text = format!("v{version_str}  ·  {size_mb:.1} MB");
    let meta = Label::new(Some(&meta_text));
    meta.add_css_class("caption");
    meta.add_css_class("dim-label");
    meta.set_halign(gtk4::Align::Start);
    info.append(&meta);

    row_box.append(&info);

    // Download button
    let dl_btn = Button::builder()
        .icon_name("folder-download-symbolic")
        .tooltip_text("Enqueue this file for download")
        .valign(gtk4::Align::Center)
        .build();
    dl_btn.add_css_class("flat");
    dl_btn.add_css_class("circular");

    // Wire: spawn a thread to fetch CDN links then send DownloadLinks.
    let tx_btn = tx.clone();
    let state_btn = Rc::clone(state);
    let file_id = file.file_id;
    let file_name = file.file_name.clone();

    dl_btn.connect_clicked(move |_| {
        let (api_key, domain, mod_id) = {
            let snap = state_btn.borrow();
            (
                snap.api_key.clone(),
                snap.current_domain.clone(),
                snap.current_mod.as_ref().map_or(0, |c| c.mod_id),
            )
        };

        if api_key.is_empty() || mod_id == 0 {
            return;
        }

        let tx = tx_btn.clone();
        let fname = file_name.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio rt for download links");

            let result = rt.block_on(async {
                let client = NexusClient::new(api_key)?;
                client.get_download_links(&domain, mod_id, file_id).await
            });

            match result {
                Ok(links) => {
                    let _ = tx.send(SearchMsg::DownloadLinks {
                        file_name: fname,
                        links,
                    });
                }
                Err(e) => {
                    let _ = tx.send(SearchMsg::Err(e.to_string()));
                }
            }
        });
    });

    row_box.append(&dl_btn);

    let list_row = gtk4::ListBoxRow::new();
    list_row.set_child(Some(&row_box));
    list_row.set_activatable(false);
    list_row
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Resolve the filesystem destination path for a downloaded file.
///
/// Preference order:
/// 1. `downloads_dir` from [`AppSettings`] if configured.
/// 2. `$HOME/Downloads/`.
/// 3. Current working directory (last resort).
///
/// # Parameters
/// - `file_name` – Archive file name (e.g. `"SomeMod-1.0-main-1234.zip"`).
///
/// # Returns
/// An absolute [`PathBuf`] pointing to the resolved destination.
fn resolve_download_dest(file_name: &str) -> PathBuf {
    let settings = AppSettings::load_or_default(&default_settings_path()).unwrap_or_default();

    if let Some(dir) = settings.paths.downloads_dir {
        return dir.join(file_name);
    }

    // Fall back to ~/Downloads/.
    if let Ok(home) = std::env::var("HOME") {
        let dl = PathBuf::from(home).join("Downloads");
        if dl.exists() || std::fs::create_dir_all(&dl).is_ok() {
            return dl.join(file_name);
        }
    }

    // Last resort: cwd.
    PathBuf::from(file_name)
}
