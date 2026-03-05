//! Downloads page — full download queue with per-row progress, cancel, and retry.
//!
//! Downloads are grouped into four sections, each rendered only when it
//! contains entries:
//!
//! 1. **In progress** — live progress bar, percentage readout, Cancel button
//! 2. **Queued**       — waiting indicator, Cancel button
//! 3. **Failed**       — error reason, Retry button
//! 4. **Completed**    — success icon, individual Clear button
//!
//! A "Clear completed" button in the toolbar removes all completed entries at
//! once. Cancel / Retry / Clear actions are wired to real core operations in
//! item y; here the buttons are rendered non-functional.
//!
//! # Empty state
//! When no downloads are queued or active, an [`adw::StatusPage`] is shown.
//!
//! # References
//! - `standards/UI_GUIDE.md` §3, §5.3, §8, §9
//! - `path.md` item v

use adw::prelude::*;
use gtk4::{Box as GtkBox, Label, ListBox, Orientation, ProgressBar, ScrolledWindow, Separator};
use libadwaita as adw;

use crate::state::{AppState, DownloadEntry, DownloadState};

// ─── Public entry point ───────────────────────────────────────────────────────

/// Build the full Downloads page widget tree.
///
/// Returns a vertical [`GtkBox`] suitable for insertion into an
/// [`adw::ViewStack`].
///
/// # Parameters
/// - `state`: Read-only application state snapshot.
pub fn build(state: &AppState) -> GtkBox {
    let outer = GtkBox::builder().orientation(Orientation::Vertical).spacing(0).build();

    outer.append(&toolbar(state));
    outer.append(&Separator::new(Orientation::Horizontal));

    if state.downloads.is_empty() {
        outer.append(&empty_state());
    } else {
        outer.append(&download_scroll(state));
    }

    outer
}

// ─── Toolbar ─────────────────────────────────────────────────────────────────

/// Top toolbar: active-download count and "Clear completed" button.
///
/// # Parameters
/// - `state`: Provides download list for count computation.
fn toolbar(state: &AppState) -> GtkBox {
    let bar = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .build();

    let active = state
        .downloads
        .iter()
        .filter(|d| matches!(d.state, DownloadState::InProgress(_) | DownloadState::Queued))
        .count();

    let count_text = if active == 0 {
        "No active downloads".to_string()
    } else {
        format!("{active} active")
    };
    let count = Label::new(Some(&count_text));
    count.add_css_class("caption");
    count.add_css_class("dim-label");
    count.set_hexpand(true);
    count.set_halign(gtk4::Align::Start);
    count.set_valign(gtk4::Align::Center);
    bar.append(&count);

    // "Clear completed" — action wired in item y
    let clear_btn = gtk4::Button::builder()
        .label("Clear completed")
        .tooltip_text("Remove all completed downloads from the list")
        .build();
    clear_btn.add_css_class("flat");
    clear_btn.add_css_class("caption");
    bar.append(&clear_btn);

    bar
}

// ─── Empty state ──────────────────────────────────────────────────────────────

/// Status page shown when the download queue is empty.
fn empty_state() -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name("folder-download-symbolic")
        .title("No Downloads")
        .description("Downloads queued from Nexus Mods or other sources will appear here.")
        .vexpand(true)
        .build()
}

// ─── Scrollable download list ─────────────────────────────────────────────────

