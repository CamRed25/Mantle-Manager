# Mantle Manager — Build Guide

> **Scope:** Build prerequisites, Cargo workspace setup, dependency compilation, Flatpak packaging, and CI build steps.
> **Last Updated:** Mar 3, 2026

---

## 1. Prerequisites

### 1.1 Rust Toolchain

```bash
# Install rustup if not present
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install the pinned stable toolchain (matches workspace rust-version)
rustup toolchain install stable
rustup default stable

# Required components
rustup component add clippy rustfmt
```

The workspace `rust-version` field in `Cargo.toml` enforces the minimum. Do not use nightly.

### 1.2 System Dependencies

These C libraries must be present on the build host. They are linked at compile time.

| Library | Package (Fedora) | Package (Ubuntu/Debian) | Package (Arch) | Notes |
|---------|-----------------|------------------------|----------------|-------|
| libarchive | `libarchive-devel` | `libarchive-dev` | `libarchive` | Archive extraction |
| libloot | Build from source | Build from source | AUR: `libloot` | Plugin sorting |
| fuse3 | `fuse3-devel` | `libfuse3-dev` | `fuse3` | FUSE backend |
| sqlite3 | `sqlite-devel` | `libsqlite3-dev` | `sqlite` | rusqlite bundled feature bypasses this |
| pkg-config | `pkgconf` | `pkg-config` | `pkgconf` | Required by build.rs files |

**SQLite note:** `rusqlite` can bundle its own SQLite via the `bundled` feature flag, which avoids the system dependency. Use `bundled` for Flatpak builds. Use the system library for development builds.

```toml
# Cargo.toml — Flatpak build
rusqlite = { version = "~0.31", features = ["bundled"] }

# Cargo.toml — development (system sqlite)
rusqlite = { version = "~0.31" }
```

### 1.3 GTK4 and libadwaita

```bash
# Fedora
sudo dnf install gtk4-devel libadwaita-devel

# Ubuntu 24.04+
sudo apt install libgtk-4-dev libadwaita-1-dev

# Arch
sudo pacman -S gtk4 libadwaita
```

### 1.4 libloot — Build from Source

`libloot` is not available in most distribution package managers. Build from source:

```bash
git clone https://github.com/loot/libloot
cd libloot
mkdir build && cd build
cmake .. -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr/local
make -j$(nproc)
sudo make install
sudo ldconfig
```

Pin the specific commit hash used in `futures.md` when updating libloot. Document the version used and any API changes.

---

## 2. Cargo Workspace

### 2.1 Workspace Root

```toml
# Cargo.toml (workspace root)
[workspace]
members = [
    "crates/mantle_core",
    "crates/mantle_ui",
]
resolver = "2"

[workspace.package]
edition = "2021"
rust-version = "1.75"
authors = ["Mantle Manager Contributors"]
license = "GPL-3.0-or-later"
repository = "https://github.com/mantle-manager/mantle-manager"

[workspace.lints.clippy]
pedantic = "warn"
correctness = "deny"

[workspace.dependencies]
# Pin minor versions for core dependencies — see CODING_STANDARDS.md §4.2
tokio = { version = "~1.36", features = ["full"] }
serde = { version = "~1.0", features = ["derive"] }
serde_json = "~1.0"
toml = "~0.8"
thiserror = "~1.0"
anyhow = "~1.0"
tracing = "~0.1"
tracing-subscriber = { version = "~0.3", features = ["env-filter"] }
rusqlite = "~0.31"
once_cell = "~1.19"
semver = { version = "~1.0", features = ["serde"] }
xxhash-rust = { version = "~0.8", features = ["xxh3"] }
rayon = "~1.8"
nix = { version = "~0.28", features = ["mount", "process", "user"] }
notify = "~6.1"
steamlocate = "~2.0"
esplugin = "~6.0"
reqwest = { version = "~0.11", features = ["json", "stream"], optional = true }
libloading = "~0.8"
rhai = "~1.17"
gtk4 = "~0.8"
libadwaita = "~0.6"
```

### 2.2 Crate Cargo.toml Template

```toml
# crates/mantle_core/Cargo.toml
[package]
name = "mantle_core"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
authors.workspace = true
license.workspace = true

[lints]
workspace = true

[dependencies]
tokio.workspace = true
serde.workspace = true
thiserror.workspace = true
anyhow.workspace = true
tracing.workspace = true
# ... add as needed

[dev-dependencies]
tokio = { workspace = true, features = ["test-util"] }
tempfile = "3"
```

---

## 3. Building

### 3.1 Development Build

```bash
# Standard incremental build
cargo build --workspace

# Release build (optimized — use for Flatpak packaging)
cargo build --workspace --release
```

### 3.2 Check and Lint

```bash
# Type check without producing binaries (faster than build)
cargo check --workspace

# Clippy — enforced lint level
cargo clippy --workspace -- -D warnings

# Formatting check
cargo fmt --all -- --check
```

### 3.3 Full Verification Protocol

Run this before any merge:

```bash
# 1. Clean incremental build
cargo build --workspace

# 2. Clippy
cargo clippy --workspace -- -D warnings

# 3. Formatting
cargo fmt --all -- --check

# 4. Dependency tree (if Cargo.toml changed)
cargo tree | grep -E "duplicate"

# 5. Full test suite
cargo test --workspace

# 6. Migration tests (if data model changed)
cargo test -p mantle_core data::migrations
```

