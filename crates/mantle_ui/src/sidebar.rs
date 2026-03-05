use adw::prelude::*;
use gtk4::{Box as GtkBox, Label, ListBox, Orientation, ProgressBar, ScrolledWindow, Separator};
use libadwaita as adw;

use crate::state::{AppState, DownloadState};

/// Build the always-visible right sidebar: downloads, profiles, overlay status.
pub fn build(state: &AppState) -> gtk4::ScrolledWindow {
    let scroll = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .width_request(280)
        .build();

    let content = GtkBox::new(Orientation::Vertical, 0);

    content.append(&downloads_section(state));
    content.append(&Separator::new(Orientation::Horizontal));
    content.append(&profiles_section(state));
    content.append(&Separator::new(Orientation::Horizontal));
    content.append(&overlay_section(state));

    scroll.set_child(Some(&content));
    scroll
}

/// Returns (`outer_box`, `inner_box`). Append children to `inner`.
fn section_box(title: &str) -> (GtkBox, GtkBox) {
    let outer = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(8)
        .margin_top(14)
        .margin_bottom(14)
        .margin_start(14)
        .margin_end(14)
        .build();

    let title_label = Label::new(Some(title));
    title_label.add_css_class("heading");
    title_label.add_css_class("caption");
    title_label.set_halign(gtk4::Align::Start);
    outer.append(&title_label);

    let inner = GtkBox::new(Orientation::Vertical, 6);
    outer.append(&inner);

    (outer, inner)
}

fn downloads_section(state: &AppState) -> GtkBox {
    let (outer, inner) = section_box("Downloads");

    for dl in &state.downloads {
        let item = GtkBox::builder().orientation(Orientation::Horizontal).spacing(10).build();

        let info_col = GtkBox::new(Orientation::Vertical, 2);
        info_col.set_hexpand(true);

        let name = Label::new(Some(&dl.name));
        name.add_css_class("caption");
        name.set_halign(gtk4::Align::Start);
        name.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        info_col.append(&name);

        match &dl.state {
            DownloadState::InProgress(progress) => {
                let bar = ProgressBar::new();
                bar.set_fraction(*progress);
                info_col.append(&bar);
                item.append(&info_col);

                let pct = Label::new(Some(&format!("{:.0}%", progress * 100.0)));
                pct.add_css_class("caption");
                pct.add_css_class("dim-label");
                pct.set_valign(gtk4::Align::Center);
                item.append(&pct);
            }
            DownloadState::Complete => {
                let status = Label::new(Some("Complete"));
                status.add_css_class("caption");
                status.add_css_class("success");
                info_col.append(&status);
                item.append(&info_col);

                let icon = gtk4::Image::from_icon_name("emblem-ok-symbolic");
                icon.add_css_class("success");
                icon.set_valign(gtk4::Align::Center);
                item.append(&icon);
            }
            DownloadState::Queued => {
                let status = Label::new(Some("Queued"));
                status.add_css_class("caption");
                status.add_css_class("dim-label");
                info_col.append(&status);
                item.append(&info_col);
            }
            DownloadState::Failed(msg) => {
                let status = Label::new(Some(&format!("Failed: {msg}")));
                status.add_css_class("caption");
                status.add_css_class("error");
                info_col.append(&status);
                item.append(&info_col);
            }
        }

        inner.append(&item);
    }

    outer
}

fn profiles_section(state: &AppState) -> GtkBox {
    let (outer, inner) = section_box("Profiles");

    let list = ListBox::builder().selection_mode(gtk4::SelectionMode::None).build();
    list.add_css_class("boxed-list");

    for profile in &state.profiles {
        let row_content = GtkBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(8)
            .margin_top(6)
            .margin_bottom(6)
            .build();

        // Colored dot indicator
        let dot = Label::new(Some("●"));
        dot.add_css_class("caption");
        if profile.active {
            dot.add_css_class("success");
        } else {
            dot.add_css_class("dim-label");
        }
        row_content.append(&dot);

        let name = Label::new(Some(&profile.name));
        name.set_hexpand(true);
        name.set_halign(gtk4::Align::Start);
        name.add_css_class("caption");
        if profile.active {
            name.add_css_class("accent");
        }
        row_content.append(&name);

        let count = Label::new(Some(&profile.mod_count.to_string()));
        count.add_css_class("caption");
        count.add_css_class("dim-label");
        row_content.append(&count);

        let row = gtk4::ListBoxRow::new();
        row.set_child(Some(&row_content));
        row.set_activatable(true);
        list.append(&row);
    }

    inner.append(&list);
    outer
}

fn overlay_section(state: &AppState) -> GtkBox {
    let (outer, inner) = section_box("Overlay");

    let layer_count = state.mod_count.to_string();
    let rows = [
        ("Status", "idle"),
        ("Backend", state.overlay_backend.as_str()),
        ("Layers", layer_count.as_str()),
        ("Namespace", "isolated"),
    ];

    for (label_text, value_text) in rows {
        let row = GtkBox::builder().orientation(Orientation::Horizontal).build();

        let lbl = Label::new(Some(label_text));
        lbl.add_css_class("caption");
        lbl.add_css_class("dim-label");
        lbl.set_hexpand(true);
        lbl.set_halign(gtk4::Align::Start);

        let val = Label::new(Some(value_text));
        val.add_css_class("caption");

        row.append(&lbl);
        row.append(&val);
        inner.append(&row);
    }

    outer
}
