/// In-memory download queue: stores [`DownloadJob`] entries and exposes the
/// CRUD operations called by the downloads page buttons.
///
/// **Scaffolding note** – `enqueue` immediately marks every new job as
/// `DownloadStatus::Failed("HTTP fetch not yet implemented")`.  Real streaming
/// downloads will be wired in a later iteration; see `futures.md`.
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::mpsc,
};

use uuid::Uuid;

use crate::state::{DownloadEntry, DownloadStatus};

// ---------------------------------------------------------------------------
// Public data types
// ---------------------------------------------------------------------------

/// A single item being (or waiting to be) downloaded.
#[allow(dead_code)] // `url` and `dest` used when HTTP fetch is implemented
pub struct DownloadJob {
    /// Stable UUID – used as a routing key for cancel / retry / clear actions.
    pub id: Uuid,
    /// Remote URL to fetch (deferred; not used in scaffolding).
    pub url: String,
    /// Human-readable mod name shown in the UI.
    pub mod_name: String,
    /// Filesystem path where the downloaded archive should be written.
    pub dest: PathBuf,
    /// Current lifecycle status of this download.
    pub status: DownloadStatus,
}

/// Status update pushed from a background download task to the UI thread.
///
/// The UI idle loop drains an `mpsc::Receiver<DownloadProgress>` and calls
/// [`DownloadQueue::apply_progress`] for each message received.
#[derive(Clone)]
pub struct DownloadProgress {
    /// Job this update belongs to.
    pub id: Uuid,
    /// New status to apply.
    pub status: DownloadStatus,
}

// ---------------------------------------------------------------------------
// DownloadQueue
// ---------------------------------------------------------------------------

/// Thread-local download queue owned by the UI window.
///
/// All mutations happen on the GTK main thread.  Background tasks
/// communicate back via the [`mpsc::Sender<DownloadProgress>`] channel.
pub struct DownloadQueue {
    /// Ordered list of all jobs (active + historical).
    jobs: VecDeque<DownloadJob>,
    /// Sender half given to future background tasks so they can push progress
    /// updates back to the UI idle loop.
    progress_tx: mpsc::Sender<DownloadProgress>,
}

impl DownloadQueue {
    // -----------------------------------------------------------------------
    // Construction
    // -----------------------------------------------------------------------

    /// Create a new, empty `DownloadQueue`.
    ///
    /// # Parameters
    /// - `progress_tx` – sender end of the progress channel.  The UI supplies
    ///   its own `Receiver`; background tasks will clone `progress_tx` when
    ///   HTTP download is implemented.
    pub fn new(progress_tx: mpsc::Sender<DownloadProgress>) -> Self {
        Self {
            jobs: VecDeque::new(),
            progress_tx,
        }
    }

    // -----------------------------------------------------------------------
    // Enqueue
    // -----------------------------------------------------------------------

    /// Add a new download job to the queue and return its stable [`Uuid`].
    ///
    /// **Scaffolding**: the job is immediately transitioned to
    /// `DownloadStatus::Failed("HTTP fetch not yet implemented")` rather than
    /// starting a real network transfer.  A progress update is also sent on
    /// the channel so the idle loop processes it uniformly.
    ///
    /// # Parameters
    /// - `url`      – remote URL (unused in scaffolding but stored for later).
    /// - `mod_name` – human-readable name shown in the UI.
    /// - `dest`     – filesystem path for the downloaded archive.
    ///
    /// # Returns
    /// The [`Uuid`] assigned to the new job.
    #[allow(dead_code)] // entry point for future HTTP download wiring
    pub fn enqueue(&mut self, url: impl Into<String>, mod_name: impl Into<String>, dest: PathBuf) -> Uuid {
        let id = Uuid::new_v4();
        let status = DownloadStatus::Failed("HTTP fetch not yet implemented".to_string());

        self.jobs.push_back(DownloadJob {
            id,
            url: url.into(),
            mod_name: mod_name.into(),
            dest,
            status: status.clone(),
        });

        // Push the initial status onto the progress channel so the idle loop
        // applies it consistently (no-op in scaffolding, but keeps the path
        // exercised for when real workers are added).
        let _ = self.progress_tx.send(DownloadProgress { id, status });

        id
    }

