//! Mods page — full mod list with enable/disable switches and conflict indicators.
//!
//! Displays every mod in the active profile in priority order (highest
//! priority = index 0 = shown first). Each row shows:
//! - An enable/disable [`gtk4::Switch`]
//! - Priority badge (1 = highest)
//! - Mod name with ellipsisation on narrow displays
//! - A conflict warning badge **or** a version string, but not both
//!
//! A conflict banner is shown above the list whenever
//! [`AppState::conflict_count`] > 0.
//!
//! # Empty state
//! When the mod list is empty an [`adw::StatusPage`] is shown instead of the
//! list.
//!
//! # Search
//! A [`gtk4::SearchEntry`] filters rows in real time via
//! [`ListBox::set_filter_func`]. Each row's widget name stores the lowercased
//! mod name so the filter closure can compare without borrowing the entry.
//!
//! # Button wiring
//! - **Switch**: `state-set` → `mod_list::set_mod_enabled` → `refresh()`
//! - **Add Mod**: opens [`crate::window::open_mod_install_dialog`]
//!
//! # References
//! - `standards/UI_GUIDE.md` §3, §5.3, §8, §9
//! - `path.md` item d

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk4::{glib, Box as GtkBox, Label, ListBox, Orientation, ScrolledWindow, SearchEntry, Separator};
use libadwaita as adw;
use mantle_core::{config::default_db_path, data::Database, Error as CoreError};

use crate::state::{AppState, ModEntry};

// ─── DB helper ────────────────────────────────────────────────────────────────

/// Open the default database, run `f`, and map any error to `String`.
///
/// Keeps mod-row callbacks free of boilerplate.
///
/// # Parameters
/// - `f`: Closure receiving a shared `&Database` reference.
///
/// # Returns
/// `Ok(T)` on success, `Err(String)` if opening the DB or running `f` fails.
fn with_db<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce(&Database) -> Result<T, CoreError>,
{
    let db = Database::open(&default_db_path()).map_err(|e| e.to_string())?;
    f(&db).map_err(|e| e.to_string())
}

// ─── Public entry point ───────────────────────────────────────────────────────

/// Build the full Mods page widget tree.
///
/// Returns a [`GtkBox`] with a search bar on top and a scrollable mod list
/// below, suitable for insertion into an [`adw::ViewStack`].
///
/// # Parameters
/// - `state`: Read-only snapshot of current application state.
/// - `window`: Main application window; transient parent for the install dialog.
/// - `refresh`: Callback to queue a full state reload after a DB mutation.
/// - `toast_overlay`: Toast target forwarded to the install dialog.
///
/// # Returns
/// A vertical `GtkBox` containing all child widgets.
pub fn build(
    state: &AppState,
    window: &adw::ApplicationWindow,
    refresh: &Rc<dyn Fn()>,
    toast_overlay: &adw::ToastOverlay,
) -> GtkBox {
    let outer = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(0)
        .build();

    // Shared search text — written by the SearchEntry, read by the filter func.
    let search_text: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));

    // Build the ListBox first so the toolbar's SearchEntry can call
    // invalidate_filter() on it.
    let list = mod_list_box(state, refresh, Rc::clone(&search_text));

    outer.append(&toolbar_bar(
        state,
        &list,
        Rc::clone(&search_text),
        window,
        toast_overlay,
    ));
    outer.append(&Separator::new(Orientation::Horizontal));

    if state.conflict_count > 0 {
        outer.append(&conflict_banner(state));
    }

    if state.mods.is_empty() {
        outer.append(&empty_state());
    } else {
        outer.append(&mod_scroll(&list));
    }

    outer
}

// ─── Toolbar (search + count + add) ──────────────────────────────────────────

