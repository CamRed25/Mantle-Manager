# Project Futures

> Tracks ideas, known limitations, technical debt, and completed work.
> See RULE_OF_LAW.md §4.3 for maintenance rules.

---

## Ideas & Enhancements

### Controller input in Desktop Mode
- **Date:** 2026-03-03
- **Reference:** PLATFORM_COMPAT.md §5.2
- Steam Deck Desktop Mode works with keyboard/mouse. Controller navigation in GTK4 UI would improve handheld usability.
- Deferred until v1.0 is stable.

### Mod browser / Nexus integration
- **Date:** 2026-03-03
- **Reference:** ARCHITECTURE.md §2.2 (`mantle_net` crate), CODING_STANDARDS.md §4.3 (`net` feature flag)
- Requires `net` feature flag and `mantle_net` crate extraction. Nexus API key stored in `settings.toml`.
- Deferred until core mod management is stable.

### Narrow Flatpak sandbox permissions
- **Date:** 2026-03-03
- **Reference:** BUILD_GUIDE.md §8.5
- Current manifest uses `--filesystem=home` which is broad. Should be narrowed to specific Steam library and game paths once the path discovery logic is settled.

### Upper directory policy on unmount
- **Date:** 2026-03-03
- **Reference:** VFS_DESIGN.md §9
- Game writes (saves, config changes) go to the overlay upper directory. On unmount, these need a defined policy: archive them, discard them, or offer the choice. Currently undefined.

### abi_stable migration for native plugins
- **Date:** 2026-03-03
- **Reference:** PLUGIN_API.md §3.2
- Current native plugin ABI requires matching rustc version. The `abi_stable` crate provides stable vtable layouts and removes this constraint. Migrate when plugin ecosystem matures.

### ConflictDeleter — UI tool surface for conflict pruning
- **Date:** 2026-03-04
- **Reference:** `mo2_linux_plugins_examples/ConflictDeleter/`, ARCHITECTURE.md §4.4, `conflict/prune.rs`
- Backend `prune_losers(conflict_map, mods_dir, backup_dir)` is fully implemented in `conflict/prune.rs` (moves overridden files to a backup directory). What's missing is the UI tool surface: a Tools menu action that builds the conflict map for the active profile, previews the list of files to be removed from each losing mod, lets the user confirm, then calls `prune_losers`. Requires a dialog or dedicated page in `mantle_ui`, and an `EventBus` `ConflictMapUpdated` trigger to refresh after pruning.

### SKSEInstaller — automated script extender download and install
- **Date:** 2026-03-04 (completed 2026-03-05)
- **Status:** ✅ Implemented in `mantle_core/src/skse/` behind `--features net`
- **Reference:** `mo2_linux_plugins_examples/SKSEInstaller/`, PLUGIN_API.md §3, `game/games.rs`
- See "Completed / Integrated" section below for full implementation notes.

### LootWarningChecker — LOOT masterlist dirty plugin detection
- **Date:** 2026-03-04
- **Reference:** `mo2_linux_plugins_examples/LootWarningChecker/`, ARCHITECTURE.md §4.4
- MO2 diagnostic plugin that downloads the LOOT masterlist from GitHub and checks all enabled plugins for dirty edits (ITMs, UDRs, deleted navmeshes), missing masters, and incompatibilities. In Mantle Manager this would be a native plugin or built-in diagnostic: parse the YAML masterlist, cross-reference plugin metadata via `esplugin` (already a `mantle_core` dependency), and surface warnings in the diagnostics UI panel. xEdit quick-auto-clean guided remediation is out of scope (requires running a Wine process). Deferred until ESP file metadata is queryable via the data layer.

### NifAnalyzer — NIF mesh file analysis
- **Date:** 2026-03-04
- **Reference:** `mo2_linux_plugins_examples/NifAnalyzer` (symlink → external NIF Analyzer plugin)
- MO2 plugin that analyzes `.nif` mesh files inside installed mods for compatibility issues: wrong NIF version for the target game, missing referenced textures, unsupported shader flags. In Mantle Manager this would be a post-install diagnostic that walks mod files for `.nif` entries and reports known problem patterns. Requires a NIF format parser — no pure-Rust NIF library exists yet; would need FFI bindings to nifskope's libnifskope or a purpose-built minimal block-header parser. Deferred until NIF parsing is feasible.

---

## Technical Debt & Refactoring

### Download HTTP fetch implementation
- **Date:** 2026-03-05
- **Reference:** `path.md` §a, `crates/mantle_ui/src/downloads/queue.rs`
- `DownloadQueue::enqueue()` currently transitions every new job immediately to
  `DownloadStatus::Failed("HTTP fetch not yet implemented")`.  Full implementation
  requires:
  - `reqwest` streaming download running in a `tokio` task (add
    `reqwest.workspace = true` and `tokio.workspace = true` to
    `mantle_ui/Cargo.toml` when implementing).
  - Per-64 KiB progress pushes via `mpsc::Sender<DownloadProgress>` updating
    `InProgress { progress, bytes_done, total_bytes }`.
  - Terminal `Complete { bytes }` or `Failed(msg)` delivery.
  - `apply_progress()` already called from the second `glib::idle_add_local` loop;
    no wiring changes needed beyond implementing the actual download task.
  - `enqueue_nxm(url: &str)` entry point (depends on `mantle_net` item g for
    NXM URL parsing and CDN redirect resolution).



### mantle_archive crate extraction
- **Date:** 2026-03-03
- **Reference:** ARCHITECTURE.md §2.2
- Extract `mantle_core::archive` → `mantle_archive` crate when archive module complexity justifies it.

### mantle_data crate extraction
- **Date:** 2026-03-03
- **Reference:** ARCHITECTURE.md §2.2, DATA_MODEL.md §6
- Extract `mantle_core::data` → `mantle_data` crate when migration system is substantial.

### mantle_plugin crate extraction
- **Date:** 2026-03-03
- **Reference:** ARCHITECTURE.md §2.2
- Extract `mantle_core::plugin` → `mantle_plugin` crate when plugin system is feature-complete and boundary is stable.

### Split game/games.rs into per-game files
- **Date:** 2026-03-04
- **Reference:** ARCHITECTURE.md §4.5, conflict.md CONFLICT-003
- If/when the 10 game definitions grow substantially (per-game VFS config, registry paths, etc.), split into `game/games/` subdirectory. Not needed at current scale.

---

## Known Limitations

### SteamOS kernel 6.1 — fuse-overlayfs only
- **Date:** 2026-03-03
- **Reference:** PLATFORM_COMPAT.md §3, VFS_DESIGN.md §9
- SteamOS ships kernel 6.1. Kernel overlayfs (Tier 1) requires 6.6+. SteamOS always uses fuse-overlayfs. This is expected behavior, not a bug.

### Native plugin ABI requires matching rustc
- **Date:** 2026-03-03
- **Reference:** PLUGIN_API.md §3.2
- `*mut dyn MantlePlugin` fat pointer vtable is not stable across rustc versions. Plugins must be compiled with the same rustc as the host. Enforced via `create_plugin_rustc_version()` export check.

### Rhai scripts cannot queue downloads
- **Date:** 2026-03-03
- **Reference:** PLUGIN_API.md §4.3
- Network operations require native plugins. Rhai sandbox intentionally excludes `queue_download()` and `data_dir()`.

