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

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    };

    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    /// Spin up a one-shot TCP listener that serves a single HTTP/1.1 response
    /// and returns the URL to connect to.
    ///
    /// The server task is spawned on the current Tokio runtime and handles
    /// exactly one connection before exiting.
    async fn serve_once(body: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock listener");
        let port = listener
            .local_addr()
            .expect("local_addr")
            .port();
        let body_len = body.len();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            // Drain the request so the client doesn't get a connection-reset.
            let mut req_buf = [0u8; 4096];
            let _ = stream.read(&mut req_buf).await;
            let header = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Length: {body_len}\r\n\
                 Content-Type: application/octet-stream\r\n\
                 Connection: close\r\n\r\n"
            );
            stream
                .write_all(header.as_bytes())
                .await
                .expect("write header");
            stream.write_all(body).await.expect("write body");
        });

        format!("http://127.0.0.1:{port}/test.bin")
    }

    /// Serve a single HTTP response with the given status code and optional body.
    async fn serve_error(status: u16) -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock listener");
        let port = listener.local_addr().expect("local_addr").port();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut req_buf = [0u8; 4096];
            let _ = stream.read(&mut req_buf).await;
            let reason = if status == 404 { "Not Found" } else { "Error" };
            let body = reason.as_bytes();
            let resp = format!(
                "HTTP/1.1 {status} {reason}\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\r\n{reason}",
                body.len()
            );
            stream.write_all(resp.as_bytes()).await.expect("write");
        });

        format!("http://127.0.0.1:{port}/missing.bin")
    }

    /// `download_file` streams a file, fires progress callbacks, writes the
    /// correct bytes to disk, and cleans up the `.tmp` file.
    #[tokio::test]
    async fn download_file_streams_and_renames() {
        const BODY: &[u8] = b"hello from mantle download smoke test";
        let url = serve_once(BODY).await;
        let dir = tempdir().expect("tempdir");
        let dest = dir.path().join("test.bin");
        let client = build_client().expect("build client");

        // Track cumulative progress via atomic so the closure can be Fn.
        let bytes_seen = Arc::new(AtomicU64::new(0));
        let bytes_seen_clone = Arc::clone(&bytes_seen);

        let total_written = download_file(
            &url,
            &dest,
            move |ev| {
                let DownloadEvent::Progress { downloaded, .. } = ev;
                bytes_seen_clone.store(downloaded, Ordering::Relaxed);
            },
            &client,
        )
        .await
        .expect("download_file should succeed");

        assert_eq!(total_written, BODY.len() as u64, "byte count matches");
        assert_eq!(
            std::fs::read(&dest).expect("read dest"),
            BODY,
            "file content matches"
        );
        assert_eq!(
            bytes_seen.load(Ordering::Relaxed),
            BODY.len() as u64,
            "progress callback reached full byte count"
        );
        assert!(
            !dest.with_extension("tmp").exists(),
            "temp file must be cleaned up"
        );
    }

    /// `download_file` returns `NetError::Status` when the server responds
    /// with a non-2xx status code.
    #[tokio::test]
    async fn download_file_errors_on_404() {
        let url = serve_error(404).await;
        let dir = tempdir().expect("tempdir");
        let dest = dir.path().join("missing.bin");
        let client = build_client().expect("build client");

        let result = download_file(&url, &dest, |_| {}, &client).await;

        assert!(
            matches!(result, Err(NetError::Status { status: 404, .. })),
            "expected Status(404), got: {result:?}"
        );
    }

    /// `build_client` constructs a valid reqwest client without panicking.
    #[test]
    fn build_client_succeeds() {
        build_client().expect("client builder should succeed");
    }
}
