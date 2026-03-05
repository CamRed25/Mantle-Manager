//! Plugins & Themes page — two inner sub-tabs sharing the Plugins tab slot.
//!
//! The outer widget is a vertical [`GtkBox`] containing a compact
//! [`gtk4::StackSwitcher`] + [`gtk4::Stack`] that hosts two sub-pages:
//!
//! - **Plugins** — existing list of loaded Mantle plugins with enable/disable
//!   switches and inline settings panels.
//! - **Themes** — all available themes: built-in (Apply only) and
//!   user-installed (Apply + Delete).  Selecting a theme here is the primary
//!   way to change the app look; the Settings › Appearance combo only covers
//!   the three native libadwaita modes.
//!
//! # References
//! - `standards/UI_GUIDE.md` §3, §5.1, §5.3, §9
//! - `standards/PLUGIN_API.md` §6–8
//! - `path.md` item u

use std::rc::Rc;

use adw::prelude::*;
use gtk4::{Box as GtkBox, Label, ListBox, Orientation, ScrolledWindow, Separator, Switch};
use libadwaita as adw;
use mantle_core::config::Theme;

use crate::settings::{apply_theme, builtin_id_to_theme, save_settings};
use crate::state::{AppState, PluginEntry, PluginSettingEntry, ThemeEntry};

// ─── Public entry point ───────────────────────────────────────────────────────

/// Build the full Plugins+Themes page widget tree.
///
/// Returns a vertical [`GtkBox`] containing a compact inner tab switcher
/// (Plugins | Themes) suitable for insertion into the main [`adw::ViewStack`].
///
/// # Parameters
/// - `state`: Read-only application state snapshot.
/// - `window`: Main window; used as transient parent for the theme-delete
///   confirmation dialog.
/// - `refresh`: Callback invoked after a user theme is deleted so the page
///   rebuilds with the updated theme list.
pub fn build(state: &AppState, window: &adw::ApplicationWindow, refresh: &Rc<dyn Fn()>) -> GtkBox {
    let outer = GtkBox::builder().orientation(Orientation::Vertical).spacing(0).build();

    // ── Inner stack switcher ──────────────────────────────────────────────────
    let stack = gtk4::Stack::builder()
        .transition_type(gtk4::StackTransitionType::SlideLeftRight)
        .transition_duration(150)
        .vexpand(true)
        .build();

    let switcher = gtk4::StackSwitcher::builder().stack(&stack).halign(gtk4::Align::Center).build();

    // Thin separator between the switcher and the page content.
    let sep = Separator::new(Orientation::Horizontal);
    sep.add_css_class("spacer");

    outer.append(&switcher_bar(&switcher));
    outer.append(&sep);
    outer.append(&stack);

    // ── Plugins sub-page ─────────────────────────────────────────────────────
    let plugins_page = plugins_subpage(state);
    stack.add_titled(&plugins_page, Some("plugins"), "Plugins");

    // ── Themes sub-page ──────────────────────────────────────────────────────
    let themes_page = themes_subpage(state, window, refresh);
    stack.add_titled(&themes_page, Some("themes"), "Themes");

    outer
}

// ─── Switcher bar ─────────────────────────────────────────────────────────────

/// Wrap the [`gtk4::StackSwitcher`] in a centred bar with top/bottom margins.
fn switcher_bar(switcher: &gtk4::StackSwitcher) -> GtkBox {
    let bar = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .margin_top(8)
        .margin_bottom(4)
        .halign(gtk4::Align::Fill)
        .build();
    bar.append(switcher);
    bar
}

// ═══════════════════════════════════════════════════════════════════════════════
// Plugins sub-page
// ═══════════════════════════════════════════════════════════════════════════════

fn plugins_subpage(state: &AppState) -> GtkBox {
    let page = GtkBox::builder().orientation(Orientation::Vertical).spacing(0).build();
    page.append(&plugins_toolbar(state));
    if state.plugins.is_empty() {
        page.append(&plugins_empty_state());
    } else {
        page.append(&plugin_scroll(state));
    }
    page
}

// ─── Plugins toolbar ─────────────────────────────────────────────────────────

/// Top toolbar showing loaded plugin count.
///
/// The "Open plugins folder" button opens `{data_dir}/plugins/` for dropping
/// new plugin files. Action wired in item y; non-functional in placeholder mode.
fn plugins_toolbar(state: &AppState) -> GtkBox {
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

// ─── Plugins empty state ──────────────────────────────────────────────────────

fn plugins_empty_state() -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name("application-x-executable-symbolic")
        .title("No Plugins Loaded")
        .description("Drop a .so or .rhai file into the plugins folder to install a plugin.")
        .vexpand(true)
        .build()
}

