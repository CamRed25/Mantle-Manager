# Changelog

All notable changes to Mantle Manager are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.1.0-alpha] — unreleased

First public alpha. Core mod management, VFS overlay, conflict resolution, plugin
system, and download queue persistence are functional. The application is suitable
for testing and development feedback; it is **not** recommended for managing real
game installations.

### Added

#### Core (`mantle_core`)
- **VFS overlay** (`vfs/`) — fuse-overlayfs and kernel overlayfs backends.
  Mods are layered non-destructively; the game installation is never touched.
- **Conflict resolution** (`conflict/`) — priority-ordered conflict map, loser
  pruning with backup, DLL conflict detection for SKSE/F4SE plugins.
- **Archive extraction** (`install/`) — BSA and BA2 extraction via `libarchive`
  FFI; zip/7z/rar via `compress-tools`. Case-folding normalization after install.
- **Diagnostics** (`diag/`) — cosave checker (SKSE/F4SE/xSE); overwrite
  classifier (DynDOLOD, Nemesis, BodySlide, ENB, 13 categories total).
- **Plugin system** (`plugin/`) — native Rust `.so` plugins via `libloading`;
  Rhai scripting plugins with sandboxed engine and deferred-drain subscription
  pattern. `EventBus` with `SubscriptionHandle` RAII unsubscription.
  `PluginRegistry` lifecycle hooks: `subscribe_lifecycle_hooks()` keeps every
  loaded plugin's `PluginContext` snapshot current when `ProfileChanged` and
  `GameLaunching` events fire.
- **Steam integration** (`game/`) — game discovery via `steamlocate`; Proton
  prefix awareness; multi-game support (Skyrim LE/SE/VR, Enderal SE, Fallout 4,
  Fallout NV, Fallout 3, Oblivion, Starfield).
- **Game version detection** (`game/version.rs`) — reads `buildid` from ACF
  manifests or scans the game EXE for `VS_FIXEDFILEINFO` as fallback.
- **SKSE launch target** — detects installed SKSE version at startup and sets
  the Nexus-redirected launch target accordingly (requires `--features net`).
- **Configuration** (`config/`) — `AppSettings` (TOML), `default_settings_path`,
  `default_db_path`, theming (`theme.toml` manifests).
- **Database layer** (`data/`) — SQLite via `rusqlite`; migration runner;
  mod list, profile, mod-file, and download-queue CRUD with full round-trip tests.
  Download queue persisted to SQLite so history survives restarts.
- **DownloadQueue** (`mantle_ui`) — `new_with_db` constructor; enqueue call
  upserts to DB via background thread; terminal status transitions (complete /
  failed / cancelled) update the row.

#### Networking (`mantle_net`, `--features net`)
- HTTP streaming download with rename-on-completion, progress reporting via
  channel, and 404 error surfacing.
- NXM URL parser (`parse_nxm_url`) and resolver (`resolve_nxm`).
- NXM deep-link handler in the UI: registers `x-scheme-handler/nxm`, handles
  `connect_open` from GApplication, resolves CDN URL on background thread.

#### UI (`mantle_ui`)
- GTK4/libadwaita window: overview, mods, plugins, downloads, profiles, and
  settings pages.
- Live state channel: `state_worker` reads DB in background; idle-add loop
  replaces placeholder content on delivery.
- Archive install: file-chooser with zip/7z/rar filter; background extraction;
  `adw::Toast` notifications.
- Profile switcher popover on the overview page.
- Mod version display column.
- Downloads page with enqueue, cancel, retry, clear-completed buttons.
- Settings dialog: paths, appearance (theme picker), network (Nexus API key,
  download directory).
- Launch button: opens `steam://run/<app_id>` via `xdg-open`.
- First-run onboarding: creates Default profile automatically.

#### Distribution
- **Flatpak manifest** (`flatpak/`) — GNOME Platform 47 runtime,
  `rust-stable` SDK extension, Wayland/X11/DRI portals, Steam socket access.
- **`.desktop` file** — `Exec=mantle-manager %U`, `MimeType=x-scheme-handler/nxm`.
- **Application icon** — SVG placeholder (`assets/*.svg`).
- **AppStream metadata** — `*.appdata.xml` with OARS rating, screenshots section,
  developer name, and `<provides><mediatype>x-scheme-handler/nxm</mediatype>` .
- **GPL-3.0 LICENSE** file.
- **CI workflow** (`.github/workflows/ci.yml`) — build, test, clippy (deny
  warnings on stable), rustfmt, and `cargo-deny` license audit on Ubuntu with
  both stable and beta toolchains.

### Known Limitations
- Network downloads require `--features net` (not included in default build).
- The VFS kernel overlayfs backend requires running as root or with
  `CAP_SYS_ADMIN`; the FUSE backend has no privilege requirement.
- Rhai plugin API surface is limited to the first `v1.0.0` feature set.
- No Nexus Mods API pagination or rate-limiting yet.
- Screenshot images in AppStream metadata are placeholders.

---

[0.1.0-alpha]: https://github.com/mantle-manager/mantle-manager/releases/tag/v0.1.0-alpha