/// Build the top toolbar: [`SearchEntry`], mod count label, and "Add Mod" button.
///
/// The [`SearchEntry`] updates `search_text` on every keystroke and calls
/// [`ListBox::invalidate_filter`] on `list` so the filter func re-runs.
///
/// # Parameters
/// - `state`: Provides the current mod count for the badge label.
/// - `list`: The [`ListBox`] to invalidate when search text changes.
/// - `search_text`: Shared cell written by the search entry.
/// - `window`: Transient parent for the "Add Mod" file chooser.
/// - `toast_overlay`: Toast target for installation feedback.
fn toolbar_bar(
    state: &AppState,
    list: &ListBox,
    search_text: Rc<RefCell<String>>,
    window: &adw::ApplicationWindow,
    toast_overlay: &adw::ToastOverlay,
) -> GtkBox {
    let bar = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .build();

    // Search entry — updates search_text and invalidates the ListBox filter.
    let search = SearchEntry::builder()
        .placeholder_text("Search mods…")
        .hexpand(true)
        .build();
    search.set_tooltip_text(Some("Filter the mod list by name"));

    let list_c = list.clone();
    search.connect_search_changed(move |entry| {
        *search_text.borrow_mut() = entry.text().to_lowercase();
        list_c.invalidate_filter();
    });
    bar.append(&search);

    // Mod count badge.
    let count = Label::new(Some(&format!("{} mods", state.mod_count)));
    count.add_css_class("caption");
    count.add_css_class("dim-label");
    count.set_valign(gtk4::Align::Center);
    bar.append(&count);

    // "Add Mod" button — opens the shared file chooser from window.rs.
    let add_btn = gtk4::Button::builder()
        .icon_name("document-save-symbolic")
        .tooltip_text("Install a mod archive")
        .build();
    add_btn.add_css_class("flat");

    add_btn.connect_clicked(glib::clone!(
        @weak window,
        @weak toast_overlay =>
        move |_| {
            crate::window::open_mod_install_dialog(&window, &toast_overlay);
        }
    ));
    bar.append(&add_btn);

    bar
}

// ─── Conflict banner ─────────────────────────────────────────────────────────

/// Adwaita banner shown when one or more file conflicts exist.
///
/// # Parameters
/// - `state`: Provides the conflict count for the title text.
fn conflict_banner(state: &AppState) -> adw::Banner {
    adw::Banner::builder()
        .title(format!(
            "{} file {} detected — some mods override the same files",
            state.conflict_count,
            if state.conflict_count == 1 { "conflict" } else { "conflicts" },
        ))
        .button_label("Dismiss")
        .revealed(true)
        .build()
}

// ─── Empty state ──────────────────────────────────────────────────────────────

/// Status page shown when no mods are installed in the active profile.
fn empty_state() -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name("application-x-addon-symbolic")
        .title("No Mods Installed")
        .description("Install a mod archive to get started.")
        .vexpand(true)
        .build()
}

// ─── Scrollable mod list ──────────────────────────────────────────────────────

/// Wraps the pre-built [`ListBox`] in a [`ScrolledWindow`] with a column header.
///
/// # Parameters
/// - `list`: Fully wired [`ListBox`] from [`mod_list_box`].
fn mod_scroll(list: &ListBox) -> ScrolledWindow {
    let scroll = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .hexpand(true)
        .build();

    let content = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(0)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    content.append(&list_header());
    content.append(list);

    scroll.set_child(Some(&content));
    scroll
}

/// Column header above the mod list.
fn list_header() -> GtkBox {
    let header = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_bottom(4)
        .margin_start(6)
        .margin_end(6)
        .build();

    // Priority column header — same fixed width as the badge in mod_row.
    let pri_col = Label::new(Some("#"));
    pri_col.add_css_class("caption");
    pri_col.add_css_class("dim-label");
    pri_col.set_width_request(32);
    pri_col.set_halign(gtk4::Align::Center);
    header.append(&pri_col);

    // Gap aligned with the switch column.
    let switch_gap = GtkBox::new(Orientation::Horizontal, 0);
    switch_gap.set_width_request(52);
    header.append(&switch_gap);

    let name_col = Label::new(Some("Mod"));
    name_col.add_css_class("caption");
    name_col.add_css_class("dim-label");
    name_col.set_hexpand(true);
    name_col.set_halign(gtk4::Align::Start);
    header.append(&name_col);

    let badge_col = Label::new(Some("Status"));
    badge_col.add_css_class("caption");
    badge_col.add_css_class("dim-label");
    badge_col.set_halign(gtk4::Align::End);
    header.append(&badge_col);

    header
}

// ─── Mod list ─────────────────────────────────────────────────────────────────

