//! Plugins page — list of loaded Mantle plugins with enable/disable switches
//! and inline settings panels.
//!
//! Each plugin is displayed as an [`adw::ExpanderRow`]:
//! - Title: plugin name
//! - Subtitle: version and author
//! - Suffix: enable/disable [`gtk4::Switch`]
//! - Expanded: description row + one [`adw::ActionRow`] per setting
//!
//! Plugins with no settings still expand to show the description.
//!
//! # Empty state
//! When no plugins are loaded, an [`adw::StatusPage`] is shown.
//!
//! # References
//! - `standards/UI_GUIDE.md` §3, §5.1, §5.3, §9
//! - `standards/PLUGIN_API.md` §6–8
//! - `path.md` item u

use adw::prelude::*;
use gtk4::{Box as GtkBox, Label, ListBox, Orientation, ScrolledWindow, Switch};
use libadwaita as adw;

use crate::state::{AppState, PluginEntry, PluginSettingEntry};

// ─── Public entry point ───────────────────────────────────────────────────────

/// Build the full Plugins page widget tree.
///
/// Returns a vertical [`GtkBox`] containing either a status page (no plugins)
/// or the scrollable plugin list, suitable for insertion into an
/// [`adw::ViewStack`].
///
/// # Parameters
/// - `state`: Read-only application state snapshot.
pub fn build(state: &AppState) -> GtkBox {
    let outer = GtkBox::builder().orientation(Orientation::Vertical).spacing(0).build();

    outer.append(&toolbar(state));

    if state.plugins.is_empty() {
        outer.append(&empty_state());
    } else {
        outer.append(&plugin_scroll(state));
    }

    outer
}

// ─── Toolbar ─────────────────────────────────────────────────────────────────

/// Top toolbar showing loaded plugin count.
///
/// The "Open plugins folder" button opens `{data_dir}/plugins/` for dropping
/// new plugin files. Action is wired in item y; the button is non-functional
/// in placeholder mode.
///
/// # Parameters
/// - `state`: Provides the current plugin count.
fn toolbar(state: &AppState) -> GtkBox {
    let bar = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .build();

    let count = Label::new(Some(&format!("{} loaded", state.plugins.len())));
    count.add_css_class("caption");
    count.add_css_class("dim-label");
    count.set_hexpand(true);
    count.set_halign(gtk4::Align::Start);
    count.set_valign(gtk4::Align::Center);
    bar.append(&count);

    let open_btn = gtk4::Button::builder()
        .icon_name("folder-symbolic")
        .tooltip_text("Open plugins folder")
        .build();
    open_btn.add_css_class("flat");
    bar.append(&open_btn);

    bar
}

// ─── Empty state ──────────────────────────────────────────────────────────────

/// Status page shown when no plugins are currently loaded.
fn empty_state() -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name("application-x-executable-symbolic")
        .title("No Plugins Loaded")
        .description("Drop a .so or .rhai file into the plugins folder to install a plugin.")
        .vexpand(true)
        .build()
}

// ─── Scrollable plugin list ───────────────────────────────────────────────────

/// Wraps the plugin list in a [`ScrolledWindow`].
///
/// # Parameters
/// - `state`: Source of the plugin list.
fn plugin_scroll(state: &AppState) -> ScrolledWindow {
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

    let list = plugin_list(state);
    content.append(&list);
    scroll.set_child(Some(&content));
    scroll
}

/// Builds the [`ListBox`] containing one expandable row per plugin.
///
/// # Parameters
/// - `state`: Source of the plugins list.
fn plugin_list(state: &AppState) -> ListBox {
    let list = ListBox::builder().selection_mode(gtk4::SelectionMode::None).build();
    list.add_css_class("boxed-list");

    for entry in &state.plugins {
        list.append(&plugin_row(entry));
    }

    list
}

// ─── Plugin row ───────────────────────────────────────────────────────────────

/// Build an [`adw::ExpanderRow`] for a single plugin.
///
/// Layout:
/// ```text
/// ╭──────────────────────────────────────────────────╮
/// │ [Plugin Name]     [v1.2.0 · Author]   [switch] ▸ │
/// ├──────────────────────────────────────────────────┤
/// │  Description text…                               │
/// │  Setting label             current value         │
/// │  …                                               │
/// ╰──────────────────────────────────────────────────╯
/// ```
///
/// The suffix [`Switch`] controls whether the plugin is enabled.
/// All plugins can be expanded regardless of enabled state so the user can
/// inspect settings before enabling.
///
/// # Parameters
/// - `entry`: Plugin data to display.
fn plugin_row(entry: &PluginEntry) -> adw::ExpanderRow {
    let row = adw::ExpanderRow::builder()
        .title(&entry.name)
        .subtitle(format!("v{} · {}", entry.version, entry.author))
        .build();
    // Set widget name from the stable plugin ID so CSS rules and automated
    // tests can target individual plugin rows without relying on title text.
    row.set_widget_name(&entry.id);

    // ── Enable / disable toggle ───────────────────────────────────────────────
    let toggle = Switch::new();
    toggle.set_active(entry.enabled);
    toggle.set_valign(gtk4::Align::Center);
    toggle.set_tooltip_text(Some(if entry.enabled {
        "Disable this plugin"
    } else {
        "Enable this plugin"
    }));
    // UI guide §9: emit action, don't modify state directly.
    // Real toggle handling wired in item y.
    row.add_suffix(&toggle);

    // ── Description sub-row ───────────────────────────────────────────────────
    let desc_row = adw::ActionRow::builder().title(&entry.description).build();
    desc_row.add_css_class("property");
    row.add_row(&desc_row);

    // ── Settings sub-rows ─────────────────────────────────────────────────────
    if entry.settings.is_empty() {
        let no_settings = adw::ActionRow::builder().title("No configurable settings").build();
        no_settings.set_sensitive(false);
        row.add_row(&no_settings);
    } else {
        for setting in &entry.settings {
            row.add_row(&setting_row(setting));
        }
    }

    row
}

// ─── Setting row ──────────────────────────────────────────────────────────────

/// Build an [`adw::ActionRow`] for a single plugin setting.
///
/// Layout:
/// ```text
/// [Setting Label]           [current value]
/// [description if present]
/// ```
///
/// The current value is shown as a right-aligned suffix label.
/// Real editing (click to change value) is deferred to item y.
///
/// # Parameters
/// - `setting`: The setting entry to display.
fn setting_row(setting: &PluginSettingEntry) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(&setting.label).build();
    // Widget name = setting key so item y can look up rows by key when
    // wiring live value changes (e.g. after an AdwDialog confirmation).
    row.set_widget_name(&setting.key);

    if let Some(desc) = &setting.description {
        row.set_subtitle(desc.as_str());
    }

    // Current value badge on the right
    let value_label = Label::new(Some(&setting.value));
    value_label.add_css_class("caption");
    value_label.add_css_class("dim-label");
    value_label.set_valign(gtk4::Align::Center);
    row.add_suffix(&value_label);

    row
}