### NixOS native install unsupported
- **Date:** 2026-03-03
- **Reference:** PLATFORM_COMPAT.md §7.1
- NixOS's non-FHS layout causes issues with library detection and steamlocate. Flatpak path is recommended on NixOS.

---

## Completed / Integrated

### game/registry.rs — Wine/Proton registry reader
- **Date:** 2026-03-04
- **Reference:** ARCHITECTURE.md §4.5, conflict.md CONFLICT-003
- Implemented `game/registry.rs`: parses Wine-format `system.reg`/`user.reg` hive files into a `RegistryHive` with `get_value(key_path, value_name) -> Option<RegistryValue>`. Supports `Sz`, `Dword`, `QWord`, `ExpandSz`, `MultiSz` value types. Tolerates malformed lines silently. Includes 7 unit tests.

### Clippy pedantic warnings — all resolved
- **Date:** 2026-03-04
- **Reference:** CODING_STANDARDS.md §1.4, CI workflow
- Fixed all 138 `doc_markdown`, `missing_errors_doc`, `unnecessary_debug_formatting`, `must_use`, `unreadable_literal`, and misc warnings across 26 source files. `cargo clippy --workspace -- -D warnings` now reports 0 errors. `-D warnings` flag re-enabled in CI.

### ba2 archive backend — decision closed
- **Date:** 2026-03-03
- **Reference:** ARCHITECTURE.md §8.2
- Verified ba2 v3.0.1 covers all required formats: TES3 (`tes3` module), TES4/SSE (`tes4` v103/v104/v105), FO4 GNRL+DX10 (`fo4` v1), Starfield (`fo4` v2/v3, LZ4 compressed), FO4 next-gen (`fo4` v7/v8). Decision: use `ba2` crate — pure Rust, no FFI needed. `ba2` workspace dependency added; `mantle_core` depends on it.

### VFS detection + selection layer — complete
- **Date:** 2026-03-03
- **Reference:** VFS_DESIGN.md §2, ARCHITECTURE.md §4.2
- `vfs/detect.rs`: 5 OnceCell-cached probes (`is_flatpak`, `kernel_version`, `parse_kernel_version`, `has_new_mount_api`, `fuse_overlayfs_available`) — 14 unit tests.
- `vfs/backend/mod.rs`: `BackendKind` enum + `select_backend()` with full priority logic — 8 unit tests + 6 integration tests.
- `vfs/types.rs`: `MountParams` shared parameter type.

### VFS Tier 3 — symlink farm backend — complete
- **Date:** 2026-03-03
- **Reference:** VFS_DESIGN.md §2.3, §4.2–4.4
- `vfs/backend/symlink.rs`: full `SymlinkFarm` implementation — `mount`, `verify`, `unmount`. Iterates lower_dirs lowest-priority-first so higher-priority entries win conflicts. Creates real dirs, symlinks files, precise teardown with best-effort empty-dir pruning — 13 unit tests + 7 integration tests.
- `vfs/backend/fuse.rs` and `vfs/backend/kernel.rs`: stub structs wired in; awaiting mount lifecycle implementation.

### Game detection layer — complete
- **Date:** 2026-03-04
- **Reference:** ARCHITECTURE.md §4.5, PLATFORM_COMPAT.md §6
- `game/games.rs`: `GameDef` + `KNOWN_GAMES` static table — 10 supported titles (Morrowind, Oblivion, SkyrimLE/SE/VR, Fallout3/NV/4, Starfield, Enderal SE) with Steam App IDs, executable sentinels, and data subdirs. `by_app_id()` / `by_slug()` lookup helpers — 9 unit tests.
- `game/steam.rs`: `detect_all(steam: &SteamDir)` (injectable), `detect_all_steam()` (production convenience), `detect_game_at_path()` (pure filesystem probe for unit tests) — 8 unit tests.
- `game/proton.rs`: `proton_prefix_in_dir()`, `proton_prefix()`, `is_prefix_initialised()` — 8 unit tests.
- `game/mod.rs`: `GameKind` enum (10 variants), `GameInfo` struct, `is_proton()` / `wine_prefix()` helpers.
- `tests/game_detection.rs`: 10 integration tests — every KNOWN_GAMES entry, Proton prefix, and Steam-live scan (graceful skip when Steam absent).
- **Total tests added:** 79 (56 unit + 23 integration + 2 doc unchanged).

### Conflict detection layer — complete
- **Date:** 2026-03-04
- **Reference:** ARCHITECTURE.md §4.4, VFS_DESIGN.md §7, DATA_MODEL.md §3.8
- `conflict/mod.rs`: `ModId` type alias, `ModEntry` (input manifest), `ConflictEntry` (winner + losers per path), `ConflictMap` (full scan result) with `role_of_mod`, `conflicts_for_mod`, `win_count_for_mod`, `loss_count_for_mod`, `entry_for_path`, `conflicted_paths`, `build_conflict_map()` public constructor.
- `conflict/detector.rs`: O(∑ files) scan algorithm — single pass winner_table + loser accumulation, zero allocations on the no-conflict path — 8 unit tests including 100-mod stress test.
- `conflict/resolution.rs`: `ModRole` enum (Winner/Loser/Both/Clean), `role_of_mod()`, `ConflictSummary`, `conflict_summary_for_mod()` — 8 unit tests.
- `conflict/dll.rs`: DLL collision detection stub (deferred until archive extraction layer exists).
- `tests/conflict_detection.rs`: 17 integration tests including realistic 5-mod Skyrim scenario.
- **Running total:** 82 unit + 40 integration + 2 doc = **124 passing, 0 warnings**.

### SQLite data layer — complete
- **Date:** 2026-03-04
- **Reference:** DATA_MODEL.md, CODING_STANDARDS.md §5.2, TESTING_GUIDE.md §4.1 & §6
- `data/migrations/m001_initial.sql`: Full initial schema — 9 tables (mods, mod_files, profiles, profile_mods, load_order, downloads, plugin_settings, conflicts, schema_version) with all FK constraints, CHECK constraints, and indices from DATA_MODEL.md.
- `data/schema.rs`: Migration runner — `MIGRATIONS: &[&str]` slice with embedded SQL via `include_str!`, idempotent `run_migrations(&conn)` public function, `current_schema_version()`, `apply_migration_sql()` — 4 unit tests.
- `data/mod.rs`: `Database` struct wrapping `Mutex<Connection>`, `open(path)` + `open_in_memory()` constructors (set PRAGMAs, run migrations), `with_conn()` accessor, public `use` re-export of `run_migrations` at `mantle_core::data::run_migrations` — 5 unit tests + 1 doc-test.
- `data/mods.rs`: `ModRecord`, `InsertMod`, `insert_mod`, `get_mod_by_slug`, `list_mods`, `delete_mod`, `mods_for_profile` — 8 unit tests.
- `data/profiles.rs`: `ProfileRecord`, `InsertProfile`, `insert_profile`, `set_active_profile` (exactly-one-active invariant, atomic transaction), `delete_profile`, `get_active_profile`, `list_profiles`, `get_profile_by_id` — 10 unit tests.
- `data/mod_files.rs`: `ModFileRecord`, `InsertModFile`, `insert_mod_files` (batch with explicit transaction, auto-lowercase paths), `delete_mod_files`, `files_for_mod`, `all_paths_for_enabled_mods_in_profile` (priority-ordered, for conflict detection input) — 8 unit tests.
- `tests/data_migrations.rs`: 15 integration tests — clean migration apply, idempotency, all-tables-exist check, mod/profile/mod_files round-trips, cascade delete verification, exactly-one-active invariant, FK rejection, `temp_db()` helper pattern.
- Also fixed stale `vfs/backend.rs` stub that conflicted with `vfs/backend/mod.rs`.
- **Running total:** 116 unit + 38 integration + 3 doc = **157 passing, 0 warnings**.

