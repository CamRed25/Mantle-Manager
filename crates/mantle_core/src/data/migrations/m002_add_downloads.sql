-- m002_add_downloads.sql
-- Adds the download queue persistence table for schema version 2.
--
-- Downloads are persisted so that:
--   • The UI shows the last session's download history on startup.
--   • In-progress downloads that were interrupted can be resumed.
--
-- Status values (text): 'queued', 'in_progress', 'complete', 'failed', 'cancelled'

-- ─── Downloads ───────────────────────────────────────────────────────────────

CREATE TABLE downloads (
    -- Stable UUID string, matches the in-memory DownloadJob.id.
    id          TEXT PRIMARY KEY,
    -- Remote URL that was or will be fetched.
    url         TEXT NOT NULL,
    -- Human-readable mod/file name shown in the UI.
    filename    TEXT NOT NULL,
    -- Absolute destination path for the downloaded archive.
    dest_path   TEXT NOT NULL,
    -- One of: queued | in_progress | complete | failed | cancelled
    status      TEXT NOT NULL DEFAULT 'queued',
    -- Download progress [0.0, 1.0]; 0 until in_progress.
    progress    REAL NOT NULL DEFAULT 0.0,
    -- Total file size in bytes (NULL if unknown / not yet received).
    total_bytes INTEGER,
    -- Unix timestamp (seconds since epoch) at which the job was created.
    added_at    INTEGER NOT NULL DEFAULT (unixepoch()),
    -- Unix timestamp of the last status update, or NULL if never updated.
    updated_at  INTEGER
);

CREATE INDEX downloads_status_idx ON downloads(status);

-- Advance schema version to 2.
INSERT INTO schema_version(version, applied_at) VALUES (2, unixepoch());
