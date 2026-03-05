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
//! # tokio_test::block_on(async {
//! use mantle_net::nexus::NexusClient;
//! let client = NexusClient::new("my-api-key").unwrap();
//! let mods = client.search("skyrimspecialedition", "SKSE", 3171).await.unwrap();
//! # });
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
            return Err(NetError::Config(
                "Nexus API key must not be empty".to_string(),
            ));
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
        let url = format!(
            "{SEARCH_BASE}/mods?terms={}&game_id={}",
            urlencoded(query),
            nexus_game_id,
        );
        // Search endpoint does not require apikey header.
        let resp = self.client.get(&url).send().await?;
        check_status(resp).await?.json::<SearchResponse>().await.map_err(NetError::Http)
    }

    // ─── Private helpers ──────────────────────────────────────────────────

    /// Issue an authenticated GET to `url` and deserialise the JSON response.
    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, NetError> {
        let resp = self
            .client
            .get(url)
            .header("apikey", &self.api_key)
            .send()
            .await?;
        check_status(resp)
            .await?
            .json::<T>()
            .await
            .map_err(NetError::Http)
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
