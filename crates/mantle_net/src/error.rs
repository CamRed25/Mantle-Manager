//! Network errors for `mantle_net`.
//!
//! All public–facing errors in this crate wrap underlying `reqwest` or I/O
//! errors through `thiserror` so callers get a clean, stable interface.

use thiserror::Error;

/// All errors that can be returned by `mantle_net` operations.
#[derive(Debug, Error)]
pub enum NetError {
    /// An HTTP request failed or returned a non-2xx status code.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// A filesystem operation (create, write, rename) failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Response body could not be deserialised as JSON.
    #[error("JSON deserialisation error: {0}")]
    Json(#[from] serde_json::Error),

    /// The server returned a non-2xx HTTP status.
    #[error("server returned HTTP {status}: {body}")]
    Status {
        /// HTTP status code.
        status: u16,
        /// Partial or full response body for diagnostics.
        body: String,
    },

    /// A required configuration value (e.g. API key) is absent or empty.
    #[error("configuration error: {0}")]
    Config(String),

    /// A URL or data value could not be parsed.
    #[error("parse error: {0}")]
    Parse(String),
}
