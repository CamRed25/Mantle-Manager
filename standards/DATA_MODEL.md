# Mantle Manager — Data Model

> **Scope:** SQLite schema, mod metadata structure, profile design, migration policy, and the boundary between SQLite and flat-file storage.
> **Last Updated:** Mar 3, 2026

---

## 1. Overview

Mantle Manager uses SQLite as its primary data store for all persistent application state that benefits from indexed queries, transactional writes, and relational structure. Flat TOML files are used for user-editable configuration that belongs outside the database.

**Guiding principle:** If it needs indexed lookup, transactional safety, or relational joins — SQLite. If it's human-editable configuration that should survive a database wipe — TOML.

| Data | Storage | Rationale |
|------|---------|-----------|
| Mod metadata (name, version, source, hashes) | SQLite | Indexed lookup, relational to profiles |
| Mod file manifest | SQLite | Per-file conflict detection queries |
| Profile definitions | SQLite | Relational to mod list entries |
| Profile mod order | SQLite | Ordered, indexed, hot on every launch |
| Download history | SQLite | Queried for duplicate detection |
| Plugin settings | SQLite | Keyed by plugin ID, queried frequently |
| Conflict cache | SQLite | Rebuilt on demand, stored for UI display |
| App settings (theme, paths) | TOML | User-editable, not relational |
| Game definitions | TOML | Static, version-controlled, human-readable |
| Rhai plugin scripts | filesystem | Source files, not data |

---

## 2. Database Location

```
~/.var/app/io.mantlemanager.MantleManager/data/mantle.db   (Flatpak)
~/.local/share/mantle-manager/mantle.db                     (native)
```

The database is opened with WAL journal mode for performance and crash safety:

```sql
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA synchronous = NORMAL;
```

`foreign_keys = ON` is enforced at every connection open — not assumed default.

---

## 3. Schema

### 3.1 mods

Stores one row per installed mod.

```sql
CREATE TABLE mods (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Stable identifier. Derived from Nexus mod ID if available,
    -- otherwise a slug of the archive filename.
    slug            TEXT NOT NULL UNIQUE,
    name            TEXT NOT NULL,
    version         TEXT,           -- semver string or raw version tag
    author          TEXT,
    description     TEXT,
    -- Source tracking
    nexus_mod_id    INTEGER,        -- NULL if not from Nexus
    nexus_file_id   INTEGER,        -- NULL if not from Nexus
    source_url      TEXT,
    -- Archive state
    archive_path    TEXT,           -- Original archive path, if retained
    install_dir     TEXT NOT NULL,  -- Extracted data directory
    -- Integrity
    archive_hash    TEXT,           -- XXH3 hex of the original archive
    -- Timestamps
    installed_at    INTEGER NOT NULL, -- Unix timestamp
    updated_at      INTEGER NOT NULL
);

CREATE INDEX idx_mods_slug ON mods(slug);
CREATE INDEX idx_mods_nexus ON mods(nexus_mod_id, nexus_file_id);
```

### 3.2 mod_files

Stores one row per file within an installed mod. Used for conflict detection.

```sql
CREATE TABLE mod_files (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    mod_id      INTEGER NOT NULL REFERENCES mods(id) ON DELETE CASCADE,
    -- Relative path within the mod's data directory.
    -- Stored lowercase for case-insensitive conflict matching.
    rel_path    TEXT NOT NULL,
    -- XXH3 hash of the file contents for integrity and dedup.
    file_hash   TEXT NOT NULL,
    file_size   INTEGER NOT NULL,
    -- Populated for BSA/BA2 contents — NULL for loose files.
    archive_name TEXT,
    UNIQUE(mod_id, rel_path)
);

CREATE INDEX idx_mod_files_path ON mod_files(rel_path);
CREATE INDEX idx_mod_files_mod ON mod_files(mod_id);
```

### 3.3 profiles

One row per profile.

