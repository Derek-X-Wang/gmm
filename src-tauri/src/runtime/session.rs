//! Tauri-side GameSession runtime.
//!
//! Owns the live child-process handle + the in-process Loader/HookSession
//! for the lifetime of the session. The durable record lives in
//! `Core::session_info`; this struct is the volatile counterpart that
//! exists only while GMM is running.

use std::process::Child;
use std::sync::{Arc, Mutex};

use gmm_loader::Loader;

use crate::core::SessionInfo;

/// Names of the Tauri events emitted to the frontend when a session
/// starts or ends. The frontend listens for these to refetch the
/// current-session query and update the banner.
pub const SESSION_STARTED_EVENT: &str = "session-started";
pub const SESSION_ENDED_EVENT: &str = "session-ended";

/// Live, in-process state for the currently-running GameSession.
///
/// Held in Tauri State as `Arc<Mutex<Option<LiveSession>>>` — `None`
/// means no session is active. The session no longer holds a
/// `HookSession`: the launch flow calls `wait_for_injection` and drops
/// the hook immediately, so the CBT hook is alive only for the brief
/// injection window. We still hold the `Loader` so the loader DLL
/// stays mapped (drives no behaviour on its own, but cheap and tidy).
///
/// Dropping the `Child` handle does NOT kill the process; the watcher
/// task is the only place we observe clean exit, and the launch flow
/// uses a separate `ChildGuard` to kill on early-failure paths.
pub struct LiveSession {
    pub info: SessionInfo,
    pub child: Child,
    pub _loader: Loader,
}

/// Tauri-state-friendly handle. Newtype around the Arc<Mutex<...>> so
/// `tauri::State<'_, SessionRuntime>` is unambiguous in command
/// signatures and so we can grow methods later (event listeners,
/// watcher join handles) without touching every call site.
#[derive(Clone, Default)]
pub struct SessionRuntime {
    inner: Arc<Mutex<Option<LiveSession>>>,
}

impl SessionRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take the current LiveSession out of the slot, leaving `None`.
    /// Used by the exit watcher to clean up after the child exits.
    pub fn take(&self) -> Option<LiveSession> {
        self.inner.lock().expect("session lock poisoned").take()
    }

    /// Install a new LiveSession. Panics if a session is already
    /// installed — Core::ensure_no_active_session prevents this at the
    /// public API surface so the assertion is a "should never happen"
    /// safeguard, not a recoverable error.
    pub fn install(&self, live: LiveSession) {
        let mut guard = self.inner.lock().expect("session lock poisoned");
        assert!(
            guard.is_none(),
            "tried to install a session while one was already active — Core::start_session contract violated",
        );
        *guard = Some(live);
    }

    /// True if a session is currently installed. Used by the watcher
    /// task to bail early after the user manually cleared via
    /// `clean_stale_session` while the watcher was mid-poll.
    pub fn has_session(&self) -> bool {
        self.inner.lock().expect("session lock poisoned").is_some()
    }

    /// Poll the child's status without blocking. Returns `Ok(Some(_))`
    /// once the process has exited; `Ok(None)` while it's still
    /// running.
    pub fn try_wait_child(&self) -> std::io::Result<Option<std::process::ExitStatus>> {
        let mut guard = self.inner.lock().expect("session lock poisoned");
        match guard.as_mut() {
            Some(live) => live.child.try_wait(),
            None => Ok(None),
        }
    }

    /// Cheap clone of the underlying Arc<Mutex>. Used by background
    /// watchers that need their own reference but can't take the
    /// `tauri::State` wrapper across spawn boundaries.
    pub fn inner_clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}