/// Build the [`ListBox`] with one row per mod, filter func, and switch wiring.
///
/// The filter func reads from `search_text` each time
/// [`ListBox::invalidate_filter`] is called. Row widget names are the
/// lowercased mod name so the closure can compare without extra state.
///
/// # Parameters
/// - `state`: Source of the ordered mod list.
/// - `refresh`: Callback to queue a state reload after a switch toggle.
/// - `search_text`: Shared cell containing the current search string.
fn mod_list_box(
    state: &AppState,
    refresh: &Rc<dyn Fn()>,
    search_text: Rc<RefCell<String>>,
) -> ListBox {
    let list = ListBox::builder()
        .selection_mode(gtk4::SelectionMode::Single)
        .build();
    list.add_css_class("boxed-list");

    for (idx, entry) in state.mods.iter().enumerate() {
        list.append(&mod_row(idx + 1, entry, refresh));
    }

    // Filter: show row if its widget_name (lowercased mod name) contains the
    // current search text.  Empty filter shows everything.
    list.set_filter_func(move |row| {
        let query = search_text.borrow();
        query.is_empty() || row.widget_name().to_lowercase().contains(query.as_str())
    });

    list
}

// ─── Mod row ──────────────────────────────────────────────────────────────────

/// Build a fully wired single mod row.
///
/// Layout (horizontal):
/// ```text
/// [priority badge] [switch] [mod name ···] [version OR ⚠ conflict]
/// ```
///
/// The row's widget name is set to the lowercased mod name so the filter func
/// in [`mod_list_box`] can match it.
///
/// The [`gtk4::Switch`] `state-set` signal calls
/// `mod_list::set_mod_enabled(conn, profile_id, mod_id, enabled)` and
/// triggers `refresh()` on success.
///
/// # Parameters
/// - `priority`: 1-based priority number (1 = highest priority).
/// - `entry`: Mod data to display and IDs to use in DB calls.
/// - `refresh`: Callback to queue a state reload after a successful toggle.
fn mod_row(priority: usize, entry: &ModEntry, refresh: &Rc<dyn Fn()>) -> gtk4::ListBoxRow {
    let content = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(6)
        .margin_end(6)
        .build();

    // ── Priority badge ────────────────────────────────────────────────────────
    let pri = Label::new(Some(&priority.to_string()));
    pri.add_css_class("caption");
    pri.add_css_class("dim-label");
    pri.set_width_request(32);
    pri.set_halign(gtk4::Align::Center);
    pri.set_valign(gtk4::Align::Center);
    content.append(&pri);

    // ── Enable / disable switch ───────────────────────────────────────────────
    let toggle = gtk4::Switch::new();
    toggle.set_active(entry.enabled);
    toggle.set_valign(gtk4::Align::Center);
    toggle.set_tooltip_text(Some(if entry.enabled { "Disable this mod" } else { "Enable this mod" }));
    toggle.set_widget_name(&format!("switch-{}-{}", entry.profile_id, entry.mod_id));

    // Wire the switch: state-set → set_mod_enabled → refresh
    let mid = entry.mod_id;
    let pid = entry.profile_id;
    let refresh_sw = Rc::clone(refresh);
    toggle.connect_state_set(move |_, enabled| {
        match with_db(|db| {
            db.with_conn(|conn| mantle_core::mod_list::set_mod_enabled(conn, pid, mid, enabled))
        }) {
            Ok(_) => refresh_sw(),
            Err(e) => tracing::warn!("set_mod_enabled failed: {e}"),
        }
        false.into()
    });
    content.append(&toggle);

    // ── Mod name ──────────────────────────────────────────────────────────────
    let name = Label::new(Some(&entry.name));
    name.set_hexpand(true);
    name.set_halign(gtk4::Align::Start);
    name.set_valign(gtk4::Align::Center);
    name.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    if !entry.enabled {
        name.add_css_class("dim-label");
    }
    content.append(&name);

    // ── Conflict badge OR version string ─────────────────────────────────────
    if entry.has_conflict {
        let badge = GtkBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(4)
            .valign(gtk4::Align::Center)
            .build();
        let icon = gtk4::Image::from_icon_name("dialog-warning-symbolic");
        icon.add_css_class("warning");
        icon.set_pixel_size(14);
        badge.append(&icon);
        let label = Label::new(Some("conflict"));
        label.add_css_class("caption");
        label.add_css_class("warning");
        badge.append(&label);
        content.append(&badge);
    } else if let Some(ver) = &entry.version {
        let ver_label = Label::new(Some(ver.as_str()));
        ver_label.add_css_class("caption");
        ver_label.add_css_class("dim-label");
        ver_label.set_valign(gtk4::Align::Center);
        content.append(&ver_label);
    }

    let row = gtk4::ListBoxRow::new();
    row.set_child(Some(&content));
    row.set_activatable(true);
    // Widget name = lowercased mod name — used by the search filter func.
    row.set_widget_name(&entry.name.to_lowercase());
    row
}
