# Mantle Manager — Testing Guide

> **Scope:** Test suite structure, test types, skip policy, required coverage by change type, and Steam Deck simulation methods.
> **Last Updated:** Mar 3, 2026

---

## 1. Philosophy

Tests verify behavior from the **user perspective**. A function that compiles and returns without panicking is not tested — it is type-checked. A test must assert that the function produces the correct output for a given input, including failure cases.

**Rules (from RULE_OF_LAW §3.3):**
- Every new Rust function has a unit test in a `#[cfg(test)]` block
- Every failing test is fixed, not deleted or ignored
- Tests are not removed to make a build pass — see RULE_OF_LAW §3.2
- `#[ignore]` requires a comment explaining the condition that will un-ignore it

---

## 2. Test Types

### 2.1 Unit Tests

Live in `#[cfg(test)]` modules within the source file under test:

```rust
// src/vfs/backend/kernel.rs

pub fn parse_kernel_version(release: &str) -> (u32, u32, u32) {
    // ... implementation
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kernel_version_extracts_major_minor_patch() {
        assert_eq!(parse_kernel_version("6.6.0-rc1-gentoo"), (6, 6, 0));
    }

    #[test]
    fn parse_kernel_version_handles_steamdeck_release_string() {
        assert_eq!(parse_kernel_version("6.1.52-valve16-1-neptune"), (6, 1, 52));
    }

    #[test]
    fn parse_kernel_version_returns_zeros_on_invalid_input() {
        assert_eq!(parse_kernel_version("not-a-version"), (0, 0, 0));
    }
}
```

**Rules:**
- Three or more test cases per function — happy path, failure path, edge case
- Use `tempfile::TempDir` for any test that needs a real filesystem
- No global mutable state in tests — tests must be order-independent

### 2.2 Integration Tests

Live in `crates/<crate>/tests/`. Access only the public API — no `use super::*`.

```
crates/
└── mantle_core/
    └── tests/
        ├── vfs_mount_lifecycle.rs   ← requires FUSE or real kernel
        ├── archive_extraction.rs    ← requires actual archive files
        ├── conflict_detection.rs    ← pure computation, no I/O
        └── data_migrations.rs       ← requires temp SQLite file
```

Integration tests that require system resources (FUSE, kernel mounts) must check availability at test startup and skip gracefully if unavailable — see §5 Skip Policy.

### 2.3 Trait Contract Tests

Every trait defined in `mantle_core` or `mantle_plugin` has a contract test that exercises the trait's required behavior:

```rust
// crates/mantle_core/tests/plugin_contract.rs

struct MinimalPlugin;
impl MantlePlugin for MinimalPlugin { /* ... */ }

#[test]
fn plugin_init_and_shutdown_run_without_error() {
    let mut plugin = MinimalPlugin;
    let ctx = Arc::new(test_plugin_context());
    plugin.init(ctx).expect("init must not fail for minimal plugin");
    plugin.shutdown();
}
```

---

## 3. Required Coverage by Change Type

| Change Type | Required Tests | Location |
|-------------|---------------|----------|
| New Rust function | Unit test — happy path + failure + edge case | Same file, `#[cfg(test)]` |
| New plugin trait method | Trait contract test | `crates/mantle_core/tests/` |
| New `ModManagerEvent` variant | Event dispatch integration test | `crates/mantle_core/tests/` |
| SQLite schema change | Migration test + data round-trip | `crates/mantle_core/tests/data_migrations.rs` |
| VFS backend change | Mount/unmount cycle on real kernel or FUSE | `crates/mantle_core/tests/vfs_mount_lifecycle.rs` |
| Archive handling change | Extraction test with real archive | `crates/mantle_core/tests/archive_extraction.rs` |
| GTK4 widget | Visual smoke test at 1280×800 | Manual, record in PR description |
| Build change | `cargo build --workspace` clean | CI |
| Flatpak change | `flatpak-builder --dry-run` | CI |
| Conflict detection logic | Unit tests with known conflict scenarios | Same file, `#[cfg(test)]` |

---

## 4. Test Helpers and Fixtures

### 4.1 Temp Database

```rust
// Common helper — available in tests/ as test_utils::temp_db()
pub fn temp_db() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory()
        .expect("in-memory db always succeeds");
    mantle_core::data::run_migrations(&conn)
        .expect("migrations must apply cleanly to fresh db");
    conn
}
```

### 4.2 Temp Mod Directory

```rust
pub fn temp_mod_dir() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("tempdir creation");
    // Populate with a minimal mod structure
    std::fs::create_dir(dir.path().join("Data")).unwrap();
    std::fs::write(dir.path().join("Data/test.esp"), b"TES4\x00").unwrap();
    dir
}
```

### 4.3 Test PluginContext

```rust
pub fn test_plugin_context() -> PluginContext {
    PluginContext::new_for_testing(
        Arc::new(RwLock::new(ModList::empty())),
        Arc::new(RwLock::new(Profile::default())),
        None, // no game
        Arc::new(EventBus::new()),
    )
}
```

