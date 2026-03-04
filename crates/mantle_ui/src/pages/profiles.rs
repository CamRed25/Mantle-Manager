//! Profiles page — full profile list with create, clone, delete, and activate.
//!
//! Each profile is shown as an [`adw::ActionRow`]:
//! - **Title**: profile name
//! - **Subtitle**: `"{n} mods"`
//! - **Suffix**: "Active" badge (active profile only), Activate button,
//!   Clone button, Delete button
//!
//! The active profile's row carries the `.accent` style and "Active" badge.
//! Its **Activate** button is hidden (already active) and **Delete** button
//! is disabled — core enforces this invariant.
//!
//! # Dialogs (pending item y)
//! "New Profile" and "Clone" open [`adw::Dialog`]s (`UI_GUIDE` §5.4).
//! The dialogs are wired to real core operations in item y once the profile
//! backend (item i) is complete. For now the buttons are rendered without
//! connected signals.
//!
//! # Empty state
//! Shown when `state.profiles` is empty (should not occur in practice; every
//! app session has at least one profile).
//!
//! # References
//! - `standards/UI_GUIDE.md` §3, §5.3, §5.4, §8, §9
//! - `path.md` item w

use std::rc::Rc;

use adw::prelude::*;
use gtk4::{glib, Box as GtkBox, Label, ListBox, Orientation, ScrolledWindow, Separator};
use libadwaita as adw;

use crate::state::{AppState, ProfileEntry};

// ─── Public entry point ───────────────────────────────────────────────────────

/// Build the full Profiles page widget tree.
///
/// Returns a vertical [`GtkBox`] suitable for insertion into an
/// [`adw::ViewStack`].
///
/// # Parameters
/// - `state`: Read-only application state snapshot.
/// - `window`: Main application window; used as `transient_for` for dialogs.
/// - `refresh`: Callback to trigger a full state reload after a DB mutation.
pub fn build(
    state: &AppState,
    window: &adw::ApplicationWindow,
    refresh: &Rc<dyn Fn()>,
) -> GtkBox {
    let outer = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(0)
        .build();

    outer.append(&toolbar(window, refresh));
    outer.append(&Separator::new(Orientation::Horizontal));

    if state.profiles.is_empty() {
        outer.append(&empty_state());
    } else {
        outer.append(&profile_scroll(state, window, refresh));
    }

    outer
}

// ─── Private DB helper ────────────────────────────────────────────────────────

/// Open the database and run `f` with the database handle, returning any error
/// as a displayable string.
///
/// # Parameters
/// - `f`: Closure receiving a `&Database`.
///
/// # Returns
/// `Ok(T)` on success, `Err(String)` with the error message on failure.
fn with_db<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce(&mantle_core::data::Database) -> Result<T, mantle_core::Error>,
{
    use mantle_core::{config::default_db_path, data::Database};
    let db = Database::open(&default_db_path()).map_err(|e| e.to_string())?;
    f(&db).map_err(|e| e.to_string())
}

// ─── Toolbar ─────────────────────────────────────────────────────────────────

