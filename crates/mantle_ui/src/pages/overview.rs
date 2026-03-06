//! Overview page — dashboard with stat tiles, profile card, mod quick-list,
//! and conflict banner.
//!
//! # Layout (top → bottom)
//! 1. **Hero card** — game name, version/backend, launch button.
//! 2. **Stat tiles** — four equal cards: mods · plugins · conflicts · overlay.
//! 3. **Active Profile** — [`adw::ActionRow`] with profile-switcher suffix.
//! 4. **Conflict banner** — [`adw::Banner`] shown only when conflicts > 0.
//! 5. **Active Mods** — [`adw::PreferencesGroup`] of up to 6 [`adw::ActionRow`]s.
//!
//! # Wired signals
//! - Profile switcher [`gtk4::MenuButton`] → [`profiles::set_active_profile`] → `refresh()`.
//! - "View all →" → `navigate_to_mods()`.
//! - Conflict banner "View conflicts" → `navigate_to_mods()`.
//! - Mod row [`gtk4::Switch`] → [`mod_list::set_mod_enabled`] → `refresh()`.
//!
//! # First-run
//! Empty `state.profiles` shows an [`adw::StatusPage`] until the
//! `state_worker` delivers the bootstrapped Default profile.

use std::rc::Rc;

use adw::prelude::*;
use gtk4::{glib, Align, Box as GtkBox, Label, Orientation, ScrolledWindow, Switch};
use libadwaita as adw;

use mantle_core;

use crate::pages::shared::with_db;
use crate::state::{AppState, DiagnosticEntry, DiagnosticSeverity, ModEntry, ProfileEntry};

// ─── Public entry point ───────────────────────────────────────────────────────

/// Build the Overview tab.
///
/// # Parameters
/// - `state`: Current application state snapshot.
/// - `navigate_to_mods`: Switches the [`adw::ViewStack`] to `"mods"`.
/// - `refresh`: Queues a full DB state reload after any mutation.
pub fn build(
    state: &AppState,
    navigate_to_mods: Rc<dyn Fn()>,
    refresh: &Rc<dyn Fn()>,
    on_launch: &Rc<dyn Fn()>,
) -> gtk4::ScrolledWindow {
    let scroll = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .hexpand(true)
        .build();

    // First-run: show welcome screen on the very first frame before the
    // state_worker delivers the bootstrapped Default profile.
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

    // adw::Clamp keeps content readable on ultra-wide screens.
    let clamp = adw::Clamp::builder().maximum_size(860).tightening_threshold(600).build();

    let content = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(16)
        .margin_top(20)
        .margin_bottom(20)
        .margin_start(16)
        .margin_end(16)
        .build();

    content.append(&hero_card(state, on_launch));
    content.append(&stat_tiles(state));
    content.append(&profile_section(state, refresh));

    if state.conflict_count > 0 {
        content.append(&conflict_banner(state, Rc::clone(&navigate_to_mods)));
    }

    if !state.diagnostics.is_empty() {
        for banner in diag_banners(&state.diagnostics) {
            content.append(&banner);
        }
    }

    content.append(&mod_list_section(state, navigate_to_mods, refresh));

    clamp.set_child(Some(&content));
    scroll.set_child(Some(&clamp));
    scroll
}

// ─── Hero card ────────────────────────────────────────────────────────────────

