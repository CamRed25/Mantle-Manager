//! Overview page — hero card, active-mod list, and conflict banner.
//!
//! # Wired signals (item l)
//! - **Profile button** → [`gtk4::MenuButton`] popover listing all profiles;
//!   selecting one calls [`profiles::set_active_profile`] then `refresh()`.
//! - **"View all →" button** → `navigate_to_mods()` navigates the
//!   [`adw::ViewStack`] to the Mods tab.
//! - **Mod toggle [`gtk4::Switch`]** → `state-set` →
//!   [`mod_list::set_mod_enabled`] → `refresh()`.
//! - **"View conflicts" banner button** → `navigate_to_mods()`.
//!
//! # First-run (item o)
//! If `state.profiles` is empty the page shows an [`adw::StatusPage`] welcome
//! screen instead of normal content.  The `state_worker` auto-creates a Default
//! profile on first launch, so this screen appears only on the very first
//! startup frame before the live state arrives.

use std::rc::Rc;

use adw::prelude::*;
use gtk4::{glib, Box as GtkBox, Label, ListBox, Orientation, ScrolledWindow, Switch};
use libadwaita as adw;

use mantle_core::{config::default_db_path, data::Database, Error as CoreError};

use crate::state::{AppState, ModEntry, ProfileEntry};

// ─── Public entry point ───────────────────────────────────────────────────────

/// Build the Overview tab content.
///
/// # Parameters
/// - `state`: Current application state snapshot.
/// - `navigate_to_mods`: Closes over the [`adw::ViewStack`] and switches it to
///   the `"mods"` named page.  Called by "View all →" and "View conflicts".
/// - `refresh`: Queues a full state reload from the database.  Called after any
///   DB-mutating action (profile switch, enable/disable).
pub fn build(
    state: &AppState,
    navigate_to_mods: Rc<dyn Fn()>,
    refresh: &Rc<dyn Fn()>,
) -> gtk4::ScrolledWindow {
    let scroll = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .hexpand(true)
        .build();

    // First-run: profiles list is empty on the very first frame before the
    // state_worker delivers the bootstrapped Default profile.  Show a welcome
    // screen instead of a broken hero card.
    if state.profiles.is_empty() {
        let status = adw::StatusPage::builder()
            .icon_name("application-x-addon-symbolic")
            .title("Welcome to Mantle Manager")
            .description(
                "No profiles found.\n\
                 A Default profile is being created — the page will update shortly.",
            )
            .build();
        status.set_vexpand(true);
        scroll.set_child(Some(&status));
        return scroll;
    }

    let content = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(16)
        .margin_top(20)
        .margin_bottom(20)
        .margin_start(20)
        .margin_end(20)
        .build();

    content.append(&hero_card(state, &state.profiles, refresh));

    if state.conflict_count > 0 {
        content.append(&conflict_banner(state, Rc::clone(&navigate_to_mods)));
    }

    content.append(&mod_list_section(state, navigate_to_mods, refresh));

    scroll.set_child(Some(&content));
    scroll
}

// ─── Hero card ────────────────────────────────────────────────────────────────

/// Build the top hero card showing game name, stats, a profile switcher, and
/// the launch button.
///
/// # Parameters
/// - `state`: Current application state.
/// - `profiles`: All known profiles, used to populate the switcher popover.
/// - `refresh`: Queues a state reload after a successful profile switch.
fn hero_card(state: &AppState, profiles: &[ProfileEntry], refresh: &Rc<dyn Fn()>) -> GtkBox {
    let card = GtkBox::new(Orientation::Vertical, 0);
    card.add_css_class("card");

    let inner = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(16)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();

    // Game icon
    let icon = gtk4::Image::from_icon_name("input-gaming-symbolic");
    icon.set_icon_size(gtk4::IconSize::Large);
    icon.set_pixel_size(48);
    icon.set_valign(gtk4::Align::Center);
    inner.append(&icon);

    // Info column
    let info = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(4)
        .hexpand(true)
        .valign(gtk4::Align::Center)
        .build();

    let title = Label::new(Some(&state.game_name));
    title.add_css_class("title-3");
    title.set_halign(gtk4::Align::Start);
    info.append(&title);

    let meta = Label::new(Some(&format!("{} · Steam · Proton", state.game_version)));
    meta.add_css_class("caption");
    meta.add_css_class("dim-label");
    meta.set_halign(gtk4::Align::Start);
    info.append(&meta);

    // Stats row
    let stats_row = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(16)
        .margin_top(8)
        .build();

    stats_row.append(&stat_label(&format!("{} mods", state.mod_count), false));
    stats_row.append(&stat_label(&format!("{} plugins", state.plugin_count), false));
    if state.conflict_count > 0 {
        stats_row.append(&stat_label(&format!("{} conflicts", state.conflict_count), true));
    }
    stats_row.append(&stat_label(&state.overlay_backend, false));
    info.append(&stats_row);

    inner.append(&info);

    // Right column: profile switcher + launch button
    let right = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(8)
        .valign(gtk4::Align::Center)
        .build();

    right.append(&profile_switcher_button(state, profiles, refresh));

    let launch_btn = gtk4::Button::with_label(&format!("▶  Launch {}", state.launch_target));
    launch_btn.add_css_class("suggested-action");
    launch_btn.set_tooltip_text(Some(&format!("Launch {}", state.launch_target)));
    right.append(&launch_btn);

    inner.append(&right);
    card.append(&inner);
    card
}

