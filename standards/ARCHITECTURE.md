# Mantle Manager — Architecture

> **Scope:** Crate structure, module organization, dependency graph, and system design.
> **Last Updated:** Mar 3, 2026

---

## 1. Design Philosophy

Mantle Manager is a ground-up Linux-native mod manager. There is no upstream codebase to inherit from and no compatibility layer to maintain. Every architectural decision is deliberate.

**Core principles:**

- **One language boundary** — Rust top to bottom except the GTK4 C library underneath `gtk4-rs`. No Python layer, no C++ layer, no proxy needed.
- **Extract, don't anticipate** — Crates are extracted when the boundary is proven by real code, not when it seems like a good idea upfront.
- **Core owns infrastructure** — VFS, overlay mounting, conflict resolution, and archive handling are core responsibilities, not plugins.
- **Plugins extend, don't replace** — Plugins add behavior through the event bus and `PluginContext`. They do not reach into core internals.
- **Flatpak-first** — Every feature is designed to work inside the Flatpak sandbox. Host-escape via `flatpak-spawn --host` is a documented fallback, not the primary path.

---

## 2. Workspace Structure

### 2.1 Day-One Crates

The project starts with two crates. Additional crates are extracted when the boundary is proven by real code.

```
mantle-manager/
├── Cargo.toml              ← workspace root
├── crates/
│   ├── mantle_core/        ← VFS, overlay, conflict resolution, archive handling
│   └── mantle_ui/          ← GTK4 application, depends on mantle_core
├── standards/              ← All .md standards documents
├── doa/                    ← Archived code (never deleted)
├── futures.md
├── cleanup.md
└── conflict.md
```

### 2.2 Planned Future Crates

These crates are **not scaffolded yet**. They are documented here so the intended boundaries are recorded. Each is extracted from `mantle_core` when the complexity justifies separation.

| Crate | Extracted From | Trigger for Extraction |
|-------|---------------|----------------------|
| `mantle_archive` | `mantle_core::archive` | BSA/BA2 + libarchive scope is substantial enough to justify own crate |
| `mantle_data` | `mantle_core::data` | SQLite complexity + migration system warrants isolation |
| `mantle_plugin` | `mantle_core::plugin` | Plugin system is feature-complete and boundary is stable |
| `mantle_net` | New crate | Nexus API integration begins; compile-time optional |

**Rule:** Do not extract a crate speculatively. Extract when the module inside `mantle_core` has grown to the point where its internal dependencies and test surface are clearly distinct from the rest of core.

---

## 3. Dependency Graph

### 3.1 Current (Day-One)

```
mantle_ui
    ├── mantle_core
    └── gtk4, libadwaita (UI layer only — never in mantle_core)

mantle_core
    ├── tokio (async runtime)
    ├── rayon (parallel CPU work)
    ├── rusqlite (SQLite)
    ├── serde / serde_json / toml (config + data)
    ├── thiserror / anyhow (error handling)
    ├── tracing / tracing-subscriber (logging)
    ├── nix (POSIX/Linux syscalls)
    ├── notify (filesystem watching)
    ├── steamlocate (Steam installation discovery)
    ├── esplugin (ESP/ESM/ESL parsing)
    ├── xxhash-rust (XXH3 hashing)
    ├── semver (version comparison)
    └── [C FFI] libarchive, libloot
```

### 3.2 Future (Post-Extraction)

```
mantle_ui
    ├── mantle_core
    ├── mantle_plugin
    └── mantle_net (optional feature flag)

mantle_core
    ├── mantle_archive
    └── mantle_data

mantle_plugin
    └── mantle_core (PluginContext, event bus types only)

mantle_archive
    └── ba2 (pure Rust), libarchive (zip/7z/rar via C FFI)

mantle_data
    └── rusqlite, serde

mantle_net (optional)
    └── reqwest, tokio, serde_json
```

**Dependency direction rules:**
- `mantle_ui` may depend on `mantle_core` and `mantle_plugin`. Never the reverse.
- `mantle_plugin` may depend on `mantle_core` for type definitions only. Never full core internals.
- `mantle_core` does not depend on `mantle_ui`.
- `mantle_net` is always optional — `mantle_core` and `mantle_ui` must build and run fully without it.

---

## 4. mantle_core — Module Structure

### 4.1 Top-Level Modules

