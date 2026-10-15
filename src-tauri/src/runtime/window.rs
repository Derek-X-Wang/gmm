//! Restoring and remembering the main window's geometry (#369).
//!
//! The decision this feature turns on — whether saved geometry is safe to
//! use on the displays connected right now — is not here. It lives in
//! [`crate::core::window_state`], which is pure and tested. This module
//! is the Tauri shell around it: read the row, ask the displays, apply
//! the plan, show the window, and from then on remember where it goes.
//!
//! Two things in here are load-bearing and easy to break:
//!
//! 1. **The window is declared `visible: false`** in `tauri.conf.json`,
//!    so restoring cannot be seen as a jump from the default size to the
//!    remembered one. [`restore_and_track`] must therefore always reach
//!    its `show()`, whatever else fails. Nothing in it returns early
//!    after the window has been found, and nothing in it can panic on a
//!    missing display, an unreadable row or a refused placement.
//! 2. **Nothing is written on close.** Geometry is written a debounce
//!    after the last move or resize, by a thread that close never waits
//!    on. See [`PendingWindowState`].

use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::time::Duration;

use tauri::{
    AppHandle, LogicalSize, Manager, PhysicalPosition, Runtime, WebviewWindow, WindowEvent,
};

use crate::core::window_state::{
    self, DisplayRect, PendingWindowState, RestorePlan, SavedWindowState,
};
use crate::core::Core;

/// The label of the window declared in `tauri.conf.json`.
pub const MAIN_WINDOW_LABEL: &str = "main";

/// How long after the last move or resize the geometry is written.
///
/// Long enough that a drag across two monitors is one write; short
/// enough that the state is durable well before a user can finish
/// dragging and reach the close button.
const WRITE_DEBOUNCE: Duration = Duration::from_millis(500);

/// How long startup will wait for the remembered row before giving up
/// and opening at the default. The startup reconcile pass is hitting the
/// same SQLite file; a slow or locked read must delay the window by a
/// bounded amount, never hold it closed.
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Restore the main window's geometry, then show it and start
/// remembering where it goes.
///
/// Best-effort throughout. Every failure path ends with a window the
/// user can see at the configured default, and with nothing said to
/// them about it (#369).
pub fn restore_and_track<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        // Nothing to restore and nothing to show. Logged because it
        // would mean the window declaration was renamed.
        tracing::error!(
            target: "gmm::window",
            label = MAIN_WINDOW_LABEL,
            "no main window to restore geometry onto",
        );
        return;
    };

    let plan = app
        .try_state::<Core>()
        .map(|core| read_plan(&core, &displays(&window)))
        .unwrap_or(None);

    match plan {
        Some(plan) => apply(&window, plan),
        None => tracing::info!(
            target: "gmm::window",
            "opening at the default size: no usable remembered geometry",
        ),
    }

    // The window is declared `visible: false` so the restore above is
    // not visible as a jump. This must happen on every path.
    if let Err(e) = window.show() {
        tracing::error!(target: "gmm::window", error = %e, "could not show the main window");
    }

    if let Some(core) = app.try_state::<Core>() {
        track(app.clone(), Core::clone(&core));
    }
}

/// The displays connected right now, as physical rectangles.
///
/// An unavailable monitor list yields an empty slice, which
/// [`window_state::plan_restore`] treats as unreachable — the safe
/// reading, since a restore judged against no displays at all is a
/// restore judged against nothing.
fn displays<R: Runtime>(window: &WebviewWindow<R>) -> Vec<DisplayRect> {
    let monitors = match window.available_monitors() {
        Ok(monitors) => monitors,
        Err(e) => {
            tracing::warn!(
                target: "gmm::window",
                error = %e,
                "could not enumerate displays; opening at the default size",
            );
            return Vec::new();
        }
    };
    monitors
        .iter()
        .map(|monitor| {
            let position = monitor.position();
            let size = monitor.size();
            DisplayRect {
                x: position.x,
                y: position.y,
                width: size.width,
                height: size.height,
            }
        })
        .collect()
}

