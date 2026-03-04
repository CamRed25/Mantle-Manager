# Mantle Manager — Coding Standards

> **Scope:** All Rust code in `mantle-manager/crates/`. Covers naming, error handling, unsafe rules, async patterns, FFI conventions, and toolchain configuration.
> **Last Updated:** Mar 3, 2026

---

## 1. Toolchain Configuration

### 1.1 Rust Edition

All crates use **Rust 2021 edition**.

```toml
# Every crate's Cargo.toml
[package]
edition = "2021"
```

Rationale: 2021 is the stable, well-documented standard. The 2024 edition is not yet fully adopted across the ecosystem. Upgrade when tooling and crate compatibility catch up — document the decision in `futures.md` when that time comes.

### 1.2 Minimum Supported Rust Version (MSRV)

```toml
# workspace Cargo.toml
[workspace.package]
rust-version = "1.75"
```

Do not use nightly features. Do not use unstable feature flags. All code must compile on stable.

### 1.3 Formatting — rustfmt

All code is formatted with `rustfmt`. A `rustfmt.toml` at the workspace root defines the configuration:

```toml
# rustfmt.toml
edition = "2021"
max_width = 100
use_small_heuristics = "Default"
imports_granularity = "Crate"     # nightly-only; silently ignored on stable
group_imports = "StdExternalCrate" # nightly-only; silently ignored on stable
reorder_imports = true
reorder_modules = true
newline_style = "Unix"
trailing_comma = "Vertical"       # nightly-only; silently ignored on stable
fn_call_width = 80
chain_width = 80
```

> **Note:** `imports_granularity`, `group_imports`, and `trailing_comma = "Vertical"` are nightly-only
> rustfmt keys. They are included for the day the project moves to a nightly formatter pass, but are
> silently ignored on stable. `cargo fmt --all -- --check` exits 0 on stable. See `conflict.md` CONFLICT-001.

**Rules:**
- `cargo fmt --all` must produce no diff before merge
- Do not suppress formatting with `#[rustfmt::skip]` except for hand-formatted tables or matrices where alignment aids readability — document why when used
- Line width is 100 characters — Rust's verbose type signatures make 80 too narrow

### 1.4 Linting — Clippy

Enforced lint groups at workspace level:

```toml
# workspace Cargo.toml
[workspace.lints.clippy]
pedantic = "warn"
correctness = "deny"
```

And in CI:

```bash
cargo clippy --workspace -- -D warnings
```

**Rules:**
- `clippy::correctness` violations are bugs — fix immediately
- `clippy::pedantic` warnings are code quality issues — fix when feasible
- When a pedantic lint is intentionally suppressed, annotate with `#[allow(clippy::lint_name)]` and a comment explaining why:
  ```rust
  // clippy::cast_possible_truncation: value is always < 256, verified by caller
  #[allow(clippy::cast_possible_truncation)]
  let byte = value as u8;
  ```
- Never use `#![allow(clippy::all)]` or `#![allow(warnings)]` at crate root

---

## 2. Naming Conventions

### 2.1 Standard Rust Naming

Follow Rust API Guidelines throughout:

| Item | Convention | Example |
|------|-----------|---------|
| Types, traits, enums | `UpperCamelCase` | `ModList`, `VfsBackend` |
| Functions, methods | `snake_case` | `mount_overlay`, `find_game` |
| Variables, parameters | `snake_case` | `mod_path`, `lower_dirs` |
| Constants | `SCREAMING_SNAKE_CASE` | `MAX_LAYERS`, `UNMOUNT_TIMEOUT` |
| Modules | `snake_case` | `vfs`, `mod_list` |
| Crates | `snake_case` | `mantle_core`, `mantle_ui` |
| Lifetimes | short lowercase | `'a`, `'ctx`, `'buf` |
| Type parameters | single uppercase or short | `T`, `E`, `K`, `V` |

