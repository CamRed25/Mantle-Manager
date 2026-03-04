# Mantle Manager — VFS Design

> **Scope:** Virtual filesystem design, overlay backend selection, mount lifecycle, namespace isolation, and performance characteristics.
> **Last Updated:** Mar 3, 2026

---

## 1. Overview

Mantle Manager uses a virtual filesystem to present a merged view of active mods to the game process without modifying the actual game directory. The VFS is the most critical infrastructure component — it must be fast, reliable, and leave no stale state after the game exits.

The design is a three-tier backend system. The best available backend is selected at runtime based on kernel version and execution environment. The game process sees a single merged directory regardless of which backend is active.

**Design goals:**
- Zero permanent modification to the game installation directory
- Clean teardown on game exit, crash, or app restart
- Minimal overhead — kernel overlayfs is the primary target
- Rootless operation — su/sudo required nowhere
- Flatpak sandbox compatible

---

## 2. Backend Tiers

### 2.1 Tier 1 — Kernel Overlayfs (Primary)

Uses the Linux `fsopen`/`fsconfig`/`fsmount` new mount API introduced in kernel 5.2, with overlayfs `userxattr` support required from kernel 5.11 for rootless operation. Targeting kernel 6.6+ as the baseline for full feature parity.

**Requirements:**
- Kernel >= 6.6 (rootless overlayfs with `userxattr`, stable new mount API)
- NOT running inside Flatpak sandbox (new mount API requires host kernel access)
- `CAP_SYS_ADMIN` in a user namespace (available by default on most distros)

**Performance:** Best. Zero FUSE overhead, in-kernel copy-on-write, native kernel semantics.

**Implementation:** `vfs/backend/kernel.rs`

### 2.2 Tier 2 — fuse-overlayfs (Flatpak / Fallback)

Uses `fuse-overlayfs` — a FUSE implementation of overlayfs semantics that works rootless without kernel privileges, including inside Flatpak sandboxes.

**Requirements:**
- `fuse-overlayfs` binary available at a known path
- FUSE kernel module loaded (kernel >= 4.18 for FUSE3)
- Used when: running inside Flatpak, OR kernel < 6.6

**Performance:** Moderate. FUSE context-switch overhead is measurable but acceptable. Adequate for mod manager use cases where game launches involve ~seconds of setup.

**Implementation:** `vfs/backend/fuse.rs`

### 2.3 Tier 3 — Symlink Farm (Last Resort)

Creates a directory of symlinks replicating the merged view. Slow to set up (one `symlink()` per file), no COW semantics, no kernel overlay — but has zero system dependencies beyond `ln` semantics.

**Requirements:** None beyond a writable temporary directory.

**Limitations:**
- Game writes go to the symlink target, not a working directory — game saves and config files may be misdirected
- Large mod counts produce large symlink directories
- No atomic teardown — partial cleanup on crash leaves symlinks behind

**Performance:** Poorest. Linear in the number of files across all active mods.

**Implementation:** `vfs/backend/symlink.rs`

---

## 3. Backend Selection

Selection logic is evaluated once at startup and cached. The Flatpak check is performed first — a machine on kernel 6.8 inside Flatpak still takes the fuse.rs path, because the new mount API requires host kernel access the sandbox does not provide.

```
is_flatpak()
    → Tier 2: fuse-overlayfs
      (sandbox constraint, regardless of kernel version)

NOT is_flatpak() AND kernel >= 6.6 AND has_new_mount_api()
    → Tier 1: kernel overlayfs
      (native, zero FUSE overhead)

NOT is_flatpak() AND (kernel >= 5.11 OR fuse_overlayfs_available())
    → Tier 2: fuse-overlayfs
      (FUSE fallback)

fallback
    → Tier 3: symlink farm
      (no kernel or FUSE dependency)
```

`has_new_mount_api()` probes via `fsopen("overlay", 0)` and checks for `ENOSYS`. This is done once at startup, result is cached. Do not call in hot paths.

`fuse_overlayfs_available()` checks for the binary at:
```
/usr/bin/fuse-overlayfs
/usr/local/bin/fuse-overlayfs
~/.local/bin/fuse-overlayfs
```
And checks FUSE device availability at `/dev/fuse`.

**Selected backend is logged at `info` level on startup:**
```
vfs: selected backend Tier1(KernelOverlayfs) — kernel 6.12, new mount API available
vfs: selected backend Tier2(FuseOverlayfs) — flatpak environment detected
```

---

## 4. Mount Lifecycle

### 4.1 Pre-Mount

1. Read active mod list in priority order from `mantle_core::mod_list`
2. Resolve lower directories — each active mod's extracted data directory
3. Verify all lower directories exist and are readable
4. Create working directory and upper directory in a temp path
5. Create merge target directory

**Lower directory ordering:**

Mods are ordered lowest priority to highest priority, left-to-right in the `lowerdir` option. The leftmost entry in the merged view takes precedence. Priority 1 (highest) is leftmost.

```
lowerdir=/tmp/mantle/mods/mod-high-priority:/tmp/mantle/mods/mod-low-priority
```

### 4.2 Mount

**Tier 1 (kernel):**
```rust
// Conceptual — actual implementation uses nix::mount::fsopen/fsconfig
fsopen("overlay") → fd
fsconfig(fd, FSCONFIG_SET_STRING, "lowerdir", lower_dirs)?
fsconfig(fd, FSCONFIG_SET_STRING, "upperdir", upper_dir)?
fsconfig(fd, FSCONFIG_SET_STRING, "workdir", work_dir)?
fsconfig(fd, FSCONFIG_SET_FLAG, "userxattr")?
fsconfig(fd, FSCONFIG_CMD_CREATE)?
fsmount(fd) → mount_fd
move_mount(mount_fd, merge_dir)?
```

