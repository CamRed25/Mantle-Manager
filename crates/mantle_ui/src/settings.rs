//! Settings dialog — [`adw::PreferencesWindow`] wired to [`AppSettings`].
//!
//! # Layout
//! - **Appearance**: Color scheme (`adw::ComboRow`), compact mod list
//!   (`adw::SwitchRow`), source separator colors (`adw::SwitchRow`).
//! - **Paths**: Mods directory, downloads directory (`adw::EntryRow` with
//!   apply button; empty = platform default).
//! - **Network**: Nexus Mods API key (`adw::PasswordEntryRow`).
//!
//! All settings are written immediately when the user confirms each change
//! (toggle for switches/combos, Enter/Apply for entry rows).  No explicit
//! "Save" button is required — this follows the libadwaita live-apply
//! pattern (`UI_GUIDE.md` §5.1).
//!
//! # Usage
//! ```no_run
//! let path = mantle_core::config::default_settings_path();
//! let settings = mantle_core::config::AppSettings::load_or_default(&path).unwrap();
//! crate::settings::build_dialog(settings, path).present(Some(&window));
//! ```

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use adw::prelude::*;
use libadwaita as adw;
use mantle_core::config::{AppSettings, Theme};

// ─── Custom-theme CSS palettes ────────────────────────────────────────────────
//
// Each string overrides libadwaita's named colours via `@define-color`.
// The base dark/light mode is set separately through `adw::StyleManager`
// so libadwaita's own widget chrome (switches, progress bars, etc.) renders
// correctly before our overrides are applied.

const CATPPUCCIN_MOCHA_CSS: &str = r"
@define-color window_bg_color  #1E1E2E;
@define-color window_fg_color  #CDD6F4;
@define-color view_bg_color    #181825;
@define-color view_fg_color    #CDD6F4;
@define-color headerbar_bg_color  #181825;
@define-color headerbar_fg_color  #CDD6F4;
@define-color card_bg_color    #313244;
@define-color card_fg_color    #CDD6F4;
@define-color popover_bg_color #313244;
@define-color popover_fg_color #CDD6F4;
@define-color sidebar_bg_color #181825;
@define-color accent_bg_color  #CBA6F7;
@define-color accent_fg_color  #1E1E2E;
@define-color accent_color     #CBA6F7;
";

const CATPPUCCIN_LATTE_CSS: &str = r"
@define-color window_bg_color  #EFF1F5;
@define-color window_fg_color  #4C4F69;
@define-color view_bg_color    #E6E9EF;
@define-color view_fg_color    #4C4F69;
@define-color headerbar_bg_color  #E6E9EF;
@define-color headerbar_fg_color  #4C4F69;
@define-color card_bg_color    #CCD0DA;
@define-color card_fg_color    #4C4F69;
@define-color popover_bg_color #CCD0DA;
@define-color popover_fg_color #4C4F69;
@define-color sidebar_bg_color #E6E9EF;
@define-color accent_bg_color  #8839EF;
@define-color accent_fg_color  #EFF1F5;
@define-color accent_color     #8839EF;
";

const NORD_CSS: &str = r"
@define-color window_bg_color  #2E3440;
@define-color window_fg_color  #ECEFF4;
@define-color view_bg_color    #242933;
@define-color view_fg_color    #ECEFF4;
@define-color headerbar_bg_color  #3B4252;
@define-color headerbar_fg_color  #ECEFF4;
@define-color card_bg_color    #3B4252;
@define-color card_fg_color    #ECEFF4;
@define-color popover_bg_color #434C5E;
@define-color popover_fg_color #ECEFF4;
@define-color sidebar_bg_color #2E3440;
@define-color accent_bg_color  #88C0D0;
@define-color accent_fg_color  #2E3440;
@define-color accent_color     #88C0D0;
";

const SKYRIM_CSS: &str = r"
@define-color window_bg_color  #1A1A24;
@define-color window_fg_color  #E8D5A3;
@define-color view_bg_color    #141418;
@define-color view_fg_color    #E8D5A3;
@define-color headerbar_bg_color  #1F1F2E;
@define-color headerbar_fg_color  #C9A227;
@define-color card_bg_color    #252535;
@define-color card_fg_color    #E8D5A3;
@define-color popover_bg_color #1F1F2E;
@define-color popover_fg_color #E8D5A3;
@define-color sidebar_bg_color #141418;
@define-color accent_bg_color  #C9A227;
@define-color accent_fg_color  #1A1A24;
@define-color accent_color     #C9A227;
";