### Config layer — complete
- **Date:** 2026-03-04
- **Reference:** DATA_MODEL.md §5.1, PLATFORM_COMPAT.md §4.2, BUILD_GUIDE.md §5
- `config/mod.rs`: Full TOML config implementation — `Theme` (`auto`/`light`/`dark`, serde lowercase), `UiSettings`, `PathSettings`, `NetworkSettings`, `AppSettings` with `Default`.
- `AppSettings::load_or_default(path)` — returns struct default if file absent; errors on malformed TOML rather than silently discarding user data.
- `AppSettings::save(path)` — atomic write via `.tmp` temp file + `rename`, crash-safe.
- `config_dir()` / `data_dir()` — full precedence chain: `MANTLE_CONFIG_DIR`/`MANTLE_DATA_DIR` env override → Flatpak path → XDG env vars → `~/.config/mantle-manager/` / `~/.local/share/mantle-manager/`.
- `default_settings_path()` / `default_db_path()` — canonical paths for startup.
- Custom `opt_path_serde` module: `Option<PathBuf>` serialises as `""` (None) or path string (Some), matching DATA_MODEL.md §5.1 TOML format.
- 16 unit tests covering defaults, all theme variants, save/load roundtrip, malformed TOML rejection, atomic save, env-var override priority.
- **Running total:** 192 passing, 0 warnings (132 unit + 55 integration + 5 doc).

### Archive extraction layer — complete
- **Date:** 2026-03-04
- **Reference:** ARCHITECTURE.md §4.3, CODING_STANDARDS.md §5.2, TESTING_GUIDE.md §4.1
- `archive/detect.rs`: Magic-byte detection for all six supported formats — `ArchiveFormat::Tes3Bsa` (Morrowind BSA, `0x00000100`), `Tes4Bsa` (Oblivion/Skyrim BSA, `BSA\0`), `Fo4Ba2` (`BTDX`), `Zip` (`PK\x03\x04`), `SevenZip` (6-byte `37 7A BC AF 27 1C`), `Rar` (`Rar!`) — `detect_format(path)` and `detect_format_from_bytes(header)` — 9 unit tests + 1 doc-test.
- `archive/bsa.rs`: Bethesda archive back-end via `ba2` crate — `list_bsa_files`/`extract_bsa` (tes3 first-try, tes4 fallback) and `list_ba2_files`/`extract_ba2` (fo4). tes3: flat iteration + `as_bytes()`. tes4: nested dir→file iteration, `decompress_into` for compressed entries, `CompressionOptions::version()` accessor. fo4: per-chunk decompression with `Chunk::decompress`. `normalise_path` (backslash→slash), `join_rel_path` (dot/empty dir handling), `safe_join` (`..` component rejection) helpers — 9 unit tests.
- `archive/zip.rs`: ZIP extraction via `compress_tools::list_archive_files` / `uncompress_archive(Ownership::Ignore)`. Includes hand-crafted stored-ZIP builder + CRC-32 implementation for zero-dependency roundtrip tests — 5 unit tests.
- `archive/sevenz.rs`: 7-Zip extraction via compress-tools (all LZMA/LZMA2 variants via libarchive) — 3 unit tests.
- `archive/rar.rs`: RAR4/RAR5 extraction via compress-tools (extract-only; libarchive 3.3.0+ for RAR5) — 3 unit tests.
- `archive/mod.rs`: Public async API — `list_files(path)` and `extract_archive(path, dest)`, both dispatching via `detect_format` → format-specific back-end, wrapped in `tokio::task::spawn_blocking`. Re-exports `ArchiveFormat`, `detect_format`, `detect_format_from_bytes` — 6 unit tests (including 2 async).
- `tests/archive_extraction.rs`: Integration test suite — detection tests for all 7 magic variants, async list/extract ZIP roundtrips, BSA/BA2 negative tests, libarchive permissiveness documentation — 15 integration tests.
- **Note:** libarchive is intentionally permissive; arbitrary bytes return `Ok([])` rather than error. Tests document this behaviour rather than asserting error on garbage input.
- **Running total:** 244 passing, 0 warnings.