**Tier 2 (fuse-overlayfs):**
```rust
Command::new("fuse-overlayfs")
    .arg("-o").arg(format!("lowerdir={lower},upperdir={upper},workdir={work}"))
    .arg(merge_dir)
    .spawn()
```

**Tier 3 (symlink):**
Iterates mods in priority order (lowest first), `symlink()`s each file to the merge directory. Higher-priority mods overwrite lower-priority symlinks.

### 4.3 Verification

After mount, before firing `OverlayMounted` event:

1. Stat the merge directory and verify it is a mount point
2. Verify at least one file from each mod is visible in the merged view
3. Log layer count and backend

If verification fails, tear down immediately and return `MountError`.

### 4.4 Teardown

**Normal teardown** (game exits cleanly):
1. Fire `GameExited` event — plugins have a chance to react
2. Unmount: `umount2(merge_dir, MNT_DETACH)` or kill fuse-overlayfs process
3. Remove working directory and upper directory
4. Fire `OverlayUnmounted` event

**Mount namespace teardown:**
If mount namespace isolation was used (`vfs/namespace.rs`), the namespace cleanup is automatic when the namespace process exits. No explicit umount needed.

**Crash/stale mount recovery:**
On startup, `vfs/cleanup.rs` scans for stale mounts at known merge paths and unmounts them before proceeding. A stale mount is any mount whose associated game PID no longer exists.

---

## 5. Namespace Isolation

When available, mount operations run in an isolated mount namespace created via `unshare(CLONE_NEWNS)`. This provides:

- Automatic unmount on process exit — no stale mount on crash
- No mount activity visible to the host system
- Cleaner teardown semantics

**Availability:** Kernel >= 3.8, user namespace support enabled. Verified at startup — if unavailable, falls back to standard mount operations with manual cleanup.

**Implementation:** `vfs/namespace.rs`

---

## 6. Nested Overlay Stacking

Linux overlayfs has a limit on the number of lower directories (typically 500 on current kernels). Mod lists exceeding this limit require stacking — building intermediate merged mounts and using those as lower directories for the final mount.

**Trigger:** Active mod count > 480 (conservative margin below the 500 limit).

**Strategy:** Divide lower dirs into groups of 200. Mount each group as an intermediate overlay. Use intermediate merge points as the lower dirs for the final mount.

```
Group 1: mods 1-200   → intermediate mount A
Group 2: mods 201-400 → intermediate mount B
Group 3: mods 401-end → intermediate mount C
Final:   lowerdir=C:B:A → game sees full merged view
```

Intermediate mounts are tracked and torn down in reverse order on unmount.

**Implementation:** `vfs/stacking.rs`

> Note: Nested stacking is tracked in `futures.md` — implementation is deferred until a mod list large enough to trigger it exists for testing.

---

## 7. File Conflict Semantics

In overlayfs, the highest-priority mod's file wins the conflict. This is a fixed semantic — there is no "merge" of conflicting files, only overwrite.

Conflict detection (`mantle_core::conflict`) identifies these overlaps before mount and surfaces them to the user. The VFS does not attempt to resolve conflicts — it simply implements priority order. Conflict resolution is the user's responsibility via the mod list ordering.

**Edge cases:**

| Case | Behavior |
|------|---------|
| Same file, different mods | Higher priority mod wins |
| Directory in one mod, file in another | Overlayfs prefers whichever is in the higher layer |
| BSA/BA2 contents | Handled by archive extraction, not by VFS layer |
| Empty directory marker | Overlayfs `.wh..opq` whiteout semantics apply |

---

## 8. Performance Characteristics

| Backend | Mount Setup | File I/O Overhead | Max Mods | Crash Safety |
|---------|------------|------------------|----------|--------------|
| Tier 1 (kernel) | ~50ms | None (in-kernel) | ~500 native, unlimited with stacking | Namespace auto-cleanup |
| Tier 2 (fuse) | ~200ms | ~5-15% FUSE overhead | ~500 native, unlimited with stacking | Manual umount required |
| Tier 3 (symlink) | Linear in file count | None (symlinks) | Unlimited | Stale symlinks on crash |

Mount setup times are approximate and hardware-dependent. Measured on NVMe storage. HDD or network storage will be significantly slower for symlink tier.

---

## 9. Known Limitations

- **SteamOS 3.x kernel 6.1:** fuse-overlayfs is the primary path. Kernel overlayfs is not available without host kernel access. This is expected and documented — see PLATFORM_COMPAT.md.
- **Starfield's data structure:** Starfield uses loose files differently from prior Bethesda titles. VFS handles this correctly but the game's own mod staging logic may interact unexpectedly. Track in `futures.md` as compatibility data is gathered.
- **Game save directory:** Overlayfs upper directory captures game writes. This is intentional — game saves go to the upper directory, not into the mod layer. On unmount, upper directory contents are archived or discarded per profile settings. Policy for upper directory handling is tracked in `futures.md`.

---

## 10. Cross-References

| Topic | Standard |
|-------|----------|
| Governance and enforcement | [RULE_OF_LAW.md](RULE_OF_LAW.md) |
| VFS module structure | [ARCHITECTURE.md §4.2](ARCHITECTURE.md) |
| Kernel version and distro support | [PLATFORM_COMPAT.md](PLATFORM_COMPAT.md) |
| Build prerequisites for FUSE | [BUILD_GUIDE.md](BUILD_GUIDE.md) |
| VFS test coverage | [TESTING_GUIDE.md](TESTING_GUIDE.md) |
| Unsafe syscall rules | [CODING_STANDARDS.md §6](CODING_STANDARDS.md) |