    // -----------------------------------------------------------------------
    // Cancel
    // -----------------------------------------------------------------------

    /// Cancel an active or queued download.
    ///
    /// Transitions the job's status from [`DownloadStatus::InProgress`] or
    /// [`DownloadStatus::Queued`] to [`DownloadStatus::Cancelled`].
    /// Jobs that are already complete, failed, or cancelled are left unchanged.
    ///
    /// # Parameters
    /// - `id` – the UUID of the job to cancel.
    pub fn cancel(&mut self, id: Uuid) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            match job.status {
                DownloadStatus::InProgress { .. } | DownloadStatus::Queued => {
                    job.status = DownloadStatus::Cancelled;
                }
                _ => {}
            }
        }
    }

    // -----------------------------------------------------------------------
    // Retry
    // -----------------------------------------------------------------------

    /// Re-queue a failed or cancelled download for another attempt.
    ///
    /// Transitions the job's status to [`DownloadStatus::Queued`].
    /// Jobs that are not in `Failed` or `Cancelled` state are left unchanged.
    ///
    /// **Scaffolding**: re-queuing will immediately fail again because
    /// `enqueue` does not start real HTTP workers yet.  A direct status
    /// write is used here so the job stays in the existing queue position.
    ///
    /// # Parameters
    /// - `id` – the UUID of the job to retry.
    pub fn retry(&mut self, id: Uuid) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            match job.status {
                DownloadStatus::Failed(_) | DownloadStatus::Cancelled => {
                    job.status = DownloadStatus::Failed(
                        "HTTP fetch not yet implemented".to_string(),
                    );
                }
                _ => {}
            }
        }
    }

    // -----------------------------------------------------------------------
    // Clear
    // -----------------------------------------------------------------------

    /// Remove a single completed job from the queue.
    ///
    /// Only removes the job if its status is [`DownloadStatus::Complete`].
    ///
    /// # Parameters
    /// - `id` – the UUID of the completed job to remove.
    pub fn remove_completed(&mut self, id: Uuid) {
        self.jobs.retain(|j| {
            !(j.id == id && matches!(j.status, DownloadStatus::Complete { .. }))
        });
    }

    /// Remove **all** completed jobs from the queue in one pass.
    pub fn clear_completed(&mut self) {
        self.jobs
            .retain(|j| !matches!(j.status, DownloadStatus::Complete { .. }));
    }

    // -----------------------------------------------------------------------
    // Progress application
    // -----------------------------------------------------------------------

    /// Apply a progress update received from the idle-loop channel drain.
    ///
    /// The job identified by `progress.id` has its `status` field replaced
    /// with `progress.status`.  If no matching job exists the update is
    /// silently discarded.
    ///
    /// # Parameters
    /// - `progress` – the [`DownloadProgress`] message received from the
    ///   channel.
    pub fn apply_progress(&mut self, progress: DownloadProgress) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == progress.id) {
            job.status = progress.status;
        }
    }

    // -----------------------------------------------------------------------
    // Snapshot
    // -----------------------------------------------------------------------

    /// Return a point-in-time snapshot of all jobs as [`DownloadEntry`] values
    /// suitable for passing directly to `pages::downloads::build`.
    ///
    /// The snapshot is an owned `Vec` so the borrow on `self` is released
    /// before the GTK widget tree is updated.
    pub fn snapshot(&self) -> Vec<DownloadEntry> {
        self.jobs
            .iter()
            .map(|job| DownloadEntry {
                id: job.id.to_string(),
                name: job.mod_name.clone(),
                state: job.status.clone(),
            })
            .collect()
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Return a reference to the channel sender so future download worker
    /// code can clone it when spawning background tasks.
    #[allow(dead_code)]
    pub(crate) fn progress_tx(&self) -> &mpsc::Sender<DownloadProgress> {
        &self.progress_tx
    }
}