/// Wraps all download sections inside a [`ScrolledWindow`].
fn download_scroll(state: &AppState) -> ScrolledWindow {
    let scroll = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .hexpand(true)
        .build();

    let content = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(16)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    // ── In Progress ───────────────────────────────────────────────────────────
    let in_progress: Vec<&DownloadEntry> = state
        .downloads
        .iter()
        .filter(|d| matches!(d.state, DownloadState::InProgress(_)))
        .collect();
    if !in_progress.is_empty() {
        content.append(&section("In Progress", &in_progress, render_in_progress_row));
    }

    // ── Queued ────────────────────────────────────────────────────────────────
    let queued: Vec<&DownloadEntry> = state
        .downloads
        .iter()
        .filter(|d| matches!(d.state, DownloadState::Queued))
        .collect();
    if !queued.is_empty() {
        content.append(&section("Queued", &queued, render_queued_row));
    }

    // ── Failed ────────────────────────────────────────────────────────────────
    let failed: Vec<&DownloadEntry> = state
        .downloads
        .iter()
        .filter(|d| matches!(d.state, DownloadState::Failed(_)))
        .collect();
    if !failed.is_empty() {
        content.append(&section("Failed", &failed, render_failed_row));
    }

    // ── Completed ─────────────────────────────────────────────────────────────
    let completed: Vec<&DownloadEntry> = state
        .downloads
        .iter()
        .filter(|d| matches!(d.state, DownloadState::Complete))
        .collect();
    if !completed.is_empty() {
        content.append(&section("Completed", &completed, render_completed_row));
    }

    scroll.set_child(Some(&content));
    scroll
}

// ─── Section builder ─────────────────────────────────────────────────────────

/// Build a titled section containing a [`ListBox`] whose rows are constructed
/// by `row_fn`.
///
/// # Parameters
/// - `title`: Section header text.
/// - `entries`: Slice of download entries belonging to this section.
/// - `row_fn`: Function that turns one `DownloadEntry` into a `ListBoxRow`.
fn section(
    title: &str,
    entries: &[&DownloadEntry],
    row_fn: impl Fn(&DownloadEntry) -> gtk4::ListBoxRow,
) -> GtkBox {
    let outer = GtkBox::builder().orientation(Orientation::Vertical).spacing(6).build();

    let header = Label::new(Some(title));
    header.add_css_class("heading");
    header.add_css_class("caption");
    header.set_halign(gtk4::Align::Start);
    outer.append(&header);

    let list = ListBox::builder().selection_mode(gtk4::SelectionMode::None).build();
    list.add_css_class("boxed-list");

    for entry in entries {
        list.append(&row_fn(entry));
    }

    outer.append(&list);
    outer
}

// ─── Row renderers ────────────────────────────────────────────────────────────

/// Row for a download currently in progress.
///
/// Layout:
/// ```text
/// [name]                            [Cancel]
/// [━━━━━━━━━━━━━━━━━━━━━━  67%]
/// ```
///
/// # Parameters
/// - `entry`: Must have `DownloadState::InProgress(_)`.
fn render_in_progress_row(entry: &DownloadEntry) -> gtk4::ListBoxRow {
    let progress = match &entry.state {
        DownloadState::InProgress(p) => *p,
        _ => 0.0,
    };

    let content = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(4)
        .margin_top(10)
        .margin_bottom(10)
        .margin_start(12)
        .margin_end(12)
        .build();

    // Name row + cancel button
    let top = GtkBox::builder().orientation(Orientation::Horizontal).spacing(8).build();

    let name = Label::new(Some(&entry.name));
    name.set_hexpand(true);
    name.set_halign(gtk4::Align::Start);
    name.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    top.append(&name);

    let cancel_btn = gtk4::Button::builder()
        .icon_name("process-stop-symbolic")
        .tooltip_text("Cancel download")
        .build();
    cancel_btn.add_css_class("flat");
    cancel_btn.add_css_class("circular");
    // Set widget name for item-y action dispatch
    cancel_btn.set_widget_name(&format!("cancel-{}", entry.id));
    top.append(&cancel_btn);

    content.append(&top);

    // Progress bar + percentage
    let prog_row = GtkBox::builder().orientation(Orientation::Horizontal).spacing(8).build();

    let bar = ProgressBar::new();
    bar.set_fraction(progress);
    bar.set_hexpand(true);
    bar.set_valign(gtk4::Align::Center);
    prog_row.append(&bar);

    let pct = Label::new(Some(&format!("{:.0}%", progress * 100.0)));
    pct.add_css_class("caption");
    pct.add_css_class("dim-label");
    pct.set_valign(gtk4::Align::Center);
    pct.set_width_request(36);
    pct.set_halign(gtk4::Align::End);
    prog_row.append(&pct);

    content.append(&prog_row);

    make_row(&content)
}