// ─── Scrollable plugin list ───────────────────────────────────────────────────

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
fn plugin_row(entry: &PluginEntry) -> adw::ExpanderRow {
    let row = adw::ExpanderRow::builder()
        .title(&entry.name)
        .subtitle(format!("v{} · {}", entry.version, entry.author))
        .build();
    row.set_widget_name(&entry.id);

    let toggle = Switch::new();
    toggle.set_active(entry.enabled);
    toggle.set_valign(gtk4::Align::Center);
    toggle.set_tooltip_text(Some(if entry.enabled {
        "Disable this plugin"
    } else {
        "Enable this plugin"
    }));
    row.add_suffix(&toggle);

    let desc_row = adw::ActionRow::builder().title(&entry.description).build();
    desc_row.add_css_class("property");
    row.add_row(&desc_row);

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

fn setting_row(setting: &PluginSettingEntry) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(&setting.label).build();
    row.set_widget_name(&setting.key);

    if let Some(desc) = &setting.description {
        row.set_subtitle(desc.as_str());
    }

    let value_label = Label::new(Some(&setting.value));
    value_label.add_css_class("caption");
    value_label.add_css_class("dim-label");
    value_label.set_valign(gtk4::Align::Center);
    row.add_suffix(&value_label);

    row
}

// ═══════════════════════════════════════════════════════════════════════════════
// Themes sub-page
// ═══════════════════════════════════════════════════════════════════════════════

fn themes_subpage(
    state: &AppState,
    window: &adw::ApplicationWindow,
    refresh: &Rc<dyn Fn()>,
) -> GtkBox {
    let page = GtkBox::builder().orientation(Orientation::Vertical).spacing(0).build();
    page.append(&themes_toolbar(state));
    if state.themes.is_empty() {
        page.append(&themes_empty_state());
    } else {
        page.append(&themes_scroll(state, window, refresh));
    }
    page
}

// ─── Themes toolbar ───────────────────────────────────────────────────────────

fn themes_toolbar(state: &AppState) -> GtkBox {
    let bar = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .build();

    let user_count = state.themes.iter().filter(|t| !t.builtin).count();
    let count = Label::new(Some(&format!(
        "{} built-in · {} installed",
        state.themes.iter().filter(|t| t.builtin).count(),
        user_count,
    )));
    count.add_css_class("caption");
    count.add_css_class("dim-label");
    count.set_hexpand(true);
    count.set_halign(gtk4::Align::Start);
    count.set_valign(gtk4::Align::Center);
    bar.append(&count);

    let open_btn = gtk4::Button::builder()
        .icon_name("folder-symbolic")
        .tooltip_text("Open themes folder")
        .build();
    open_btn.add_css_class("flat");
    bar.append(&open_btn);

    bar
}

// ─── Themes empty state ───────────────────────────────────────────────────────

fn themes_empty_state() -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name("applications-graphics-symbolic")
        .title("No Themes")
        .description(
            "Drop a .css file into the themes folder to install a theme.\n\
             An optional theme.toml alongside it provides name and author metadata.",
        )
        .vexpand(true)
        .build()
}

// ─── Scrollable theme list ────────────────────────────────────────────────────

fn themes_scroll(
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
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    // Built-in themes section
    let builtin_entries: Vec<&ThemeEntry> = state.themes.iter().filter(|t| t.builtin).collect();
    if !builtin_entries.is_empty() {
        let builtin_list = ListBox::builder().selection_mode(gtk4::SelectionMode::None).build();
        builtin_list.add_css_class("boxed-list");
        for entry in builtin_entries {
            builtin_list.append(&theme_row(entry, window, refresh));
        }
        content.append(&builtin_list);
    }

    // User-installed themes section
    let user_entries: Vec<&ThemeEntry> = state.themes.iter().filter(|t| !t.builtin).collect();
    if !user_entries.is_empty() {
        let user_label = Label::new(Some("Installed"));
        user_label.add_css_class("heading");
        user_label.set_halign(gtk4::Align::Start);
        user_label.set_margin_top(4);
        content.append(&user_label);

        let user_list = ListBox::builder().selection_mode(gtk4::SelectionMode::None).build();
        user_list.add_css_class("boxed-list");
        for entry in user_entries {
            user_list.append(&theme_row(entry, window, refresh));
        }
        content.append(&user_list);
    }

    scroll.set_child(Some(&content));
    scroll
}

// ─── Theme row ────────────────────────────────────────────────────────────────

