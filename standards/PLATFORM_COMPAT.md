# Mantle Manager — Platform Compatibility

> **Scope:** Supported Linux distributions, kernel version gates, Steam Deck specifics, and runtime detection methods.
> **Last Updated:** Mar 3, 2026

---

## 1. Overview

Mantle Manager targets Linux exclusively. No Windows or macOS support is planned or will be accepted. Every feature is designed to work on the Flatpak path — native installation is a convenience, not the primary target.

---

## 2. Platform Support Matrix

| Platform | Status | Kernel | Install Method | Notes |
|----------|--------|--------|---------------|-------|
| SteamOS 3.x (Steam Deck) | **Primary target** | 6.1 | Flatpak only | Immutable OS, fuse-overlayfs path |
| Fedora 40+ | **Primary development** | 6.8+ | Native + Flatpak | Development platform |
| Ubuntu 24.04 LTS | Supported | 6.8 | Native + Flatpak | |
| Arch Linux | Supported | Rolling | Native + Flatpak | Always current kernel |
| openSUSE Tumbleweed | Supported | Rolling | Native + Flatpak | |
| NixOS | Best effort | Variable | Flatpak preferred | Nix packaging not maintained |
| Debian 12 | Best effort | 6.1 | Flatpak preferred | Old kernel, fuse path |
| Other distributions | Not tested | — | Flatpak | May work, unsupported |

**Primary target** means: tested on hardware before every release, all features verified.
**Supported** means: CI passes, known to work, not tested on hardware every release.
**Best effort** means: builds and likely works, not actively tested.

---

## 3. Kernel Version Gates

Kernel version determines which VFS backend is selected. See VFS_DESIGN.md for full backend selection logic.

| Kernel Version | VFS Capability | Backend Selected |
|---------------|---------------|-----------------|
| < 4.18 | FUSE3 unavailable | Symlink farm only |
| 4.18 – 5.10 | FUSE3 available, no rootless overlayfs | fuse-overlayfs |
| 5.11 – 6.5 | Rootless overlayfs available, old mount API | fuse-overlayfs (preferred) or old overlayfs API |
| 6.6+ | New mount API stable, rootless overlayfs mature | Kernel overlayfs (Tier 1) |

**Targeting kernel 6.6+ as the baseline for Tier 1 operation.** By the time Mantle Manager reaches stable, this kernel version will be widespread outside of immutable/LTS distributions.

**SteamOS 3.x ships kernel 6.1.** This is below the Tier 1 threshold. SteamOS is Flatpak-only and always takes the fuse-overlayfs path. This is expected, tested, and documented.

### 3.1 Kernel Version Detection

```rust
use nix::sys::utsname::uname;

/// Returns the running kernel version as (major, minor, patch).
/// Safe to call; uname() always succeeds on Linux.
pub fn kernel_version() -> (u32, u32, u32) {
    let uts = uname().expect("uname always succeeds on Linux");
    parse_kernel_version(uts.release().to_string_lossy().as_ref())
}

/// Parse a kernel release string like "6.6.0-arch1-1" or "6.1.52-valve16-1-neptune".
pub fn parse_kernel_version(release: &str) -> (u32, u32, u32) {
    let mut parts = release.split('.');
    let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch = parts.next()
        .and_then(|s| s.split('-').next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (major, minor, patch)
}
```

Never shell out to `uname -r`. Use `nix::sys::utsname::uname` directly.

---

## 4. Flatpak Environment

### 4.1 Detection

```rust
/// Returns true if running inside a Flatpak sandbox.
/// Cached on first call — do not call in hot paths.
pub fn is_flatpak() -> bool {
    std::path::Path::new("/.flatpak-info").exists()
}
```

### 4.2 Path Differences

| Resource | Native Path | Flatpak Path |
|----------|------------|-------------|
| App data | `~/.local/share/mantle-manager/` | `~/.var/app/io.mantlemanager.MantleManager/data/` |
| Config | `~/.config/mantle-manager/` | `~/.var/app/io.mantlemanager.MantleManager/config/` |
| Cache | `~/.cache/mantle-manager/` | `~/.var/app/io.mantlemanager.MantleManager/cache/` |
| Plugin dir | `<data>/plugins/` | `<data>/plugins/` |
| Database | `<data>/mantle.db` | `<data>/mantle.db` |

Path resolution uses `dirs` crate or XDG environment variables. Never hardcode path prefixes.

### 4.3 Flatpak-Specific Constraints

- New mount API (`fsopen`/`fsconfig`) is not available — the sandbox does not allow kernel mount operations
- fuse-overlayfs must be bundled in the Flatpak or accessed via `flatpak-spawn --host`
- `/dev/fuse` must be accessible — requires `--device=fuse` in the manifest
- Steam library access requires explicit filesystem permissions in `finish-args`

