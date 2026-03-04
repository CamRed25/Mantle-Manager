# Mantle Manager — UI Guide

> **Scope:** GTK4 and libadwaita conventions, adaptive layout requirements, GNOME HIG compliance, widget usage rules, theming, and the boundary between UI and core logic.
> **Last Updated:** Mar 3, 2026

---

## 1. Overview

Mantle Manager uses GTK4 with libadwaita for its user interface. The UI is Linux-native, follows the GNOME Human Interface Guidelines, and must function correctly at 1280×800 (Steam Deck resolution).

**UI layer responsibilities:**
- Display application state provided by `mantle_core`
- Accept user input and dispatch actions to `mantle_core`
- Provide feedback on async operations (progress, errors, notifications)

**UI layer is NOT responsible for:**
- Business logic of any kind
- Mod list ordering rules
- Conflict resolution decisions
- Overlay mount operations
- Any direct I/O

All logic lives in `mantle_core`. Widgets display state and emit actions. This boundary is strict.

---

## 2. Technology Stack

| Library | Version | Purpose |
|---------|---------|---------|
| `gtk4` crate | ~0.8 | Core GTK4 bindings |
| `libadwaita` crate | ~0.6 | GNOME adaptive widgets |
| `glib` | (transitive) | Event loop integration, `MainContext` |
| `gio` | (transitive) | File operations, async I/O |