---

## 5. Skip Policy

Tests that require real kernel features, hardware, or system binaries must skip gracefully rather than fail when the requirement is unavailable.

```rust
#[test]
fn kernel_overlayfs_mount_succeeds_on_supported_kernel() {
    if !mantle_core::vfs::has_new_mount_api() {
        eprintln!("SKIP: new mount API not available on this kernel");
        return;
    }
    // ... test body
}

#[test]
fn fuse_overlayfs_mount_succeeds_when_binary_present() {
    if !mantle_core::vfs::fuse_overlayfs_available() {
        eprintln!("SKIP: fuse-overlayfs binary not found");
        return;
    }
    // ... test body
}
```

**Rules:**
- Skip by returning early — not by panicking
- Print a `SKIP:` message to stderr so CI logs show why the test did not run
- Do not use `#[ignore]` for system-availability skips — use runtime checks
- `#[ignore]` is reserved for tests that are explicitly deferred to a future state, with a comment

---

## 6. Migration Tests

Every schema migration must be tested with:

1. **Clean apply test** — apply migration to empty database, verify schema
2. **Data round-trip test** — insert test data, apply migration, verify data survives

```rust
#[test]
fn migration_002_adds_conflicts_table_and_preserves_mods() {
    let conn = apply_migrations_up_to(1); // Apply only migration 1
    insert_test_mod(&conn, "test-mod");

    apply_migration(&conn, 2); // Apply migration 2

    // Verify schema
    let table_exists: bool = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='conflicts'",
        [],
        |row| row.get(0),
    ).unwrap();
    assert!(table_exists, "conflicts table must exist after migration 2");

    // Verify data survived
    let mod_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM mods",
        [],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(mod_count, 1, "existing mod must survive migration 2");
}
```

---

## 7. Steam Deck Simulation

Steam Deck-specific behavior must be verified before any release. The Deck runs SteamOS with kernel 6.1 and requires the Flatpak path.

### 7.1 UI Layout at 1280×800

Every GTK4 widget change must be verified at 1280×800 before merge. On a development machine:

```bash
# Set window size explicitly in development builds
# Add to main() when running in dev mode:
window.set_default_size(1280, 800);
```

Or resize the window manually. Check:
- No UI elements are clipped or overflow the window
- All buttons and controls are reachable without scrolling
- Text is readable at the default font size

### 7.2 Flatpak Path Simulation

To simulate the Flatpak environment locally:

```bash
# Build the Flatpak
flatpak-builder --force-clean build-dir packaging/io.mantlemanager.MantleManager.yml

# Install locally
flatpak-builder --user --install build-dir packaging/io.mantlemanager.MantleManager.yml

# Run
flatpak run io.mantlemanager.MantleManager
```

### 7.3 Kernel 6.1 VFS Behavior

To simulate kernel 6.1 behavior (fuse-overlayfs path, no new mount API):

```rust
// In vfs/mod.rs — development override
#[cfg(feature = "simulate-deck")]
fn select_backend() -> VfsBackend {
    VfsBackend::FuseOverlayfs // Force fuse path regardless of actual kernel
}
```

Add `simulate-deck` as a dev-only feature flag. Never ship code with this flag enabled in production.

### 7.4 Actual Steam Deck Testing

Before any significant release, test on hardware or via SSH:

```bash
# SSH into Deck in desktop mode
ssh deck@steamdeck.local

# Run the Flatpak build
flatpak run io.mantlemanager.MantleManager
```

At minimum verify:
- Application launches without error
- Mod list displays and scrolls correctly
- A mod can be installed and activated
- Game launch (with overlay mount) succeeds
- Game exit cleans up the overlay mount

---

## 8. Running the Test Suite

```bash
# All tests
cargo test --workspace

# Specific crate
cargo test -p mantle_core

# Specific test
cargo test -p mantle_core parse_kernel_version

# With output (for SKIP messages)
cargo test --workspace -- --nocapture

# Integration tests only
cargo test -p mantle_core --test vfs_mount_lifecycle

# Migration tests
cargo test -p mantle_core data::migrations
```

---

## 9. Cross-References

| Topic | Standard |
|-------|----------|
| Governance and test mandate | [RULE_OF_LAW.md §3.3](RULE_OF_LAW.md) |
| Test naming conventions | [CODING_STANDARDS.md §8.3](CODING_STANDARDS.md) |
| Test suppression policy | [CODING_STANDARDS.md §8.4](CODING_STANDARDS.md) |
| Migration schema | [DATA_MODEL.md §4](DATA_MODEL.md) |
| VFS backend availability checks | [VFS_DESIGN.md §3](VFS_DESIGN.md) |
| Steam Deck platform details | [PLATFORM_COMPAT.md](PLATFORM_COMPAT.md) |
| Build verification protocol | [BUILD_GUIDE.md §3.3](BUILD_GUIDE.md) |
