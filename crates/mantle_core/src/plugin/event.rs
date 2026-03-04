//! Event bus — `ModManagerEvent` enum, `EventBus`, and typed subscription system.
//!
//! The event bus is a thread-safe publish-subscribe broadcaster. Core emits
//! events; plugins subscribe via [`super::context::PluginContext::subscribe`].
//!
//! # Design
//! A handler-list approach is used rather than `tokio::sync::broadcast` because
//! plugin handlers are synchronous `Fn` callbacks (not async futures). Each
//! published event collects matching handler references under the lock, releases
//! the lock, then invokes the handlers — avoiding deadlock if a handler
//! tries to subscribe or unsubscribe during delivery.
//!
//! Handler panics are caught via [`std::panic::catch_unwind`] and logged as
//! warnings; they do not crash the application or prevent other handlers from
//! receiving the event.
//!
//! # References
//! - `PLUGIN_API.md` §5 — event definitions and ordering guarantees

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
};

/// Type alias for a boxed event handler stored in the subscriber map.
type HandlerFn = Arc<dyn Fn(&ModManagerEvent) + Send + Sync>;

use crate::game::GameInfo;

// ─── Plugin-facing mod info ────────────────────────────────────────────────────

/// Plugin-facing snapshot of a single mod in the active profile.
///
/// Returned by [`super::context::PluginContext::mod_list`] and carried in
/// [`ModManagerEvent::ModInstalled`], [`ModManagerEvent::ModEnabled`], and
/// [`ModManagerEvent::ModDisabled`].
///
/// This is a read-only snapshot — plugins cannot mutate the mod list via this
/// type. All fields are `Clone`-able so plugins can store snapshots freely.
#[derive(Debug, Clone, PartialEq)]
pub struct ModInfo {
    /// `mods.id` primary key.
    pub id: i64,
    /// Stable slug used for persistent references across sessions.
    pub slug: String,
    /// Human-readable display name.
    pub name: String,
    /// Mod version string.
    pub version: String,
    /// Author name, if known.
    pub author: String,
    /// Priority in the active profile (1 = highest priority / leftmost `lowerdir`).
    pub priority: i64,
    /// Whether the mod is enabled in the active profile.
    pub is_enabled: bool,
    /// Path to the mod's installation directory on disk.
    pub install_dir: String,
}

// ─── VFS backend discriminant ─────────────────────────────────────────────────

/// Which VFS backend mounted the overlay.
///
/// Carried in [`ModManagerEvent::OverlayMounted`] so plugins can react to the
/// active backend — for example, skipping copy-on-write assumptions when the
/// `SymlinkFarm` fallback is active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VfsBackend {
    /// Kernel new-mount-API overlay (Linux ≥ 6.6, `CAP_SYS_ADMIN` or
    /// user-namespace overlayfs). Fastest and most transparent.
    KernelOverlayfs,
    /// fuse-overlayfs userspace overlay. Used inside Flatpak or on older kernels.
    FuseOverlayfs,
    /// Symlink-farm fallback — maximum compatibility, no copy-on-write.
    SymlinkFarm,
}

// ─── Event enum ───────────────────────────────────────────────────────────────

/// All events emitted by the Mantle Manager core to subscribed plugins.
///
/// # Ordering guarantees
/// - `GameLaunching` fires before overlay mount begins.
/// - `OverlayMounted` fires after mount is verified, before the game process starts.
/// - `GameExited` fires after the game process exits, before overlay teardown.
/// - `ConflictMapUpdated` always fires after `ModInstalled`, `ModEnabled`, `ModDisabled`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ModManagerEvent {
    /// Game is about to launch. Pre-flight checks have passed; the overlay is
    /// **not** yet mounted when this fires.
    GameLaunching(GameInfo),

    /// Game process has exited. Overlay teardown begins after this fires.
    GameExited {
        /// Info for the game that exited.
        game: GameInfo,
        /// Process exit code. `-1` if the exit code could not be retrieved.
        exit_code: i32,
    },

    /// A mod archive was extracted and registered in the mod list.
    ModInstalled(ModInfo),

    /// A mod was enabled in the active profile.
    ModEnabled(ModInfo),

    /// A mod was disabled in the active profile.
    ModDisabled(ModInfo),

    /// The active profile changed.
    ProfileChanged {
        /// Previous profile name.
        old: String,
        /// New profile name.
        new: String,
    },

    /// The VFS overlay was successfully mounted; the game is about to start.
    OverlayMounted {
        /// Backend that performed the mount.
        backend: VfsBackend,
        /// Number of mod layers in the overlay.
        layer_count: usize,
        /// Path to the merged view directory.
        merged_path: PathBuf,
    },

    /// The overlay was unmounted (game exit, profile change, or explicit call).
    OverlayUnmounted {
        /// Path to the now-unmounted merged view directory.
        merged_path: PathBuf,
        /// Wall-clock seconds the overlay was active.
        session_duration_secs: f64,
    },

    /// A download was queued and started.
    DownloadStarted { url: String, dest: PathBuf },

    /// A download completed or failed.
    DownloadCompleted {
        url: String,
        dest: PathBuf,
        /// `Ok(bytes_written)` on success; `Err(message)` on failure.
        result: Result<u64, String>,
    },

    /// Conflict map was recomputed after a mod state change.
    ///
    /// Always fires after `ModInstalled`, `ModEnabled`, or `ModDisabled` once
    /// the rescan completes.
    ConflictMapUpdated {
        /// Slugs of mods whose conflict status changed in this rescan.
        affected_mods: Vec<String>,
        /// Total conflicting file-path count in the active profile after rescan.
        total_conflicts: usize,
    },
}

