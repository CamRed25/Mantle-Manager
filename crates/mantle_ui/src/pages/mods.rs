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
use std::sync::Arc;

use adw::prelude::*;
use gtk4::{
    glib, Box as GtkBox, Label, ListBox, Orientation, ScrolledWindow, SearchEntry, Separator,
};
use libadwaita as adw;
use mantle_core::{
    plugin::{EventBus, ModManagerEvent, ModInfo},
};

use crate::pages::shared::with_db_s as with_db;
use crate::state::{AppState, ModEntry};

// ─── Page handle ───────────────────────────────────────────────────────────────────

/// Stable widget references returned by [`build`] and consumed by [`update`].
///
/// Holds the mutable data-bound parts of the Mods page so that [`update`]
/// can refresh them without rebuilding the widget tree, preserving user input
/// state (search query, scroll position).
pub struct ModsHandle {
    /// `GtkBox` that holds the conflict banner (if any) plus the list scroll
    /// or empty status page.  Cleared and repopulated on each [`update`].
    pub data_box: GtkBox,
    /// The stable list box — rows are cleared and re-added in place so the
    /// search filter function (which captures `search_text`) keeps working.
    pub list: ListBox,
    /// The scroll wrapper around `list` — added to `data_box` when non-empty.
    pub list_scroll: ScrolledWindow,
    /// Shared search text cell written by the [`gtk4::SearchEntry`] and read
    /// by the `ListBox` filter closure.  Must outlive both widgets.
    /// Current search filter text.  Kept alive here so external callers can
    /// read or reset the filter (e.g., for save/restore).  The filter func
    /// holds its own `Rc` clone, so the filtering works even before any external
    /// reader is added.  Will be used in Tier 3/g when search state is persisted.
    #[allow(dead_code)]
    pub search_text: Rc<RefCell<String>>,
    /// "N mods" badge in the toolbar — updated in place to avoid rebuilding
    /// the toolbar row (which would destroy the `SearchEntry`).
    pub count_label: Label,
    /// Empty-state status page — appended to `data_box` when the mod list is empty.
    pub empty_status: adw::StatusPage,
}

// ─── DB helper ────────────────────────────────────────────────────────────────

// ─── Public entry point ───────────────────────────────────────────────────────

/// Build the full Mods page widget tree.
///
/// Returns `(root_widget, handle)`: the root is placed into an
/// [`adw::ViewStack`]; the handle is stored by `window.rs` for subsequent
/// in-place updates via [`update`], which preserves the
/// [`gtk4::SearchEntry`] text and focus state across state refreshes.
///
/// # Parameters
/// - `state`: Read-only snapshot of current application state.
/// - `window`: Main application window; transient parent for install dialogs.
/// - `refresh`: Callback to queue a full state reload after a DB mutation.
/// - `toast_overlay`: Toast target forwarded to the install dialog.
/// - `event_bus`: Shared event bus for publishing mod-enabled/disabled events.
pub fn build(
    state: &AppState,
    window: &adw::ApplicationWindow,
    refresh: &Rc<dyn Fn()>,
    toast_overlay: &adw::ToastOverlay,
    event_bus: &Arc<EventBus>,
) -> (GtkBox, ModsHandle) {
    let outer = GtkBox::builder().orientation(Orientation::Vertical).spacing(0).build();

    // Shared search text — written once by the SearchEntry; the filter func on
    // the stable ListBox reads it on every invalidate_filter() call, so the
    // current query survives data-area rebuilds.
    let search_text: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));

    // Stable count badge in the toolbar so update() can change the text
    // without touching the SearchEntry.
    let count_label = Label::new(Some(&format!("{} mods", state.mod_count)));
    count_label.add_css_class("caption");
    count_label.add_css_class("dim-label");
    count_label.set_valign(gtk4::Align::Center);

    // Build the ListBox once; it lives for the page lifetime so the filter
    // closure captured by the SearchEntry always refers to the same object.
    let list = mod_list_box(state, refresh, Rc::clone(&search_text), event_bus);

    // Stable scroll wrapper — added/removed from data_box without recreation.
    let list_scroll = mod_scroll(&list);

    // Persistent empty state page — toggled into data_box when mods is empty.
    let empty_status = empty_state();

    outer.append(&toolbar_bar(&count_label, &list, Rc::clone(&search_text), window, toast_overlay));
    outer.append(&Separator::new(Orientation::Horizontal));

    // data_box holds the conflict banner + list scroll or empty status.
    // Cleared and repopulated by update() and populate_data_box().
    let data_box = GtkBox::builder().orientation(Orientation::Vertical).spacing(0).build();
    populate_data_box(&data_box, state, &list_scroll, &empty_status);
    outer.append(&data_box);

    (outer, ModsHandle { data_box, list, list_scroll, search_text, count_label, empty_status })
}