```
mantle_core/
├── src/
│   ├── lib.rs
│   ├── vfs/            ← Virtual filesystem — overlay mounting, backend selection
│   ├── archive/        ← BSA/BA2/zip/7z handling (future: mantle_archive)
│   ├── conflict/       ← Conflict detection, resolution, DLL collision checks
│   ├── game/           ← Game discovery, Steam integration, Proton detection
│   ├── mod_list/       ← Mod list state, priority ordering, activation
│   ├── profile/        ← Profile management, load order persistence
│   ├── data/           ← SQLite layer, schema, migrations (future: mantle_data)
│   ├── plugin/         ← Plugin loading, PluginContext, event bus (future: mantle_plugin)
│   ├── install/        ← Post-extraction pipeline: case folding, BSA extraction
│   ├── diag/           ← Post-session diagnostics: cosave checks, overwrite scan
│   ├── config/         ← TOML config files, settings persistence
│   └── error.rs        ← Crate-level error types (thiserror)
```

### 4.2 vfs/ — Virtual Filesystem

The VFS module is the most critical component. It implements a three-tier backend:

```
vfs/
├── mod.rs              ← Public interface; re-exports select_backend(), mount(), mount_with()
├── detect.rs           ← Environment probes (Flatpak, kernel version, binary availability)
├── types.rs            ← Shared parameter types (MountParams, BackendKind, etc.)
├── mount.rs            ← MountHandle lifecycle, mount_with() dispatch
├── namespace.rs        ← Mount namespace isolation via unshare(CLONE_NEWNS)
├── stacking.rs         ← Nested overlay stacking for mod lists > 480 mods
├── cleanup.rs          ← Stale-mount recovery scan on startup
└── backend/
    ├── mod.rs          ← Backend selection logic and BackendKind enum
    ├── kernel.rs       ← fsopen/fsconfig new mount API (kernel 6.6+)
    ├── fuse.rs         ← fuse-overlayfs fallback (kernel 5.11+)
    └── symlink.rs      ← Symlink farm last resort
```

**Backend selection logic:**

```
is_flatpak()
    → fuse.rs regardless of kernel version
      (new mount API requires host kernel access, unavailable in sandbox)

NOT is_flatpak() AND kernel >= 6.6 AND has_new_mount_api()
    → kernel.rs (native, zero FUSE overhead)

NOT is_flatpak() AND (kernel >= 5.11 OR fuse_overlayfs_available())
    → fuse.rs (fuse-overlayfs, rootless)

fallback
    → symlink.rs (no kernel or FUSE dependency)
```

> Note: `is_flatpak()` is checked first. A machine running kernel 6.8 inside Flatpak takes the fuse.rs path — the sandbox is the constraint, not the kernel version.

> **Full VFS design, mount lifecycle, and performance characteristics:** [VFS_DESIGN.md](VFS_DESIGN.md)

### 4.3 archive/ — Archive Handling

```
archive/
├── mod.rs              ← Public Archive interface
├── bsa.rs              ← BSA/BA2 — ba2-rs crate or libbsarch FFI
├── zip.rs              ← ZIP via libarchive
├── sevenz.rs           ← 7z via libarchive
├── rar.rs              ← RAR via libarchive (extraction only)
└── detect.rs           ← Magic byte detection, format identification
```

**Format coverage:**

| Format | Backend | Versions Covered |
|--------|---------|-----------------|
| BSA (Morrowind) | ba2 | `tes3` module — distinct format, separate parser |
| BSA (Oblivion/Skyrim LE) | ba2 | `tes4` v103 (TES4), v104 (FO3/FNV/TES5) |
| BSA (Skyrim SE/AE) | ba2 | `tes4` v105 (`Version::SSE`) |
| BA2 (Fallout 4) | ba2 | `fo4` v1 — Format::GNRL, Format::DX10 |
| BA2 (Fallout 4 next-gen) | ba2 | `fo4` v7, v8 (next-gen update) |
| BA2 (Starfield) | ba2 | `fo4` v2/v3 — LZ4 compressed |
| ZIP | libarchive | All standard ZIP variants |
| 7z | libarchive | All 7z variants including LZMA2 |
| RAR | libarchive | RAR4, RAR5 (extraction only — no creation) |

> Note: TES3 (Morrowind) BSA is a completely different binary format from TES4+. These are distinct parsers, not one BSA parser covering all versions. ba2 v3.x covers all rows above — decision closed 2026-03-03.

### 4.4 conflict/ — Conflict Detection

```
conflict/
├── mod.rs              ← Public ConflictMap interface
├── detector.rs         ← File-level conflict scanning across mod layers
├── dll.rs              ← SKSE/F4SE/xNVSE DLL collision detection
├── address_lib.rs      ← Address Library version mismatch detection
└── resolution.rs       ← Conflict resolution rules, winner/loser tracking
```

### 4.5 game/ — Game Discovery

