//! Session Logger — example native Mantle Manager plugin.
//!
//! Subscribes to [`ModManagerEvent::GameLaunching`] and [`ModManagerEvent::GameExited`].
//! On each session it appends one line to `session.log` inside the plugin's data
//! directory:
//!
//! ```text
//! 2026-03-04T14:22:01Z  Skyrim SE  exit=0  duration=3647s
//! ```
//!
//! # What this demonstrates
//! - Subscribing to multiple event types with [`EventFilter::All`] and dispatching manually
//! - Using [`PluginContext::data_dir`] to persist data between sessions
//! - Sharing mutable state across event callbacks with [`std::sync::Mutex`]
//! - Reading game info from the event payload

use std::ffi::CString;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mantle_core::plugin::{
    context::{
        Capability, MantlePlugin, NotifyLevel, PluginContext, PluginError, PluginSetting,
        RUSTC_TOOLCHAIN_VERSION,
    },
    event::{EventFilter, ModManagerEvent, SubscriptionHandle},
};
use semver::Version;

// ─── Shared launch-time state ─────────────────────────────────────────────────

/// Unix timestamp (seconds) of the most recent `GameLaunching` event.
///
/// Shared between the two event callbacks so `GameExited` can calculate the
/// session duration without requiring a mutable reference to `self`.
type LaunchTimestamp = Arc<Mutex<Option<u64>>>;

// ─── Plugin struct ────────────────────────────────────────────────────────────

/// The plugin state persisted for the lifetime of the plugin.
pub struct SessionLogger {
    /// Keeps the event subscription alive.
    handle: Option<SubscriptionHandle>,
}

impl SessionLogger {
    fn new() -> Self {
        Self { handle: None }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Returns the current UTC time as a rough ISO-8601 string.
///
/// Uses only `std` (no chrono dependency) by formatting the Unix epoch offset
/// manually. Precision is seconds only.
fn now_utc_string() -> String {
    // Basic epoch-to-date conversion for logging purposes.
    // For production use, pull in `chrono` or `time` instead.
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{secs}s-since-epoch") // Replace with chrono::Utc::now() in real plugins
}

/// Returns seconds since Unix epoch.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Appends one log line to `<data_dir>/session.log`.
///
/// Creates the file if it does not exist. Silently ignores write errors (logs
/// them via `tracing::warn` instead).
///
/// # Parameters
/// - `data_dir`: Plugin data directory from [`PluginContext::data_dir`].
/// - `line`: Text line to append (no trailing newline needed).
fn append_log(data_dir: &std::path::Path, line: &str) {
    let path = data_dir.join("session.log");
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut f) => {
            if let Err(e) = writeln!(f, "{line}") {
                tracing::warn!("[session_logger] failed to write log: {e}");
            }
        }
        Err(e) => tracing::warn!("[session_logger] failed to open {}: {e}", path.display()),
    }
}

// ─── MantlePlugin implementation ─────────────────────────────────────────────

impl MantlePlugin for SessionLogger {
    fn name(&self) -> &str {
        "Session Logger"
    }

    fn version(&self) -> Version {
        Version::new(0, 1, 0)
    }

    fn author(&self) -> &str {
        "Your Name"
    }

    fn description(&self) -> &str {
        "Appends one line per game session to session.log in the plugin data directory."
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![]
    }

    fn settings(&self) -> Vec<PluginSetting> {
        // This plugin has no user-configurable settings.
        vec![]
    }

    /// Subscribes to all events and handles `GameLaunching` / `GameExited`.
    ///
    /// A shared [`Mutex`] records the launch timestamp so the exit handler can
    /// calculate duration without a mutable borrow of `self`.
    ///
    /// # Parameters
    /// - `ctx`: Shared plugin context.
    fn init(&mut self, ctx: Arc<PluginContext>) -> Result<(), PluginError> {
        let data_dir = ctx.data_dir();
        let ctx_clone = Arc::clone(&ctx);

        // Shared state: the launch timestamp set by GameLaunching and consumed
        // by GameExited.
        let launch_time: LaunchTimestamp = Arc::new(Mutex::new(None));
        let lt_clone = Arc::clone(&launch_time);

        self.handle = Some(ctx.subscribe(EventFilter::All, move |event| {
            match event {
                ModManagerEvent::GameLaunching(game) => {
                    let ts = unix_now();
                    *lt_clone.lock().expect("launch_time lock poisoned") = Some(ts);

                    let line = format!(
                        "{} | {} | LAUNCHED",
                        now_utc_string(),
                        game.name
                    );
                    append_log(&data_dir, &line);
                    ctx_clone.notify(
                        NotifyLevel::Info,
                        &format!("Session Logger: {} session started", game.name),
                    );
                }

                ModManagerEvent::GameExited { game, exit_code } => {
                    let start = lt_clone
                        .lock()
                        .expect("launch_time lock poisoned")
                        .take()
                        .unwrap_or(unix_now());

                    let duration_secs = unix_now().saturating_sub(start);

                    let line = format!(
                        "{} | {} | EXIT={exit_code} | duration={duration_secs}s",
                        now_utc_string(),
                        game.name
                    );
                    append_log(&data_dir, &line);
                    ctx_clone.notify(
                        NotifyLevel::Info,
                        &format!(
                            "Session Logger: {} exited (code {exit_code}) after {duration_secs}s",
                            game.name
                        ),
                    );
                }

                // Ignore all other events.
                _ => {}
            }
        }));

        tracing::info!("[session_logger] initialised — logging to {:?}", ctx.data_dir());
        Ok(())
    }

    fn shutdown(&mut self) {
        self.handle = None;
        tracing::info!("[session_logger] shut down");
    }
}

// ─── Required C exports ───────────────────────────────────────────────────────

/// Allocate and return the plugin as a raw fat pointer owned by the host.
///
/// # Safety
/// The host must call this exactly once per library load and must call
/// `shutdown()` before dropping the returned pointer.
#[no_mangle]
pub extern "C" fn create_plugin() -> *mut dyn MantlePlugin {
    Box::into_raw(Box::new(SessionLogger::new()))
}

/// Return the Rust toolchain version this plugin was compiled with.
///
/// # Safety
/// Returns a pointer to a `'static` nul-terminated C string.
#[no_mangle]
pub extern "C" fn create_plugin_rustc_version() -> *const std::ffi::c_char {
    // SAFETY: RUSTC_TOOLCHAIN_VERSION is &'static str with no interior nuls.
    CString::new(RUSTC_TOOLCHAIN_VERSION)
        .expect("RUSTC_TOOLCHAIN_VERSION contains nul")
        .into_raw()
}
