//! Downloads page — full download queue with per-row progress, cancel, and retry.
//!
//! Downloads are grouped into five sections, each rendered only when it
//! contains entries:
//!
//! 1. **In progress** — live progress bar, percentage readout, Cancel button
//! 2. **Queued**       — waiting indicator, Cancel button
//! 3. **Failed**       — error reason, Retry button
//! 4. **Cancelled**    — dim label, Retry button
//! 5. **Completed**    — success icon, individual Clear button
//!
//! A "Clear completed" button in the toolbar removes all completed entries at
//! once.  All buttons are wired to [`DownloadQueue`] operations and call the
//! `refresh` callback so the page state is immediately reflected in the UI.
//!
//! # Empty state
//! When no downloads are queued or active, an [`adw::StatusPage`] is shown.
//!
//! # References
//! - `standards/UI_GUIDE.md` §3, §5.3, §8, §9
//! - `path.md` item a

use std::{cell::RefCell, rc::Rc};

use adw::prelude::*;
use gtk4::{Box as GtkBox, Label, ListBox, Orientation, ProgressBar, ScrolledWindow, Separator};
use libadwaita as adw;
use uuid::Uuid;

use crate::{
    downloads::DownloadQueue,
    state::{DownloadEntry, DownloadStatus},
};

// ─── Public entry point ───────────────────────────────────────────────────────

/// Build the full Downloads page widget tree.
///
/// Returns a vertical [`GtkBox`] suitable for insertion into an
/// [`adw::ViewStack`].
///
/// # Parameters
/// - `entries`  – Point-in-time snapshot from [`DownloadQueue::snapshot`].
/// - `queue`    – Shared queue used by action buttons.
/// - `refresh`  – Callback invoked after every queue mutation to trigger a
///   full UI rebuild.
pub fn build(
    entries: &[DownloadEntry],
    queue: &Rc<RefCell<DownloadQueue>>,
    refresh: &Rc<dyn Fn()>,
) -> GtkBox {
    let outer = GtkBox::builder().orientation(Orientation::Vertical).spacing(0).build();

    outer.append(&toolbar(entries, queue, refresh));
    outer.append(&Separator::new(Orientation::Horizontal));

    if entries.is_empty() {
        outer.append(&empty_state());
    } else {
        outer.append(&download_scroll(entries, queue, refresh));
    }

    outer
}

// ─── Toolbar ─────────────────────────────────────────────────────────────────