/// Top banner: game name, version · Steam · backend subtitle, and launch button.
///
/// The launch button is wired to `on_launch` so both the hero card and the
/// header bar button trigger identical VFS mount + `xdg-open` logic.
///
/// # Parameters
/// - `state`: Current application state snapshot.
/// - `on_launch`: Shared launch handler from `build_ui`.
fn hero_card(state: &AppState, on_launch: &Rc<dyn Fn()>) -> GtkBox {
    let card = GtkBox::new(Orientation::Vertical, 0);
    card.add_css_class("card");

    let inner = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(14)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();

    // Game icon
    let icon = gtk4::Image::from_icon_name("input-gaming-symbolic");
    icon.set_pixel_size(44);
    icon.set_valign(Align::Center);
    inner.append(&icon);

    // Info column
    let info = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(3)
        .hexpand(true)
        .valign(Align::Center)
        .build();

    let title = Label::new(Some(&state.game_name));
    title.add_css_class("title-3");
    title.set_halign(Align::Start);
    info.append(&title);

    // Subtitle: version · Steam · backend  (omit version when empty)
    let sub = if state.game_version.is_empty() {
        format!("Steam · {}", state.overlay_backend)
    } else {
        format!("{} · Steam · {}", state.game_version, state.overlay_backend)
    };
    let meta = Label::new(Some(&sub));
    meta.add_css_class("caption");
    meta.add_css_class("dim-label");
    meta.set_halign(Align::Start);
    info.append(&meta);

    inner.append(&info);

    // Launch button — right-aligned
    let launch_label = if state.launch_target.is_empty() {
        "No Game Detected".to_string()
    } else {
        format!("▶  Launch {}", state.launch_target)
    };
    let launch_btn = gtk4::Button::with_label(&launch_label);
    launch_btn.add_css_class("suggested-action");
    launch_btn.set_valign(Align::Center);
    launch_btn.set_sensitive(state.steam_app_id.is_some());
    launch_btn.set_tooltip_text(Some("Launch via Steam"));
    let on_launch = Rc::clone(on_launch);
    launch_btn.connect_clicked(move |_| on_launch());
    inner.append(&launch_btn);

    card.append(&inner);
    card
}

// ─── Stat tiles ───────────────────────────────────────────────────────────────

/// Four equal-width stat tiles: mods · plugins · conflicts · overlay backend.
///
/// Each tile is a `.card` with a large `title-2` number and a dim caption.
/// The conflicts tile uses the `warning` CSS class when `conflict_count > 0`.
fn stat_tiles(state: &AppState) -> GtkBox {
    let row = GtkBox::builder().orientation(Orientation::Horizontal).spacing(8).build();

    row.append(&stat_tile(&state.mod_count.to_string(), "mods", false));
    row.append(&stat_tile(&state.plugin_count.to_string(), "plugins", false));

    let conflict_val = if state.conflict_count > 0 {
        format!("⚠ {}", state.conflict_count)
    } else {
        "0".to_string()
    };
    row.append(&stat_tile(&conflict_val, "conflicts", state.conflict_count > 0));

    // Shorten the backend string so it fits the narrow tile (e.g. "fuse" not "fuse-overlayfs").
    let backend_short = state
        .overlay_backend
        .split_whitespace()
        .next()
        .unwrap_or(&state.overlay_backend);
    row.append(&stat_tile(backend_short, "overlay", false));

    row
}

/// A single stat tile card.
///
/// # Parameters
/// - `value`: Large primary text (the number or short string).
/// - `caption`: Small dim label below the value.
/// - `warning`: Colours the value with the `warning` CSS class when `true`.
fn stat_tile(value: &str, caption: &str, warning: bool) -> GtkBox {
    let card = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .halign(Align::Fill)
        .valign(Align::Center)
        .margin_top(12)
        .margin_bottom(12)
        .build();
    card.add_css_class("card");

    let val = Label::new(Some(value));
    val.add_css_class("title-2");
    if warning {
        val.add_css_class("warning");
    }
    val.set_halign(Align::Center);
    card.append(&val);

    let cap = Label::new(Some(caption));
    cap.add_css_class("caption");
    cap.add_css_class("dim-label");
    cap.set_halign(Align::Center);
    card.append(&cap);

    card
}

// ─── Profile section ──────────────────────────────────────────────────────────