### 2.2 Mantle-Specific Conventions

- **Event types** use past tense for completed events, present participle for in-progress:
  - `ModInstalled`, `GameExited` — completed
  - `GameLaunching`, `ModLoading` — in-progress
- **Error types** are named for the operation that failed, suffixed with `Error`:
  - `MountError`, `ArchiveExtractError`, `PluginLoadError`
- **Builder types** are suffixed with `Builder`: `OverlayBuilder`, `ModListBuilder`
- **Config/settings types** are suffixed with `Config` or `Settings`: `VfsConfig`, `AppSettings`
- **Trait implementations** follow the standard: `impl Display for ModInfo`

### 2.3 Module-Level Naming

Public API items exported from a module must be named so they read clearly at the call site without the module prefix being required for disambiguation:

```rust
// Good — reads clearly as mantle_core::vfs::mount
pub fn mount(config: VfsConfig) -> Result<VfsMount, MountError>

// Avoid — redundant prefix when called as mantle_core::vfs::vfs_mount
pub fn vfs_mount(config: VfsConfig) -> Result<VfsMount, MountError>
```

---

## 3. Error Handling

### 3.1 Core Philosophy

**Errors are values, not exceptions.** Every fallible function returns `Result<T, E>`. Errors are propagated explicitly, annotated with context, and handled at the boundary closest to the user.

Silent failure is never acceptable. A function that swallows an error and returns `Ok(())` is a bug.

### 3.2 Library vs Application Errors

| Layer | Crate | Pattern |
|-------|-------|---------|
| Library (mantle_core, mantle_archive) | `thiserror` | Typed error enums, structured variants |
| Application (mantle_ui, bin targets) | `anyhow` | Context-annotated propagation |

```rust
// mantle_core — typed library error
#[derive(Debug, thiserror::Error)]
pub enum MountError {
    #[error("no overlay backend available: fuse-overlayfs not found and kernel mount not supported")]
    NoBackend,
    #[error("mount point {path} is not a directory")]
    InvalidMountPoint { path: PathBuf },
    #[error("fuse-overlayfs process failed: {0}")]
    FuseProcess(#[from] std::io::Error),
}

// mantle_ui — application propagation with context
fn launch_game(game: &GameInfo) -> anyhow::Result<()> {
    mount_overlay(&config)
        .context("failed to mount overlay before game launch")?;
    Ok(())
}
```

### 3.3 The ? Operator

Use `?` for propagation. Do not use `.unwrap()` or `.expect()` in production code paths.

```rust
// Good
let contents = fs::read_to_string(&path)?;

// Never in production
let contents = fs::read_to_string(&path).unwrap();
```

`.expect()` is permitted **only** in:
- `#[cfg(test)]` blocks
- `main()` for fatal startup failures where there is genuinely no recovery
- Initialization of static/const data where failure is impossible by construction

When `.expect()` is used outside tests, the message must explain the invariant that guarantees it cannot fail:

```rust
// Acceptable in main() — explains why this cannot fail
let config = Config::default().expect("default config is always valid");
```

### 3.4 Error Context

Always add context when propagating errors across module boundaries:

```rust
use anyhow::Context;

fn load_mod_metadata(path: &Path) -> anyhow::Result<ModMetadata> {
    let data = fs::read(path)
        .with_context(|| format!("failed to read mod metadata from {}", path.display()))?;
    serde_json::from_slice(&data)
        .with_context(|| format!("failed to parse mod metadata in {}", path.display()))
}
```

### 3.5 Never Silently Discard Errors

```rust
// Never — silently swallows the error
let _ = cleanup_stale_mount(path);

// Correct — log and continue if non-fatal
if let Err(e) = cleanup_stale_mount(path) {
    tracing::warn!("stale mount cleanup failed for {}: {}", path.display(), e);
}

// Correct — propagate if fatal
cleanup_stale_mount(path)
    .context("failed to clean up stale mount on startup")?;
```