/// Top toolbar with the "New Profile" button.
///
/// Clicking "New Profile" opens a naming dialog and creates the profile on confirm.
///
/// # Parameters
/// - `window`: Transient parent for the naming dialog.
/// - `refresh`: Callback to queue a state reload after the profile is created.
fn toolbar(window: &adw::ApplicationWindow, refresh: &Rc<dyn Fn()>) -> GtkBox {
    let bar = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .build();

    let spacer = GtkBox::new(Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    bar.append(&spacer);

    let new_btn = gtk4::Button::builder()
        .label("New Profile")
        .tooltip_text("Create a new empty profile")
        .build();
    new_btn.add_css_class("suggested-action");
    new_btn.set_widget_name("btn-new-profile");

    let refresh_c = Rc::clone(refresh);
    new_btn.connect_clicked(glib::clone!(
        @weak window =>
        move |_| {
            let refresh_c2 = Rc::clone(&refresh_c);
            show_name_dialog(
                &window,
                "New Profile",
                "Enter a name for the new profile.",
                "",
                "Create",
                move |name| {
                    match with_db(|db| {
                        db.with_conn(|conn| mantle_core::profile::create_profile(conn, &name, None))
                    }) {
                        Ok(_) => refresh_c2(),
                        Err(e) => tracing::warn!("create_profile failed: {e}"),
                    }
                },
            );
        }
    ));

    bar.append(&new_btn);
    bar
}

// ─── Name dialog helper ───────────────────────────────────────────────────────

/// Show a modal [`adw::MessageDialog`] with a text entry for naming.
///
/// Calls `on_confirm(name)` with the trimmed text on confirmation.
/// Ignores empty names.
///
/// # Parameters
/// - `window`: Transient parent.
/// - `heading`: Dialog title.
/// - `body`: Explanatory sub-text.
/// - `prefill`: Initial entry value (empty string for blank).
/// - `confirm_label`: Label for the confirm response.
/// - `on_confirm`: Closure called with the entered name.
fn show_name_dialog(
    window: &adw::ApplicationWindow,
    heading: &str,
    body: &str,
    prefill: &str,
    confirm_label: &str,
    on_confirm: impl Fn(String) + 'static,
) {
    let dialog = adw::MessageDialog::builder()
        .heading(heading)
        .body(body)
        .transient_for(window)
        .build();

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("confirm", confirm_label);
    dialog.set_response_appearance("confirm", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("confirm"));
    dialog.set_close_response("cancel");

    let entry = gtk4::Entry::builder()
        .placeholder_text("Profile name")
        .activates_default(true)
        .text(prefill)
        .build();
    dialog.set_extra_child(Some(&entry));

    dialog.connect_response(Some("confirm"), move |_, _| {
        let name = entry.text().trim().to_string();
        if !name.is_empty() {
            on_confirm(name);
        }
    });

    dialog.present();
}

// ─── Confirm dialog helper ────────────────────────────────────────────────────

/// Show a modal [`adw::MessageDialog`] for confirming a destructive action.
///
/// Calls `on_confirm()` when the user clicks the destructive button.
///
/// # Parameters
/// - `window`: Transient parent.
/// - `heading`: Dialog title.
/// - `body`: Description of what will happen.
/// - `confirm_label`: Label for the destructive button.
/// - `on_confirm`: Closure called if the user proceeds.
fn show_confirm_dialog(
    window: &adw::ApplicationWindow,
    heading: &str,
    body: &str,
    confirm_label: &str,
    on_confirm: impl Fn() + 'static,
) {
    let dialog = adw::MessageDialog::builder()
        .heading(heading)
        .body(body)
        .transient_for(window)
        .build();

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("confirm", confirm_label);
    dialog.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    dialog.connect_response(Some("confirm"), move |_, _| {
        on_confirm();
    });

    dialog.present();
}

// ─── Empty state ──────────────────────────────────────────────────────────────

/// Status page shown when no profiles exist.
fn empty_state() -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name("avatar-default-symbolic")
        .title("No Profiles")
        .description("Create a profile to start organising your mods.")
        .vexpand(true)
        .build()
}

// ─── Scrollable profile list ──────────────────────────────────────────────────

/// Wraps the profile list in a [`ScrolledWindow`].
///
/// # Parameters
/// - `state`: Source of the profile list.
/// - `window`: Transient parent for row-level dialogs.
/// - `refresh`: Callback to queue a state reload after a DB mutation.
fn profile_scroll(
    state: &AppState,
    window: &adw::ApplicationWindow,
    refresh: &Rc<dyn Fn()>,
) -> ScrolledWindow {
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

    content.append(&profile_list(state, window, refresh));

    scroll.set_child(Some(&content));
    scroll
}

/// Builds the [`ListBox`] containing one row per profile.
///
/// # Parameters
/// - `state`: Source of the profile list.
/// - `window`: Transient parent for row dialogs.
/// - `refresh`: Callback to queue a state reload.
fn profile_list(
    state: &AppState,
    window: &adw::ApplicationWindow,
    refresh: &Rc<dyn Fn()>,
) -> ListBox {
    let list = ListBox::builder()
        .selection_mode(gtk4::SelectionMode::None)
        .build();
    list.add_css_class("boxed-list");

    for entry in &state.profiles {
        list.append(&profile_row(entry, window, refresh));
    }

    list
}

// ─── Profile row ──────────────────────────────────────────────────────────────

/// Build a fully wired [`adw::ActionRow`] for a single profile.
///
/// Active profile layout:
/// ```text
/// ● Profile Name          [Active]  [Clone]  [Delete — disabled]
///   147 mods
/// ```
/// Inactive profile layout:
/// ```text
///   Profile Name          [Activate]  [Clone]  [Delete]
///   23 mods
/// ```
///
/// # Parameters
/// - `entry`: Profile data to display.
/// - `window`: Transient parent for dialogs.
/// - `refresh`: Callback to queue a state reload after any DB mutation.
fn profile_row(
    entry: &ProfileEntry,
    window: &adw::ApplicationWindow,
    refresh: &Rc<dyn Fn()>,
) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(&entry.name)
        .subtitle(format!("{} mod{}", entry.mod_count, if entry.mod_count == 1 { "" } else { "s" }))
        .build();

    row.set_widget_name(&entry.id);

    if entry.active {
        row.add_css_class("accent");
    }

    // ── Active badge / Activate button ────────────────────────────────────────
    if entry.active {
        let badge = Label::new(Some("Active"));
        badge.add_css_class("caption");
        badge.add_css_class("accent");
        badge.set_valign(gtk4::Align::Center);
        row.add_suffix(&badge);
    } else {
        // Activation changes only the DB record; VFS remount added in item e.
        let activate_btn = gtk4::Button::builder()
            .label("Activate")
            .tooltip_text(format!("Switch to profile \u{ab}{}\u{bb}", entry.name))
            .valign(gtk4::Align::Center)
            .build();
        activate_btn.add_css_class("flat");
        activate_btn.set_widget_name(&format!("activate-{}", entry.id));

        let pid_str = entry.id.clone();
        let refresh_act = Rc::clone(refresh);
        activate_btn.connect_clicked(move |_| {
            if let Ok(pid) = pid_str.parse::<i64>() {
                match with_db(|db| {
                    db.with_conn(|conn| mantle_core::data::profiles::set_active_profile(conn, pid))
                }) {
                    Ok(()) => refresh_act(),
                    Err(e) => tracing::warn!("activate profile failed: {e}"),
                }
            }
        });
        row.add_suffix(&activate_btn);
    }

    row.add_suffix(&build_clone_button(&entry.id, &entry.name, window, refresh));
    row.add_suffix(&build_delete_button(&entry.id, &entry.name, entry.active, window, refresh));
    row
}