/// [`adw::PreferencesGroup`] showing the active profile as an [`adw::ActionRow`]
/// with mod/plugin count subtitle and a profile-switcher [`gtk4::MenuButton`]
/// in the suffix slot.
fn profile_section(state: &AppState, refresh: &Rc<dyn Fn()>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title("Active Profile").build();

    // Resolve the active profile entry; fall back to state fields if not found.
    let active = state.profiles.iter().find(|p| p.active).or_else(|| state.profiles.first());
    let (name, mod_count) = active.map_or((state.active_profile.as_str(), state.mod_count), |p| {
        (p.name.as_str(), p.mod_count)
    });

    let row = adw::ActionRow::builder()
        .title(name)
        .subtitle(format!(
            "{} mod{} · {} plugin{}",
            mod_count,
            if mod_count == 1 { "" } else { "s" },
            state.plugin_count,
            if state.plugin_count == 1 { "" } else { "s" },
        ))
        .build();

    let switcher = profile_switcher_button(state, refresh);
    switcher.set_valign(Align::Center);
    row.add_suffix(&switcher);
    row.set_activatable_widget(Some(&switcher));

    group.add(&row);
    group
}

// ─── Profile switcher popover ─────────────────────────────────────────────────

/// Icon [`gtk4::MenuButton`] whose popover lists all profiles.
///
/// Selecting a row calls [`profiles::set_active_profile`] and `refresh()`.
fn profile_switcher_button(state: &AppState, refresh: &Rc<dyn Fn()>) -> gtk4::MenuButton {
    let btn = gtk4::MenuButton::builder()
        .icon_name("view-list-symbolic")
        .tooltip_text("Switch profile")
        .build();
    btn.add_css_class("flat");

    let list = gtk4::ListBox::builder().selection_mode(gtk4::SelectionMode::None).build();
    list.add_css_class("boxed-list");

    for profile in &state.profiles {
        list.append(&profile_popover_row(profile, &btn, refresh));
    }

    let wrapper = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    wrapper.append(&list);

    let popover = gtk4::Popover::new();
    popover.set_child(Some(&wrapper));
    btn.set_popover(Some(&popover));
    btn
}

/// A single row in the profile switcher popover.
///
/// Activating it calls [`profiles::set_active_profile`], closes the popover,
/// and queues a state reload.
fn profile_popover_row(
    profile: &ProfileEntry,
    menu_btn: &gtk4::MenuButton,
    refresh: &Rc<dyn Fn()>,
) -> gtk4::ListBoxRow {
    let row_box = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(10)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(8)
        .margin_end(8)
        .build();

    // Active checkmark
    let check = Label::new(Some(if profile.active { "✓" } else { "   " }));
    check.add_css_class("caption");
    if profile.active {
        check.add_css_class("accent");
    }
    row_box.append(&check);

    let name = Label::new(Some(&profile.name));
    name.set_hexpand(true);
    name.set_halign(Align::Start);
    row_box.append(&name);

    let count = Label::new(Some(&format!("{} mods", profile.mod_count)));
    count.add_css_class("caption");
    count.add_css_class("dim-label");
    row_box.append(&count);

    let row = gtk4::ListBoxRow::new();
    row.set_child(Some(&row_box));
    row.set_activatable(true);

    let profile_id: i64 = profile.id.parse().unwrap_or(0);
    let refresh_c = Rc::clone(refresh);
    let btn_c = menu_btn.clone();

    row.connect_activate(move |_| {
        btn_c.popdown();
        match with_db(|db| {
            db.with_conn(|conn| mantle_core::data::profiles::set_active_profile(conn, profile_id))
        }) {
            Ok(()) => refresh_c(),
            Err(e) => tracing::warn!("overview: set_active_profile failed: {e}"),
        }
    });

    row
}

// ─── Conflict banner ──────────────────────────────────────────────────────────

