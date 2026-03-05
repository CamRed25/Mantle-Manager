/// Download management module.
///
/// Exposes [`DownloadQueue`] and [`DownloadProgress`] for use by
/// `window.rs` (queue ownership + progress idle loop) and
/// `pages::downloads` (button dispatch).
pub mod queue;

pub use queue::{DownloadProgress, DownloadQueue};