/// Read the remembered row and decide what to restore.
///
/// Runs on its own short-lived runtime rather than Tauri's: the SQLite
/// pool was built on the startup runtime, and a current-thread runtime
/// per blocking read is the pattern the rest of GMM's startup already
/// uses. Bounded by [`READ_TIMEOUT`] so a locked database delays the
/// window instead of withholding it.
fn read_plan(core: &Core, displays: &[DisplayRect]) -> Option<RestorePlan> {
    let core = core.clone();
    let raw = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        runtime.block_on(async move {
            match tokio::time::timeout(READ_TIMEOUT, core.remembered_window_state()).await {
                Ok(Ok(raw)) => raw,
                Ok(Err(e)) => {
                    tracing::warn!(
                        target: "gmm::window",
                        error = %e,
                        "could not read remembered window geometry",
                    );
                    None
                }
                Err(_) => {
                    tracing::warn!(
                        target: "gmm::window",
                        "timed out reading remembered window geometry",
                    );
                    None
                }
            }
        })
    })
    .join()
    .unwrap_or(None)?;

    window_state::plan_restore(Some(&raw), displays)
}

/// Apply a plan. Each step is independent: a refused size must not stop
/// the position from being applied, or the window could end up
/// remembered-size at a default position.
fn apply<R: Runtime>(window: &WebviewWindow<R>, plan: RestorePlan) {
    if let Err(e) = window.set_size(LogicalSize::new(plan.width, plan.height)) {
        tracing::warn!(target: "gmm::window", error = %e, "could not restore window size");
    }
    if let Err(e) = window.set_position(PhysicalPosition::new(plan.x, plan.y)) {
        tracing::warn!(target: "gmm::window", error = %e, "could not restore window position");
    }
    if plan.maximized {
        if let Err(e) = window.maximize() {
            tracing::warn!(target: "gmm::window", error = %e, "could not restore maximised state");
        }
    }
}

/// Start remembering the window's geometry.
///
/// Move and resize events only touch memory and poke a channel; the
/// write happens on a dedicated thread a debounce later. Close is not
/// involved at any point.
fn track<R: Runtime>(app: AppHandle<R>, core: Core) {
    let pending = Arc::new(PendingWindowState::default());
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return;
    };

    let flush = spawn_writer(core, pending.clone());

    // The handler holds an `AppHandle`, not the window: a closure
    // installed on a window that also owns that window keeps it alive
    // forever.
    window.on_window_event(move |event| {
        if !matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_)) {
            return;
        }
        let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
            return;
        };
        let Some(state) = capture(&window) else {
            return;
        };
        if pending.record(state) {
            // A closed channel means the writer thread is gone, which
            // only happens at shutdown. Nothing to report.
            let _ = flush.send(());
        }
    });
}

/// The writer thread: wake, wait out the debounce, write the newest
/// geometry.
///
/// Deliberately not a Tauri async task. It owns its runtime, it is
/// detached, and no part of the application — least of all window close
/// — has a handle to wait on it with.
fn spawn_writer(core: Core, pending: Arc<PendingWindowState>) -> Sender<()> {
    let (tx, rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(e) => {
                tracing::warn!(
                    target: "gmm::window",
                    error = %e,
                    "could not start the window-geometry writer; geometry will not be remembered",
                );
                return;
            }
        };
        // `recv` ends only when every sender is dropped, i.e. at shutdown.
        while rx.recv().is_ok() {
            std::thread::sleep(WRITE_DEBOUNCE);
            let Some(state) = pending.take() else {
                continue;
            };
            if let Err(e) = runtime.block_on(core.remember_window_state(&state)) {
                tracing::warn!(
                    target: "gmm::window",
                    error = %e,
                    "could not remember window geometry",
                );
            }
        }
    });
    tx
}

/// Read the window's current geometry.
///
/// A minimised window is skipped: Windows reports it at -32000,-32000,
/// and remembering that is exactly the stranded window this feature
/// exists to avoid. [`SavedWindowState::is_plausible`] would catch it
/// anyway; this just keeps the obvious case obvious.
fn capture<R: Runtime>(window: &WebviewWindow<R>) -> Option<SavedWindowState> {
    if window.is_minimized().unwrap_or(false) {
        return None;
    }
    let position = window.outer_position().ok()?;
    let size = window.outer_size().ok()?;
    Some(SavedWindowState {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        scale: window.scale_factor().ok()?,
        maximized: window.is_maximized().unwrap_or(false),
    })
}
