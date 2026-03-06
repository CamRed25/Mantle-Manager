//! HTTP file-download engine.
//!
//! Streams a remote URL to a local file, emitting byte-count progress events
//! via a user-supplied callback.  The download is written to a temp file in
//! the same directory as `dest`, then atomically renamed on completion.
//!
//! # Usage
//! ```no_run
//! use std::path::Path;
//! use mantle_net::download::{download_file, DownloadEvent};
//!
//! # async fn _example() {
//! let client = reqwest::Client::new();
//! download_file(
//!     "https://example.com/mod.zip",
//!     Path::new("/tmp/mod.zip"),
//!     |ev| match ev {
//!         DownloadEvent::Progress { downloaded, total } => {
//!             println!("Downloaded {downloaded} / {total:?}");
//!         }
//!     },
//!     &client,
//! ).await.unwrap();
//! # }
//! ```

use std::path::Path;

use reqwest::Client;
use tokio::io::AsyncWriteExt;

use crate::error::NetError;

// ─── Progress events ──────────────────────────────────────────────────────────

/// Progress notification emitted during a [`download_file`] call.
///
/// The callback receives one `Progress` event per HTTP chunk received.
/// When `total` is `None` the server did not send a `Content-Length` header.
#[derive(Debug, Clone)]
pub enum DownloadEvent {
    /// A chunk of bytes has been written to the temp file.
    Progress {
        /// Total bytes written so far.
        downloaded: u64,
        /// Expected total file size, or `None` if unknown.
        total: Option<u64>,
    },
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Download `url` to `dest`, streaming with progress callbacks.
///
/// Writes the response body to `{dest}.tmp` in the same directory, then
/// atomically renames it to `dest` on success.  If the function returns an
/// error the temp file may remain on disk and should be cleaned up by the
/// caller.
///
/// The parent directory of `dest` must already exist.
///
/// # Parameters
/// - `url`         – remote URL to fetch (`GET`).
/// - `dest`        – final destination path for the downloaded file.
/// - `on_progress` – callback invoked once per chunk with byte-count stats.
/// - `client`      – shared [`reqwest::Client`]; pass a pre-configured client
///   to reuse TLS sessions and apply timeouts/headers.
///
/// # Returns
/// Total number of bytes written to `dest` on success.
///
/// # Errors
/// Returns a [`NetError`] if the request fails, the server returns a non-2xx
/// status, or any I/O operation fails.
///
/// # Side Effects
/// Creates or overwrites `dest` (via an intermediate `.tmp` file).
pub async fn download_file(
    url: &str,
    dest: &Path,
    on_progress: impl Fn(DownloadEvent),
    client: &Client,
) -> Result<u64, NetError> {
    let mut response = client.get(url).send().await?;

    // Fail early on non-2xx status before streaming the body.
    let status = response.status();
    if !status.is_success() {
        let code = status.as_u16();
        let body = response.text().await.unwrap_or_default();
        return Err(NetError::Status { status: code, body });
    }

    let total = response.content_length();

    // Write to a temp file; rename atomically on completion.
    let tmp_path = dest.with_extension("tmp");
    let mut file = tokio::fs::File::create(&tmp_path).await?;

    let mut downloaded: u64 = 0;

    // `chunk()` reads one response chunk at a time without requiring
    // external StreamExt imports.
    while let Some(bytes) = response.chunk().await? {
        file.write_all(&bytes).await?;
        downloaded += bytes.len() as u64;
        on_progress(DownloadEvent::Progress { downloaded, total });
    }

    file.flush().await?;
    drop(file);

    tokio::fs::rename(&tmp_path, dest).await?;
    Ok(downloaded)
}

/// Build a default [`reqwest::Client`] configured for Mantle Manager.
///
/// Uses `rustls` TLS (no OpenSSL linkage), a 30-second connect timeout,
/// and a user-agent identifying the client.
///
/// # Errors
/// Returns a [`NetError`] if the client builder fails (extremely unlikely).
pub fn build_client() -> Result<Client, NetError> {
    Client::builder()
        .user_agent(concat!("mantle-manager/", env!("CARGO_PKG_VERSION"),))
        .build()
        .map_err(NetError::Http)
}
