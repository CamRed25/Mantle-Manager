# Mantle Manager — Architecture Decision Record

> **Scope:** The *why* behind major architectural choices. This document is project history, not governance.
> Where these decisions produced rules, those rules live in the relevant standard.
> **Last Updated:** Mar 3, 2026

---

## 1. Why Build From Scratch

The predecessor project (MO2 Linux port) was constrained by the MO2 shell — every architectural decision was inherited rather than chosen. The Python layer, the Qt UI, the proxy daemon, the C++ core — none of those were decisions we made. They were constraints we worked within.

This project starts with no upstream. Every boundary, every crate choice, every module split is deliberate. The cost is that there's no existing foundation to stand on. The benefit is that the architecture can match the actual problem instead of the inherited one.

The proxy pattern from the MO2 port — SHM, bincode bridge, UDS — is carried forward as knowledge, not as code. There is no proxy here because there is no language boundary that requires one. Rust top to bottom means the proxy is the core.

---

## 2. UI Toolkit Decision

**GTK4 + gtk4-rs chosen.** The decision trail:

| Toolkit | Decision | Reason |
|---------|----------|--------|
| Qt | Rejected | Qt is the constraint in the MO2 port. Replacing it with Qt again solves nothing. |
| Electron | Rejected | Web-based UI. Slow, memory-heavy, requires Node runtime. Direct experience confirming this. |
| Tauri | Rejected | Still web-based for the UI layer. Rust backend doesn't change the webview front end. |
| Iced | Watch, don't commit | Pure Rust, no C dependency, trajectory is good. Not mature enough at decision time. No libadwaita equivalent. |
| GTK4 + gtk4-rs | **Chosen** | Production-grade, Linux-first, bindings maintained by the GNOME team, libadwaita gives adaptive layout for free. |

**On Iced:** Worth prototyping a throwaway weekend project against both GTK4 and Iced before starting real UI work if significant time has passed since this decision. The ecosystem moves.

---

## 3. Plugin Scripting Language Decision

Three candidates were evaluated for the plugin scripting layer:

| Option | Reason Considered | Reason Not Primary |
|--------|------------------|---------------------|
| `mlua` (Lua) | Large existing modder community knows Lua. Proven in game tool ecosystems. | C dependency (liblua). Sandbox requires explicit construction. |
| `deno_core` (JS/TS via V8) | Familiar to more people. TypeScript is well-typed. | V8 is enormous. Heavy runtime for plugin scripts. |
| `rhai` | Pure Rust, no C dependency, genuine sandbox — scripts cannot do anything the host doesn't explicitly expose. | Smaller community, less familiar syntax. |

**Rhai chosen** for the scripting layer. The sandbox is genuine in a way that mlua's is not by default, and the zero-C-dependency profile matters for Flatpak portability.

Native `.so` plugins via `libloading` are retained for cases that genuinely need performance or system access. Most plugins do not need this — they subscribe to events and react. The scripting layer covers that case cleanly.

---

## 4. MO2 Plugin Migration Map

What the MO2 Linux port's plugins become in this project:

| MO2 Plugin | Disposition | Rationale |
|------------|-------------|-----------|
| `OverlayFSMountManager` | **Core** | VFS is infrastructure, not optional behavior |
| `FUSEMetadataOrchestrator` | **Core** | Backend selection logic belongs in VFS core |
| `CaseFoldingNormalizer` | **Core** | Filesystem normalization is infrastructure |
| `ConflictDeleter` | **Core feature** | Conflict management is a first-class concern |
| `SKSEInstaller` | Rhai script or native plugin | Game-specific, not infrastructure |
| `NifAnalyzer` | Native plugin or built-in diagnostic | Diagnostic tool, optional |
| `SidebarSettings` | UI config option | Cosmetic preference, not a plugin |
| `SeparatorColorManager` | UI config option | Cosmetic preference, not a plugin |

The pattern: Linux infrastructure → core. Game-specific or cosmetic → plugin or UI option.

---

## 5. Knowledge Carried Forward (Not Code)

The MO2 port produced hard-won knowledge that informs this project's design without being copied directly:

- Kernel version gates for overlayfs — documented in VFS_DESIGN.md and PLATFORM_COMPAT.md
- FUSE backend detection and fallback logic — documented in VFS_DESIGN.md
- Proton integration and Steam library scanning patterns — inform `game/` module design
- Overlay stacking logic for large mod counts — documented in VFS_DESIGN.md §6
- The SHM/bincode/UDS proxy patterns — understood, but no proxy needed here

---

## 6. Timeline

Starting before the current project stabilizes splits attention between two active codebases. The standards, architecture, and this document are written now while the thinking is fresh. Code waits.
