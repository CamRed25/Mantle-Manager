use adw::prelude::*;
use gtk4::{Box as GtkBox, Label, ListBox, Orientation, ScrolledWindow, Switch};
use libadwaita as adw;

use crate::state::{AppState, ModEntry};

/// Build the Overview tab content.
pub fn build(state: &AppState) -> gtk4::ScrolledWindow {
    let scroll = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .hexpand(true)
        .build();

    let content = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(16)
        .margin_top(20)
        .margin_bottom(20)
        .margin_start(20)
        .margin_end(20)
        .build();

    content.append(&hero_card(state));

    if state.conflict_count > 0 {
        let banner = conflict_banner(state);
        content.append(&banner);
    }

    content.append(&mod_list_section(state));

    scroll.set_child(Some(&content));
    scroll
}

fn hero_card(state: &AppState) -> GtkBox {
    // Outer box carries the `.card` class (background + border-radius).
    let card = GtkBox::new(Orientation::Vertical, 0);
    card.add_css_class("card");

    // Inner box provides padding.
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

    // Right column: profile selector + launch button
    let right = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(8)
        .valign(gtk4::Align::Center)
        .build();

    let profile_btn = gtk4::Button::with_label(&format!("{}  ▾", state.active_profile));
    profile_btn.add_css_class("flat");
    profile_btn.set_tooltip_text(Some("Switch profile"));
    right.append(&profile_btn);

    let launch_btn = gtk4::Button::with_label(&format!("▶  Launch {}", state.launch_target));
    launch_btn.add_css_class("suggested-action");
    launch_btn.set_tooltip_text(Some(&format!("Launch {}", state.launch_target)));
    right.append(&launch_btn);

    inner.append(&right);
    card.append(&inner);
    card
}

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

fn conflict_banner(state: &AppState) -> adw::Banner {
    adw::Banner::builder()
        .title(format!(
            "{} file conflicts detected — some mods override the same files",
            state.conflict_count
        ))
        .button_label("View conflicts")
        .revealed(true)
        .build()
}

fn mod_list_section(state: &AppState) -> GtkBox {
    let section = GtkBox::new(Orientation::Vertical, 8);

    // Section header row
    let header_row = GtkBox::builder().orientation(Orientation::Horizontal).build();

    let title = Label::new(Some("Active Mods"));
    title.add_css_class("heading");
    title.set_hexpand(true);
    title.set_halign(gtk4::Align::Start);
    header_row.append(&title);

    let view_all = gtk4::Button::with_label(&format!("View all {} →", state.mod_count));
    view_all.add_css_class("flat");
    header_row.append(&view_all);

    section.append(&header_row);

    // Mod rows
    let list = ListBox::builder().selection_mode(gtk4::SelectionMode::Single).build();
    list.add_css_class("boxed-list");

    for entry in &state.mods {
        list.append(&mod_row(entry));
    }

    section.append(&list);
    section
}

fn mod_row(entry: &ModEntry) -> gtk4::ListBoxRow {
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
    toggle.set_tooltip_text(Some(if entry.enabled {
        "Disable mod"
    } else {
        "Enable mod"
    }));
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
    row
}
