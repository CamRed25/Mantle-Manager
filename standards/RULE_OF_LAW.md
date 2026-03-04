# Mantle Manager — Rule of Law

> **Scope:** All contributors (human and AI agents) working on `mantle-manager/`.
> **Last Updated:** Mar 3, 2026

---

## 1. Purpose

This document codifies **how** the project's standards are applied, enforced, and evolved. Where the other standards define *what* to do, this one defines *the rules about the rules* — governance, verification, change management, and accountability.

Mantle Manager is a ground-up Rust + GTK4 Linux-native mod manager. There is no upstream to defer to. Every architectural decision is owned here.

---

## 2. Standards Hierarchy

All `.md` files in `standards/` are authoritative. When conflicts arise, resolve using this precedence:

| Priority | Document | Governs |
|----------|----------|---------|
| 1 | **RULE_OF_LAW.md** (this file) | Meta-rules, enforcement, governance |
| 2 | **CODING_STANDARDS.md** | Rust style, naming, error handling, unsafe rules |
| 3 | **ARCHITECTURE.md** | Module structure, crate graph, dependency chain |
| 4 | **PLUGIN_API.md** | Plugin contract, PluginContext boundaries, event bus |
| 5 | **VFS_DESIGN.md** | Overlay design, mount lifecycle, backend tiers |
| 6 | **DATA_MODEL.md** | SQLite schema, mod metadata, profile structure |
| 7 | **BUILD_GUIDE.md** | Cargo workspace, Flatpak, build steps |
| 8 | **TESTING_GUIDE.md** | Test suite structure, skip policy, Deck verification |
| 9 | **PLATFORM_COMPAT.md** | Distro support, kernel gates, Steam Deck |
| 10 | **UI_GUIDE.md** | GTK4/libadwaita conventions, adaptive layout rules |

If a standard contradicts a higher-priority standard, the higher-priority document wins. Update the lower-priority document to resolve the conflict.

If a standard is silent on a topic, apply the nearest-scoped standard that addresses a related concern. Document the gap and reasoning in `conflict.md` (create if it does not exist) so the relevant standard can be updated explicitly.

**conflict.md format:**
```markdown
### [Short description of the gap or conflict]
- **Date:** YYYY-MM-DD
- **Standards involved:** [e.g. CODING_STANDARDS §3, PLUGIN_API §2]
- **Situation:** [What decision needed to be made]
- **Resolution applied:** [What was done and why]
- **Follow-up:** [Which standard needs updating to close this permanently]
```

Entries in `conflict.md` are temporary. Once the relevant standard is updated to cover the gap, move the entry to a `## Resolved` section with the date it was closed. Do not delete entries — they are a record of how the standards evolved.

---

## 3. Core Principles

### 3.1 Build Must Pass

No change is complete until the build succeeds with exit code 0. A broken build blocks all other work.

```bash
cargo build --workspace
# Exit code MUST be 0
```

- Zero compilation errors required.
- Zero warnings on project-owned crates (`-D warnings` enforced in CI).
- Third-party crate warnings do not block merges.
- Build verification happens **after every logical unit of work** — a logical unit is a self-contained change (one function, one module, one feature). Not after every line, not only at the end of a session.

### 3.2 Fix, Don't Skip

When code fails, fix the root cause. Do not:

- Comment out failing code to make the build pass
- Delete tests that fail instead of fixing the tested code
- Suppress errors with empty `match _ => {}` arms or `let _ =`
- Use `.unwrap()` to silence a type error and call it fixed
- Mark features as "TODO" when the fix is within reach

**"Fixed" means the code works as intended** — not that the symptom is hidden.

### 3.3 Test Every Change

Every feature and bugfix must have corresponding verification:

| Change Type | Required Verification |
|-------------|----------------------|
| New Rust function | Unit test in `#[cfg(test)]` block |
| New plugin trait method | Trait contract test in `plugin_api` crate |
| New event type | Event bus integration test |
| SQLite schema change | Migration test + data round-trip test |
| Overlay behavior | Mount/unmount cycle on real kernel |
| GTK4 widget | Visual smoke test at 1280×800 |
| Build change | Full `cargo build --workspace` clean |
| Flatpak change | `flatpak-builder` dry run |

Test from the **user perspective**. A function that compiles but produces wrong output is not fixed.

### 3.4 Check Before Changing

Before creating or modifying any file:

1. **Read existing code** — understand what's there and why.
2. **Search for usages** — find all call sites, trait impls, and references.
3. **Verify assumptions** — dead code may have hidden consumers.
4. **Ensure no regressions** — new changes must not break existing features.

### 3.5 Error Handling Philosophy

Error handling policy is defined in full in CODING_STANDARDS.md §3. The governing principle stated here for reference:

> **Errors are values, not exceptions.** Every fallible function returns `Result<T, E>`. Errors are propagated explicitly, annotated with context, and handled at the boundary closest to the user. Silent failure is never acceptable.

