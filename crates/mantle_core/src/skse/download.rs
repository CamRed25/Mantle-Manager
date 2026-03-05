//! HTTP download with retry / exponential-backoff.
//!
//! [`download_file`] downloads a URL to a local path.  The response body is
//! streamed in chunks and written to a [`tempfile::NamedTempFile`] in the same
//! directory as `dest`; on completion the temp file is atomically renamed to
//! `dest`.
//!
//! # Retry policy
//!
//! | Condition               | Action                          |
//! |-------------------------|---------------------------------|
//! | Network / timeout error | Retry with exponential backoff  |
//! | HTTP 5xx                | Retry with exponential backoff  |
//! | HTTP 4xx                | Return immediately (permanent)  |
//!
//! Backoff delay = `initial_backoff_ms × 2^(attempt − 1)`, capped at 64 s.

use std::{path::Path, time::Duration};

use tokio::io::AsyncWriteExt as _;

use crate::error::MantleError;

// ── Public types ──────────────────────────────────────────────────────────────

/// Tuning knobs for [`download_file`].
#[derive(Debug, Clone)]
pub struct DownloadConfig {
    /// Maximum number of retry attempts after the initial try.  Default: 3.
    pub max_retries: u32,
    /// Base delay in milliseconds before the first retry.  Default: 1 000 ms.
    /// Doubles on each subsequent retry, capped at 64 000 ms.
    pub initial_backoff_ms: u64,
    /// Per-request connection + read timeout.  Default: 60 s.
    pub timeout_secs: u64,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff_ms: 1_000,
            timeout_secs: 60,
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Downloads `url` to `dest`, calling `progress(bytes_downloaded, total)` after
/// each received chunk.  `total` is `None` when the server omits
/// `Content-Length`.
///
/// The file at `dest` is replaced atomically on success; on failure `dest` is
/// left untouched (the temp file is discarded automatically).
///
/// # Errors
///
/// Returns [`MantleError::Skse`] on non-retryable HTTP errors, or after
/// exhausting all retry attempts on transient failures.
/// Returns [`MantleError::Io`] on filesystem errors.
pub async fn download_file<F>(
    url: &str,
    dest: &Path,
    cfg: &DownloadConfig,
    progress: F,
) -> Result<(), MantleError>
where
    F: Fn(u64, Option<u64>),
{
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(cfg.timeout_secs))
        .build()
        .map_err(|e| MantleError::Skse(format!("Failed to build HTTP client: {e}")))?;

    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;

    for attempt in 0..=cfg.max_retries {
        if attempt > 0 {
            let delay = cfg.initial_backoff_ms.saturating_mul(1u64 << (attempt - 1).min(6));
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }

        // ── Send request ─────────────────────────────────────────────────────
        let mut resp = match client.get(url).send().await {
            Ok(r) => r,
            Err(e) => {
                if attempt >= cfg.max_retries {
                    return Err(MantleError::Skse(format!(
                        "Download of {url} failed after {} attempt(s): {e}",
                        attempt + 1
                    )));
                }
                tracing::warn!(
                    url,
                    attempt,
                    max_retries = cfg.max_retries,
                    "Network error, will retry: {e}"
                );
                continue;
            }
        };

        if resp.status().is_client_error() {
            return Err(MantleError::Skse(format!(
                "HTTP {} fetching {url} — permanent error",
                resp.status()
            )));
        }
        if resp.status().is_server_error() {
            if attempt >= cfg.max_retries {
                return Err(MantleError::Skse(format!(
                    "HTTP {} fetching {url} after {} attempt(s)",
                    resp.status(),
                    attempt + 1
                )));
            }
            tracing::warn!(url, status = %resp.status(), attempt, "Server error, will retry");
            continue;
        }

        // ── Stream to temp file ───────────────────────────────────────────────
        let content_length = resp.content_length();
        let tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| {
            MantleError::Skse(format!("Failed to create temp file in {}: {e}", parent.display()))
        })?;
        let (std_file, tmp_path) = tmp.into_parts();
        let mut writer = tokio::fs::File::from_std(std_file);
        let mut bytes_written: u64 = 0;
        let mut net_err: Option<String> = None;

        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    writer.write_all(&chunk).await?;
                    bytes_written += chunk.len() as u64;
                    progress(bytes_written, content_length);
                }
                Ok(None) => break,
                Err(e) => {
                    net_err = Some(e.to_string());
                    break;
                }
            }
        }

        writer.flush().await?;
        drop(writer);

        if let Some(msg) = net_err {
            let _ = tmp_path.close();
            if attempt >= cfg.max_retries {
                return Err(MantleError::Skse(format!("Stream error downloading {url}: {msg}")));
            }
            tracing::warn!(url, attempt, "Stream error, will retry: {msg}");
            continue;
        }

        tmp_path.persist(dest).map_err(|e| {
            MantleError::Skse(format!("Failed to persist download to {}: {e}", dest.display()))
        })?;

        tracing::debug!(url, dest = %dest.display(), bytes = bytes_written, "Download complete");
        return Ok(());
    }

    Err(MantleError::Skse(format!(
        "Download of {url} failed after {} attempt(s)",
        cfg.max_retries + 1
    )))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_config_defaults() {
        let cfg = DownloadConfig::default();
        assert_eq!(cfg.max_retries, 3);
        assert_eq!(cfg.timeout_secs, 60);
        assert!(cfg.initial_backoff_ms > 0);
    }

    #[tokio::test]
    async fn download_fails_on_invalid_url() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.7z");
        let cfg = DownloadConfig {
            max_retries: 0,
            ..DownloadConfig::default()
        };
        let result = download_file("http://127.0.0.1:1", &dest, &cfg, |_, _| {}).await;
        assert!(result.is_err());
        assert!(!dest.exists());
    }
}