**Rule:** Use a `libadwaita` component before writing a custom widget. Check the [libadwaita component gallery](https://gnome.pages.gitlab.gnome.org/libadwaita/doc/main/) first.

---

## 3. Application Structure

### 3.1 Window Hierarchy

```
AdwApplicationWindow (main window)
└── AdwToolbarView
    ├── AdwHeaderBar (top)
    │   ├── [profile selector — AdwComboRow or custom]
    │   ├── [search toggle]
    │   └── [menu button]
    └── [content area]
        ├── AdwNavigationSplitView (two-pane layout)
        │   ├── [sidebar — mod list]
        │   └── [content — mod details / tools]
        └── AdwStatusPage (empty state)
```

### 3.2 Application Entry Point

```rust
// src/main.rs
fn main() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id("io.mantlemanager.MantleManager")
        .flags(gio::ApplicationFlags::empty())
        .build();
    app.connect_activate(build_ui);
    app.run()
}
```

Application ID `io.mantlemanager.MantleManager` matches the Flatpak manifest. Do not change without updating packaging.

---

## 4. Adaptive Layout

### 4.1 Minimum Resolution

**1280×800 is the minimum.** Every layout must be verified at this size. No horizontal scrolling, no clipped content, no unreachable controls.

The Steam Deck runs at 1280×800 in Desktop Mode. This is not a target to accommodate eventually — it is a hard requirement for every change.

### 4.2 AdwNavigationSplitView

The two-pane layout (mod list + detail panel) uses `AdwNavigationSplitView`. It automatically collapses to a single panel with navigation on narrow displays:

```rust
let split_view = adw::NavigationSplitView::new();
split_view.set_min_sidebar_width(280.0);
split_view.set_max_sidebar_width(400.0);
split_view.set_sidebar_width_fraction(0.35);
```

At 1280×800, the sidebar and content panel both fit. Do not set fixed widths that would cause overlap at this resolution.

### 4.3 AdwBreakpoint

For content that needs to adapt between small and large displays, use `AdwBreakpoint`:

```rust
let breakpoint = adw::Breakpoint::new(
    adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        900.0,
        adw::LengthUnit::Sp,
    )
);
breakpoint.add_setter(&some_widget, "visible", &false.to_value());
window.add_breakpoint(breakpoint);
```

Use breakpoints for showing/hiding secondary content, not for fundamental layout changes.

---

## 5. libadwaita Components — Usage Rules

### 5.1 Prefer AdwPreferencesPage for Settings

All settings dialogs use `AdwPreferencesPage` → `AdwPreferencesGroup` → `AdwActionRow` / `AdwSwitchRow` / `AdwComboRow`:

```rust
let page = adw::PreferencesPage::new();
let group = adw::PreferencesGroup::builder()
    .title("Overlay Settings")
    .build();

let row = adw::SwitchRow::builder()
    .title("Enable Namespace Isolation")
    .subtitle("Automatically cleans up mounts on crash")
    .build();

group.add(&row);
page.add(&group);
```

Do not build settings UIs with raw `Gtk::Grid` or `Gtk::Box` when `AdwPreferencesPage` applies.

### 5.2 AdwToast for Non-Blocking Notifications

Transient notifications use `AdwToast` via `AdwToastOverlay`:

```rust
let toast = adw::Toast::builder()
    .title("Mod installed successfully")
    .timeout(3)
    .build();
toast_overlay.add_toast(toast);
```

Do not use modal dialogs for informational messages that don't require user action.

### 5.3 AdwStatusPage for Empty States

Empty states (no mods installed, no game selected) use `AdwStatusPage`:

```rust
let status = adw::StatusPage::builder()
    .icon_name("folder-open-symbolic")
    .title("No Mods Installed")
    .description("Drop a mod archive here or click Install to get started.")
    .build();
```

### 5.4 AdwDialog for Modal Actions

Destructive confirmations and multi-step installers use `AdwDialog` (libadwaita 1.5+):

```rust
let dialog = adw::AlertDialog::builder()
    .heading("Remove Mod?")
    .body("This will permanently delete the mod files. This cannot be undone.")
    .build();
dialog.add_response("cancel", "Cancel");
dialog.add_response("remove", "Remove");
dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
dialog.set_default_response("cancel");
dialog.set_close_response("cancel");
```

Always set a default response (usually the safe/cancel action) and a close response.

### 5.5 Progress — AdwProgressBar

Long-running operations (mod installation, archive extraction) show progress via an `AdwProgressBar` in the header or content area. Never block the UI main thread waiting for progress — use `glib::MainContext::channel` to receive progress updates from tokio:

```rust
let (sender, receiver) = glib::MainContext::channel(glib::Priority::DEFAULT);

// In tokio task:
tokio::spawn(async move {
    for progress in 0..=100 {
        sender.send(progress as f64 / 100.0).unwrap();
    }
});

// On GTK main thread:
receiver.attach(None, move |progress| {
    progress_bar.set_fraction(progress);
    glib::ControlFlow::Continue
});
```

---

## 6. Theming

### 6.1 Automatic Dark/Light Mode

GTK4 + libadwaita handle dark/light theme switching automatically via the system color scheme preference. **Do not hardcode colors.** Use semantic color names from the GTK4/libadwaita palette:

```css
/* Forbidden — hardcoded color */
.mod-item { background-color: #2d2d2d; }

/* Correct — semantic color that adapts to theme */
.mod-item { background-color: @card_bg_color; }
```

### 6.2 CSS Custom Properties

Allowed CSS custom properties from libadwaita:

| Property | Use For |
|----------|---------|
| `@window_bg_color` | Window background |
| `@card_bg_color` | Card/list item background |
| `@accent_color` | Highlight, selected state |
| `@destructive_color` | Destructive actions |
| `@success_color` | Success state |
| `@warning_color` | Warning state |
| `@error_color` | Error state |
| `@headerbar_bg_color` | Header bar |

### 6.3 Custom CSS

Load custom CSS via `CssProvider`:

```rust
let provider = gtk::CssProvider::new();
provider.load_from_data(include_str!("style.css"));
gtk::style_context_add_provider_for_display(
    &gdk::Display::default().unwrap(),
    &provider,
    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
);
```

Keep custom CSS minimal. Every custom CSS rule is maintenance burden. Prefer libadwaita components that style themselves.

---

## 7. Accessibility

### 7.1 Minimum Requirements

- Every `GtkButton` and interactive element has an accessible label
- Every image-only button has a tooltip and accessible name:
  ```rust
  button.set_tooltip_text(Some("Install Mod"));
  button.update_property(&[gtk::accessible::Property::Label("Install Mod")]);
  ```
- Keyboard navigation must reach all interactive elements via Tab/Shift-Tab
- Focus indicators must be visible (GTK4 default is acceptable — do not suppress)

### 7.2 Touch Targets

Minimum 44×44 CSS pixels per GNOME HIG. Do not set widget sizes smaller than this for interactive elements.

---

## 8. Icons

Use symbolic icons from the GNOME icon theme. Do not bundle custom icons for actions that have a standard symbolic equivalent.

```rust
let button = gtk::Button::from_icon_name("document-open-symbolic");
```

Standard icons used in Mantle Manager:

| Action | Icon Name |
|--------|----------|
| Install mod | `document-save-symbolic` |
| Remove mod | `edit-delete-symbolic` |
| Enable mod | `object-select-symbolic` |
| Open settings | `preferences-system-symbolic` |
| Launch game | `media-playback-start-symbolic` |
| Refresh | `view-refresh-symbolic` |
| Search | `system-search-symbolic` |
| Conflict | `dialog-warning-symbolic` |

---

## 9. No Business Logic in Widgets

This rule is stated once and applies everywhere:

> **Widgets display state. Widgets emit actions. Widgets do not contain logic.**

Specifically:

```rust
// Forbidden — logic in widget
fn on_enable_clicked(&self) {
    if self.mod_info.is_compatible_with_game(&self.game) { // <-- logic
        self.mod_list.enable(&self.mod_info);
    }
}

// Correct — widget emits action, core decides
fn on_enable_clicked(&self) {
    self.action_sender.send(UiAction::EnableMod(self.mod_info.id.clone()));
    // mantle_core handles compatibility check and enables if valid
}
```

Compatibility checks, conflict detection, priority ordering, and all data transformations live in `mantle_core`. The UI receives the result and displays it.

---

## 10. State Management

### 10.1 UI State Struct

UI state lives in `mantle_ui::state`:

```rust
pub struct AppState {
    pub mod_list: Vec<ModInfo>,        // Snapshot from core
    pub active_profile: String,
    pub game: Option<GameInfo>,
    pub conflicts: Vec<ConflictInfo>,  // Snapshot from core
    pub is_game_running: bool,
    pub install_progress: Option<f64>, // 0.0–1.0 or None
}
```

This is a snapshot — it is updated when `mantle_core` emits state changes. The UI never holds a lock into core state.

### 10.2 State → Widget Binding

Use GTK4 `Property` bindings or `glib::MainContext::channel` for state updates. Do not poll state from a timer.

---

## 11. Cross-References

| Topic | Standard |
|-------|----------|
| Governance and enforcement | [RULE_OF_LAW.md](RULE_OF_LAW.md) |
| UI module structure | [ARCHITECTURE.md §5](ARCHITECTURE.md) |
| GTK4 on core dependency rule | [ARCHITECTURE.md §3.1](ARCHITECTURE.md) |
| Async and GTK4 main loop | [CODING_STANDARDS.md §5.3](CODING_STANDARDS.md) |
| Steam Deck display testing | [TESTING_GUIDE.md §7.1](TESTING_GUIDE.md) |
| Steam Deck platform details | [PLATFORM_COMPAT.md §5](PLATFORM_COMPAT.md) |
