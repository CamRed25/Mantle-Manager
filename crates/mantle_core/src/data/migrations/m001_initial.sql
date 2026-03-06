-- m001_initial.sql
-- Initial schema for Mantle Manager.
-- Creates all core tables for schema version 1.

-- ─── Schema Version ──────────────────────────────────────────────────────────

CREATE TABLE schema_version (
    version     INTEGER PRIMARY KEY,
    applied_at  INTEGER NOT NULL
);

-- ─── Mods ────────────────────────────────────────────────────────────────────

-- Stores one row per installed mod.
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

CREATE INDEX idx_mods_slug  ON mods(slug);
CREATE INDEX idx_mods_nexus ON mods(nexus_mod_id, nexus_file_id);

-- ─── Mod Files ───────────────────────────────────────────────────────────────

-- Stores one row per file within an installed mod. Used for conflict detection.
CREATE TABLE mod_files (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    mod_id       INTEGER NOT NULL REFERENCES mods(id) ON DELETE CASCADE,
    -- Relative path within the mod's data directory.
    -- Stored lowercase for case-insensitive conflict matching.
    rel_path     TEXT NOT NULL,
    -- XXH3 hash of the file contents for integrity and dedup.
    file_hash    TEXT NOT NULL,
    file_size    INTEGER NOT NULL,
    -- Populated for BSA/BA2 contents — NULL for loose files.
    archive_name TEXT,
    UNIQUE(mod_id, rel_path)
);

CREATE INDEX idx_mod_files_path ON mod_files(rel_path);
CREATE INDEX idx_mod_files_mod  ON mod_files(mod_id);

-- ─── Profiles ────────────────────────────────────────────────────────────────

-- One row per mod-list profile.
-- Rule: exactly one profile has is_active = 1 at any time.
-- This invariant is enforced in Rust (not SQL) due to SQLite limitations.
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

-- ─── Profile Mods ────────────────────────────────────────────────────────────

-- Join table between profiles and mods, carrying per-profile activation state
-- and priority order.
CREATE TABLE profile_mods (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    profile_id  INTEGER NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    mod_id      INTEGER NOT NULL REFERENCES mods(id)     ON DELETE CASCADE,
    -- Priority order. Lower number = higher priority.
    -- 1 is highest priority (leftmost in overlay lowerdir).
    priority    INTEGER NOT NULL,
    is_enabled  INTEGER NOT NULL DEFAULT 1 CHECK(is_enabled IN (0, 1)),
    UNIQUE(profile_id, mod_id),
    UNIQUE(profile_id, priority)
);

CREATE INDEX idx_profile_mods_profile ON profile_mods(profile_id, priority);

-- ─── Load Order ──────────────────────────────────────────────────────────────

-- Plugin load order (ESP/ESM/ESL) per profile.
-- Separate from mod priority: a mod's position in the mod list and the load
-- order of its plugins are independent.
CREATE TABLE load_order (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    profile_id  INTEGER NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    -- Plugin filename including extension. Case-insensitive match in practice.
    plugin_name TEXT NOT NULL,
    -- 0-based load order index.
    load_index  INTEGER NOT NULL,
    is_enabled  INTEGER NOT NULL DEFAULT 1 CHECK(is_enabled IN (0, 1)),
    UNIQUE(profile_id, plugin_name),
    UNIQUE(profile_id, load_index)
);

CREATE INDEX idx_load_order_profile ON load_order(profile_id, load_index);

-- ─── Downloads ───────────────────────────────────────────────────────────────

-- Download history and queue state.
-- `id` is a stable UUID string matching the in-memory DownloadJob.id.
-- Status values: 'queued' | 'in_progress' | 'complete' | 'failed' | 'cancelled'
CREATE TABLE downloads (
    id          TEXT PRIMARY KEY,
    url         TEXT NOT NULL,
    filename    TEXT NOT NULL,
    dest_path   TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'queued',
    progress    REAL NOT NULL DEFAULT 0.0,
    total_bytes INTEGER,
    added_at    INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at  INTEGER
);

CREATE INDEX idx_downloads_status ON downloads(status);

-- ─── Plugin Settings ─────────────────────────────────────────────────────────

-- Key-value store for plugin-persisted settings. Scoped by plugin ID.
CREATE TABLE plugin_settings (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    plugin_id   TEXT NOT NULL,  -- matches MantlePlugin::id()
    key         TEXT NOT NULL,
    -- Serialized SettingValue as JSON.
    value       TEXT NOT NULL,
    UNIQUE(plugin_id, key)
);

CREATE INDEX idx_plugin_settings_plugin ON plugin_settings(plugin_id);

-- ─── Conflicts ───────────────────────────────────────────────────────────────

-- Cached conflict map. Rebuilt after any mod state change.
CREATE TABLE conflicts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    winner_mod_id   INTEGER NOT NULL REFERENCES mods(id)     ON DELETE CASCADE,
    loser_mod_id    INTEGER NOT NULL REFERENCES mods(id)     ON DELETE CASCADE,
    -- The conflicting file path (lowercase, relative).
    rel_path        TEXT NOT NULL,
    -- Profile this conflict applies to.
    profile_id      INTEGER NOT NULL REFERENCES profiles(id) ON DELETE CASCADE,
    UNIQUE(profile_id, rel_path, winner_mod_id, loser_mod_id)
);

CREATE INDEX idx_conflicts_profile ON conflicts(profile_id);
CREATE INDEX idx_conflicts_mod     ON conflicts(winner_mod_id);

-- ─── Seed Version Row ────────────────────────────────────────────────────────

INSERT INTO schema_version(version, applied_at) VALUES (1, unixepoch());