/// Row for a queued-but-not-yet-started download.
///
/// # Parameters
/// - `entry`: Must have `DownloadState::Queued`.
fn render_queued_row(entry: &DownloadEntry) -> gtk4::ListBoxRow {
    let content = row_content();

    let name = Label::new(Some(&entry.name));
    name.set_hexpand(true);
    name.set_halign(gtk4::Align::Start);
    name.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    content.append(&name);

    let badge = Label::new(Some("Queued"));
    badge.add_css_class("caption");
    badge.add_css_class("dim-label");
    badge.set_valign(gtk4::Align::Center);
    content.append(&badge);

    let cancel_btn = gtk4::Button::builder()
        .icon_name("process-stop-symbolic")
        .tooltip_text("Remove from queue")
        .build();
    cancel_btn.add_css_class("flat");
    cancel_btn.add_css_class("circular");
    cancel_btn.set_widget_name(&format!("cancel-{}", entry.id));
    content.append(&cancel_btn);

    make_row(&content)
}

/// Row for a download that failed.
///
/// Shows the error reason and a Retry button.
///
/// # Parameters
/// - `entry`: Must have `DownloadState::Failed(_)`.
fn render_failed_row(entry: &DownloadEntry) -> gtk4::ListBoxRow {
    let outer = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(2)
        .margin_top(10)
        .margin_bottom(10)
        .margin_start(12)
        .margin_end(12)
        .build();

    let top = GtkBox::builder().orientation(Orientation::Horizontal).spacing(8).build();

    let name = Label::new(Some(&entry.name));
    name.set_hexpand(true);
    name.set_halign(gtk4::Align::Start);
    name.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    top.append(&name);

    let retry_btn = gtk4::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text("Retry download")
        .build();
    retry_btn.add_css_class("flat");
    retry_btn.add_css_class("circular");
    retry_btn.set_widget_name(&format!("retry-{}", entry.id));
    top.append(&retry_btn);

    outer.append(&top);

    if let DownloadState::Failed(reason) = &entry.state {
        let err_label = Label::new(Some(reason.as_str()));
        err_label.add_css_class("caption");
        err_label.add_css_class("error");
        err_label.set_halign(gtk4::Align::Start);
        outer.append(&err_label);
    }

    make_row(&outer)
}

/// Row for a completed download.
///
/// Shows a success icon and a per-row Clear button.
///
/// # Parameters
/// - `entry`: Must have `DownloadState::Complete`.
fn render_completed_row(entry: &DownloadEntry) -> gtk4::ListBoxRow {
    let content = row_content();

    let icon = gtk4::Image::from_icon_name("emblem-ok-symbolic");
    icon.add_css_class("success");
    icon.set_pixel_size(16);
    icon.set_valign(gtk4::Align::Center);
    content.append(&icon);

    let name = Label::new(Some(&entry.name));
    name.set_hexpand(true);
    name.set_halign(gtk4::Align::Start);
    name.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    content.append(&name);

    let clear_btn = gtk4::Button::builder()
        .icon_name("edit-delete-symbolic")
        .tooltip_text("Remove from list")
        .build();
    clear_btn.add_css_class("flat");
    clear_btn.add_css_class("circular");
    clear_btn.set_widget_name(&format!("clear-{}", entry.id));
    content.append(&clear_btn);

    make_row(&content)
}

// ─── Small helpers ────────────────────────────────────────────────────────────

/// Create a standard horizontal content box with consistent row margins.
fn row_content() -> GtkBox {
    GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_top(10)
        .margin_bottom(10)
        .margin_start(12)
        .margin_end(12)
        .build()
}

/// Wrap a content widget in a non-activatable [`gtk4::ListBoxRow`].
fn make_row(child: &GtkBox) -> gtk4::ListBoxRow {
    let row = gtk4::ListBoxRow::new();
    row.set_child(Some(child));
    row.set_activatable(false);
    row
}
