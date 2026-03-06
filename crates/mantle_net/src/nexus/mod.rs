//! Nexus Mods API client.
//!
//! Wraps `reqwest` to provide typed methods against the Nexus Mods v1 REST
//! API (`api.nexusmods.com/v1/`) and the search endpoint
//! (`search.nexusmods.com/mods`).
//!
//! # Authentication
//! All requests to `api.nexusmods.com` require an API key supplied in the
//! `apikey` header.  Free Nexus accounts have rate limits; Premium accounts
//! get higher limits.  Keys are generated in the user's Nexus profile page.
//!
//! # Usage
//! ```no_run
//! # async fn _example() {
//! use mantle_net::nexus::NexusClient;
//! let client = NexusClient::new("my-api-key").unwrap();
//! let mods = client.search("skyrimspecialedition", "SKSE", 3171).await.unwrap();
//! # }
//! ```

pub mod models;

use reqwest::Client;
use tracing::instrument;

use crate::error::NetError;
use models::{DownloadLink, FileListResponse, NexusMod, SearchResponse};

// ─── Constants ────────────────────────────────────────────────────────────────

const API_BASE: &str = "https://api.nexusmods.com/v1";
const SEARCH_BASE: &str = "https://search.nexusmods.com";

// ─── Client ──────────────────────────────────────────────────────────────────

/// Nexus Mods API client with per-request authentication.
///
/// Construct once and reuse across calls to share the internal TLS session
/// pool (`reqwest::Client` is cheaply cloneable and backed by an `Arc`).
#[derive(Clone)]
pub struct NexusClient {
    /// Underlying HTTP client.
    client: Client,
    /// Nexus API key (sent as `apikey` header on every request).
    api_key: String,
}

impl NexusClient {
    // ─── Construction ─────────────────────────────────────────────────────