/// Top toolbar: active-download count and "Clear completed" button.
///
/// # Parameters
/// - `entries`  – Current download snapshot for count computation.
/// - `queue`    – Shared queue; mutated by the "Clear completed" button.
/// - `refresh`  – Rebuild callback invoked after clear.
fn toolbar(
    entries: &[DownloadEntry],
    queue: &Rc<RefCell<DownloadQueue>>,
    refresh: &Rc<dyn Fn()>,
) -> GtkBox {
    let bar = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .build();

    let active = entries
        .iter()
        .filter(|d| {
            matches!(
                d.state,
                DownloadStatus::InProgress { .. } | DownloadStatus::Queued
            )
        })
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

    let clear_btn = gtk4::Button::builder()
        .label("Clear completed")
        .tooltip_text("Remove all completed downloads from the list")
        .build();
    clear_btn.add_css_class("flat");
    clear_btn.add_css_class("caption");

    // Wire: clear all Complete entries from the queue then rebuild the page.
    let queue_c = Rc::clone(queue);
    let refresh_c = Rc::clone(refresh);
    clear_btn.connect_clicked(move |_| {
        queue_c.borrow_mut().clear_completed();
        refresh_c();
    });

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
///
/// # Parameters
/// - `entries`  – Current download snapshot.
/// - `queue`    – Shared queue forwarded to each row's action buttons.
/// - `refresh`  – Rebuild callback forwarded to each row's action buttons.
fn download_scroll(
    entries: &[DownloadEntry],
    queue: &Rc<RefCell<DownloadQueue>>,
    refresh: &Rc<dyn Fn()>,
) -> ScrolledWindow {
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
    let in_progress: Vec<&DownloadEntry> = entries
        .iter()
        .filter(|d| matches!(d.state, DownloadStatus::InProgress { .. }))
        .collect();
    if !in_progress.is_empty() {
        content.append(&section(
            "In Progress",
            &in_progress,
            |e| render_in_progress_row(e, Rc::clone(queue), Rc::clone(refresh)),
        ));
    }

    // ── Queued ────────────────────────────────────────────────────────────────
    let queued: Vec<&DownloadEntry> = entries
        .iter()
        .filter(|d| matches!(d.state, DownloadStatus::Queued))
        .collect();
    if !queued.is_empty() {
        content.append(&section(
            "Queued",
            &queued,
            |e| render_queued_row(e, Rc::clone(queue), Rc::clone(refresh)),
        ));
    }

    // ── Failed ────────────────────────────────────────────────────────────────
    let failed: Vec<&DownloadEntry> = entries
        .iter()
        .filter(|d| matches!(d.state, DownloadStatus::Failed(_)))
        .collect();
    if !failed.is_empty() {
        content.append(&section(
            "Failed",
            &failed,
            |e| render_failed_row(e, Rc::clone(queue), Rc::clone(refresh)),
        ));
    }

    // ── Cancelled ─────────────────────────────────────────────────────────────
    let cancelled: Vec<&DownloadEntry> = entries
        .iter()
        .filter(|d| matches!(d.state, DownloadStatus::Cancelled))
        .collect();
    if !cancelled.is_empty() {
        content.append(&section(
            "Cancelled",
            &cancelled,
            |e| render_cancelled_row(e, Rc::clone(queue), Rc::clone(refresh)),
        ));
    }

    // ── Completed ─────────────────────────────────────────────────────────────
    let completed: Vec<&DownloadEntry> = entries
        .iter()
        .filter(|d| matches!(d.state, DownloadStatus::Complete { .. }))
        .collect();
    if !completed.is_empty() {
        content.append(&section(
            "Completed",
            &completed,
            |e| render_completed_row(e, Rc::clone(queue), Rc::clone(refresh)),
        ));
    }

    scroll.set_child(Some(&content));
    scroll
}

// ─── Section builder ─────────────────────────────────────────────────────────

/// Build a titled section containing a [`ListBox`] whose rows are constructed
/// by `row_fn`.
///
/// # Parameters
/// - `title`    – Section header text.
/// - `entries`  – Slice of download entries belonging to this section.
/// - `row_fn`   – Closure that turns one `DownloadEntry` into a `ListBoxRow`.
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
/// - `entry`   – Must have `DownloadStatus::InProgress { .. }`.
/// - `queue`   – Queue mutated when Cancel is clicked.
/// - `refresh` – Rebuild callback invoked after cancel.
fn render_in_progress_row(
    entry: &DownloadEntry,
    queue: Rc<RefCell<DownloadQueue>>,
    refresh: Rc<dyn Fn()>,
) -> gtk4::ListBoxRow {
    let progress = match &entry.state {
        DownloadStatus::InProgress { progress, .. } => *progress,
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

    let entry_id = entry.id.clone();
    cancel_btn.connect_clicked(move |_| {
        if let Ok(id) = entry_id.parse::<Uuid>() {
            queue.borrow_mut().cancel(id);
            refresh();
        }
    });

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
/// - `entry`   – Must have `DownloadStatus::Queued`.
/// - `queue`   – Queue mutated when Cancel is clicked.
/// - `refresh` – Rebuild callback invoked after cancel.
fn render_queued_row(
    entry: &DownloadEntry,
    queue: Rc<RefCell<DownloadQueue>>,
    refresh: Rc<dyn Fn()>,
) -> gtk4::ListBoxRow {
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

    let entry_id = entry.id.clone();
    cancel_btn.connect_clicked(move |_| {
        if let Ok(id) = entry_id.parse::<Uuid>() {
            queue.borrow_mut().cancel(id);
            refresh();
        }
    });

    content.append(&cancel_btn);
    make_row(&content)
}

/// Row for a download that failed.
///
/// Shows the error reason and a Retry button.
///
/// # Parameters
/// - `entry`   – Must have `DownloadStatus::Failed(_)`.
/// - `queue`   – Queue mutated when Retry is clicked.
/// - `refresh` – Rebuild callback invoked after retry.
fn render_failed_row(
    entry: &DownloadEntry,
    queue: Rc<RefCell<DownloadQueue>>,
    refresh: Rc<dyn Fn()>,
) -> gtk4::ListBoxRow {
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

    let entry_id = entry.id.clone();
    retry_btn.connect_clicked(move |_| {
        if let Ok(id) = entry_id.parse::<Uuid>() {
            queue.borrow_mut().retry(id);
            refresh();
        }
    });

    top.append(&retry_btn);
    outer.append(&top);

    if let DownloadStatus::Failed(reason) = &entry.state {
        let err_label = Label::new(Some(reason.as_str()));
        err_label.add_css_class("caption");
        err_label.add_css_class("error");
        err_label.set_halign(gtk4::Align::Start);
        outer.append(&err_label);
    }

    make_row(&outer)
}

/// Row for a cancelled download.
///
/// Shows a dim "Cancelled" badge and a Retry button.
///
/// # Parameters
/// - `entry`   – Must have `DownloadStatus::Cancelled`.
/// - `queue`   – Queue mutated when Retry is clicked.
/// - `refresh` – Rebuild callback invoked after retry.
fn render_cancelled_row(
    entry: &DownloadEntry,
    queue: Rc<RefCell<DownloadQueue>>,
    refresh: Rc<dyn Fn()>,
) -> gtk4::ListBoxRow {
    let content = row_content();

    let name = Label::new(Some(&entry.name));
    name.set_hexpand(true);
    name.set_halign(gtk4::Align::Start);
    name.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    content.append(&name);

    let badge = Label::new(Some("Cancelled"));
    badge.add_css_class("caption");
    badge.add_css_class("dim-label");
    badge.set_valign(gtk4::Align::Center);
    content.append(&badge);

    let retry_btn = gtk4::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text("Re-queue download")
        .build();
    retry_btn.add_css_class("flat");
    retry_btn.add_css_class("circular");

    let entry_id = entry.id.clone();
    retry_btn.connect_clicked(move |_| {
        if let Ok(id) = entry_id.parse::<Uuid>() {
            queue.borrow_mut().retry(id);
            refresh();
        }
    });

    content.append(&retry_btn);
    make_row(&content)
}

/// Row for a completed download.
///
/// Shows a success icon and a per-row Clear button.
///
/// # Parameters
/// - `entry`   – Must have `DownloadStatus::Complete { .. }`.
/// - `queue`   – Queue mutated when Clear is clicked.
/// - `refresh` – Rebuild callback invoked after removal.
fn render_completed_row(
    entry: &DownloadEntry,
    queue: Rc<RefCell<DownloadQueue>>,
    refresh: Rc<dyn Fn()>,
) -> gtk4::ListBoxRow {
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

    let entry_id = entry.id.clone();
    clear_btn.connect_clicked(move |_| {
        if let Ok(id) = entry_id.parse::<Uuid>() {
            queue.borrow_mut().remove_completed(id);
            refresh();
        }
    });

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