All steps must exit 0.

---

## 4. Feature Flags

```toml
# crates/mantle_core/Cargo.toml
[features]
default = []
net = ["dep:reqwest", "dep:tokio-rustls"]
```

Build without optional features:
```bash
cargo build --workspace --no-default-features
cargo test --workspace --no-default-features
```

Both must succeed. The `net` feature is strictly additive.

---

## 5. Environment Variables

| Variable | Purpose | Default |
|----------|---------|---------|
| `MANTLE_LOG` | Log level filter for `tracing-subscriber` | `info` |
| `MANTLE_DATA_DIR` | Override app data directory | XDG standard |
| `MANTLE_CONFIG_DIR` | Override config directory | XDG standard |
| `RUST_BACKTRACE` | Enable backtraces on panic | `0` |

Set `MANTLE_LOG=debug` during development for verbose output.

---

## 6. Cross-Compilation (Steam Deck)

Steam Deck (SteamOS) is x86_64. No cross-compilation target is needed — the Flatpak build runs on the same architecture.

If cross-compilation to a different target is ever needed, document the target triple and toolchain setup in `futures.md` before adding it here.

---

## 7. C Library build.rs

Each crate that links a C library has a `build.rs` that invokes `pkg-config`:

```rust
// crates/mantle_core/build.rs
fn main() {
    // libarchive
    let lib = pkg_config::probe_library("libarchive")
        .expect("libarchive not found — install libarchive-devel");
    for path in &lib.include_paths {
        println!("cargo:include={}", path.display());
    }

    // Emit rustc version for plugin ABI enforcement
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = std::process::Command::new(&rustc)
        .arg("--version")
        .output()
        .expect("failed to run rustc --version");
    let version = String::from_utf8(out.stdout).expect("rustc output is utf8");
    println!("cargo:rustc-env=RUSTC_VERSION_STRING={}", version.trim());
}
```

`build.rs` must not perform network requests, download files, or run arbitrary shell scripts.

---

## 8. Flatpak Packaging

### 8.1 Manifest Location

```
packaging/
└── io.mantlemanager.MantleManager.yml
```

### 8.2 Build Command

```bash
# Dry run — validates manifest without producing output
flatpak-builder --dry-run build-dir packaging/io.mantlemanager.MantleManager.yml

# Full build
flatpak-builder --force-clean build-dir packaging/io.mantlemanager.MantleManager.yml
```

### 8.3 Runtime and SDK

```yaml
runtime: org.gnome.Platform
runtime-version: '48'
sdk: org.gnome.Sdk
```

GNOME Platform 48 includes GTK 4.16 and libadwaita 1.6. Update the runtime version when GNOME releases a new stable version — document the change in `futures.md`.

### 8.4 Rust Build Module

```yaml
modules:
  - name: mantle-manager
    buildsystem: simple
    build-commands:
      - cargo build --release --no-default-features
      - install -Dm755 target/release/mantle_ui /app/bin/mantle-manager
    sources:
      - type: dir
        path: .
```

### 8.5 Sandbox Permissions

```yaml
finish-args:
  - --share=ipc
  - --socket=fallback-x11
  - --socket=wayland
  - --device=dri
  # Steam library access
  - --filesystem=~/.steam:ro
  - --filesystem=~/.local/share/Steam:ro
  # Game directories — broad access required
  - --filesystem=home
  # FUSE for fuse-overlayfs
  - --device=fuse
  # Host escape for fuse-overlayfs binary (if not bundled)
  - --talk-name=org.freedesktop.Flatpak
```

`--filesystem=home` is broad. Narrowed access is tracked in `futures.md` as a hardening item.

### 8.6 fuse-overlayfs in Flatpak

`fuse-overlayfs` must either be bundled in the Flatpak or accessed via `flatpak-spawn --host`. Bundling is preferred to avoid depending on the host system having it installed.

Add as a build module in the Flatpak manifest:

```yaml
  - name: fuse-overlayfs
    buildsystem: autotools
    sources:
      - type: git
        url: https://github.com/containers/fuse-overlayfs
        tag: v1.14
```

---

## 9. CI

CI runs the full verification protocol from §3.3 on every push. Minimum CI steps:

```yaml
# .github/workflows/ci.yml (conceptual)
steps:
  - cargo build --workspace
  - cargo clippy --workspace -- -D warnings
  - cargo fmt --all -- --check
  - cargo test --workspace
  - cargo build --workspace --no-default-features
  - flatpak-builder --dry-run build-dir packaging/*.yml
```

CI does not deploy. Deployment is manual.

---

## 10. Cross-References

| Topic | Standard |
|-------|----------|
| Governance and enforcement | [RULE_OF_LAW.md](RULE_OF_LAW.md) |
| Cargo version pinning policy | [CODING_STANDARDS.md §4.2](CODING_STANDARDS.md) |
| Workspace crate structure | [ARCHITECTURE.md §2](ARCHITECTURE.md) |
| build.rs unsafe rules | [CODING_STANDARDS.md §4.4](CODING_STANDARDS.md) |
| Flatpak paths in VFS | [VFS_DESIGN.md §3](VFS_DESIGN.md) |
| Distro package availability | [PLATFORM_COMPAT.md](PLATFORM_COMPAT.md) |