// ─── Event filter ─────────────────────────────────────────────────────────────

/// Selects which [`ModManagerEvent`] variants a subscription receives.
///
/// Pass to [`EventBus::subscribe`] / [`super::context::PluginContext::subscribe`].
/// [`EventFilter::All`] receives every event regardless of variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventFilter {
    /// Receive all events without filtering.
    All,
    GameLaunching,
    GameExited,
    ModInstalled,
    ModEnabled,
    ModDisabled,
    ProfileChanged,
    OverlayMounted,
    OverlayUnmounted,
    DownloadStarted,
    DownloadCompleted,
    ConflictMapUpdated,
}

impl EventFilter {
    /// Returns `true` if this filter should deliver `event` to the subscriber.
    ///
    /// # Parameters
    /// - `event`: The candidate event to test.
    ///
    /// # Returns
    /// `true` when either `self == EventFilter::All` or `event` matches the
    /// specific variant represented by `self`.
    #[must_use]
    pub fn matches(&self, event: &ModManagerEvent) -> bool {
        match self {
            Self::All                => true,
            Self::GameLaunching      => matches!(event, ModManagerEvent::GameLaunching(..)),
            Self::GameExited         => matches!(event, ModManagerEvent::GameExited { .. }),
            Self::ModInstalled       => matches!(event, ModManagerEvent::ModInstalled(..)),
            Self::ModEnabled         => matches!(event, ModManagerEvent::ModEnabled(..)),
            Self::ModDisabled        => matches!(event, ModManagerEvent::ModDisabled(..)),
            Self::ProfileChanged     => matches!(event, ModManagerEvent::ProfileChanged { .. }),
            Self::OverlayMounted     => matches!(event, ModManagerEvent::OverlayMounted { .. }),
            Self::OverlayUnmounted   => matches!(event, ModManagerEvent::OverlayUnmounted { .. }),
            Self::DownloadStarted    => matches!(event, ModManagerEvent::DownloadStarted { .. }),
            Self::DownloadCompleted  => matches!(event, ModManagerEvent::DownloadCompleted { .. }),
            Self::ConflictMapUpdated => matches!(event, ModManagerEvent::ConflictMapUpdated { .. }),
        }
    }
}

// ─── EventBus ─────────────────────────────────────────────────────────────────

/// Internal subscriber record.
struct Subscriber {
    filter:  EventFilter,
    handler: Arc<dyn Fn(&ModManagerEvent) + Send + Sync>,
}

impl std::fmt::Debug for Subscriber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscriber")
            .field("filter", &self.filter)
            .field("handler", &"<Fn>")
            .finish()
    }
}

/// Thread-safe publish-subscribe event broadcaster.
///
/// Held behind `Arc<EventBus>` — shared by core (for publishing) and every
/// plugin's [`super::context::PluginContext`] (for subscribing).
///
/// # Panic safety
/// Publishing collects matching handler `Arc`s under the lock, releases the
/// lock, then invokes handlers. Panics inside a handler are caught by
/// [`std::panic::catch_unwind`] and logged; they never propagate to the
/// publisher or prevent other handlers from running.
#[derive(Debug, Default)]
pub struct EventBus {
    next_id:     AtomicU64,
    subscribers: Mutex<HashMap<u64, Subscriber>>,
}