`thiserror` for library errors, `anyhow` for application-level propagation. `.unwrap()` in production code is a bug.

> **Full error handling rules:** [CODING_STANDARDS.md §3](CODING_STANDARDS.md)

### 3.6 Warnings Policy

| Warning Class | Action |
|---------------|--------|
| Compiler (rustc) | Fix immediately — `-D warnings` is enforced |
| Clippy lints | Fix when feasible; document if intentional with `#[allow(...)]` + comment |
| Deprecation | Replace immediately |
| Unsafe block | Requires safety comment — see CODING_STANDARDS §6 |
| Third-party crate | Document; do not suppress with workspace-level allows |

**Never use `#![allow(warnings)]` at crate root** — it hides real problems.

### 3.7 Plugin Boundary Integrity

Any change to `PluginContext`, the event bus API, or the plugin trait definitions:

1. Requires a `PLUGIN_API.md` update in the same commit
2. Requires a semver bump on the plugin API version constant
3. Must document whether the change is breaking or non-breaking per PLUGIN_API.md §4

This rule exists because Mantle Manager defines its own plugin API. There is no upstream to blame for boundary drift — if the boundary erodes it is because this rule was not followed.

---

## 4. Change Management

### 4.1 The doa/ Archive

Code removed from the active build is moved to `doa/`, never deleted:

```
doa/
├── crates/        ← Full crate directories
├── plugins/       ← Individual plugin files
└── misc/          ← Other archived items
```

**Rules:**

- Move, don't delete.
- Comment out the corresponding `Cargo.toml` workspace member or plugin registration with an explanation.
- Update `cleanup.md` with the date, item moved, and reason:
  ```
  ### [Item Name]
  - **Date:** YYYY-MM-DD
  - **Source:** original/path/
  - **Destination:** doa/crates/ (or doa/plugins/, doa/misc/)
  - **Reason:** [why it was removed from the active build]
  ```
- Update `futures.md` if the item was tracked there.

### 4.2 Documentation Updates

Every code change that affects architecture, build, or interfaces must update the relevant standard:

| Code Change | Update Required |
|-------------|----------------|
| New crate added | ARCHITECTURE.md §2, §4 |
| New dependency added/removed | BUILD_GUIDE.md §1, ARCHITECTURE.md |
| Plugin API changed | PLUGIN_API.md + semver bump |
| Overlay behavior changed | VFS_DESIGN.md |
| SQLite schema changed | DATA_MODEL.md |
| Platform support changed | PLATFORM_COMPAT.md |
| GTK4 convention changed | UI_GUIDE.md |
| Naming convention changed | CODING_STANDARDS.md |
| Crate moved to doa/ | cleanup.md, futures.md |

### 4.3 The futures.md Record

All technical notes, ideas, and future enhancements are recorded in `futures.md`:

```markdown
# Project Futures
## Ideas & Enhancements
## Technical Debt & Refactoring
## Known Limitations
## Completed/Integrated
```

**Rules:**

- Record ideas **when they arise**, not later.
- Move items to "Completed/Integrated" when done — do not delete them.
- Include date stamps on entries for traceability.
- Reference relevant standards or code paths.

### 4.4 Commit Messages

```
crate/component: brief description (imperative mood)

Detailed explanation of what changed and why.
Reference any relevant kernel version, crate version, or design doc.
```

Examples:
```
vfs: add fsopen/fsconfig new mount API for kernel 6.6+
plugin_api: add ModEnabled event to event bus (non-breaking)
data: add migration v2 — profile mod order index
ui: fix mod list adaptive layout at 1280x800
```

### 4.5 Branch Naming

```
feature/vfs-nested-stacking
fix/plugin-context-mod-list-lock
docs/architecture-crate-graph
```

### 4.6 Upstream Policy

Mantle Manager has no upstream. All architectural decisions are owned here. When external crates release breaking changes:

| Situation | Action |
|-----------|--------|
| Dependency minor/patch update | Update freely, run full test suite |
| Dependency major version bump | Evaluate breaking changes, update DATA_MODEL/ARCHITECTURE if affected, document in futures.md |
| Crate abandoned | Evaluate replacement, document decision in futures.md, move to doa/ if forked |
| Security advisory | Fix immediately regardless of breaking changes |

---

## 5. Code Quality Gates

### 5.1 Unsafe Code Policy

`unsafe` blocks are permitted only when:

1. Interfacing with C libraries via FFI (libarchive, libloot, fuse-overlayfs)
2. Performance-critical kernel syscall wrappers where safe alternatives are not available
3. The block is accompanied by a `// SAFETY:` comment explaining the invariants upheld

Every `unsafe` block without a `// SAFETY:` comment is a bug.

> **Full unsafe rules and patterns:** [CODING_STANDARDS.md §6](CODING_STANDARDS.md)

### 5.2 Dead Code Elimination