### Address Library version mismatch detection — complete
- **Date:** 2026-03-04
- **Reference:** path.md §k, ARCHITECTURE.md §4.4, §8
- `conflict/address_lib.rs`: full implementation of Address Library conflict detection.
  - `parse_address_lib_path(path)` — private regex-free filename parser for `versionlib[64]-<v1>-<v2>-<v3>-<v4>.bin`; returns `ParsedAddressLib { library_type, game_version }` or `None`.
  - `AddressLibMismatch` enum — `InterModMismatch { versions }` (two mods ship conflicting game-version strings) and `GameVersionMismatch { game_version, library_version }` (library doesn't match installed game).
  - `AddressLibConflict` struct — `library_type`, `shipping_mods: Vec<(mod_id, version)>`, `kind: AddressLibMismatch`.
  - `detect_address_lib_conflicts(mod_files, game_version)` — groups paths by library type, raises `InterModMismatch` when distinct versions are present across mods, `GameVersionMismatch` when single version disagrees with `game_version` arg; result sorted by `library_type`.
  - All three public items re-exported from `conflict/mod.rs`.
  - **19 unit tests.**
- **Running total:** 313 passing, 0 failed, 0 warnings.

### DLL conflict detection — complete
- **Date:** 2026-03-04
- **Reference:** path.md §j, ARCHITECTURE.md §4.4
- `conflict/dll.rs`: replaced stub with full implementation.
  - `is_se_plugin_dll(path)`: classifies a path as an SE-plugin DLL by checking `.dll` extension (case-insensitive via `Path::extension()`) and prefix against 6 known script-extender directories (`skse`, `f4se`, `nvse`, `fose`, `obse`, `mwse`).
  - `detect_dll_conflicts(mod_dll_files)`: O(∑ paths) scan — builds path→mods map, returns all paths claimed by >1 mod as `Vec<DllConflict>`, sorted by path for deterministic output.
  - `dll_files_for_profile(conn, profile_id)`: SQL query joining `mod_files` + `profile_mods` + `mods` for all enabled mods with `.dll` extension; filters to `is_se_plugin_dll`; returns grouped by slug in priority order — ready to pass directly to `detect_dll_conflicts`.
  - `DllConflict` struct: `dll_path: String` + `mods: Vec<String>` (priority order, index 0 = winner).
  - `conflict::detect_dll_conflicts`, `conflict::dll_files_for_profile`, `conflict::is_se_plugin_dll`, `conflict::DllConflict` re-exported from `conflict/mod.rs`.
  - Also fixed pre-existing `address_lib.rs` clippy panic: replaced `.expect()` with `let-else` `continue`.
  - **12 unit tests** (2 is_se_plugin_dll, 6 detect_dll_conflicts, 4 dll_files_for_profile including full pipeline DB round-trip).
- **Running total:** 313 passing, 0 failed, 0 warnings.

### mod_list/ — ordering, enable/disable, transactional load-order updates
- **Date:** 2026-03-04
- **Reference:** path.md §h, DATA_MODEL.md §3, CODING_STANDARDS.md §5.2
- `mod_list/mod.rs`: `ProfileModEntry` struct; `list_profile_mods`, `add_mod_to_profile`, `remove_mod_from_profile`, `set_mod_enabled`, `move_mod_to`, `reorder_profile_mods` — all mutations go through `replace_all_profile_mods` (atomic delete+reinsert to avoid `UNIQUE(profile_id, priority)` violations). Priority 1 = highest (leftmost `lowerdir`). 19 unit tests.

### profile/ — CRUD and activation with VFS coordination
- **Date:** 2026-03-04
- **Reference:** path.md §i, VFS_DESIGN.md §3, data/profiles.rs
- `profile/mod.rs`: Profile activation coordinates VFS teardown of the old profile's mount and remount for the new profile via `vfs::teardown_stale` / `vfs::mount_with`. Profile CRUD delegates to `data::profiles`. Module declared in `lib.rs`.
- **Running total at this milestone:** 279 passing, 0 failed, 0 warnings.

### VFS mount lifecycle — all tiers complete
- **Date:** 2026-03-05
- **Reference:** VFS_DESIGN.md §2–6, ARCHITECTURE.md §4.2
- `vfs/backend/kernel.rs`: `KernelOverlay::mount/verify/unmount` using raw `fsopen`/`fsconfig`/`fsmount`/`move_mount` syscalls (Linux new mount API). Graceful skip on `EPERM` (no `CAP_SYS_ADMIN`) in integration test.
- `vfs/backend/fuse.rs`: `FuseOverlay::mount/verify/unmount` — spawns `fuse-overlayfs -f -o lowerdir=…,upperdir=…,workdir=… <merge>` child process, 150 ms settle delay, `fusermount3 -u` teardown.
- `vfs/cleanup.rs`: `teardown_stale(merge_dir)` — parses `/proc/self/mountinfo`, dispatches to `umount2(MNT_DETACH)` for kernel overlay or `fusermount3 -u` for FUSE — 4 unit tests.
- `vfs/namespace.rs`: `enter_mount_namespace()` via `libc::unshare(CLONE_NEWNS)` + `MS_PRIVATE|MS_REC` propagation stop, `is_namespace_available()` (`OnceCell`-cached) — 2 unit tests.
- `vfs/stacking.rs`: `mount_stacked()` — CHUNK_SIZE=200, STACK_TRIGGER=480, LIFO unmount on `StackedMount::unmount()`, best-effort cleanup on partial failure — 3 tests (unit + integration).
- `tempfile = "3"` promoted to regular dependency for `TempDir` RAII upper/work dirs in both backends.
- Integration tests `kernel_overlayfs_mount_unmount_round_trip` and `fuse_overlayfs_mount_unmount_round_trip` un-ignored with runtime skip guards.
- **Total tests:** 0 failing, 0 warnings (`cargo clippy --workspace -- -D warnings` clean).

### plugin/event.rs — EventBus, ModManagerEvent, EventFilter, SubscriptionHandle
- **Date:** 2026-03-04
- **Reference:** path.md §n, PLUGIN_API.md §5
- `plugin/event.rs` replaces stub with full implementation.
- `ModInfo` plugin-facing mod snapshot defined in event.rs (avoids circular import from context.rs).
- `VfsBackend` enum: `KernelOverlayfs`, `FuseOverlayfs`, `SymlinkFarm` (mirrors `BackendKind`).
- `ModManagerEvent` with 12 variants: `GameLaunching`, `GameExited`, `ModInstalled`, `ModEnabled`, `ModDisabled`, `ProfileChanged`, `OverlayMounted`, `OverlayUnmounted`, `DownloadStarted`, `DownloadCompleted`, `ConflictMapUpdated`. `#[non_exhaustive]`.
- `EventFilter` enum with `matches(&ModManagerEvent) -> bool` using `matches!` macro.
- `EventBus`: handler-list design (`Mutex<HashMap<u64, Subscriber>>`); collect-then-invoke pattern prevents subscriber-lock deadlock; panics caught via `catch_unwind`.
- `SubscriptionHandle`: holds `Weak<EventBus>` + `u64` ID; `Drop` impl auto-unsubscribes.
- `HandlerFn` type alias avoids clippy `type_complexity` lint.
- **7 unit tests** in `plugin::event::tests`.

### plugin/context.rs — PluginContext, MantlePlugin trait, full PLUGIN_API.md §2–4 surface
- **Date:** 2026-03-04
- **Reference:** path.md §l, §m, PLUGIN_API.md §2–4, §7–8
- `PluginError` (thiserror): 7 variants including `ApiVersionMismatch`, `CapabilityNotGranted`, `NetFeatureDisabled`.
- `SettingValue`: `Bool`, `String`, `Int`, `Float`.
- `PluginSetting`: `key`, `label`, `description`, `default`.
- `ModState`, `NotifyLevel` (Copy), `Capability` (Copy+Hash).
- `DownloadHandle` stub — `queue_download` always returns `NetFeatureDisabled`.
- `PLUGIN_API_VERSION: Lazy<semver::Version>` — v1.0.0.
- `RUSTC_TOOLCHAIN_VERSION: &str = env!("RUSTC_VERSION_STRING")` baked in by `build.rs`.
- `MantlePlugin` trait: 9 methods including `settings()` default impl.
- `PluginContext`: `RwLock<Vec<ModInfo>>`, `RwLock<String>` profile, `RwLock<Option<GameInfo>>`, `Arc<EventBus>`, `Mutex<HashMap<String, SettingValue>>`, `HashSet<Capability>`, `PathBuf` data_dir.
- All §4.1 accessor methods with `# Panics` docs; host-mutator methods `update_mod_list` / `update_active_profile` (`#[allow(dead_code)]` pending wiring).
- `PluginContext::subscribe` delegates to `Arc<EventBus>::subscribe`.
- `for_tests()` test constructor.
- **11 unit tests** in `plugin::context::tests`.

### build.rs — RUSTC_VERSION_STRING baked at compile time
- **Date:** 2026-03-04
- **Reference:** PLUGIN_API.md §3.2
- `crates/mantle_core/build.rs` emits `RUSTC_VERSION_STRING` env var for ABI enforcement.

### plugin/mod.rs — full re-exports
- **Date:** 2026-03-04
- Re-exports all public types from `plugin::event` and `plugin::context` at `plugin::*` level.
- Also fixed pre-existing `registry.rs` clippy lints: `assigning_clones`, `manual_strip`, `doc_markdown`, `should_implement_trait`.
- **Running total:** 353 passing, 0 failed, 0 warnings (`cargo clippy --workspace -- -D warnings` clean).

### plugin/sandbox.rs — Rhai sandbox (item q)
- **Date:** 2026-03-05
- **Reference:** path.md §q, PLUGIN_API.md §4.3, §7
- `SandboxConfig` struct with 5 `Option<_>` resource limit fields (operations, call depth, string, array, map sizes); `Default` impl with production-safe values.
- `build_sandboxed_engine(config: &SandboxConfig) -> Engine`: applies all limits, attaches `StaticModuleResolver` (blocks all `import` statements), disables `eval`, routes `print`/`debug` to `tracing::info!`/`tracing::debug!`.
- **4 unit tests**: `max_operations_blocks_infinite_loop`, `max_call_depth_blocks_deep_recursion`, `module_import_is_blocked`, `normal_script_runs_successfully`.

### plugin/native.rs — native `.so` plugin loader (item o)
- **Date:** 2026-03-05
- **Reference:** path.md §o, PLUGIN_API.md §3.2, §5
- C-ABI function pointer types: `CreatePluginFn` (`*mut dyn MantlePlugin`) and `RustcVersionFn` (`*const c_char`).
- `NativePlugin` struct: `plugin: Box<dyn MantlePlugin>` declared *before* `_lib: Library` — Rust drops fields in declaration order, ensuring the trait object drops before the library is unmapped (prevents use-after-unmap of vtable).
- `check_api_compat(required: &Version, loaded: &Version)`: semver compatibility guard — major version must match, `required <= loaded`.
- `load_native_plugin(path)`: `dlopen` → rustc toolchain version check via `create_plugin_rustc_version` export → `create_plugin` call → `Box::from_raw` → `check_api_compat`. Full error path for each failure mode.
- `impl MantlePlugin for NativePlugin`: full delegation to inner `plugin`.
- **5 unit tests**: 4 for `check_api_compat` (exact match, older required, newer required, major mismatch), 1 for nonexistent path.

### plugin/scripted.rs — Rhai scripted plugin loader (item p)
- **Date:** 2026-03-05
- **Reference:** path.md §p, PLUGIN_API.md §4.3, §6, §9.2
- `RhaiPluginContext`: `Arc<PluginContext>` + `Arc<Mutex<Vec<(String, FnPtr)>>>` sub_requests buffer. `Clone` for Rhai pass-by-value.
- `build_scripted_engine(config)`: wraps `build_sandboxed_engine`, registers `RhaiPluginContext` type as `"PluginContext"` with 7 API methods: `subscribe`, `notify`, `active_profile`, `profiles`, `mod_list`, `get_setting`, `set_setting` (4 type overloads: bool/i64/f64/String).
- `event_to_dynamic(event)`: converts all 11 `ModManagerEvent` variants to Rhai maps with a `"type"` key. `mod_info_to_dynamic` helper maps all 8 `ModInfo` fields.
- `parse_filter_str(s)`: maps 12 filter name strings to `EventFilter` variants.
- Deferred-drain subscription pattern: `init()` calls script's `init(ctx)`, drains `sub_requests` *after* engine returns (prevents re-entrant deadlock), creates `SubscriptionHandle`s with closures capturing `Arc<Engine>` + `Arc<AST>` + `FnPtr`.
- `shutdown()`: drops all handles (unsubscribes), then calls script's `shutdown()` best-effort.
- `load_scripted_plugin(path)` / `load_scripted_plugin_with_config(path, config)`: read source → compile AST → extract 6 metadata functions → semver API compat check → return `ScriptedPlugin`.
- `rhai = { version = "~1.17", features = ["sync"] }` added to workspace — required for `Engine + AST + FnPtr: Send + Sync` in event handler closures.
- **8 unit tests**: nonexistent path, missing metadata, compile error, metadata extraction, API mismatch, init with no init fn, shutdown with no shutdown fn, sandbox blocks infinite loop in init.
- **Running total: 377 passing, 0 failed, 0 warnings** (`cargo clippy --workspace -- -D warnings` clean).

### plugin/registry.rs — Plugin registry, load/unload lifecycle (item r)
- **Date:** 2026-03-05
- **Reference:** path.md §r, PLUGIN_API.md §6–8
- `PluginManifest` / `ManifestCapabilities`: `serde::Deserialize` structs for optional `plugin.toml` alongside each plugin file. All fields optional; absent fields fall back to `MantlePlugin` trait methods.
- `PluginLoadError`: non-fatal per-plugin error enum with `LoadFailed`, `InitFailed`, `InitPanicked`, `DuplicateId` variants. Never fatal to the application (§8.2).
- `PluginRegistry::new(event_bus, base_data_dir)`: creates empty registry. Per-plugin writable dirs live at `{base_data_dir}/plugin-data/{plugin_id}/`.
- `PluginRegistry::load_dir(plugins_dir, mod_list, active_profile, profiles, game)`: non-recursive alphabetical scan for `.so`/`.rhai`; creates `PluginContext::new(...)` per plugin; calls `init()` wrapped in `std::panic::catch_unwind` (§8.2 panic policy); enforces unique IDs (§6.3); returns `Vec<PluginLoadError>` for all per-file failures without blocking others (§6.1).
- `PluginRegistry::unload_all()`: reverse-order `shutdown()` + drain.
- `PluginRegistry::get(id)` / `get_mut(id)`: look up loaded plugin by stable ID.
- `PluginRegistry::plugin_count()` / `plugin_ids()`: introspection helpers.
- `resolve_capabilities(manifest, is_rhai)`: maps manifest strings to `HashSet<Capability>`; drops `downloads` for Rhai scripts per §7.3.
- `read_manifest(plugin_path)`: reads adjacent `plugin.toml`; absent file → silent default; parse errors → warn + default.
- `extract_panic_message`: best-effort `&dyn Any` → `String` for catch_unwind payloads.
- `pub mod registry` + re-exports (`PluginRegistry`, `PluginLoadError`, `PluginManifest`, `ManifestCapabilities`) added to `plugin/mod.rs`.
- **8 new unit tests** (capability resolution, panic extraction, empty registry, missing/empty/non-plugin dirs, manifest valid/missing/malformed).
- **Running total: 385 passing, 0 failed, 0 warnings** (`cargo clippy --workspace -- -D warnings` clean).

### pages/mods.rs — UI Mods page (item t)
- **Date:** 2026-03-05
- **Reference:** path.md §t, UI_GUIDE.md §3, §5.3, §8
- `pages/mods.rs` added; `pub mod mods` declared in `pages/mod.rs`.
- `build(state: &AppState) -> GtkBox`: top-level builder for the Mods page; placeholder-powered.
- `toolbar`: search entry (`gtk4::SearchEntry`, placeholder text "Search mods…"), mod count badge, "Add Mod" flat button (`document-save-symbolic`). Search filter and Add Mod action wired in item y/z.
- `conflict_banner`: `adw::Banner` shown only when `state.conflict_count > 0` — pluralised title, "Dismiss" button.
- `empty_state`: `adw::StatusPage` (`application-x-addon-symbolic`, "No Mods Installed") shown when `state.mods` is empty.
- `list_header`: column header row for priority, switch gap, Mod name, Status columns.
- `mod_row(priority, entry)`: each row has priority badge (1-based), enable/disable `gtk4::Switch`, ellipsised mod name (`EllipsizeMode::End`), and either a conflict badge (`dialog-warning-symbolic` + "conflict" label with `.warning` class) or a version dim-label. Disabled mods render name in `.dim-label`.
- `window.rs`: replaces the "Mods" placeholder `adw::StatusPage` with `mods::build(&state)`.
- No new tests (UI-only, no testable logic — UI guide §9 compliance).
- **Running total: 385 passing, 0 failed, 0 warnings** (`cargo clippy --workspace -- -D warnings` clean).

### pages/plugins.rs — UI Plugins page (item u)
- **Date:** 2026-03-05
- **Reference:** path.md §u, UI_GUIDE.md §3, §5.1, §5.3, §9, PLUGIN_API.md §7
- `pages/plugins.rs` added; `pub mod plugins` declared in `pages/mod.rs`.
- `state.rs` extended: `PluginSettingEntry { key, label, description, value }`, `PluginEntry { id, name, version, author, description, enabled, settings }`, `plugins: Vec<PluginEntry>` added to `AppState`. `placeholder()` populated with 3 sample plugins (2 enabled, 1 disabled; 2 with settings, 1 without).
- `build(state: &AppState) -> GtkBox`: top-level builder; delegates to toolbar + empty state or plugin scroll.
- `toolbar`: loaded count badge, "Open plugins folder" flat button (`folder-symbolic`). Action wired in item y.
- `empty_state`: `adw::StatusPage` (`application-x-executable-symbolic`, "No Plugins Loaded") shown when `state.plugins` is empty.
- `plugin_row(entry)`: `adw::ExpanderRow` with subtitle `"v{version} · {author}"`, `gtk4::Switch` suffix for enable/disable (tooltip varies by state), description sub-row (`adw::ActionRow` with `.property` class), per-setting `adw::ActionRow` children or a disabled "No configurable settings" row. Widget names set from `entry.id` / `setting.key` for CSS targeting and future item-y action lookup.
- `setting_row(setting)`: `adw::ActionRow` with title = label, subtitle = description, suffix `Label` showing current value (`.caption.dim-label`).
- `window.rs`: replaces the "Plugins" placeholder `adw::StatusPage` with `plugins::build(&state)`.
- `#[allow(clippy::too_many_lines)]` added to `AppState::placeholder()` (pure data initialiser).
- No new tests (UI-only; no testable logic — UI guide §9 compliance).
- **Running total: 385 passing, 0 failed, 0 warnings** (`cargo clippy --workspace -- -D warnings` clean).

### pages/downloads.rs — UI Downloads page (item v)
- **Date:** 2026-03-05
- **Reference:** path.md §v, UI_GUIDE.md §3, §5, §9
- `state.rs` extended: `id: String` added to `DownloadEntry`; `DownloadState::Failed(String)` variant added. `AppState::placeholder()` now has 4 entries: dl-1 (67% in progress), dl-2 (complete), dl-3 (queued), dl-4 (failed: "Connection timeout").
- `sidebar.rs`: `Failed(msg)` arm added to the `DownloadState` match — displays `"Failed: {msg}"` in `.caption.error`.
- `pages/downloads.rs` added; `pub mod downloads` declared in `pages/mod.rs`.
- `build(state: &AppState) -> GtkBox`: top-level builder; delegates to toolbar + empty state or scrolled download list.
- `toolbar`: active download count label, "Clear completed" flat button (`widget_name("btn-clear-completed")`). Action wired in item y.
- `empty_state`: `adw::StatusPage` (`folder-download-symbolic`, "No Downloads") shown when `state.downloads` is empty.
- `download_scroll`: builds 4 optional sections (In Progress, Queued, Failed, Completed); each only rendered when non-empty.
- `section(title, entries, row_fn)`: section header label + `ListBox boxed-list`.
- `render_in_progress_row`: name + cancel button on top; `gtk4::ProgressBar` + percentage on bottom. `widget_name("cancel-{id}")`.
- `render_queued_row`: name + `.dim-label` "Queued" badge + cancel button.
- `render_failed_row`: name + retry button + error reason label. `widget_name("retry-{id}")`.
- `render_completed_row`: success icon (`emblem-ok-symbolic`, `.success`) + name + clear button. `widget_name("clear-{id}")`.
- `make_row(&GtkBox) -> ListBoxRow`: shared helper wrapping content in non-activatable row.
- `window.rs`: replaces the "Downloads" placeholder `adw::StatusPage` with `downloads::build(&state)`.
- No new tests (UI-only — UI guide §9 compliance).

### pages/profiles.rs — UI Profiles page (item w)
- **Date:** 2026-03-05
- **Reference:** path.md §w, UI_GUIDE.md §3, §5.4, §9
- `state.rs` extended: `id: String` added to `ProfileEntry` (slug-formatted). Placeholder profiles: "survival-playthrough" (active, 147 mods), "vanilla-plus" (23 mods), "testing" (5 mods).
- `pages/profiles.rs` added; `pub mod profiles` declared in `pages/mod.rs`.
- `build(state: &AppState) -> GtkBox`: top-level builder; delegates to toolbar + empty state or profile list.
- `toolbar`: spacer + "New Profile" suggested-action button (`widget_name("btn-new-profile")`). Dialog wired in item y/i.
- `empty_state`: `adw::StatusPage` (`avatar-default-symbolic`, "No Profiles") shown when `state.profiles` is empty.
- `profile_list(state)`: `ListBox boxed-list` of `adw::ActionRow` rows; `row.set_widget_name(&entry.id)`.
- `profile_row(entry)`: title = name, subtitle = "{n} mods". Active: `.accent` class + "Active" label suffix. Inactive: "Activate" flat button (`widget_name("activate-{id}")`). Always: Clone button (`edit-copy-symbolic`, `widget_name("clone-{id}")`). Delete button (`edit-delete-symbolic`, `.error`, `set_sensitive(!entry.active)`, `widget_name("delete-{id}")`).
- All action buttons wired in item y/i.
- `window.rs`: replaces the "Profiles" placeholder `adw::StatusPage` with `profiles::build(&state)`.
- No new tests (UI-only — UI guide §9 compliance).
- **Running total: 392 passing, 0 failed, 0 warnings** (`cargo clippy --workspace -- -D warnings` clean).

### settings.rs — UI Settings dialog (item x)
- **Date:** 2026-03-04
- **Reference:** path.md §x, UI_GUIDE.md §5.1, DATA_MODEL.md §5.1
- `mantle_core::data::Database::with_conn` promoted from `pub(crate)` to `pub` to allow UI crate access. `#[allow(dead_code)]` removed.
- `crates/mantle_ui/src/settings.rs` added; `pub mod settings` declared in `main.rs`.
- `build_dialog(settings: AppSettings, path: PathBuf) -> adw::PreferencesWindow`: builds a fully-populated `adw::PreferencesWindow` with 3 pages:
  - **Appearance**: `adw::ComboRow` (Color scheme: Auto/Light/Dark), two `adw::SwitchRow`s (Compact layout, Source separator colors).
  - **Paths**: Two `adw::EntryRow`s with apply button (Mods directory, Downloads directory; empty = platform default).
  - **Network**: `adw::PasswordEntryRow` with apply button (Nexus Mods API key).
- `pub fn apply_theme(theme: Theme)`: applies `adw::ColorScheme` via `adw::StyleManager`; called at startup and on combo change.
- All changes written immediately on toggle/apply via `AppSettings::save` (atomic write via `.tmp` rename).
- `Rc<RefCell<AppSettings>>` shared across all row callbacks; `save_settings` helper creates parent dirs on first launch.
- Settings gear button (`preferences-system-symbolic`) added to header bar in `window.rs`; opens dialog via `set_transient_for` + `present()`.
- No new tests (UI-only — UI guide §9 compliance).

### state_worker.rs — Live glib channel state wiring (item y)
- **Date:** 2026-03-04
- **Reference:** path.md §y, UI_GUIDE.md §5.5
- `crates/mantle_ui/src/state_worker.rs` added; `pub mod state_worker` declared in `main.rs`.
- `pub fn spawn(sender: std::sync::mpsc::Sender<AppState>)`: spawns one OS thread that loads the initial `AppState` from `mantle_core` and sends it over `std::sync::mpsc`.
- `fn load_state() -> anyhow::Result<AppState>`: opens (or creates) `Database` at `default_db_path()`, runs schema migrations, reads all profiles via `data::profiles::list_profiles`, reads active profile mods via `mod_list::list_profile_mods`.
- DB parent directory created with `create_dir_all` on first launch before `Database::open`.
- `window.rs` restructured: initial UI built with `AppState::placeholder()`; `glib::idle_add_local` polls `std::sync::mpsc::Receiver<AppState>` each GTK main-loop idle cycle; on receive, `adw::ViewStack` + sidebar are rebuilt from the live state and hot-swapped into the `gtk4::Paned` via `set_start_child`/`set_end_child`; `adw::ViewSwitcher` re-targeted via `set_stack`.
- `build_main_content(state: &AppState) -> (adw::ViewStack, gtk4::ScrolledWindow)`: extracted helper shared by placeholder and live-data paths.
- Deferred (item z): `game_name`, `game_version`, `launch_target`, `overlay_backend` (require game detection); `plugin_count`/`plugins` (registry not started); `conflict_count` (scan after VFS mount); per-profile `mod_count` (N+1 query optimisation); live updates (re-send on profile/mod change via EventBus).
- `glib::MainContext::channel` not available in glib 0.19.9 (removed); replaced with `std::sync::mpsc` + `glib::idle_add_local` pattern.
- No new tests (UI-only — UI guide §9 compliance).
- **Running total: 392 passing, 0 failed, 0 warnings** (`cargo clippy --workspace -- -D warnings` clean).
### Post-extraction install pipeline — case-fold + BSA/BA2 extract
- **Date:** 2026-03-04
- **Reference:** ARCHITECTURE.md §4.3, `mo2_linux_plugins_examples/CaseFoldingNormalizer`, `mo2_linux_plugins_examples/bsa_extractor`
- `install/case_fold.rs`: `NormalizeResult` (renamed_dirs, renamed_files, collisions, errors, skipped, total_scanned; `total_renamed()`, `has_issues()`). `normalize_dir(root, dry_run, exclusions)` public API. Bottom-up post-order tree walk (`normalize_recursive`) — children renamed before parents. `normalize_entries` groups by lowercase name for O(entries) collision detection; neither entry renamed on collision. `rename_two_step` via `.__case_tmp__` intermediate handles case-only renames on case-insensitive filesystems. Exclusion patterns skip both recursion and renaming of matched directories. 13 unit tests.
- `install/bsa.rs`: `BsaExtractResult` (extracted, failed, deleted; `is_ok()`). `find_bsa_archives(mod_dir)` — recursive, case-insensitive `.bsa`/`.ba2` extension match, sorted output. `extract_mod_archives(mod_dir, delete_after)` — extracts each archive to its containing directory; delegates to `crate::archive::bsa::extract_bsa`/`extract_ba2`; captures per-archive failures without aborting. 11 unit tests.
- `install/mod.rs`: wires both submodules with `pub use` re-exports at `mantle_core::install::*`.
- `lib.rs`: `pub mod install` declared; module-layout doc comment updated.
- **24 unit tests added** (13 case_fold + 11 bsa).

### diag/ module — cosave checker, overwrite classifier, conflict pruner
- **Date:** 2026-03-04
- **Reference:** `mo2_linux_plugins_examples/SKSECosaveTracker`, `mo2_linux_plugins_examples/OverwriteAutoCategorize`, `mo2_linux_plugins_examples/ConflictDeleter`
- `diag/cosave.rs`: `CosaveConfig { save_ext, cosave_ext, se_plugin_dir }`. `cosave_config_for(GameKind)` — per-game table for all 9 supported SE games (SkyrimLE/SE/VR, EnderalSE, Fallout4, FalloutNV, Fallout3, Oblivion, Starfield); Morrowind returns `None`. `se_is_installed(mods_dir, se_plugin_dir)` — scans each mod directory for `.dll` files (case-insensitive) in the SE plugin subdirectory. `scan_missing_cosaves(saves_dir, mods_dir, config)` — skips entire scan when SE absent; finds saves without matching cosave; sorted output. `CosaveScanResult { missing_cosaves, se_detected, is_ok() }`. **17 unit tests.**
- `diag/overwrite.rs`: `FileCategory` struct with `name`, `mod_target`, `description`, `dir_markers`, `prefix_patterns`, `suffix_patterns`, `contains_patterns`, `exact_matches`. `CATEGORIES` static with 13 categories: Creation Club, DynDOLOD Output, TexGen Output, Nemesis Output, FNIS Output, BodySlide Output, xEdit Backups, SKSE Data, ENB / ReShade, Crash Logs, Synthesis Output, Bashed Patch, Smashed Patch. `scan_overwrite(overwrite_dir)` — recursive walk, first-match classification; `"Uncategorized"` bucket for unmatched files. `scan_overwrite_with_categories` for custom category slices. `OverwriteScanResult { by_category, total_files(), is_empty(), non_empty_categories() }`. **12 unit tests.**
- `conflict/prune.rs`: `PruneResult { moved, skipped_missing, errors; moved_count(), is_ok() }`. `prune_losers(conflict_map, mods_dir, backup_dir)` — iterates all conflict losers, copies `{mods_dir}/{mod_id}/{path}` → `{backup_dir}/{mod_id}/{path}`, removes original; missing files go to `skipped_missing`; copy-or-remove failures go to `errors`. Backup parent dirs created automatically. **8 unit tests.**
- `diag/mod.rs`: wires both submodules with `pub use` re-exports at `mantle_core::diag::*`. `lib.rs` updated with `pub mod diag`.
- **37 unit tests added** (17 cosave + 12 overwrite + 8 prune). **Running total: 465 passing, 11 ignored, 0 warnings** (`cargo clippy --workspace -- -D warnings` clean).

### UI launch, install, and polish — AdwNavigationSplitView / AdwToastOverlay / real launch / archive install (item z)
- **Date:** 2026-03-04
- **Reference:** `UI_GUIDE.md` §4.2, §5.2, `CODING_STANDARDS.md` §5.3
- **`AppState.steam_app_id: Option<u32>`** added — holds detected Steam App ID; used by the launch button; `None` = button disabled.
- **Game detection in `state_worker::load_state`**: calls `mantle_core::game::detect_all_steam()`, picks the first detected game, populates `game_name`, `launch_target`, and `steam_app_id`.  `game_version` deferred (requires EXE / manifest inspection).
- **`adw::NavigationSplitView`** replaces `gtk4::Paned` in `window.rs`.  Summary sidebar on the left (220–360 px, 25% fraction), `adw::ViewStack` on the right.  Auto-collapses at narrow widths (Steam Deck 1280×800 compliant — `UI_GUIDE.md` §4.2).
- **`adw::ToastOverlay`** wraps the split view — all content toasts rendered above page content without blocking the UI (`UI_GUIDE.md` §5.2).
- **Real launch button** (`wire_launch_button`): opens `steam://run/<app_id>` via `xdg-open`; button is `set_sensitive(false)` until game detection completes; label shows "▶  No Game Detected" / "▶  Launch \<name\>" appropriately.
- **"Install Mod" button** (`document-save-symbolic`) added to header bar; opens `gtk4::FileChooserNative` filtered to `*.zip / *.7z / *.rar`.
- **`install_mod_archive`** helper: derives mod folder name from archive stem; shows persistent "Installing…" `adw::Toast` (timeout = 0); spawns OS thread with a per-call `tokio::runtime::Runtime`; calls `mantle_core::archive::extract_archive`; delivers `Ok(name)` / `Err(msg)` back to GTK thread via `std::sync::mpsc` + `glib::idle_add_local`; dismisses pending toast and shows success (4 s) or error (6 s) toast.
- Fallback mods directory: `AppSettings::paths.mods_dir` or `<data_dir>/mods/` when no override configured.
- Partial empty directory cleanup on extraction failure (`std::fs::remove_dir`).
- `launch_button_label(state: &AppState) -> String` extracted helper.
- Deferred: `game_version` (EXE / manifest version probe); xSE/SKSE launch target detection; VFS overlay mount before launch; per-profile mod count batch query; live state refresh on user action.
- No new tests (UI-only — `UI_GUIDE.md` §9 compliance).
- **Running total: 392 passing, 0 failed, 0 warnings** (`cargo clippy --workspace -- -D warnings` clean).
### SKSEInstaller — automated script extender download and install (net feature)
- **Date:** 2026-03-05
- **Reference:** `mo2_linux_plugins_examples/SKSEInstaller/`, `crates/mantle_core/src/skse/`
- **Feature flag:** `--features net` (both `mantle_core` and `mantle_ui`); no-op when feature is absent.
- **`skse/config.rs`** — `SkseGameConfig` struct; `SKSE_GAME_MAP` static with 8 entries (SkyrimLE/SE/VR, EnderalSE, Fallout4/NV/3, Oblivion); `config_for_game(kind)` returns `None` for Morrowind and Starfield. 5 unit tests.
- **`skse/version.rs`** — `SkseVersion { major, minor, patch }` (Copy, PartialOrd, Display); `parse_version_str` handles both space-separated (`"2 2 6 0"`) and dot-separated (`"2.2.6"`) formats; `installed_version(game_dir, config)` reads `{game_dir}/{config.version_file}`; `async fn latest_version(config, timeout_secs)` HTTP GETs the version endpoint. 10 unit tests.
- **`skse/download.rs`** — `DownloadConfig { max_retries: 3, initial_backoff_ms: 1000, timeout_secs: 60 }`; `async fn download_file<F>(url, dest, cfg, progress)`; streams chunks into `tempfile::NamedTempFile` then `persist(dest)` (atomic); retries on network/5xx with exponential backoff; returns immediately on 4xx. 2 unit tests.
- **`skse/proton.rs`** — `write_dll_overrides(user_reg, dlls)` appends `"dll"="native,builtin"` entries to `[Software\Wine\DllOverrides]` in Proton prefix `user.reg`; idempotent; creates section if absent (with Unix timestamp); atomic write via `.tmp` + rename; no-ops silently when `user.reg` is absent (prefix not initialised). 5 unit tests.
- **`skse/mod.rs`** — `SkseInstallConfig`, `SkseInstallResult`, `SkseProgress` enum; `async fn install_skse(kind, cfg, progress)` 12-step pipeline: config lookup → version check → optional skip-if-current → download → magic-bytes validation → `extract_and_flatten` (strips single versioned top-level wrapper dir) → `normalize_dir` (case-fold for Linux FS) → loader presence validation → DLL overrides → version file write → temp cleanup.
- **`error.rs`** — `MantleError::Skse(String)` variant added.
- **UI** (`mantle_ui/src/window.rs`): `[cfg(feature = "net")]` "Script Extender" header bar button (`system-software-update-symbolic`); disabled until a supported game is detected; `wire_skse_button` / `run_skse_install` helpers follow OS-thread + mpsc + `glib::idle_add_local` pattern; progress shown via `adw::Toast` title mutation.
- **Tests:** 33 new unit tests across 4 new modules. `cargo test --package mantle_core --features net` — all passing. `cargo clippy --package mantle_core --features net -- -D warnings` clean.

### game/ini.rs — per-profile INI management (item d)
- **Date:** 2026-03-05
- **Reference:** path.md §d, commit e54d733
- `game/ini.rs`: `GameIni` struct backed by `IndexMap<String, IndexMap<String, String>>` for section-order preservation; hand-rolled parser (no extra dependency). `load(path)` returns an empty struct when the file is absent. `get(section, key)` is case-insensitive on both axes. `set(section, key, value)` creates the section if absent. `save()` and `save_to(path)` write back preserving comments and blank lines. `apply_profile_ini(profile_ini_dir, game_ini_dir)` bulk-copies every `*.ini` from the profile snapshot into the Proton prefix My Games dir, creating missing destination directories. `snapshot_profile_ini(game_ini_dir, profile_ini_dir)` captures current game INIs into the profile dir. `activate_profile` signature extended with `game_ini_dir: Option<&Path>` and `event_bus: &Arc<EventBus>`; applies INIs after overlay mount succeeds then emits `ProfileChanged`. **12 inline unit tests.**

### game/steam.rs — registry fallback in game detection (item e)
- **Date:** 2026-03-05
- **Reference:** path.md §e, commit e54d733
- `find_extra_install_path(pfx, app_id)` in `game/steam.rs`: loads `system.reg` from the Proton prefix via the existing `load_system_reg` helper, looks up `Software\\Valve\\Steam\\Apps\\{app_id}\\InstallPath`, converts Wine backslash path to `PathBuf`, returns `Some(path)` only when the path exists on disk. `detect_all` (and `detect_all_steam`) deduplicate the combined steamlocate + registry results by `steam_app_id` before returning, so steamlocate and registry agreement produces one canonical entry rather than a duplicate.

### state_worker.rs / window.rs — EventBus wiring (item f)
- **Date:** 2026-03-05
- **Reference:** path.md §f, commit e54d733
- `state_worker::spawn(sender: Sender<AppState>, event_bus: Arc<EventBus>)`: worker thread now stays alive after the initial load; subscribes to `ModEnabled`, `ModDisabled`, and `ProfileChanged` events; each handler calls `resend_state` which re-runs `load_state()` and pushes a fresh `AppState` snapshot. `trigger_reload` exposed as a direct call path for other callers. `Arc<EventBus>` constructed once in `window.rs::build_ui`, threaded through `build_main_content` and `apply_state_update` into each page builder that wires action buttons. `mods::build` publishes `ModEnabled`/`ModDisabled` on switch toggle; `profiles::build` publishes `ProfileChanged` on profile activation. `SubscriptionHandle`s owned by the worker thread for its full lifetime to prevent premature unsubscription.