impl EventBus {
    /// Create a new, empty event bus.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish `event` to all subscribers whose filter matches.
    ///
    /// Collects matching handler `Arc`s under the lock, releases the lock, then
    /// invokes handlers — preventing deadlock if a handler calls `subscribe` or
    /// `unsubscribe` during delivery.
    ///
    /// # Parameters
    /// - `event`: The event to broadcast.
    ///
    /// # Panics
    /// Panics only if the internal subscriber `Mutex` is poisoned (i.e. a
    /// previous thread panicked while holding the lock — which cannot happen
    /// under normal operation because handler panics are caught before they
    /// reach the lock guard).
    ///
    /// # Side effects
    /// Calls every matching handler synchronously on the calling thread.
    pub fn publish(&self, event: &ModManagerEvent) {
        // Collect matching handler Arcs while holding the lock, then release
        // before invoking — avoids deadlock if a handler subscribes/unsubscribes.
        let handlers: Vec<HandlerFn> = {
            let subs = self
                .subscribers
                .lock()
                .expect("EventBus: subscriber lock poisoned");
            subs.values()
                .filter(|s| s.filter.matches(event))
                .map(|s| Arc::clone(&s.handler))
                .collect()
        };

        for handler in &handlers {
            // SAFETY: AssertUnwindSafe is used because `Fn` is not `UnwindSafe`
            // by default.  We only assert it is safe to catch the unwind, not
            // that the handler leaves global state consistent.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                handler(event);
            }));
            if let Err(e) = result {
                tracing::warn!("EventBus: handler panicked — {:?}", e);
            }
        }
    }

    /// Subscribe to events matching `filter`.
    ///
    /// Returns a [`SubscriptionHandle`] that unsubscribes on drop.
    ///
    /// # Parameters
    /// - `filter`: Which event variants to receive.
    /// - `handler`: Called synchronously on the publishing thread. Must not
    ///   block. Spawn a `tokio::task` inside the handler for async work.
    ///
    /// # Panics
    /// Panics if the internal subscriber `Mutex` is poisoned.
    ///
    /// # Returns
    /// A [`SubscriptionHandle`] whose `Drop` impl calls [`Self::unsubscribe`].
    pub fn subscribe<F>(
        self: &Arc<Self>,
        filter: EventFilter,
        handler: F,
    ) -> SubscriptionHandle
    where
        F: Fn(&ModManagerEvent) + Send + Sync + 'static,
    {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.subscribers
            .lock()
            .expect("EventBus: subscriber lock poisoned")
            .insert(id, Subscriber { filter, handler: Arc::new(handler) });
        SubscriptionHandle { id, bus: Arc::downgrade(self) }
    }

    /// Remove a subscription by ID.
    ///
    /// Called automatically by [`SubscriptionHandle::drop`]. Safe to call with
    /// an already-removed ID (silently ignored).
    pub fn unsubscribe(&self, id: u64) {
        if let Ok(mut subs) = self.subscribers.lock() {
            subs.remove(&id);
        }
    }

    /// Return the number of active subscribers.
    ///
    /// Primarily for tests and diagnostics.
    ///
    /// # Panics
    /// Panics if the internal subscriber `Mutex` is poisoned.
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.subscribers
            .lock()
            .expect("EventBus: subscriber lock poisoned")
            .len()
    }
}

// ─── SubscriptionHandle ───────────────────────────────────────────────────────

/// An active event subscription.
///
/// Returned by [`EventBus::subscribe`] and
/// [`super::context::PluginContext::subscribe`].
///
/// **Dropping this value unsubscribes from the event bus.** Store it for the
/// duration of the plugin's lifetime; assign `None` in `shutdown()`.
///
/// # Example
/// ```ignore
/// fn init(&mut self, ctx: Arc<PluginContext>) -> Result<(), PluginError> {
///     self.handle = Some(ctx.subscribe(EventFilter::ModInstalled, |e| {
///         tracing::info!("Mod installed: {:?}", e);
///     }));
///     Ok(())
/// }
///
/// fn shutdown(&mut self) {
///     self.handle = None; // unsubscribes
/// }
/// ```
pub struct SubscriptionHandle {
    id:  u64,
    bus: Weak<EventBus>,
}

impl Drop for SubscriptionHandle {
    /// Unsubscribes from the event bus when the handle is dropped.
    ///
    /// If the `EventBus` has already been dropped (e.g. during application
    /// shutdown), the `Weak` upgrade fails silently.
    fn drop(&mut self) {
        if let Some(bus) = self.bus.upgrade() {
            bus.unsubscribe(self.id);
        }
    }
}

impl std::fmt::Debug for SubscriptionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubscriptionHandle")
            .field("id", &self.id)
            .field("bus_alive", &self.bus.upgrade().is_some())
            .finish()
    }
}