/// Create the Clone button for a profile row and wire its dialog.
///
/// # Parameters
/// - `profile_id`: String ID of the profile to clone.
/// - `profile_name`: Display name used as dialog prefill base.
/// - `window`: Transient parent for the naming dialog.
/// - `refresh`: Callback to queue a state reload after a successful clone.
fn build_clone_button(
    profile_id: &str,
    profile_name: &str,
    window: &adw::ApplicationWindow,
    refresh: &Rc<dyn Fn()>,
) -> gtk4::Button {
    let btn = gtk4::Button::builder()
        .icon_name("edit-copy-symbolic")
        .tooltip_text(format!("Clone profile \u{ab}{profile_name}\u{bb}"))
        .valign(gtk4::Align::Center)
        .build();
    btn.add_css_class("flat");
    btn.add_css_class("circular");
    btn.set_widget_name(&format!("clone-{profile_id}"));

    let pid_str = profile_id.to_owned();
    let name = profile_name.to_owned();
    let refresh_c = Rc::clone(refresh);
    btn.connect_clicked(glib::clone!(
        @weak window =>
        move |_| {
            let pid2 = pid_str.clone();
            let refresh_c2 = Rc::clone(&refresh_c);
            let prefill = format!("Copy of {name}");
            show_name_dialog(&window, "Clone Profile", "Enter a name for the cloned profile.",
                &prefill, "Clone", move |n| {
                    if let Ok(pid) = pid2.parse::<i64>() {
                        match with_db(|db| db.with_conn(|conn| mantle_core::profile::clone_profile(conn, pid, &n))) {
                            Ok(_) => refresh_c2(),
                            Err(e) => tracing::warn!("clone_profile failed: {e}"),
                        }
                    }
                });
        }
    ));
    btn
}

/// Create the Delete button for a profile row and wire its confirmation dialog.
///
/// The button is disabled and unstyled when the profile is active (cannot be deleted).
///
/// # Parameters
/// - `profile_id`: String ID of the profile to delete.
/// - `profile_name`: Display name used in dialog body text.
/// - `is_active`: Whether this profile is currently active (disables the button).
/// - `window`: Transient parent for the confirmation dialog.
/// - `refresh`: Callback to queue a state reload after a successful delete.
fn build_delete_button(
    profile_id: &str,
    profile_name: &str,
    is_active: bool,
    window: &adw::ApplicationWindow,
    refresh: &Rc<dyn Fn()>,
) -> gtk4::Button {
    let btn = gtk4::Button::builder()
        .icon_name("edit-delete-symbolic")
        .tooltip_text(if is_active { "Cannot delete the active profile" } else { "Delete this profile" })
        .valign(gtk4::Align::Center)
        .build();
    btn.add_css_class("flat");
    btn.add_css_class("circular");
    if !is_active { btn.add_css_class("error"); }
    btn.set_sensitive(!is_active);
    btn.set_widget_name(&format!("delete-{profile_id}"));

    if !is_active {
        let pid_str = profile_id.to_owned();
        let name = profile_name.to_owned();
        let refresh_c = Rc::clone(refresh);
        btn.connect_clicked(glib::clone!(
            @weak window =>
            move |_| {
                let pid2 = pid_str.clone();
                let refresh_c2 = Rc::clone(&refresh_c);
                let body = format!("Delete profile \u{ab}{name}\u{bb}?\n\nThis cannot be undone.");
                show_confirm_dialog(&window, "Delete Profile", &body, "Delete", move || {
                    if let Ok(pid) = pid2.parse::<i64>() {
                        match with_db(|db| db.with_conn(|conn| mantle_core::profile::delete_profile(conn, pid))) {
                            Ok(_) => refresh_c2(),
                            Err(e) => tracing::warn!("delete_profile failed: {e}"),
                        }
                    }
                });
            }
        ));
    }
    btn
}