// ─── Profile switcher popover ─────────────────────────────────────────────────

/// Build a [`gtk4::MenuButton`] that reveals a [`gtk4::Popover`] listing all
/// profiles.  Selecting a row activates that profile in the DB and calls
/// `refresh()`.
///
/// # Parameters
/// - `state`: Provides the current active profile label for the button face.
/// - `profiles`: All profiles to list inside the popover.
/// - `refresh`: Called after a successful [`profiles::set_active_profile`] call.
fn profile_switcher_button(
    state: &AppState,
    profiles: &[ProfileEntry],
    refresh: &Rc<dyn Fn()>,
) -> gtk4::MenuButton {
    let menu_btn = gtk4::MenuButton::builder()
        .label(format!("{}  ▾", state.active_profile))
        .tooltip_text("Switch profile")
        .build();
    menu_btn.add_css_class("flat");

    let list = ListBox::builder()
        .selection_mode(gtk4::SelectionMode::None)
        .build();
    list.add_css_class("boxed-list");

    for profile in profiles {
        let row = profile_row(profile, &menu_btn, refresh);
        list.append(&row);
    }

    // Wrap the ListBox in a small-margin box so it has breathing room inside
    // the popover.
    let list_wrapper = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    list_wrapper.append(&list);

    let popover = gtk4::Popover::new();
    popover.set_child(Some(&list_wrapper));
    menu_btn.set_popover(Some(&popover));
    menu_btn
}

/// Build a single row in the profile switcher popover.
///
/// Activating the row calls [`profiles::set_active_profile`], closes the
/// popover, and triggers a full state reload via `refresh()`.
///
/// # Parameters
/// - `profile`: The profile this row represents.
/// - `menu_btn`: The parent [`gtk4::MenuButton`]; closed via `popdown()` after
///   selection.
/// - `refresh`: Called after a successful DB write.
fn profile_row(
    profile: &ProfileEntry,
    menu_btn: &gtk4::MenuButton,
    refresh: &Rc<dyn Fn()>,
) -> gtk4::ListBoxRow {
    let row_box = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();

    // Checkmark visible for the currently active profile.
    let check = Label::new(Some(if profile.active { "✓" } else { "  " }));
    check.add_css_class("caption");
    row_box.append(&check);

    let name_label = Label::new(Some(&profile.name));
    name_label.set_hexpand(true);
    name_label.set_halign(gtk4::Align::Start);
    row_box.append(&name_label);

    let count_label = Label::new(Some(&format!("{} mods", profile.mod_count)));
    count_label.add_css_class("caption");
    count_label.add_css_class("dim-label");
    row_box.append(&count_label);

    let row = gtk4::ListBoxRow::new();
    row.set_child(Some(&row_box));
    row.set_activatable(true);

    // ProfileEntry::id is a serialised i64 from profiles.id.
    let profile_id: i64 = profile.id.parse().unwrap_or(0);
    let refresh_clone = Rc::clone(refresh);
    let btn_clone = menu_btn.clone();

    row.connect_activate(move |_| {
        // Close the popover immediately for responsiveness.
        btn_clone.popdown();

        match with_db(|db| {
            db.with_conn(|conn| {
                mantle_core::data::profiles::set_active_profile(conn, profile_id)
            })
        }) {
            Ok(()) => refresh_clone(),
            Err(e) => tracing::warn!("overview: set_active_profile failed: {e}"),
        }
    });

    row
}

// ─── Conflict banner ──────────────────────────────────────────────────────────

/// Build the conflict warning [`adw::Banner`].
///
/// Clicking "View conflicts" calls `navigate_to_mods()` to take the user
/// directly to the Mods tab.
///
/// # Parameters
/// - `state`: Provides the conflict count for the banner title.
/// - `navigate_to_mods`: Called when the banner action button is clicked.
fn conflict_banner(state: &AppState, navigate_to_mods: Rc<dyn Fn()>) -> adw::Banner {
    let banner = adw::Banner::builder()
        .title(format!(
            "{} file conflicts detected — some mods override the same files",
            state.conflict_count
        ))
        .button_label("View conflicts")
        .revealed(true)
        .build();

    banner.connect_button_clicked(move |_| navigate_to_mods());
    banner
}