---

## 4. Cargo and Dependencies

### 4.1 Workspace Structure

All crates are members of the workspace root `Cargo.toml`. Shared dependencies are declared at workspace level:

```toml
# workspace Cargo.toml
[workspace]
members = [
    "crates/mantle_core",
    "crates/mantle_ui",
]
resolver = "2"

[workspace.dependencies]
# Pin minor versions per §4.2 — "~X.Y" means >= X.Y.0, < X.(Y+1).0
tokio = { version = "~1.36", features = ["full"] }
serde = { version = "~1.0", features = ["derive"] }
thiserror = "~1.0"
anyhow = "~1.0"
tracing = "~0.1"
```

```toml
# crate Cargo.toml — reference workspace versions
[dependencies]
tokio.workspace = true
serde.workspace = true
```

### 4.2 Version Pinning Policy

| Dependency Class | Pinning Policy |
|-----------------|---------------|
| Core dependencies (tokio, serde, gtk4) | Pin minor version: `"~1.75"` not `"1"` — `"~1.75"` means `>= 1.75.0, < 1.76.0`; `"1.75"` is equivalent to `"^1.75"` which allows any `1.x` |
| Utility crates | Pin major version: `"1"` is acceptable |
| C FFI wrappers (libarchive, libloot) | Pin exact version until tested: `"=3.7.2"` |
| Dev/test dependencies | Pin major version |

Never use `*` version specifications.

### 4.3 Feature Flags

Optional features must not affect the core build:

```toml
[features]
default = []
net = ["dep:reqwest", "dep:tokio-rustls"]   # Nexus API, mod browser
```

`mantle_core` and `mantle_ui` must build and all tests must pass with `--no-default-features`. The `net` feature is additive only.

### 4.4 build.rs Rules

`build.rs` is permitted only for:
- Linking C libraries (libarchive, libloot, fuse-overlayfs)
- Code generation (protobuf, flatbuffers if adopted)
- Compile-time platform detection

No arbitrary logic, no network access, no file downloads in `build.rs`.

---

## 5. Async Patterns

### 5.1 Tokio Runtime

One `tokio` runtime per process, created in `main()`. Do not create secondary runtimes.

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ...
}
```

### 5.2 Blocking Operations

Never call blocking operations on the async executor thread. Use `spawn_blocking` for:
- Filesystem operations on large directories
- SQLite queries (rusqlite is synchronous)
- Archive extraction
- Any operation that may take > 1ms

```rust
// Good — blocking work off the async thread
let metadata = tokio::task::spawn_blocking(move || {
    read_mod_metadata_sync(&path)
}).await??;

// Never — blocks the executor
let metadata = read_mod_metadata_sync(&path)?;
```

### 5.3 GTK4 Main Loop Integration

The GTK4 main loop runs on the main thread. Cross-thread communication uses channels:

```rust
// Send results from tokio back to GTK4 main thread
let (tx, rx) = tokio::sync::oneshot::channel();
tokio::spawn(async move {
    let result = download_mod(url).await;
    let _ = tx.send(result);
});
// GTK4 side polls or uses glib::MainContext::channel
```

Do not block the GTK4 main thread waiting for async operations.

### 5.4 Cancellation

Long-running operations must support cancellation via `tokio::select!` or `CancellationToken`:

```rust
use tokio_util::sync::CancellationToken;

async fn download_mod(url: &str, cancel: CancellationToken) -> Result<(), DownloadError> {
    tokio::select! {
        result = do_download(url) => result,
        _ = cancel.cancelled() => Err(DownloadError::Cancelled),
    }
}
```

---

## 6. Unsafe Code

### 6.1 Policy

`unsafe` blocks are permitted only when:

1. Interfacing with C libraries via FFI (libarchive, libloot, fuse-overlayfs bindings)
2. Performance-critical kernel syscall wrappers where safe alternatives are unavailable
3. The block is accompanied by a `// SAFETY:` comment