/// Build an [`adw::ActionRow`] for a single theme entry.
///
/// Layout:
/// ```text
/// ╭────────────────────────────────────────────────────────────────╮
/// │ [Theme Name]   [author — description]  [Built-in?] [Apply/✓] │
/// │                                                    [Delete?]  │
/// ╰────────────────────────────────────────────────────────────────╯
/// ```
///
/// - Built-in themes: "Built-in" badge + Apply (or "Applied ✓").  No delete.
/// - User themes: Apply (or "Applied ✓") + trash-icon Delete button.
fn theme_row(
    entry: &ThemeEntry,
    window: &adw::ApplicationWindow,
    refresh: &Rc<dyn Fn()>,
) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(&entry.name).build();
    row.set_widget_name(&entry.id);

    // Subtitle: "author — description" (skip empty halves).
    let subtitle = match (entry.author.is_empty(), entry.description.is_empty()) {
        (false, false) => format!("{} — {}", entry.author, entry.description),
        (false, true) => entry.author.clone(),
        (true, false) => entry.description.clone(),
        (true, true) => String::new(),
    };
    if !subtitle.is_empty() {
        row.set_subtitle(&subtitle);
    }

    // "Built-in" badge — shown before the Apply/status widget.
    if entry.builtin {
        let badge = Label::new(Some("Built-in"));
        badge.add_css_class("caption");
        badge.add_css_class("dim-label");
        badge.set_valign(gtk4::Align::Center);
        row.add_suffix(&badge);
    }

    // Apply button or "Applied ✓" indicator.
    if entry.active {
        let active_label = Label::new(Some("Applied ✓"));
        active_label.add_css_class("caption");
        active_label.add_css_class("dim-label");
        active_label.add_css_class("success");
        active_label.set_valign(gtk4::Align::Center);
        row.add_suffix(&active_label);
    } else {
        let apply_btn = gtk4::Button::builder().label("Apply").valign(gtk4::Align::Center).build();
        apply_btn.add_css_class("flat");

        if entry.builtin {
            // Built-in themes map directly to a Theme enum variant.
            let id = entry.id.clone();
            apply_btn.connect_clicked(move |_| {
                if let Some(theme) = builtin_id_to_theme(&id) {
                    apply_theme(&theme, None);
                    let settings_path = mantle_core::config::default_settings_path();
                    if let Ok(mut settings) =
                        mantle_core::config::AppSettings::load_or_default(&settings_path)
                    {
                        settings.ui.theme = theme;
                        save_settings(&settings, &settings_path);
                    }
                }
            });
        } else {
            // User themes use Theme::Custom(id) + inline CSS.
            let id = entry.id.clone();
            let css = entry.css.clone();
            let is_dark = entry.color_scheme != "light";
            apply_btn.connect_clicked(move |_| {
                let theme = Theme::Custom(id.clone());
                apply_theme(&theme, Some((css.as_str(), is_dark)));
                let settings_path = mantle_core::config::default_settings_path();
                if let Ok(mut settings) =
                    mantle_core::config::AppSettings::load_or_default(&settings_path)
                {
                    settings.ui.theme = theme;
                    save_settings(&settings, &settings_path);
                }
            });
        }

        row.add_suffix(&apply_btn);
    }

    // Delete button — user themes only.
    if !entry.builtin {
        let delete_btn = gtk4::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text("Delete theme")
            .valign(gtk4::Align::Center)
            .build();
        delete_btn.add_css_class("flat");

        let id = entry.id.clone();
        let name = entry.name.clone();
        let window_clone = window.clone();
        let refresh_clone = Rc::clone(refresh);
        delete_btn.connect_clicked(move |_| {
            show_delete_theme_dialog(&window_clone, &name, &id, Rc::clone(&refresh_clone));
        });

        row.add_suffix(&delete_btn);
    }

    row
}

// ─── Delete helpers ───────────────────────────────────────────────────────────

/// Show a confirmation dialog before permanently deleting a user theme.
fn show_delete_theme_dialog(
    window: &adw::ApplicationWindow,
    theme_name: &str,
    theme_id: &str,
    refresh: Rc<dyn Fn()>,
) {
    let dialog = adw::MessageDialog::builder()
        .heading("Delete Theme?")
        .body(format!(
            "\"{theme_name}\" will be permanently removed from your themes folder."
        ))
        .transient_for(window)
        .build();

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", "Delete");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let id = theme_id.to_string();
    dialog.connect_response(Some("delete"), move |_, _| {
        delete_theme_files(&id);
        refresh();
    });

    dialog.present();
}

/// Remove a user theme's CSS (and optional TOML manifest) from disk.
///
/// If the deleted theme was the active one, resets the saved preference to
/// `Theme::Auto` and immediately applies the default color scheme so the UI
/// doesn't remain styled with a deleted theme's CSS.
fn delete_theme_files(theme_id: &str) {
    let themes_dir = mantle_core::theme::themes_data_dir(&mantle_core::config::data_dir());
    let css_path = themes_dir.join(format!("{theme_id}.css"));
    let toml_path = themes_dir.join(format!("{theme_id}.toml"));

    if let Err(e) = std::fs::remove_file(&css_path) {
        tracing::warn!("delete_theme: failed to remove {}: {e}", css_path.display());
    }
    // TOML manifest is optional — silently ignore if absent.
    let _ = std::fs::remove_file(&toml_path);

    // If the deleted theme was active, fall back to System Default.
    let settings_path = mantle_core::config::default_settings_path();
    if let Ok(mut settings) = mantle_core::config::AppSettings::load_or_default(&settings_path) {
        if matches!(&settings.ui.theme, Theme::Custom(id) if id == theme_id) {
            settings.ui.theme = Theme::Auto;
            save_settings(&settings, &settings_path);
            apply_theme(&Theme::Auto, None);
        }
    }
}