Dead code rots. Remove it when encountered — don't file a ticket to remove it later.

- Unreachable match arms: remove them
- Unused imports: `cargo fix` removes them automatically
- Feature-gated code for features that no longer exist: remove the gate and the code

### 5.3 Plugin Boundary Enforcement

The `PluginContext` struct is the only sanctioned interface between plugins and core. Plugins must not:

- Hold references to internal core structs
- Bypass the event bus by directly calling core functions
- Access the SQLite handle
- Perform filesystem operations outside their designated plugin data directory

These are not suggestions. A plugin that reaches past `PluginContext` is a bug in the plugin API design, not a clever workaround.

---

## 6. Build System Rules

### 6.1 Cargo Discipline

> **Full Cargo conventions:** [CODING_STANDARDS.md §4](CODING_STANDARDS.md)

Key policies:

| Rule | Rationale |
|------|-----------|
| Workspace-level `[patch]` only for forks | Prevents version conflicts across crates |
| Pin minor versions for core dependencies | Reproducible builds across machines |
| `build.rs` only for C FFI and codegen | No arbitrary build logic |
| No `*` version specifications | Always breaks eventually |

### 6.2 Build Verification Protocol

After any modification:

```bash
# 1. Incremental build (fast check)
cargo build --workspace

# 2. Full check with clippy
cargo clippy --workspace -- -D warnings

# 3. If Cargo.toml changed, verify dependency tree
cargo tree | grep -E "duplicate|conflict"

# 4. If schema changed, run migration tests
cargo test -p mantle_data migration

# 5. Full test suite before merge
cargo test --workspace
```

Always verify exit code 0. Do not proceed to the next task while the build is broken.

---

## 7. Operational Rules

### 7.1 Standards Review — Lookup Table

| Work Type | Read Before Starting |
|-----------|---------------------|
| New Rust crate or module | ARCHITECTURE.md, CODING_STANDARDS.md |
| Plugin system work | PLUGIN_API.md, CODING_STANDARDS.md §3 |
| VFS / overlay work | VFS_DESIGN.md, PLATFORM_COMPAT.md |
| SQLite / data model work | DATA_MODEL.md |
| GTK4 / UI work | UI_GUIDE.md, CODING_STANDARDS.md §5 |
| Flatpak / packaging | BUILD_GUIDE.md §8 |
| New test suite | TESTING_GUIDE.md |
| Starting a major feature | Full read of all relevant standards |
| Returning after a break | RULE_OF_LAW.md + feature-relevant standards |

### 7.2 Batch, Don't Thrash

- Group related file edits into a single logical change.
- Read enough context to understand before modifying.
- Test once after each **logical unit of work** — a self-contained change that could stand alone. Not after every line, not only at the end of a session.
- Only request user input after features are 100% confirmed working.

### 7.3 Steam Deck Awareness

Every UI change must be verified at 1280×800. Every overlay change must be verified against kernel 6.1 behavior (nested stacking path). Every Flatpak change must be verified with `flatpak-builder --dry-run`.

> **Simulation methods:** [TESTING_GUIDE.md §7](TESTING_GUIDE.md)

---

## 8. Enforcement

### 8.1 Violation Severity

| Severity | Examples | Action |
|----------|----------|--------|
| **Critical** | Build broken, data loss, plugin boundary bypassed, unsafe without SAFETY comment | Fix immediately, block all work |
| **High** | Dead code introduced, tests removed, standards ignored, plugin API changed without doc update | Fix before proceeding |
| **Medium** | Missing doc comment, naming violation, futures.md not updated | Fix in current session |
| **Low** | Minor formatting, optional optimization | Fix when convenient |

### 8.2 Accountability

Every code change is attributable. Whether made by a human or an AI agent:

- The change must follow all applicable standards.
- The build must pass after the change.
- Documentation must be updated if affected.
- futures.md must be updated if relevant items exist.

**No exceptions for "quick fixes."**

---

## 9. Cross-References

| Topic | Standard |
|-------|----------|
| Rust style & conventions | [CODING_STANDARDS.md](CODING_STANDARDS.md) |
| Module structure & crate graph | [ARCHITECTURE.md](ARCHITECTURE.md) |
| Plugin contract & event bus | [PLUGIN_API.md](PLUGIN_API.md) |
| Overlay & VFS design | [VFS_DESIGN.md](VFS_DESIGN.md) |
| SQLite schema & data model | [DATA_MODEL.md](DATA_MODEL.md) |
| Build prerequisites & steps | [BUILD_GUIDE.md](BUILD_GUIDE.md) |
| Test suite & verification | [TESTING_GUIDE.md](TESTING_GUIDE.md) |
| Distro & kernel support | [PLATFORM_COMPAT.md](PLATFORM_COMPAT.md) |
| GTK4 & UI conventions | [UI_GUIDE.md](UI_GUIDE.md) |