Every `unsafe` block without a `// SAFETY:` comment is a bug.

```rust
// Good
// SAFETY: `ptr` is non-null and valid for `len` bytes, as guaranteed by
// libarchive's contract that entry_data is valid until the next read call.
let slice = unsafe { std::slice::from_raw_parts(ptr, len) };

// Bug — no safety justification
let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
```

### 6.2 FFI Wrappers

All C library calls must be wrapped in safe Rust APIs. Raw FFI must not appear in business logic:

```rust
// ffi.rs — raw bindings, internal only
mod ffi {
    extern "C" {
        fn archive_read_new() -> *mut archive;
    }
}

// archive.rs — safe wrapper, public API
pub struct Archive { inner: *mut ffi::archive }

impl Archive {
    pub fn new() -> Result<Self, ArchiveError> {
        let ptr = unsafe {
            // SAFETY: archive_read_new() returns a valid pointer or null.
            // Null is handled immediately below.
            ffi::archive_read_new()
        };
        if ptr.is_null() {
            return Err(ArchiveError::AllocationFailed);
        }
        Ok(Self { inner: ptr })
    }
}
```

### 6.3 Unsafe in Plugin Code

Mantle Manager cannot enforce what a native `.so` plugin author puts in their compiled binary. The enforceable rule is at the API boundary:

**Plugins must not use `unsafe` to bypass `PluginContext`.** Specifically:

- Plugins must not cast raw pointers obtained from `PluginContext` to reach internal core structs
- Plugins must not use `unsafe` transmutes or pointer arithmetic to access state outside their sanctioned interface
- Plugins must not call internal core functions via raw function pointers

The `PluginContext` API is designed so that correct, safe usage does not require `unsafe`. A plugin that uses `unsafe` to work around the API boundary is exploiting an implementation detail, not a sanctioned interface — it will break without warning when core internals change.

Rhai scripts are fully sandboxed — `unsafe` is not expressible in Rhai and cannot be introduced by script authors.

---

## 7. Documentation

### 7.1 Doc Comments

Every public item must have a doc comment:

```rust
/// Mounts an overlay filesystem combining the given mod layers.
///
/// Selects the best available backend (kernel overlayfs, fuse-overlayfs,
/// or symlink farm) based on the current environment.
///
/// # Errors
///
/// Returns [`MountError::NoBackend`] if no overlay backend is available.
/// Returns [`MountError::FuseProcess`] if fuse-overlayfs fails to start.
///
/// # Panics
///
/// Does not panic. All error conditions return `Err`.
pub fn mount(config: VfsConfig) -> Result<VfsMount, MountError> {
```

Required sections for non-trivial public functions:
- Description (first line, concise)
- `# Errors` — every `Err` variant that can be returned
- `# Panics` — if the function can panic, explain when; if it cannot, say so
- `# Examples` — for public API surface that benefits from usage demonstration

### 7.2 Inline Comments

Comment the *why*, not the *what*:

```rust
// Good — explains the decision
// Defer mount discovery by 2s — the VFS backend probe races with
// Steam library scanning on startup; give steamlocate time to finish.
tokio::time::sleep(Duration::from_secs(2)).await;
vfs.discover_mounts().await?;

// Useless — restates the code
// Sleep for 2 seconds
tokio::time::sleep(Duration::from_secs(2)).await;
```

### 7.3 TODO / FIXME Policy

`TODO` and `FIXME` comments in committed code must reference a `futures.md` entry:

```rust
// TODO(futures.md#nested-stacking): implement nested overlay for > 500 mods
```

Standalone `TODO` comments with no reference are not permitted — they become invisible debt.

---

## 8. Testing

### 8.1 Unit Tests