// ─── Active mod list section ──────────────────────────────────────────────────

/// Build the "Active Mods" section: a header row with "View all →" and a
/// [`ListBox`] showing up to five mods by priority.
///
/// # Parameters
/// - `state`: Provides `mod_count` and the `mods` list.
/// - `navigate_to_mods`: Wired to the "View all →" button.
/// - `refresh`: Passed down to each mod row's enable/disable switch.
fn mod_list_section(
    state: &AppState,
    navigate_to_mods: Rc<dyn Fn()>,
    refresh: &Rc<dyn Fn()>,
) -> GtkBox {
    let section = GtkBox::new(Orientation::Vertical, 8);

    // Header row
    let header_row = GtkBox::builder().orientation(Orientation::Horizontal).build();

    let title = Label::new(Some("Active Mods"));
    title.add_css_class("heading");
    title.set_hexpand(true);
    title.set_halign(gtk4::Align::Start);
    header_row.append(&title);

    let view_all = gtk4::Button::with_label(&format!("View all {} →", state.mod_count));
    view_all.add_css_class("flat");
    view_all.connect_clicked(move |_| navigate_to_mods());
    header_row.append(&view_all);

    section.append(&header_row);

    // Show at most 5 rows to keep the overview card compact.
    let list = ListBox::builder()
        .selection_mode(gtk4::SelectionMode::Single)
        .build();
    list.add_css_class("boxed-list");

    for entry in state.mods.iter().take(5) {
        list.append(&mod_row(entry, refresh));
    }

    section.append(&list);
    section
}

/// Build a single mod row: enable switch, name label, and either a conflict
/// badge or the version string.
///
/// The switch's `state-set` signal calls [`mod_list::set_mod_enabled`] and
/// then `refresh()` to reload the full state.
///
/// # Parameters
/// - `entry`: Snapshot of the mod.
/// - `refresh`: Called after a successful enable/disable toggle.
fn mod_row(entry: &ModEntry, refresh: &Rc<dyn Fn()>) -> gtk4::ListBoxRow {
    let content = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(10)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(6)
        .margin_end(6)
        .build();

    let toggle = Switch::new();
    toggle.set_active(entry.enabled);
    toggle.set_valign(gtk4::Align::Center);
    toggle.set_tooltip_text(Some(if entry.enabled { "Disable mod" } else { "Enable mod" }));
    content.append(&toggle);

    let name = Label::new(Some(&entry.name));
    name.set_hexpand(true);
    name.set_halign(gtk4::Align::Start);
    name.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    content.append(&name);

    if entry.has_conflict {
        let badge = Label::new(Some("⚠ conflict"));
        badge.add_css_class("caption");
        badge.add_css_class("warning");
        content.append(&badge);
    } else if let Some(ver) = &entry.version {
        let badge = Label::new(Some(ver.as_str()));
        badge.add_css_class("caption");
        badge.add_css_class("dim-label");
        content.append(&badge);
    }

    let row = gtk4::ListBoxRow::new();
    row.set_child(Some(&content));
    row.set_activatable(true);

    // Wire the enable/disable switch.
    let mod_id = entry.mod_id;
    let profile_id = entry.profile_id;
    let refresh_sw = Rc::clone(refresh);
    toggle.connect_state_set(move |_, enabled| {
        match with_db(|db| {
            db.with_conn(|conn| {
                mantle_core::mod_list::set_mod_enabled(conn, profile_id, mod_id, enabled)
            })
        }) {
            Ok(_) => refresh_sw(),
            Err(e) => tracing::warn!("overview: set_mod_enabled failed: {e}"),
        }
        glib::Propagation::Proceed
    });

    row
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn stat_label(text: &str, warning: bool) -> Label {
    let label = Label::new(Some(text));
    label.add_css_class("caption");
    if warning {
        label.add_css_class("warning");
    } else {
        label.add_css_class("dim-label");
    }
    label
}

/// Open the database and run `f` against it, mapping any error to [`CoreError`].
///
/// Mirrors the helper pattern used in `mods.rs` for one-shot DB mutations.
///
/// # Errors
/// Returns [`CoreError`] if the database cannot be opened or if `f` fails.
fn with_db<T, F>(f: F) -> Result<T, CoreError>
where
    F: FnOnce(&Database) -> Result<T, CoreError>,
{
    let db = Database::open(&default_db_path())?;
    f(&db)
}
