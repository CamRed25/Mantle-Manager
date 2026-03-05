//! Nexus Mods API response types.
//!
//! All structs implement `serde::Deserialize` to parse JSON responses from
//! `api.nexusmods.com/v1/` and `search.nexusmods.com`.  Unknown fields are
//! ignored using `#[serde(deny_unknown_fields)]` is intentionally NOT set so
//! that API additions don't break existing builds.

use serde::Deserialize;

// ─── Mod info ─────────────────────────────────────────────────────────────────

/// Summary information about a Nexus mod.
///
/// Returned by:
/// - `GET /v1/games/{game_domain}/mods/{mod_id}.json` (full detail)
/// - `GET /search.nexusmods.com/mods` (search results via `mod_list`)
#[derive(Debug, Clone, Deserialize)]
pub struct NexusMod {
    /// Nexus numeric mod ID.
    pub mod_id: u32,
    /// Display name of the mod.
    pub name: String,
    /// Short description (HTML may be present; strip before display).
    pub summary: Option<String>,
    /// Version string as entered by the author.
    pub version: Option<String>,
    /// Nexus username of the uploader.
    pub uploaded_by: Option<String>,
    /// Number of endorsements.
    pub endorsement_count: Option<u32>,
    /// Whether the mod is marked NSFW ("Not Safe For Work").
    pub contains_adult_content: Option<bool>,
    /// URL to the mod's thumbnail image.
    pub picture_url: Option<String>,
}

// ─── File list ────────────────────────────────────────────────────────────────

/// Response envelope for the files endpoint.
///
/// Returned by `GET /v1/games/{game_domain}/mods/{mod_id}/files.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct FileListResponse {
    /// The actual file entries.
    pub files: Vec<ModFile>,
    /// Files that have been marked "archived" by the author.
    pub file_updates: Option<Vec<serde_json::Value>>,
}

/// A single file entry within a mod's file list.
#[derive(Debug, Clone, Deserialize)]
pub struct ModFile {
    /// Nexus numeric file ID.
    pub file_id: u64,
    /// Numeric file type: 1=Main, 2=Update, 3=Optional, 4=Old, 6=Misc.
    pub category_id: Option<u8>,
    /// Human-readable category name (e.g. `"MAIN"`, `"OPTIONAL"`).
    pub category_name: Option<String>,
    /// Uploader-given filename (e.g. `"MyMod-1-0-0.zip"`).
    pub file_name: String,
    /// Version string of this specific file.
    pub version: Option<String>,
    /// Size in kilobytes (as reported by Nexus).
    pub size_kb: Option<u64>,
    /// Unix timestamp of when this file was uploaded.
    pub uploaded_timestamp: Option<i64>,
    /// Changelog excerpt for this file, if present.
    pub changelog_html: Option<String>,
}

// ─── Download links ───────────────────────────────────────────────────────────

/// One CDN download link for a specific file.
///
/// Returned as a list by
/// `GET /v1/games/{game_domain}/mods/{mod_id}/files/{file_id}/download_link.json`.
///
/// Multiple links from different CDN locations may be returned; the caller
/// should prefer the one with the highest `short_name` priority or simply use
/// the first entry.
#[derive(Debug, Clone, Deserialize)]
pub struct DownloadLink {
    /// Human-readable CDN name (e.g. `"Nexus CDN"`, `"Premium CDN"`).
    pub name: String,
    /// Short name / slug used to identify the CDN.
    pub short_name: String,
    /// Full HTTPS download URL (valid for a short window — fetch immediately).
    #[serde(rename = "URI")]
    pub uri: String,
}

// ─── Search results ───────────────────────────────────────────────────────────

/// Response from `https://search.nexusmods.com/mods`.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchResponse {
    /// List of matching mod summaries.
    #[serde(rename = "results")]
    pub results: Vec<SearchResult>,
    /// Total number of results available across all pages.
    pub total: Option<u64>,
}

/// A single result from the Nexus search endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchResult {
    /// Nexus numeric mod ID.
    pub mod_id: u32,
    /// Display name.
    pub name: String,
    /// Short description.
    pub summary: Option<String>,
    /// Mod version.
    pub version: Option<String>,
    /// Nexus username of the uploader.
    pub username: Option<String>,
    /// Endorsement count.
    pub endorsements: Option<u32>,
    /// Number of downloads (all-time).
    pub downloads: Option<u64>,
    /// Thumbnail image URL.
    pub image: Option<String>,
    /// Game domain name the result belongs to.
    pub game_name: Option<String>,
}