Unit tests live in `#[cfg(test)]` modules in the same file as the code under test:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mount_options_handles_bool_flags() {
        let opts = parse_mount_options("rw,noatime,size=100m");
        assert_eq!(opts["rw"], MountOpt::Flag);
        assert_eq!(opts["size"], MountOpt::Value("100m".into()));
    }
}
```

### 8.2 Integration Tests

Integration tests live in `crates/<crate>/tests/`. They test public API only — no `use super::*` in integration tests.

### 8.3 Test Naming

Test names are full sentences describing the behavior under test:

```rust
#[test]
fn mount_overlay_returns_no_backend_when_fuse_binary_missing() { }

#[test]
fn mod_list_priority_orders_highest_first() { }
```

### 8.4 No Test Suppression

Do not `#[ignore]` tests without a comment explaining why and what condition will un-ignore them. Do not delete failing tests — fix them.

> **Full test suite structure, skip policy, and Steam Deck verification:** [TESTING_GUIDE.md](TESTING_GUIDE.md)

---

## 9. Logging

### 9.1 tracing Conventions

Use `tracing` macros throughout. Never use `println!` for diagnostic output in library code.

```rust
use tracing::{debug, info, warn, error};

// Structured fields preferred over format strings
tracing::info!(mod_count = mods.len(), profile = %profile_name, "overlay mount starting");
tracing::warn!(path = %mount_path, "stale mount detected on startup");
tracing::error!(error = %e, path = %path, "archive extraction failed");
```

### 9.2 Log Levels

| Level | Use For |
|-------|---------|
| `error` | Unrecoverable failures visible to the user |
| `warn` | Recoverable problems, degraded operation, unexpected state |
| `info` | Significant lifecycle events (mount, unmount, game launch, mod install) |
| `debug` | Detailed operational flow for developer diagnosis |
| `trace` | High-frequency events (per-file operations, tight loops) — off by default |

### 9.3 Sensitive Data

Never log:
- API keys or authentication tokens
- Full filesystem paths that contain usernames (truncate or hash)
- Mod file contents

---

## 10. Platform and Linux-Specific Code

### 10.1 Kernel Version Checks

Kernel version checks use the `nix` crate, never shell out to `uname`:

```rust
use nix::sys::utsname::uname;

fn kernel_version() -> (u32, u32, u32) {
    let uts = uname().expect("uname always succeeds on Linux");
    parse_kernel_version(uts.release().to_string_lossy().as_ref())
}
```

### 10.2 Flatpak Detection

```rust
fn is_flatpak() -> bool {
    std::path::Path::new("/.flatpak-info").exists()
}
```

This check is performed once at startup and cached. Do not call it in hot paths.

### 10.3 Syscall Wrappers

Direct syscalls use the `nix` crate. Raw `libc` calls are permitted only when `nix` does not cover the required syscall. All raw `libc` calls require a `// SAFETY:` comment per §6.1.

### 10.4 Proton/Wine Paths

Proton prefix paths contain the Steam user ID which must not be hardcoded. Always use `steamlocate` for discovery:

```rust
use steamlocate::SteamDir;

let steam = SteamDir::locate()?;
let app = steam.app(&489830)?; // Skyrim SE
```

---

## 11. Cross-References

| Topic | Standard |
|-------|----------|
| Governance and enforcement | [RULE_OF_LAW.md](RULE_OF_LAW.md) |
| Module structure and crate graph | [ARCHITECTURE.md](ARCHITECTURE.md) |
| Plugin contract and boundaries | [PLUGIN_API.md](PLUGIN_API.md) |
| VFS and overlay design | [VFS_DESIGN.md](VFS_DESIGN.md) |
| Build prerequisites and Cargo workspace | [BUILD_GUIDE.md](BUILD_GUIDE.md) |
| Test suite structure and skip policy | [TESTING_GUIDE.md](TESTING_GUIDE.md) |
| GTK4 and UI conventions | [UI_GUIDE.md](UI_GUIDE.md) |
