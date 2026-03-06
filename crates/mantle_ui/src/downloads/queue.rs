//! In-memory download queue: stores [`DownloadJob`] entries and exposes the
//! CRUD operations called by the downloads page buttons.
//!
//! When compiled with the `net` feature, [`DownloadQueue::enqueue`] spawns a
//! real HTTP download task via `mantle_net`.  Without `net` every job
//! immediately fails with a "not implemented" message.
//!
//! When a `db_path` is supplied (via [`DownloadQueue::new_with_db`]), each
//! enqueue call and each terminal status transition is persisted to `SQLite` so
//! downloads survive an application restart.
//!
//! See futures.md "Download HTTP fetch implementation" for the full
//! implementation plan.
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
                // f64 progress ratio from u64 byte counts; sub-byte precision loss is
                // acceptable for a 0.0–1.0 display value.
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
///
/// When `db_path` is `Some`, each enqueue call and each terminal status
/// transition is persisted to the `SQLite` database so downloads survive
/// an application restart.
pub struct DownloadQueue {
    /// Ordered list of all jobs (active + historical).
    jobs: VecDeque<DownloadJob>,
    /// Sender half given to future background tasks so they can push progress
    /// updates back to the UI idle loop.
    progress_tx: mpsc::Sender<DownloadProgress>,
    /// Optional `SQLite` database path for persisting download state across
    /// application restarts.  `None` means no persistence (tests / offline
    /// builds).
    db_path: Option<std::path::PathBuf>,
}

impl DownloadQueue {
    // -----------------------------------------------------------------------
    // Construction
    // -----------------------------------------------------------------------

    /// Create a new, empty `DownloadQueue` **without** `SQLite` persistence.
    ///
    /// Suitable for unit tests or any code path that does not need to survive
    /// a restart.  Use [`Self::new_with_db`] for production use.
    ///
    /// # Parameters
    /// - `progress_tx` – sender end of the progress channel.
    #[allow(dead_code)]
    pub fn new(progress_tx: mpsc::Sender<DownloadProgress>) -> Self {
        Self {
            jobs: VecDeque::new(),
            progress_tx,
            db_path: None,
        }
    }

    /// Create a new, empty `DownloadQueue` **with** `SQLite` persistence.
    ///
    /// Each [`enqueue`][Self::enqueue] call upserts the job into the
    /// `downloads` table.  Each terminal status transition (Complete, Failed,
    /// Cancelled) updates the row via a fire-and-forget background thread so
    /// the GTK main thread is never blocked.
    ///
    /// # Parameters
    /// - `progress_tx` – sender end of the progress channel.
    /// - `db_path`     – path to the `SQLite` database file (will be created if
    ///   absent, including parent directories, by `SQLite` itself).
    pub fn new_with_db(
        progress_tx: mpsc::Sender<DownloadProgress>,
        db_path: std::path::PathBuf,
    ) -> Self {
        Self {
            jobs: VecDeque::new(),
            progress_tx,
            db_path: Some(db_path),
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
    /// transitions to `Failed`.
    ///
    /// See futures.md "Download HTTP fetch implementation" for when the HTTP
    /// layer will be implemented.
    ///
    /// # Parameters
    /// - `url`      – remote HTTPS URL to fetch.
    /// - `mod_name` – human-readable name shown in the UI.
    /// - `dest`     – filesystem path for the downloaded archive.
    ///
    /// # Returns
    /// The [`Uuid`] assigned to the new job.
    // UI button for enqueue is wired in window.rs; `dest` is consumed by
    // `spawn_download` in the `net` feature path; without `net` it is only
    // cloned, causing a needless-pass-by-value warning.
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
            mod_name: mod_name.clone(),
            dest: dest.clone(),
            status: DownloadStatus::Queued,
        });
        let _ = self.progress_tx.send(DownloadProgress {
            id,
            status: DownloadStatus::Queued,
        });

        // ── Persist to SQLite (fire-and-forget background thread) ─────
        if let Some(db_path) = self.db_path.clone() {
            // Epoch seconds fit comfortably in i64 until the year 292,277,026,596.
            #[allow(clippy::cast_possible_wrap)]
            let added_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let persisted = mantle_core::data::downloads::PersistedDownload {
                id: id.to_string(),
                url: url.clone(),
                filename: mod_name.clone(),
                dest_path: dest.display().to_string(),
                status: "queued".to_string(),
                progress: 0.0,
                total_bytes: None,
                added_at,
            };
            std::thread::spawn(move || {
                if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                    let _ = mantle_core::data::downloads::upsert_download(&conn, &persisted);
                }
            });
        }

        // ── Real HTTP download (net feature only) ─────────────────────
        #[cfg(feature = "net")]
        spawn_download(id, url, dest, self.progress_tx.clone());

        // ── Stub: immediately fail when net feature is absent (see futures.md "Download HTTP fetch implementation") ─
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
    /// Without the `net` feature, re-queuing immediately fails again because
    /// no HTTP worker is started.  See futures.md "Download HTTP fetch
    /// implementation" for the full implementation plan.
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
    pub fn apply_progress(&mut self, progress: &DownloadProgress) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == progress.id) {
            job.status = progress.status.clone();
        } else {
            return;
        }

        // ── Persist terminal status to SQLite (fire-and-forget) ───────
        let is_terminal = matches!(
            progress.status,
            DownloadStatus::Complete { .. } | DownloadStatus::Failed(_) | DownloadStatus::Cancelled
        );
        if is_terminal {
            if let Some(db_path) = self.db_path.clone() {
                let status_str = match &progress.status {
                    DownloadStatus::Complete { .. } => "complete",
                    DownloadStatus::Failed(_) => "failed",
                    DownloadStatus::Cancelled => "cancelled",
                    _ => return,
                };
                let prog = match &progress.status {
                    DownloadStatus::Complete { .. } => 1.0_f64,
                    _ => 0.0_f64,
                };
                let id_str = progress.id.to_string();
                let status_str = status_str.to_string();
                std::thread::spawn(move || {
                    if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                        let _ = mantle_core::data::downloads::update_download_status(
                            &conn,
                            &id_str,
                            &status_str,
                            prog,
                        );
                    }
                });
            }
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
    // Retained as the natural API surface for net-feature progress wiring;
    // no caller yet — see futures.md "Download HTTP fetch implementation".
    #[allow(dead_code)]
    pub(crate) fn progress_tx(&self) -> &mpsc::Sender<DownloadProgress> {
        &self.progress_tx
    }
}