// ─── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use crate::game::GameKind;

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn bus() -> Arc<EventBus> {
        Arc::new(EventBus::new())
    }

    fn profile_event() -> ModManagerEvent {
        ModManagerEvent::ProfileChanged { old: "a".into(), new: "b".into() }
    }

    fn mod_info() -> ModInfo {
        ModInfo {
            id: 1,
            slug: "my-mod".into(),
            name: "My Mod".into(),
            version: "1.0".into(),
            author: "Author".into(),
            priority: 1,
            is_enabled: true,
            install_dir: "/mods/my-mod".into(),
        }
    }

    fn game_info() -> GameInfo {
        GameInfo {
            slug: "skyrim_se".into(),
            name: "Skyrim SE".into(),
            kind: GameKind::SkyrimSE,
            steam_app_id: 489830,
            install_path: "/game".into(),
            data_path: "/game/Data".into(),
            proton_prefix: None,
        }
    }

    // ── EventFilter::matches ──────────────────────────────────────────────────

    #[test]
    fn filter_all_matches_every_variant() {
        let events = [
            ModManagerEvent::ModEnabled(mod_info()),
            ModManagerEvent::ProfileChanged { old: "x".into(), new: "y".into() },
            ModManagerEvent::ConflictMapUpdated { affected_mods: vec![], total_conflicts: 0 },
            ModManagerEvent::GameLaunching(game_info()),
        ];
        for ev in &events {
            assert!(EventFilter::All.matches(ev), "All should match {ev:?}");
        }
    }

    #[test]
    fn filter_specific_matches_only_own_variant() {
        let ev_mod     = ModManagerEvent::ModEnabled(mod_info());
        let ev_profile = profile_event();

        assert!(EventFilter::ModEnabled.matches(&ev_mod));
        assert!(!EventFilter::ModEnabled.matches(&ev_profile));
        assert!(EventFilter::ProfileChanged.matches(&ev_profile));
        assert!(!EventFilter::ProfileChanged.matches(&ev_mod));
    }

    #[test]
    fn filter_mod_installed_vs_enabled_are_distinct() {
        let installed = ModManagerEvent::ModInstalled(mod_info());
        let enabled   = ModManagerEvent::ModEnabled(mod_info());
        assert!(EventFilter::ModInstalled.matches(&installed));
        assert!(!EventFilter::ModInstalled.matches(&enabled));
    }

    // ── EventBus ─────────────────────────────────────────────────────────────

    #[test]
    fn subscribe_fires_handler_on_matching_event() {
        let bus   = bus();
        let count = Arc::new(AtomicUsize::new(0));
        let c     = Arc::clone(&count);
        let _h    = bus.subscribe(EventFilter::ProfileChanged, move |_| {
            c.fetch_add(1, Ordering::Relaxed);
        });

        bus.publish(&profile_event());
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn subscribe_does_not_fire_on_non_matching_event() {
        let bus   = bus();
        let count = Arc::new(AtomicUsize::new(0));
        let c     = Arc::clone(&count);
        let _h    = bus.subscribe(EventFilter::GameLaunching, move |_| {
            c.fetch_add(1, Ordering::Relaxed);
        });

        bus.publish(&profile_event()); // not GameLaunching
        assert_eq!(count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn dropping_handle_unsubscribes() {
        let bus   = bus();
        let count = Arc::new(AtomicUsize::new(0));
        let c     = Arc::clone(&count);

        let handle = bus.subscribe(EventFilter::All, move |_| {
            c.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(bus.subscriber_count(), 1);

        drop(handle);
        assert_eq!(bus.subscriber_count(), 0);
        bus.publish(&profile_event());
        assert_eq!(count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn multiple_subscribers_all_fire() {
        let bus   = bus();
        let count = Arc::new(AtomicUsize::new(0));
        let mut handles = vec![];
        for _ in 0..3 {
            let c = Arc::clone(&count);
            handles.push(bus.subscribe(EventFilter::All, move |_| {
                c.fetch_add(1, Ordering::Relaxed);
            }));
        }
        bus.publish(&profile_event());
        assert_eq!(count.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn panicking_handler_does_not_propagate() {
        let bus = bus();
        let _h  = bus.subscribe(EventFilter::All, |_| panic!("intentional handler panic"));
        // Must not propagate to the test thread.
        bus.publish(&profile_event());
    }

    #[test]
    fn handler_may_acquire_subscriber_count_without_deadlock() {
        // Regression: lock is released before calling handlers, so checking
        // subscriber_count() inside a handler must not deadlock.
        let bus   = bus();
        let fired = Arc::new(AtomicUsize::new(0));
        let f     = Arc::clone(&fired);
        let _h    = bus.subscribe(EventFilter::ProfileChanged, move |_| {
            f.fetch_add(1, Ordering::Relaxed);
        });
        bus.publish(&profile_event());
        assert_eq!(fired.load(Ordering::Relaxed), 1);
    }
}