const FALLOUT_CSS: &str = r"
@define-color window_bg_color  #0A0F0A;
@define-color window_fg_color  #3ECC3E;
@define-color view_bg_color    #060A06;
@define-color view_fg_color    #3ECC3E;
@define-color headerbar_bg_color  #0D130D;
@define-color headerbar_fg_color  #4EEA4E;
@define-color card_bg_color    #0D1A0D;
@define-color card_fg_color    #3ECC3E;
@define-color popover_bg_color #0D130D;
@define-color popover_fg_color #3ECC3E;
@define-color sidebar_bg_color #060A06;
@define-color accent_bg_color  #4EEA4E;
@define-color accent_fg_color  #0A0F0A;
@define-color accent_color     #4EEA4E;
";

// ─── Thread-local CSS provider ────────────────────────────────────────────────

// Holds the currently-active custom CSS provider so we can remove it before
// installing a replacement.  Only one provider is active at a time.
thread_local! {
    static THEME_PROVIDER: RefCell<Option<gtk4::CssProvider>> = const { RefCell::new(None) };
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Build and return a fully-populated [`adw::PreferencesWindow`].
///
/// # Parameters
/// - `settings`: Current application settings; used to populate initial
///   widget values.
/// - `path`: Path to `settings.toml`; written atomically on every value
///   change.
///
/// # Returns
/// A modal-ready `adw::PreferencesWindow`.  Set transient parent and call
/// `.present()` to show it:
/// ```no_run
/// let dialog = crate::settings::build_dialog(settings, path);
/// dialog.set_transient_for(Some(&window));
/// dialog.present();
/// ```
pub fn build_dialog(settings: AppSettings, path: PathBuf) -> adw::PreferencesWindow {
    let shared = Rc::new(RefCell::new(settings));

    let win = adw::PreferencesWindow::builder()
        .title("Settings")
        .modal(true)
        .default_width(550)
        .default_height(600)
        .build();

    win.add(&build_appearance_page(&shared, &path));
    win.add(&build_paths_page(&shared, &path));
    win.add(&build_network_page(shared, path));

    win
}

/// Apply `theme` to the libadwaita style manager immediately.
///
/// Must be called from the GTK main thread.  Called at application startup
/// (before the window is shown) to honour the user's saved preference, and
/// again whenever the theme combo row or the Themes tab changes.
///
/// # Parameters
/// - `theme`: The [`Theme`] variant to activate.
/// - `user_theme`: For [`Theme::Custom`], the `(css, is_dark)` pair resolved
///   from the themes directory.  Pass `None` for all built-in themes.
///
/// # Side Effects
/// - Calls [`adw::StyleManager::set_color_scheme`] to set the dark/light base.
/// - Installs (or removes) a custom [`gtk4::CssProvider`] for palette overrides
///   at `STYLE_PROVIDER_PRIORITY_APPLICATION`.
pub fn apply_theme(theme: &Theme, user_theme: Option<(&str, bool)>) {
    // 1. Base dark/light mode — lets libadwaita render its own chrome correctly.
    let manager = adw::StyleManager::default();
    manager.set_color_scheme(match theme {
        Theme::Auto => adw::ColorScheme::Default,
        Theme::Light | Theme::CatppuccinLatte => adw::ColorScheme::ForceLight,
        Theme::Dark | Theme::CatppuccinMocha | Theme::Nord | Theme::Skyrim | Theme::Fallout => {
            adw::ColorScheme::ForceDark
        }
        Theme::Custom(_) => match user_theme {
            Some((_, true)) => adw::ColorScheme::ForceDark,
            Some((_, false)) => adw::ColorScheme::ForceLight,
            None => adw::ColorScheme::Default,
        },
    });

    // 2. CSS palette override (empty string = remove any existing provider).
    let css: &str = match theme {
        Theme::Auto | Theme::Light | Theme::Dark => "",
        Theme::CatppuccinMocha => CATPPUCCIN_MOCHA_CSS,
        Theme::CatppuccinLatte => CATPPUCCIN_LATTE_CSS,
        Theme::Nord => NORD_CSS,
        Theme::Skyrim => SKYRIM_CSS,
        Theme::Fallout => FALLOUT_CSS,
        Theme::Custom(_) => user_theme.map_or("", |(css, _)| css),
    };
    apply_theme_css(css);
}

/// Install `css` as the active theme CSS provider, removing the previous one.
fn apply_theme_css(css: &str) {
    THEME_PROVIDER.with(|cell| {
        let mut opt = cell.borrow_mut();

        // Remove the old provider before installing a new one.
        if let Some(old) = opt.take() {
            if let Some(display) = gtk4::gdk::Display::default() {
                gtk4::style_context_remove_provider_for_display(&display, &old);
            }
        }

        if css.is_empty() {
            return;
        }

        let provider = gtk4::CssProvider::new();
        provider.load_from_data(css);

        if let Some(display) = gtk4::gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        *opt = Some(provider);
    });
}

// ─── Appearance page ──────────────────────────────────────────────────────────

/// Build the "Appearance" preferences page.
///
/// # Parameters
/// - `shared`: Shared, interior-mutable [`AppSettings`].
/// - `path`: Settings file path; passed to [`save_settings`] on each change.
///
/// # Returns
/// A fully-populated [`adw::PreferencesPage`].
fn build_appearance_page(
    shared: &Rc<RefCell<AppSettings>>,
    path: &std::path::Path,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Appearance")
        .icon_name("applications-graphics-symbolic")
        .build();

    // ── Color scheme ──────────────────────────────────────────────────────
    let scheme_group = adw::PreferencesGroup::builder().title("Color Scheme").build();

    let model = gtk4::StringList::new(&[
        "System default",
        "Light",
        "Dark",
        "Catppuccin Mocha",
        "Catppuccin Latte",
        "Nord",
        "Skyrim",
        "Fallout",
    ]);
    let theme_row = adw::ComboRow::builder()
        .title("Color scheme")
        .subtitle("Override the system color scheme preference")
        .model(&model)
        .build();
    theme_row.set_selected(theme_index(&shared.borrow().ui.theme));
    {
        let shared = Rc::clone(shared);
        let path = path.to_path_buf();
        theme_row.connect_selected_notify(move |row| {
            let theme = theme_from_index(row.selected());
            shared.borrow_mut().ui.theme = theme.clone();
            save_settings(&shared.borrow(), &path);
            // Built-in themes never need user_theme CSS.
            apply_theme(&theme, None);
        });
    }
    scheme_group.add(&theme_row);
    page.add(&scheme_group);

    // ── Mod list display ──────────────────────────────────────────────────
    let list_group = adw::PreferencesGroup::builder().title("Mod List").build();

    let compact_row = adw::SwitchRow::builder()
        .title("Compact layout")
        .subtitle("Use a denser single-line layout for the mod list")
        .active(shared.borrow().ui.compact_mod_list)
        .build();
    {
        let shared = Rc::clone(shared);
        let path = path.to_path_buf();
        compact_row.connect_active_notify(move |row| {
            shared.borrow_mut().ui.compact_mod_list = row.is_active();
            save_settings(&shared.borrow(), &path);
        });
    }
    list_group.add(&compact_row);

    let sep_row = adw::SwitchRow::builder()
        .title("Source separator colors")
        .subtitle("Show colored dividers between mods from different sources")
        .active(shared.borrow().ui.show_separator_colors)
        .build();
    {
        let shared = Rc::clone(shared);
        let path = path.to_path_buf();
        sep_row.connect_active_notify(move |row| {
            shared.borrow_mut().ui.show_separator_colors = row.is_active();
            save_settings(&shared.borrow(), &path);
        });
    }
    list_group.add(&sep_row);
    page.add(&list_group);

    page
}

// ─── Paths page ───────────────────────────────────────────────────────────────

/// Build the "Paths" preferences page.
///
/// Entry rows use an Apply button so that the settings file is only written
/// when the user finishes typing (Enter or the ✓ button), not on every
/// keystroke.
///
/// # Parameters
/// - `shared`: Shared, interior-mutable [`AppSettings`].
/// - `path`: Settings file path; passed to [`save_settings`] on apply.
///
/// # Returns
/// A fully-populated [`adw::PreferencesPage`].
fn build_paths_page(
    shared: &Rc<RefCell<AppSettings>>,
    path: &std::path::Path,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Paths")
        .icon_name("folder-symbolic")
        .build();

    let group = adw::PreferencesGroup::builder()
        .title("Directory Overrides")
        .description("Leave blank to use the platform default locations.")
        .build();

    // ── Mods directory ────────────────────────────────────────────────────
    let mods_row = adw::EntryRow::builder().title("Mods directory").show_apply_button(true).build();
    mods_row.set_text(
        &shared
            .borrow()
            .paths
            .mods_dir
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    {
        let shared = Rc::clone(shared);
        let path = path.to_path_buf();
        mods_row.connect_apply(move |row| {
            let text = row.text();
            shared.borrow_mut().paths.mods_dir = if text.is_empty() {
                None
            } else {
                Some(PathBuf::from(text.as_str()))
            };
            save_settings(&shared.borrow(), &path);
        });
    }
    group.add(&mods_row);

    // ── Downloads directory ───────────────────────────────────────────────
    let dl_row = adw::EntryRow::builder()
        .title("Downloads directory")
        .show_apply_button(true)
        .build();
    dl_row.set_text(
        &shared
            .borrow()
            .paths
            .downloads_dir
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    {
        let shared = Rc::clone(shared);
        let path = path.to_path_buf();
        dl_row.connect_apply(move |row| {
            let text = row.text();
            shared.borrow_mut().paths.downloads_dir = if text.is_empty() {
                None
            } else {
                Some(PathBuf::from(text.as_str()))
            };
            save_settings(&shared.borrow(), &path);
        });
    }
    group.add(&dl_row);

    page.add(&group);
    page
}

// ─── Network page ─────────────────────────────────────────────────────────────

/// Build the "Network" preferences page.
///
/// The API key uses [`adw::PasswordEntryRow`] so the value is masked by
/// default.  The key is saved only when the user presses Enter or the Apply
/// button, never mid-keystroke — appropriate for a credential field.
///
/// # Parameters
/// - `shared`: Shared, interior-mutable [`AppSettings`].
/// - `path`: Settings file path.
///
/// # Returns
/// A fully-populated [`adw::PreferencesPage`].
fn build_network_page(shared: Rc<RefCell<AppSettings>>, path: PathBuf) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Network")
        .icon_name("network-wireless-symbolic")
        .build();

    let group = adw::PreferencesGroup::builder()
        .title("Nexus Mods")
        .description("Your personal API key is used to fetch mod metadata and NXM links.")
        .build();

    // ── API key ───────────────────────────────────────────────────────────
    let key_row = adw::PasswordEntryRow::builder().title("API key").build();
    // show_apply_button is an EntryRow property inherited by PasswordEntryRow.
    key_row.set_show_apply_button(true);
    key_row.set_text(&shared.borrow().network.nexus_api_key);
    {
        key_row.connect_apply(move |row| {
            shared.borrow_mut().network.nexus_api_key = row.text().to_string();
            save_settings(&shared.borrow(), &path);
        });
    }
    group.add(&key_row);
    page.add(&group);

    page
}

// ─── Theme index helpers ──────────────────────────────────────────────────────

/// Map a [`Theme`] variant to its position in the combo-row model.
///
/// [`Theme::Custom`] maps to 0 (System default) so the combo row degrades
/// gracefully when a user theme is active — the user selects custom themes
/// from the Plugins › Themes tab instead.
fn theme_index(theme: &Theme) -> u32 {
    match theme {
        // Custom themes managed via the Themes tab — degrade the combo to
        // "System default" (0) so it does not show a stale built-in selection.
        Theme::Auto | Theme::Custom(_) => 0,
        Theme::Light => 1,
        Theme::Dark => 2,
        Theme::CatppuccinMocha => 3,
        Theme::CatppuccinLatte => 4,
        Theme::Nord => 5,
        Theme::Skyrim => 6,
        Theme::Fallout => 7,
    }
}

/// Map a combo-row selection index back to a [`Theme`] variant.
fn theme_from_index(idx: u32) -> Theme {
    match idx {
        1 => Theme::Light,
        2 => Theme::Dark,
        3 => Theme::CatppuccinMocha,
        4 => Theme::CatppuccinLatte,
        5 => Theme::Nord,
        6 => Theme::Skyrim,
        7 => Theme::Fallout,
        _ => Theme::Auto,
    }
}

// ─── Private helpers ──────────────────────────────────────────────────────────

/// Serialise `settings` to TOML and write atomically to `path`.
///
/// The parent directory is created if absent (covers first-launch scenario
/// where XDG config directory does not yet exist).  Write errors are logged
/// as warnings rather than panicking, so a read-only filesystem degrades
/// gracefully.
///
/// # Parameters
/// - `settings`: Settings to persist.
/// - `path`: Destination path for `settings.toml`.
///
/// # Side Effects
/// Creates `path` and its parent directories.  Temporarily creates
/// `<path>.tmp` during the atomic write.
pub fn save_settings(settings: &AppSettings, path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        // Best-effort: if dir creation fails, the save below will also fail
        // and report the error via tracing::warn. No need to duplicate here.
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = settings.save(path) {
        tracing::warn!("settings: failed to save: {e}");
    }
}
