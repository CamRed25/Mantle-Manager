/// In-memory download queue: stores [`DownloadJob`] entries and exposes the
/// CRUD operations called by the downloads page buttons.
///
/// When compiled with the `net` feature, [`DownloadQueue::enqueue`] spawns a
/// real HTTP download task via `mantle_net`.  Without `net` every job
/// immediately fails with a "not implemented" message (scaffolding behaviour).
use std::{collections::VecDeque, path::PathBuf, sync::mpsc};

use uuid::Uuid;

use crate::state::{DownloadEntry, DownloadStatus};

// ---------------------------------------------------------------------------
// Public data types
// ---------------------------------------------------------------------------

/// A single item being (or waiting to be) downloaded.
// `url` and `dest` are only read by `spawn_download` (net feature) and retry;
// without the net feature they appear unused to the linter.
#[allow(dead_code)]
pub struct DownloadJob {
    /// Stable UUID – used as a routing key for cancel / retry / clear actions.
    pub id: Uuid,
    /// Remote URL to fetch — stored so [`DownloadQueue::retry`] can
    /// restart the job; also moved into `spawn_download` (net feature).
    pub url: String,
    /// Human-readable mod name shown in the UI.
    pub mod_name: String,
    /// Destination path — stored for retry; passed to `spawn_download`
    /// (net feature).  Not read by snapshot / cancel paths.
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
// Background download task
// ---------------------------------------------------------------------------

/// Spawn a detached OS thread that downloads `url` to `dest` and reports
/// progress via `progress_tx`.
///
/// A single-threaded Tokio runtime is created per spawn so the async
/// `mantle_net` functions can be `await`-ed without requiring a shared
/// runtime.  Status transitions emitted:
///
/// `Queued` (caller) → `InProgress{…}` (per chunk) → `Complete{bytes}` | `Failed(msg)`
///
/// # Parameters
/// - `id`          – UUID that identifies the job in the UI queue.
/// - `url`         – remote HTTPS URL to stream from.
/// - `dest`        – filesystem path for the finished archive.
/// - `progress_tx` – channel to send status updates back to the UI thread.
#[cfg(feature = "net")]
fn spawn_download(
    id: Uuid,
    url: String,
    dest: std::path::PathBuf,
    progress_tx: mpsc::Sender<DownloadProgress>,
) {
    std::thread::spawn(move || {
        // Build a single-threaded Tokio runtime for this download.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio rt for spawn_download");

        // Build the reqwest client; if it fails, immediately mark as failed.
        let client = match mantle_net::download::build_client() {
            Ok(c) => c,
            Err(e) => {
                let _ = progress_tx.send(DownloadProgress {
                    id,
                    status: DownloadStatus::Failed(e.to_string()),
                });
                return;
            }
        };

        // Notify UI that the download is now active.
        let _ = progress_tx.send(DownloadProgress {
            id,
            status: DownloadStatus::InProgress {
                progress: 0.0,
                bytes_done: 0,
                total_bytes: None,
            },
        });

        // Stream the file, forwarding chunk-level progress updates.
        let tx = progress_tx.clone();
        let result = rt.block_on(mantle_net::download::download_file(
            &url,
            &dest,
            move |ev| {
                let mantle_net::download::DownloadEvent::Progress { downloaded, total } = ev;
                #[allow(clippy::cast_precision_loss)]
                let progress = total.map_or(0.0, |t| downloaded as f64 / t as f64);
                let _ = tx.send(DownloadProgress {
                    id,
                    status: DownloadStatus::InProgress {
                        progress,
                        bytes_done: downloaded,
                        total_bytes: total,
                    },
                });
            },
            &client,
        ));

        // Send the terminal status regardless of success or failure.
        let final_status = match result {
            Ok(bytes) => DownloadStatus::Complete { bytes },
            Err(e) => DownloadStatus::Failed(e.to_string()),
        };
        let _ = progress_tx.send(DownloadProgress {
            id,
            status: final_status,
        });
    });
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
    /// With the `net` feature enabled, spawns a background OS thread that
    /// streams the file from `url` and sends [`DownloadProgress`] updates
    /// back to the idle-poll loop.  Without `net`, the job immediately
    /// transitions to `Failed` (scaffolding behaviour).
    ///
    /// # Parameters
    /// - `url`      – remote HTTPS URL to fetch.
    /// - `mod_name` – human-readable name shown in the UI.
    /// - `dest`     – filesystem path for the downloaded archive.
    ///
    /// # Returns
    /// The [`Uuid`] assigned to the new job.
    // Will be called from the downloads page once item-14 wires the UI.
    // `dest` is consumed by `spawn_download` in the `net` feature path;
    // without `net` it is only cloned, causing a needless-pass-by-value warning.
    #[allow(dead_code)]
    #[cfg_attr(not(feature = "net"), allow(clippy::needless_pass_by_value))]
    pub fn enqueue(
        &mut self,
        url: impl Into<String>,
        mod_name: impl Into<String>,
        dest: PathBuf,
    ) -> Uuid {
        let id = Uuid::new_v4();
        let url = url.into();
        let mod_name = mod_name.into();

        // Record the job as Queued immediately so the UI reflects it.
        self.jobs.push_back(DownloadJob {
            id,
            url: url.clone(),
            mod_name,
            dest: dest.clone(),
            status: DownloadStatus::Queued,
        });
        let _ = self.progress_tx.send(DownloadProgress {
            id,
            status: DownloadStatus::Queued,
        });

        // ── Real HTTP download (net feature only) ─────────────────────
        #[cfg(feature = "net")]
        spawn_download(id, url, dest, self.progress_tx.clone());

        // ── Stub: immediately fail when net feature is absent ─────────
        #[cfg(not(feature = "net"))]
        {
            let status = DownloadStatus::Failed("HTTP fetch not yet implemented".to_string());
            let _ = self.progress_tx.send(DownloadProgress { id, status });
        }

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
                    job.status = DownloadStatus::Queued;
                    let _ = self.progress_tx.send(DownloadProgress {
                        id,
                        status: DownloadStatus::Queued,
                    });

                    #[cfg(feature = "net")]
                    spawn_download(id, job.url.clone(), job.dest.clone(), self.progress_tx.clone());

                    #[cfg(not(feature = "net"))]
                    {
                        let status =
                            DownloadStatus::Failed("HTTP fetch not yet implemented".to_string());
                        let _ = self.progress_tx.send(DownloadProgress { id, status });
                    }
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
        self.jobs
            .retain(|j| !(j.id == id && matches!(j.status, DownloadStatus::Complete { .. })));
    }

    /// Remove **all** completed jobs from the queue in one pass.
    pub fn clear_completed(&mut self) {
        self.jobs.retain(|j| !matches!(j.status, DownloadStatus::Complete { .. }));
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