/// [`adw::Banner`] shown only when `conflict_count > 0`.
///
/// "View conflicts" calls `navigate_to_mods()`.
fn conflict_banner(state: &AppState, navigate_to_mods: Rc<dyn Fn()>) -> adw::Banner {
    let banner = adw::Banner::builder()
        .title(format!(
            "{} file conflict{} detected — some mods override the same files",
            state.conflict_count,
            if state.conflict_count == 1 { "" } else { "s" },
        ))
        .button_label("View conflicts")
        .revealed(true)
        .build();
    banner.connect_button_clicked(move |_| navigate_to_mods());
    banner
}

// ─── Diagnostic banners ─────────────────────────────────────────────────────

/// Build one [`adw::Banner`] per diagnostic entry, warnings first.
///
/// Each banner uses:
/// - `"warning"` CSS class for [`DiagnosticSeverity::Warning`].
/// - `"accent"` CSS class for [`DiagnosticSeverity::Info`].
/// - Tooltip set to `entry.detail` when present.
///
/// # Parameters
/// - `entries`: Ordered slice of entries (pre-sorted by `state_worker`).
///
/// # Returns
/// A `Vec<adw::Banner>` — one element per entry, may be appended to any container.
fn diag_banners(entries: &[DiagnosticEntry]) -> Vec<adw::Banner> {
    entries.iter().map(diag_banner).collect()
}

/// Build a single [`adw::Banner`] for one [`DiagnosticEntry`].
///
/// # Parameters
/// - `entry`: The diagnostic to render.
fn diag_banner(entry: &DiagnosticEntry) -> adw::Banner {
    let banner = adw::Banner::builder()
        .title(&entry.title)
        .revealed(true)
        .build();

    match entry.severity {
        DiagnosticSeverity::Warning => banner.add_css_class("warning"),
        DiagnosticSeverity::Info    => banner.add_css_class("accent"),
    }

    if let Some(detail) = &entry.detail {
        banner.set_tooltip_text(Some(detail.as_str()));
    }

    banner
}

// ─── Active mod list ──────────────────────────────────────────────────────────

/// [`adw::PreferencesGroup`] with up to 6 [`adw::ActionRow`] mod entries.
///
/// Header suffix: "View all N →" flat button wired to `navigate_to_mods()`.
fn mod_list_section(
    state: &AppState,
    navigate_to_mods: Rc<dyn Fn()>,
    refresh: &Rc<dyn Fn()>,
) -> adw::PreferencesGroup {
    let view_all = gtk4::Button::with_label(&format!("View all {} →", state.mod_count));
    view_all.add_css_class("flat");
    view_all.connect_clicked(move |_| navigate_to_mods());

    let group = adw::PreferencesGroup::builder().title("Active Mods").build();
    group.set_header_suffix(Some(&view_all));

    for entry in state.mods.iter().take(6) {
        group.add(&mod_action_row(entry, refresh));
    }

    group
}

/// [`adw::ActionRow`] for a single mod.
///
/// - **Prefix**: enable/disable [`Switch`].
/// - **Title**: mod name (ellipsized).
/// - **Subtitle**: version string, if known.
/// - **Suffix**: `⚠ conflict` warning label when `has_conflict`.
///
/// Switch `state-set` → [`mod_list::set_mod_enabled`] → `refresh()`.
fn mod_action_row(entry: &ModEntry, refresh: &Rc<dyn Fn()>) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(&entry.name).build();

    if let Some(ver) = &entry.version {
        row.set_subtitle(ver.as_str());
    }

    // Prefix: enable/disable switch
    let toggle = Switch::new();
    toggle.set_active(entry.enabled);
    toggle.set_valign(Align::Center);
    toggle.set_tooltip_text(Some(if entry.enabled {
        "Disable mod"
    } else {
        "Enable mod"
    }));
    row.add_prefix(&toggle);
    row.set_activatable_widget(Some(&toggle));

    // Suffix: conflict badge
    if entry.has_conflict {
        let badge = Label::new(Some("⚠ conflict"));
        badge.add_css_class("caption");
        badge.add_css_class("warning");
        badge.set_valign(Align::Center);
        row.add_suffix(&badge);
    }

    // Wire switch → DB → refresh
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