    /// Create a new [`NexusClient`] with the given API key.
    ///
    /// Builds a shared `reqwest::Client` with `rustls-tls` and a Mantle
    /// user-agent.  Returns a [`NetError::Config`] if `api_key` is empty.
    ///
    /// # Parameters
    /// - `api_key`: Nexus Mods personal API key.
    ///
    /// # Errors
    /// Returns [`NetError::Config`] if `api_key` is blank, or
    /// [`NetError::Http`] if the HTTP client builder fails.
    pub fn new(api_key: impl Into<String>) -> Result<Self, NetError> {
        let api_key = api_key.into();
        if api_key.is_empty() {
            return Err(NetError::Config("Nexus API key must not be empty".to_string()));
        }
        let client = Client::builder()
            .user_agent(concat!("mantle-manager/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(NetError::Http)?;
        Ok(Self { client, api_key })
    }

    // ─── Mod info ─────────────────────────────────────────────────────────

    /// Fetch full details for a single mod.
    ///
    /// Calls `GET /v1/games/{game_domain}/mods/{mod_id}.json`.
    ///
    /// # Parameters
    /// - `game_domain`: Nexus game slug (e.g. `"skyrimspecialedition"`).
    /// - `mod_id`:      Nexus numeric mod identifier.
    ///
    /// # Errors
    /// Returns [`NetError`] on HTTP or deserialisation failure.
    #[instrument(skip(self), fields(game = %game_domain, mod_id))]
    pub async fn get_mod(&self, game_domain: &str, mod_id: u32) -> Result<NexusMod, NetError> {
        let url = format!("{API_BASE}/games/{game_domain}/mods/{mod_id}.json");
        self.get_json(&url).await
    }

    // ─── File list ────────────────────────────────────────────────────────

    /// List all files associated with a mod.
    ///
    /// Calls `GET /v1/games/{game_domain}/mods/{mod_id}/files.json`.
    ///
    /// # Parameters
    /// - `game_domain`: Nexus game slug.
    /// - `mod_id`:      Nexus numeric mod identifier.
    ///
    /// # Errors
    /// Returns [`NetError`] on HTTP or deserialisation failure.
    #[instrument(skip(self), fields(game = %game_domain, mod_id))]
    pub async fn get_files(
        &self,
        game_domain: &str,
        mod_id: u32,
    ) -> Result<FileListResponse, NetError> {
        let url = format!("{API_BASE}/games/{game_domain}/mods/{mod_id}/files.json");
        self.get_json(&url).await
    }

    // ─── Download links ───────────────────────────────────────────────────

    /// Retrieve CDN download links for a specific file.
    ///
    /// Calls `GET /v1/games/{game_domain}/mods/{mod_id}/files/{file_id}/download_link.json`.
    ///
    /// Download links expire quickly; begin the download immediately after
    /// receiving them.
    ///
    /// # Parameters
    /// - `game_domain`: Nexus game slug.
    /// - `mod_id`:      Nexus numeric mod identifier.
    /// - `file_id`:     Nexus numeric file identifier.
    ///
    /// # Errors
    /// Returns [`NetError`] on HTTP or deserialisation failure.
    #[instrument(skip(self), fields(game = %game_domain, mod_id, file_id))]
    pub async fn get_download_links(
        &self,
        game_domain: &str,
        mod_id: u32,
        file_id: u64,
    ) -> Result<Vec<DownloadLink>, NetError> {
        let url = format!(
            "{API_BASE}/games/{game_domain}/mods/{mod_id}/files/{file_id}/download_link.json"
        );
        self.get_json(&url).await
    }

    // ─── Search ───────────────────────────────────────────────────────────

    /// Search for mods using the Nexus search endpoint.
    ///
    /// Calls `GET https://search.nexusmods.com/mods?terms={query}&game_id={nexus_game_id}`.
    ///
    /// Note: the search endpoint uses a different base domain and does **not**
    /// require an API key, though the same client is reused.
    ///
    /// # Parameters
    /// - `game_domain`:   Nexus game slug (used for display; not passed in query).
    /// - `query`:         Free-text search terms.
    /// - `nexus_game_id`: Numeric Nexus game ID (e.g. `1704` for Skyrim SE).
    ///
    /// # Errors
    /// Returns [`NetError`] on HTTP or deserialisation failure.
    #[instrument(skip(self), fields(game = %game_domain, query))]
    pub async fn search(
        &self,
        game_domain: &str,
        query: &str,
        nexus_game_id: u32,
    ) -> Result<SearchResponse, NetError> {
        let url =
            format!("{SEARCH_BASE}/mods?terms={}&game_id={}", urlencoded(query), nexus_game_id,);
        // Search endpoint does not require apikey header.
        let resp = self.client.get(&url).send().await?;
        check_status(resp).await?.json::<SearchResponse>().await.map_err(NetError::Http)
    }

    // ─── Private helpers ──────────────────────────────────────────────────

    /// Issue an authenticated GET to `url` and deserialise the JSON response.
    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, NetError> {
        let resp = self.client.get(url).header("apikey", &self.api_key).send().await?;
        check_status(resp).await?.json::<T>().await.map_err(NetError::Http)
    }
}

// ─── Private utilities ────────────────────────────────────────────────────────

/// Return `Ok(response)` if status is 2xx, else return a [`NetError::Status`].
async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response, NetError> {
    let status = resp.status();
    if status.is_success() {
        Ok(resp)
    } else {
        let code = status.as_u16();
        let body = resp.text().await.unwrap_or_default();
        Err(NetError::Status { status: code, body })
    }
}

/// Very minimal percent-encoder for search query strings.
///
/// Encodes all bytes that are not unreserved URI characters (ALPHA / DIGIT / `-._~`).
fn urlencoded(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
            out.push(c);
        } else {
            for b in c.to_string().as_bytes() {
                use std::fmt::Write;
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

// ─── NXM URL handling ─────────────────────────────────────────────────────────

/// Parsed representation of a `nxm://` deep-link URL.
///
/// Nexus Mods uses this scheme to trigger mod manager downloads from the
/// browser.  The format is:
/// ```text
/// nxm://{game_domain}/mods/{mod_id}/files/{file_id}?key={key}&expires={expires}
/// ```
/// The `key` and `expires` fields are only present for free-tier downloads
/// and must be forwarded to the Nexus download-link endpoint unchanged.
#[derive(Debug, PartialEq)]
pub struct NxmParams {
    /// Nexus game slug, e.g. `"skyrimspecialedition"`.
    pub game_domain: String,
    /// Nexus mod ID.
    pub mod_id: u32,
    /// Nexus file ID.
    pub file_id: u64,
    /// CDN expiry key for free-tier accounts (forwarded to download endpoint).
    pub key: Option<String>,
    /// UNIX timestamp at which `key` expires.
    pub expires: Option<u64>,
}

/// Parse a `nxm://` URL into its constituent parts.
///
/// # Format
/// ```text
/// nxm://{game_domain}/mods/{mod_id}/files/{file_id}[?key={key}&expires={ts}]
/// ```
///
/// # Parameters
/// - `url`: the raw `nxm://` URL string to parse.
///
/// # Returns
/// A populated [`NxmParams`] on success.
///
/// # Errors
/// Returns [`NetError::Parse`] if the scheme is not `nxm`, if path segments
/// are missing or malformed, or if the numeric IDs cannot be parsed.
pub fn parse_nxm_url(url: &str) -> Result<NxmParams, NetError> {
    // Validate scheme prefix.
    let rest = url
        .strip_prefix("nxm://")
        .ok_or_else(|| NetError::Parse(format!("not an nxm:// URL: {url}")))?;

    // Split query string (optional).
    let (authority_and_path, query) = match rest.split_once('?') {
        Some((l, r)) => (l, Some(r)),
        None => (rest, None),
    };

    // Path: {game_domain}/mods/{mod_id}/files/{file_id}
    let mut parts = authority_and_path.splitn(5, '/');
    let game_domain = parts
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| NetError::Parse("missing game domain in nxm URL".to_string()))?
        .to_string();

    // Skip literal "mods" segment.
    match parts.next() {
        Some("mods") => {}
        other => return Err(NetError::Parse(format!("expected 'mods' segment, got: {other:?}"))),
    }

    let mod_id_str = parts
        .next()
        .ok_or_else(|| NetError::Parse("missing mod_id in nxm URL".to_string()))?;
    let mod_id: u32 = mod_id_str
        .parse()
        .map_err(|_| NetError::Parse(format!("invalid mod_id '{mod_id_str}'")))?;

    // Skip literal "files" segment.
    match parts.next() {
        Some("files") => {}
        other => return Err(NetError::Parse(format!("expected 'files' segment, got: {other:?}"))),
    }

    let file_id_str = parts
        .next()
        .ok_or_else(|| NetError::Parse("missing file_id in nxm URL".to_string()))?;
    let file_id: u64 = file_id_str
        .parse()
        .map_err(|_| NetError::Parse(format!("invalid file_id '{file_id_str}'")))?;

    // Parse optional query params: key=…&expires=…
    let mut key: Option<String> = None;
    let mut expires: Option<u64> = None;
    if let Some(q) = query {
        for pair in q.split('&') {
            if let Some(v) = pair.strip_prefix("key=") {
                key = Some(v.to_string());
            } else if let Some(v) = pair.strip_prefix("expires=") {
                expires = v.parse().ok();
            }
        }
    }

    Ok(NxmParams {
        game_domain,
        mod_id,
        file_id,
        key,
        expires,
    })
}

/// Resolve an `nxm://` URL to a direct CDN download URL.
///
/// Creates a temporary [`NexusClient`], parses the NXM URL, calls the Nexus
/// download-link endpoint (forwarding `key`/`expires` for free-tier), and
/// returns the first CDN URI from the response.
///
/// Download links expire within seconds; begin the download immediately after
/// receiving the result.
///
/// # Parameters
/// - `nxm_url`:  Raw `nxm://` URL string (e.g. from a browser deep-link).
/// - `api_key`:  Nexus Mods personal API key used to authenticate the request.
///
/// # Returns
/// Direct HTTPS CDN URL for the file download.
///
/// # Errors
/// Returns [`NetError::Parse`] for malformed NXM URLs, [`NetError::Config`]
/// for an empty API key, [`NetError::Status`] for non-2xx API responses, or
/// [`NetError::Http`] for transport failures.
pub async fn resolve_nxm(nxm_url: &str, api_key: &str) -> Result<String, NetError> {
    let params = parse_nxm_url(nxm_url)?;
    let client = NexusClient::new(api_key)?;

    // Build the download-link URL, appending key/expires for free accounts.
    let mut url = format!(
        "{API_BASE}/games/{}/mods/{}/files/{}/download_link.json",
        params.game_domain, params.mod_id, params.file_id
    );
    let mut sep = '?';
    if let Some(k) = &params.key {
        url.push(sep);
        url.push_str("key=");
        url.push_str(k);
        sep = '&';
    }
    if let Some(exp) = params.expires {
        url.push(sep);
        url.push_str(&format!("expires={exp}"));
    }

    let links: Vec<models::DownloadLink> = client.get_json(&url).await?;
    links
        .into_iter()
        .next()
        .map(|l| l.uri)
        .ok_or_else(|| NetError::Parse("Nexus returned empty download link list".to_string()))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A well-formed `nxm://` URL with key and expires query params parses
    /// correctly.
    #[test]
    fn parse_nxm_url_valid_with_query() {
        let url = "nxm://skyrimspecialedition/mods/12345/files/67890?key=abc123&expires=9999999";
        let params = parse_nxm_url(url).expect("should parse");
        assert_eq!(params.game_domain, "skyrimspecialedition");
        assert_eq!(params.mod_id, 12345);
        assert_eq!(params.file_id, 67890);
        assert_eq!(params.key.as_deref(), Some("abc123"));
        assert_eq!(params.expires, Some(9_999_999));
    }

    /// A minimal `nxm://` URL with no query string (premium accounts) parses
    /// correctly and leaves `key`/`expires` as `None`.
    #[test]
    fn parse_nxm_url_valid_no_query() {
        let url = "nxm://fallout4/mods/1/files/2";
        let params = parse_nxm_url(url).expect("should parse");
        assert_eq!(params.game_domain, "fallout4");
        assert_eq!(params.mod_id, 1);
        assert_eq!(params.file_id, 2);
        assert!(params.key.is_none());
        assert!(params.expires.is_none());
    }

    /// A URL with the wrong scheme returns `NetError::Parse`.
    #[test]
    fn parse_nxm_url_wrong_scheme() {
        let result = parse_nxm_url("https://nexusmods.com/mods/1/files/2");
        assert!(
            matches!(result, Err(NetError::Parse(_))),
            "expected Parse error, got: {result:?}"
        );
    }

    /// A `nxm://` URL missing the file_id segment returns `NetError::Parse`.
    #[test]
    fn parse_nxm_url_missing_file_id() {
        let result = parse_nxm_url("nxm://skyrimspecialedition/mods/1/files/");
        // file_id "" cannot be parsed as u64
        assert!(
            matches!(result, Err(NetError::Parse(_))),
            "expected Parse error for empty file_id, got: {result:?}"
        );
    }
}