```sql
CREATE TABLE profiles (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL UNIQUE,
    -- Foreign key to games table (future). NULL = no game locked to profile.
    game_slug   TEXT,
    -- Whether this is the currently active profile.
    is_active   INTEGER NOT NULL DEFAULT 0 CHECK(is_active IN (0, 1)),
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
```

**Rule:** Exactly one profile has `is_active = 1` at any time. Enforced in Rust, not via SQL constraint (SQLite doesn't support single-row constraints natively without triggers).

### 3.4 profile_mods

Join table between profiles and mods, carrying per-profile activation state and priority order.

```sql
CREATE TABLE profile_mods (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    profile_id  INTEGER NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    mod_id      INTEGER NOT NULL REFERENCES mods(id) ON DELETE CASCADE,
    -- Priority order. Lower number = higher priority.
    -- 1 is highest priority (leftmost in overlay lowerdir).
    priority    INTEGER NOT NULL,
    is_enabled  INTEGER NOT NULL DEFAULT 1 CHECK(is_enabled IN (0, 1)),
    UNIQUE(profile_id, mod_id),
    UNIQUE(profile_id, priority)
);

CREATE INDEX idx_profile_mods_profile ON profile_mods(profile_id, priority);
```

### 3.5 load_order

Plugin load order (ESP/ESM/ESL files) per profile. Separate from mod priority — a mod's position in the mod list and the load order of its plugins are independent.

```sql
CREATE TABLE load_order (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    profile_id  INTEGER NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    -- Plugin filename, including extension. Case-insensitive match in practice.
    plugin_name TEXT NOT NULL,
    -- 0-based load order index.
    load_index  INTEGER NOT NULL,
    is_enabled  INTEGER NOT NULL DEFAULT 1 CHECK(is_enabled IN (0, 1)),
    UNIQUE(profile_id, plugin_name),
    UNIQUE(profile_id, load_index)
);

CREATE INDEX idx_load_order_profile ON load_order(profile_id, load_index);
```

### 3.6 downloads

Download history and queue state.

```sql
CREATE TABLE downloads (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    url             TEXT NOT NULL,
    dest_path       TEXT NOT NULL,
    -- 'queued' | 'in_progress' | 'complete' | 'failed'
    status          TEXT NOT NULL DEFAULT 'queued',
    -- Bytes downloaded so far (updated periodically).
    bytes_received  INTEGER NOT NULL DEFAULT 0,
    -- Total bytes, NULL if content-length unknown.
    bytes_total     INTEGER,
    error_message   TEXT,           -- NULL unless status = 'failed'
    nexus_mod_id    INTEGER,
    nexus_file_id   INTEGER,
    queued_at       INTEGER NOT NULL,
    completed_at    INTEGER         -- NULL until status = 'complete' or 'failed'
);

CREATE INDEX idx_downloads_status ON downloads(status);
CREATE INDEX idx_downloads_nexus ON downloads(nexus_mod_id, nexus_file_id);
```

### 3.7 plugin_settings

Key-value store for plugin-persisted settings. Scoped by plugin ID.

```sql
CREATE TABLE plugin_settings (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    plugin_id   TEXT NOT NULL,  -- matches MantlePlugin::id()
    key         TEXT NOT NULL,
    -- Serialized SettingValue as JSON.
    value       TEXT NOT NULL,
    UNIQUE(plugin_id, key)
);

CREATE INDEX idx_plugin_settings_plugin ON plugin_settings(plugin_id);
```

### 3.8 conflicts

Cached conflict map. Rebuilt after any mod state change.

```sql
CREATE TABLE conflicts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Both mod_id fields reference mods(id).
    winner_mod_id   INTEGER NOT NULL REFERENCES mods(id) ON DELETE CASCADE,
    loser_mod_id    INTEGER NOT NULL REFERENCES mods(id) ON DELETE CASCADE,
    -- The conflicting file path (lowercase, relative).
    rel_path        TEXT NOT NULL,
    -- Profile this conflict applies to.
    profile_id      INTEGER NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    UNIQUE(profile_id, rel_path, winner_mod_id, loser_mod_id)
);

CREATE INDEX idx_conflicts_profile ON conflicts(profile_id);
CREATE INDEX idx_conflicts_mod ON conflicts(winner_mod_id);
```

---

## 4. Migration Policy

### 4.1 Schema Version Tracking

```sql
CREATE TABLE schema_version (
    version     INTEGER PRIMARY KEY,
    applied_at  INTEGER NOT NULL
);
```

Version 1 is inserted at database creation. Migrations are applied in sequence on startup.

### 4.2 Migration Rules

- Every schema change is a numbered migration file in `mantle_core/src/data/migrations/`
- Migrations are written in pure SQL
- Migrations must be forward-only — no rollback
- Every migration is tested with a data round-trip test before merge (see RULE_OF_LAW §3.3)
- Migration file naming: `m001_initial.sql`, `m002_add_conflict_cache.sql`

### 4.3 Migration Example Structure

```sql
-- m002_add_conflict_cache.sql
-- Adds the conflicts table introduced in schema version 2.

CREATE TABLE conflicts (
    -- ... (full definition)
);

CREATE INDEX idx_conflicts_profile ON conflicts(profile_id);

INSERT INTO schema_version(version, applied_at) VALUES (2, unixepoch());
```

### 4.4 Destructive Migration Policy

Dropping a column or table requires:
1. A migration that preserves the data in the new structure first
2. A separate migration that drops the old structure
3. Both migrations document the data transformation in a SQL comment
4. Entry in `futures.md` tracking the migration's expected deployment window

Never drop data in a single step without a documented preservation step.

---

## 5. What Lives in TOML

### 5.1 App Settings (`settings.toml`)

```toml
# ~/.var/app/.../config/settings.toml  (Flatpak)
# ~/.config/mantle-manager/settings.toml  (native)

[ui]
theme = "auto"          # "auto" | "light" | "dark"
compact_mod_list = false
show_separator_colors = true

[paths]
# Overrides for default paths. Usually empty.
mods_dir = ""
downloads_dir = ""

[network]
nexus_api_key = ""      # User sets this in the UI — stored here, not in DB
```

### 5.2 Game Definitions (`games/*.toml`)

Bundled with the application. Not user-editable.

```toml
# games/skyrim_se.toml
slug = "skyrim_se"
name = "The Elder Scrolls V: Skyrim Special Edition"
steam_app_id = 489830
executable = "SkyrimSE.exe"
data_dir = "Data"
plugin_extensions = ["esm", "esp", "esl"]
ini_files = ["Skyrim.ini", "SkyrimPrefs.ini"]
```

---

## 6. Rust Layer

The `mantle_core::data` module (future `mantle_data` crate) owns all database access. No other module reads from or writes to SQLite directly. SQL does not appear outside of `data/`.

**Connection management:**
- One `rusqlite::Connection` per thread that needs database access
- Use `rusqlite::Connection::open_with_flags` with `SQLITE_OPEN_FULLMUTEX` for shared access
- Blocking queries run in `tokio::task::spawn_blocking`

**Query conventions:**
- Named parameters over positional: `?1` is acceptable; `:name` is preferred for clarity
- No raw string interpolation in SQL — always use parameters
- All writes use explicit transactions

---

## 7. Cross-References

| Topic | Standard |
|-------|----------|
| Governance and enforcement | [RULE_OF_LAW.md](RULE_OF_LAW.md) |
| mantle_data crate extraction trigger | [ARCHITECTURE.md §2.2](ARCHITECTURE.md) |
| SQLite async patterns | [CODING_STANDARDS.md §5.2](CODING_STANDARDS.md) |
| Migration test requirements | [TESTING_GUIDE.md](TESTING_GUIDE.md) |
| Build prerequisites for rusqlite | [BUILD_GUIDE.md](BUILD_GUIDE.md) |