---

## 5. Steam Deck Specifics

### 5.1 Hardware Profile

| Property | Value |
|----------|-------|
| OS | SteamOS 3.x |
| Kernel | 6.1.x (valve patchset) |
| Display | 1280×800 (rotated, landscape) |
| Input | Controller in Game Mode, touchpad in Desktop Mode |
| Storage | NVMe SSD, frequently low on space |
| Install method | Flatpak only (immutable OS) |

### 5.2 Game Mode vs Desktop Mode

Mantle Manager is a Desktop Mode application. It does not run in Game Mode during normal operation. The overlay mount created by Mantle Manager persists into Game Mode when the game is launched.

All UI must be functional in Desktop Mode with keyboard and mouse/touchpad. Controller input in the UI is not required for v1.0 — tracked in `futures.md`.

### 5.3 Display Requirements

Every UI element must be functional at **1280×800**. This is the Deck's native display resolution and the minimum supported resolution.

- No horizontal scrolling in any view at 1280×800
- No vertically clipped content at 1280×800
- Touch targets minimum 44×44 logical pixels (follows GNOME HIG)

See UI_GUIDE.md for GTK4 implementation details.

### 5.4 Storage Considerations

Steam Deck storage fills quickly with games and mods. Mantle Manager must:
- Never silently retain archive files after extraction unless the user opts in
- Report mod sizes clearly before installation
- Provide a storage usage view in the UI

---

## 6. Proton and Wine

Bethesda games on Linux run via Proton (Valve's Wine-based compatibility layer). Mantle Manager interacts with the Proton prefix for:
- Reading game configuration (INI files, registry values)
- Configuring DLL overrides for SKSE, F4SE, etc.
- Detecting the Proton version and prefix path

### 6.1 Proton Prefix Detection

```rust
use steamlocate::SteamDir;

/// Locate the Proton prefix for a given Steam app ID.
pub fn proton_prefix(app_id: u32) -> Option<PathBuf> {
    let steam = SteamDir::locate().ok()?;
    let app = steam.app(&app_id)?;
    // Proton prefix is at <steam_dir>/steamapps/compatdata/<app_id>/pfx/
    Some(app.path.parent()?.parent()?.join("compatdata").join(app_id.to_string()).join("pfx"))
}
```

Do not hardcode Steam paths or use registry stubs. Use `steamlocate` for all Steam library discovery.

### 6.2 Proton Registry

Proton prefix contains a Wine registry at `<pfx>/user.reg` and `<pfx>/system.reg`. Mantle Manager reads these for game configuration — it does not write to the registry directly.

Registry parsing uses a Rust INI/registry parser. No `wine` or `regedit` process invocations.

---

## 7. Distro-Specific Notes

### 7.1 NixOS

NixOS's immutable filesystem and non-standard FHS layout can cause issues with:
- `steamlocate` locating Steam installations in non-standard paths
- `fuse-overlayfs` binary detection
- System library paths for C FFI

The Flatpak path is the recommended install method on NixOS. Native install issues are tracked in `futures.md` but are not a priority.

### 7.2 SELinux (Fedora, RHEL)

SELinux may block FUSE operations or mount namespace creation in some configurations. If issues are reported, document the required SELinux policy changes in this section.

### 7.3 AppArmor (Ubuntu, Debian)

Similar to SELinux — AppArmor profiles may restrict mount operations. Document as reports come in.

---

## 8. Minimum Requirements Summary

| Requirement | Minimum | Recommended |
|-------------|---------|-------------|
| Kernel | 4.18 | 6.6+ |
| FUSE3 | Required for Tier 2 | |
| fuse-overlayfs | Required for Tier 2 | 1.14+ |
| GTK4 | 4.12 | 4.14+ |
| libadwaita | 1.4 | 1.5+ |
| RAM | 512 MB | 2 GB |
| Storage (app) | 50 MB | |
| Storage (mods) | User dependent | |

---

## 9. Cross-References

| Topic | Standard |
|-------|----------|
| Governance and enforcement | [RULE_OF_LAW.md](RULE_OF_LAW.md) |
| VFS backend selection | [VFS_DESIGN.md §3](VFS_DESIGN.md) |
| Build prerequisites | [BUILD_GUIDE.md §1](BUILD_GUIDE.md) |
| Steam Deck UI testing | [TESTING_GUIDE.md §7](TESTING_GUIDE.md) |
| Flatpak manifest | [BUILD_GUIDE.md §8](BUILD_GUIDE.md) |
| GTK4 display requirements | [UI_GUIDE.md](UI_GUIDE.md) |