```
game/
├── mod.rs              ← GameInfo struct, public interface
├── steam.rs            ← steamlocate integration, library scanning
├── proton.rs           ← Proton prefix detection, DLL override config
├── registry.rs         ← Wine/Proton registry reader (system.reg / user.reg)
└── games.rs            ← All 10 supported game definitions (flat file; compact structs)
```

> **Deferred:** Splitting `games.rs` into a `games/` subdirectory is tracked in `futures.md`.
> See `conflict.md` CONFLICT-003. `registry.rs` is implemented — CONFLICT-003 deferred note
> for that file is superseded.

### 4.6 install/ — Mod Installation Pipeline

Post-extraction processing that runs after an archive is unpacked into the mods directory:

```
install/
├── mod.rs              ← Re-exports for case_fold and bsa sub-modules
├── case_fold.rs        ← Rename all loose files to lowercase (Linux FS compatibility)
└── bsa.rs              ← Extract embedded BSA/BA2 archives inside mod directories
```

### 4.7 diag/ — Post-Session Diagnostics

Checks that run after a game session ends or on demand:

```
diag/
├── mod.rs              ← Re-exports for cosave and overwrite sub-modules
├── cosave.rs           ← Detect saves missing script-extender cosaves (SKSE/xSE)
└── overwrite.rs        ← Classify files written to the VFS upper directory
```

### 4.8 plugin/ — Plugin System

```
plugin/
├── mod.rs              ← Plugin registry, loader, lifecycle
├── context.rs          ← PluginContext — the only sanctioned plugin↔core interface
├── event.rs            ← ModManagerEvent enum, EventBus
├── native.rs           ← Native .so plugin loading via libloading
├── scripted.rs         ← Rhai scripting engine integration
└── sandbox.rs          ← Rhai sandbox — what scripts can and cannot access
```

> **Full plugin contract, PluginContext API, and event definitions:** [PLUGIN_API.md](PLUGIN_API.md)

---

## 5. mantle_ui — Module Structure

```
mantle_ui/
├── src/
│   ├── main.rs         ← Application entry point; GTK4 app init
│   ├── window.rs       ← Main AdwApplicationWindow — install, launch, drag-drop
│   ├── sidebar.rs      ← Navigation sidebar (AdwNavigationPage list)
│   ├── state.rs        ← AppState snapshot struct; placeholder builder
│   ├── state_worker.rs ← Background thread that builds AppState from mantle_core
│   ├── settings.rs     ← Settings dialog (AdwPreferencesWindow)
│   └── pages/
│       ├── mod.rs      ← Page registry / shared page helpers
│       ├── overview.rs ← Dashboard — game info, launch button, stats
│       ├── mods.rs     ← Mod list page — enable/disable, priority, install
│       ├── profiles.rs ← Profile management page — create/rename/delete/activate
│       ├── plugins.rs  ← Plugin list page — installed plugins and their settings
│       └── downloads.rs← Download queue page (in-memory; DB persistence deferred)
```

**UI rules:**
- No business logic in widgets — all logic lives in `mantle_core`.
- Every widget must function at 1280×800 (Steam Deck resolution).
- Use `libadwaita` components before writing custom widgets.
- Dark/light theme handled by GTK4 automatically — do not hardcode colors.

> **Full GTK4 conventions, adaptive layout rules, and component usage:** [UI_GUIDE.md](UI_GUIDE.md)

---

## 6. Data Flow

### 6.1 Game Launch Sequence

```
User clicks Launch
    │
    ▼
mantle_ui: emit launch request
    │
    ▼
mantle_core::plugin: fire ModManagerEvent::GameLaunching(game_info)
    │   ├── Plugin: SKSEInstaller → pre-flight checks
    │   └── Plugin: any registered GameLaunching handlers
    │
    ▼
mantle_core::conflict: scan for DLL collisions, Address Library mismatch
    │   └── warnings surfaced to UI if found
    │
    ▼
mantle_core::vfs: select backend, build lower dirs from active mod list
    │   ├── kernel 6.6+ → kernel.rs
    │   ├── fuse-overlayfs → fuse.rs
    │   └── fallback → symlink.rs
    │
    ▼
mantle_core::vfs: mount overlay
    │   └── namespace isolation if available
    │
    ▼
game process launched against merged view
    │
    ▼
game exits
    │
    ▼
mantle_core::plugin: fire ModManagerEvent::GameExited(game_info, exit_code)
    │
    ▼
mantle_core::vfs: unmount overlay (or namespace auto-cleanup)
```

### 6.2 Mod Install Sequence