/// Refresh the Mods page in place without rebuilding the widget tree.
///
/// Preserves the [`gtk4::SearchEntry`] text by only clearing and repopulating
/// the data-bound widgets inside `handle`.  Called by `window.rs` on every
/// state delivery instead of a full `build()` call.
///
/// # Parameters
/// - `handle`: Stable widget refs returned by the initial [`build`] call.
/// - `state`: New application state snapshot.
/// - `refresh`: Updated callback (re-wired each state delivery).
/// - `event_bus`: Shared event bus for switch-toggle events.
pub fn update(
    handle: &ModsHandle,
    state: &AppState,
    refresh: &Rc<dyn Fn()>,
    event_bus: &Arc<EventBus>,
) {
    // Update the mod count badge in the toolbar.
    handle.count_label.set_label(&format!("{} mods", state.mod_count));

    // Clear and repopulate rows in the stable ListBox.
    // The filter func is still wired to handle.search_text so the current
    // query is applied to the new rows immediately after invalidate_filter.
    while let Some(child) = handle.list.first_child() {
        handle.list.remove(&child);
    }
    for (idx, entry) in state.mods.iter().enumerate() {
        handle.list.append(&mod_row(idx + 1, entry, refresh, event_bus));
    }
    handle.list.invalidate_filter();

    // Rebuild the data area (conflict banner + list or empty state).
    populate_data_box(&handle.data_box, state, &handle.list_scroll, &handle.empty_status);
}

// ─── Data area helper ─────────────────────────────────────────────────────────

/// Clear and repopulate the data container with an optional conflict banner
/// and either the list scroll or the empty-state page.
///
/// Called during initial [`build`] and on every [`update`] call.
///
/// # Parameters
/// - `data_box`: Container to clear and refill.
/// - `state`: Provides conflict count and empty-mod-list check.
/// - `list_scroll`: Scrolled list shown when mods are present.
/// - `empty_status`: Status page shown when the mod list is empty.
fn populate_data_box(
    data_box: &GtkBox,
    state: &AppState,
    list_scroll: &ScrolledWindow,
    empty_status: &adw::StatusPage,
) {
    while let Some(child) = data_box.first_child() {
        data_box.remove(&child);
    }
    if state.conflict_count > 0 {
        data_box.append(&conflict_banner(state));
    }
    if state.mods.is_empty() {
        data_box.append(empty_status);
    } else {
        data_box.append(list_scroll);
    }
}

// ─── Toolbar (search + count + add) ──────────────────────────────────────────

/// Build the top toolbar: [`SearchEntry`], mod count label, and "Add Mod" button.
///
/// The [`SearchEntry`] and `count_label` are built outside this function so
/// they can be stored in [`ModsHandle`] and survive data-area rebuilds.
/// `count_label` is passed in; `SearchEntry` is created here.
///
/// # Parameters
/// - `count_label`: Stable label showing "N mods" (updated by [`update`]).
/// - `list`: The [`ListBox`] to invalidate when search text changes.
/// - `search_text`: Shared cell written by the search entry.
/// - `window`: Transient parent for the "Add Mod" file chooser.
/// - `toast_overlay`: Toast target for installation feedback.
fn toolbar_bar(
    count_label: &Label,
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

    // Search entry — built once; lives for the toolbar lifetime.
    // The search_text cell and the list invalidation are wired here;
    // update() never touches this bar so the query survives refreshes.
    let search = SearchEntry::builder().placeholder_text("Search mods…").hexpand(true).build();
    search.set_tooltip_text(Some("Filter the mod list by name"));

    let list_c = list.clone();
    search.connect_search_changed(move |entry| {
        *search_text.borrow_mut() = entry.text().to_lowercase();
        list_c.invalidate_filter();
    });
    bar.append(&search);

    // Stable count label passed in from build().
    bar.append(count_label);

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
            if state.conflict_count == 1 {
                "conflict"
            } else {
                "conflicts"
            },
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
    event_bus: &Arc<EventBus>,
) -> ListBox {
    let list = ListBox::builder().selection_mode(gtk4::SelectionMode::Single).build();
    list.add_css_class("boxed-list");

    for (idx, entry) in state.mods.iter().enumerate() {
        list.append(&mod_row(idx + 1, entry, refresh, event_bus));
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
fn mod_row(priority: usize, entry: &ModEntry, refresh: &Rc<dyn Fn()>, event_bus: &Arc<EventBus>) -> gtk4::ListBoxRow {
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
    toggle.set_tooltip_text(Some(if entry.enabled {
        "Disable this mod"
    } else {
        "Enable this mod"
    }));
    toggle.set_widget_name(&format!("switch-{}-{}", entry.profile_id, entry.mod_id));

    // Wire the switch: state-set → set_mod_enabled → publish event → refresh
    let mid = entry.mod_id;
    let pid = entry.profile_id;
    let mod_name = entry.name.clone();
    let mod_version = entry.version.clone().unwrap_or_default();
    let mod_priority = i64::try_from(priority).unwrap_or(i64::MAX);
    let refresh_sw = Rc::clone(refresh);
    let bus_sw = Arc::clone(event_bus);
    toggle.connect_state_set(move |_, enabled| {
        match with_db(|db| {
            db.with_conn(|conn| mantle_core::mod_list::set_mod_enabled(conn, pid, mid, enabled))
        }) {
            Ok(_) => {
                let info = ModInfo {
                    id: mid,
                    slug: mod_name.to_lowercase().replace(' ', "_"),
                    name: mod_name.clone(),
                    version: mod_version.clone(),
                    author: String::new(),
                    priority: mod_priority,
                    is_enabled: enabled,
                    install_dir: String::new(),
                };
                let event = if enabled {
                    ModManagerEvent::ModEnabled(info)
                } else {
                    ModManagerEvent::ModDisabled(info)
                };
                bus_sw.publish(&event);
                refresh_sw();
            }
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
