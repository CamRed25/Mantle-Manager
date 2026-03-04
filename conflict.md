# Standards Conflict Register

Documents gaps, contradictions, or deliberate deviations between the standards documents in `standards/`. Required by RULE_OF_LAW §2 and §4.2. All entries are resolved — open conflicts are not permitted.

---

## Format

```
### CONFLICT-NNN — <short title>
- **Documents:** Which standards files are in tension
- **Section(s):** Specific section references
- **Description:** What the tension is
- **Resolution:** Which document wins and why, or what the compromise is
- **Action taken:** What was updated to reflect the resolution
- **Date:** YYYY-MM-DD
```

---

## Register

### CONFLICT-001 — `rustfmt.toml` vs CODING_STANDARDS §1.3 key mismatch

- **Documents:** `standards/CODING_STANDARDS.md` vs `rustfmt.toml`
- **Section(s):** CODING_STANDARDS §1.3
- **Description:** CODING_STANDARDS §1.3 specified `use_field_init_shorthand = true` and `use_try_shorthand = true` in the canonical `rustfmt.toml` block. Both keys are nightly-only and are rejected by `rustfmt` on stable Rust. The actual `rustfmt.toml` has neither key and instead has `use_small_heuristics = "Default"`, which is stable.
- **Resolution:** `rustfmt.toml` wins. The project rule (CODING_STANDARDS §1.1 and RULE_OF_LAW §1.1) is stable-only Rust. No nightly keys belong in the config.
- **Action taken:** `standards/CODING_STANDARDS.md §1.3` updated to remove the two nightly keys and document the stable canonical set. `rustfmt.toml` unchanged.
- **Date:** 2026-03-04

### CONFLICT-002 — ARCHITECTURE.md §4.2 vfs/ layout vs actual tree

- **Documents:** `standards/ARCHITECTURE.md` vs `crates/mantle_core/src/vfs/`
- **Section(s):** ARCHITECTURE.md §4.2
- **Description:** The spec called for `vfs/mount.rs`, `vfs/namespace.rs`, `vfs/stacking.rs`, and `vfs/cleanup.rs` as top-level files. The implementation evolved to `vfs/detect.rs`, `vfs/types.rs`, and `vfs/backend/` (with per-backend files). The actual layout is more granular and better separates detection from types from backend dispatch.
- **Resolution:** Actual tree wins. The implementation is sound, tested, and complete for the current scope. The spec was aspirational at authoring time.
- **Action taken:** `standards/ARCHITECTURE.md §4.2` updated to reflect the actual layout. The four planned files (`mount.rs`, `namespace.rs`, `stacking.rs`, `cleanup.rs`) are tracked in `futures.md` for when mount lifecycle is implemented.
- **Date:** 2026-03-04

### CONFLICT-003 — ARCHITECTURE.md §4.5 game/ layout vs actual tree

- **Documents:** `standards/ARCHITECTURE.md` vs `crates/mantle_core/src/game/`
- **Section(s):** ARCHITECTURE.md §4.5
- **Description:** The spec called for a `games/` subdirectory with 10 per-game definition files and a `registry.rs`. The implementation uses a single flat `games.rs` containing all 10 definitions, and `registry.rs` was initially absent.
- **Resolution:** Actual tree wins for `games.rs`. The 10 game definitions are compact structs; a subdirectory adds navigation overhead with no benefit at this scale. `registry.rs` has since been implemented (Wine/Proton `system.reg`/`user.reg` parser). Only the `games/` subdirectory split remains deferred.
- **Action taken:** `standards/ARCHITECTURE.md §4.5` updated to include `registry.rs` and note only the subdirectory split as deferred. 2026-03-04 update reflects registry.rs now live.
- **Date:** 2026-03-04