```
User drops archive / selects file
    │
    ▼
mantle_core::archive: detect format, validate integrity
    │
    ▼
mantle_core::archive: extract to staging directory
    │
    ▼
mantle_ui: show install options (FOMOD if present, flat install otherwise)
    │
    ▼
mantle_core::mod_list: register mod, assign priority
    │
    ▼
mantle_core::data: persist mod metadata to SQLite
    │
    ▼
mantle_core::plugin: fire ModManagerEvent::ModInstalled(mod_info)
    │
    ▼
mantle_core::conflict: rescan conflict map for affected mods
```

---

## 7. Platform Support

| Platform | Status | Notes |
|----------|--------|-------|
| Fedora 40+ | Primary | Development platform |
| SteamOS 3.x | Primary target | Kernel 6.1 baseline, Flatpak required |
| Ubuntu 24.04 | Supported | kernel 6.8, native overlayfs |
| Arch Linux | Supported | Rolling, always current kernel |
| NixOS | Best effort | Flatpak path tested |

> **Full distro matrix, kernel gates, and compatibility notes:** [PLATFORM_COMPAT.md](PLATFORM_COMPAT.md)

---

## 8. Key Design Decisions

### 8.1 No Proxy Layer

The MO2 Linux port required a Rust proxy daemon as a bridge between C++ and Python. Mantle Manager has no such boundary — everything is Rust. The proxy architecture patterns (SHM, bincode, UDS) are carried forward as knowledge but not as code.

### 8.2 ba2 (pure Rust) — decision closed 2026-03-03

**Decision: use the `ba2` crate (pure Rust, no FFI).** Format coverage verified against ba2 v3.0.1:

| ba2 module | Coverage |
|-----------|----------|
| `tes3` | TES3 (Morrowind) BSA |
| `tes4` | v103/v104/v105 — Oblivion through Skyrim SE |
| `fo4` | v1 (FO4 GNRL + DX10), v2/v3 (Starfield LZ4), v7/v8 (FO4 next-gen) |

libbsarch FFI is not needed. `ba2` is a port of the authoritative C++ bsa library and shares its test suite. The `ba2` crate uses `Format::GNRL`, `Format::DX10`, and `Format::GNMF` for ba2 archive types; Starfield archives use LZ4 (not Zstandard). See `futures.md` Completed section.

### 8.3 Rhai + Native Plugin Hybrid

Simple plugins use Rhai scripting — safe sandbox, no unsafe code possible from scripts, no compilation step for users writing simple automations. Performance-critical or system-level plugins use native `.so` loading via `libloading`.

Both plugin types interact with core exclusively through `PluginContext` — there is no alternative interface. The distinction is which **capabilities** are available through `PluginContext`:

- **Rhai scripts** receive a restricted `PluginContext` — read access to mod list, game info, event subscription, and download queue. No filesystem access outside the plugin data directory, no direct SQLite access.
- **Native plugins** receive an extended `PluginContext` with additional capability grants — filesystem access within defined scopes, performance-sensitive APIs that would be too slow through the scripting layer.

Neither type bypasses `PluginContext`. A native plugin that reaches into core internals directly is a bug in the plugin API design. See RULE_OF_LAW §5.3.

### 8.4 SQLite for Mod Metadata

Flat files (INI, JSON) don't scale beyond a few hundred mods. SQLite provides indexed queries, transactions for safe writes, and a migration path as the schema evolves. The `mantle_data` crate (when extracted) owns the schema and all migration logic.

### 8.5 tokio for Async

Downloads, Nexus API calls, and concurrent file operations run on `tokio`. The GTK4 main loop runs on the main thread. Cross-thread communication uses `tokio` channels. Blocking operations (filesystem, SQLite) run in `tokio::task::spawn_blocking`.

---

## 9. Cross-References

| Topic | Standard |
|-------|----------|
| Governance and enforcement | [RULE_OF_LAW.md](RULE_OF_LAW.md) |
| Rust coding conventions | [CODING_STANDARDS.md](CODING_STANDARDS.md) |
| Plugin contract and event bus | [PLUGIN_API.md](PLUGIN_API.md) |
| VFS design and mount lifecycle | [VFS_DESIGN.md](VFS_DESIGN.md) |
| SQLite schema and data model | [DATA_MODEL.md](DATA_MODEL.md) |
| Build prerequisites and steps | [BUILD_GUIDE.md](BUILD_GUIDE.md) |
| Test suite structure | [TESTING_GUIDE.md](TESTING_GUIDE.md) |
| Distro and kernel support | [PLATFORM_COMPAT.md](PLATFORM_COMPAT.md) |
| GTK4 and UI conventions | [UI_GUIDE.md](UI_GUIDE.md) |
