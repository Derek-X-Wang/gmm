//! Restoring and remembering the main window's geometry (#369).
//!
//! The decision this feature turns on — whether saved geometry is safe to
//! use on the displays connected right now — is not here. It lives in
//! [`crate::core::window_state`], which is pure and tested. This module
//! is the Tauri shell around it: read the row, ask the displays, apply
//! the plan, show the window, and from then on remember where it goes.
//!
//! Three things in here are load-bearing and easy to break:
//!
//! 1. **The window is declared `visible: false`** in `tauri.conf.json`,
//!    so restoring cannot be seen as a jump from the default size to the
//!    remembered one. [`restore_and_track`] must therefore always reach
//!    its `show()`, whatever else fails. Nothing in it returns early
//!    after the window has been found, and nothing in it can panic on a
//!    missing display, an unreadable row or a refused placement.
//! 2. **[`apply`] converts nothing.** The plan is physical pixels and it
//!    goes out as `PhysicalSize` and `PhysicalPosition`, so the rectangle
//!    the window gets is the rectangle `is_reachable` accepted. A
//!    `LogicalSize` here would be converted by the scale factor of
//!    whatever display the window is on at that instant, which is how a
//!    checked rectangle and an applied rectangle come apart.
//! 3. **Nothing is written on close.** Geometry is written once the
//!    window has been still for [`WRITE_DEBOUNCE`], by a thread that
//!    close never waits on.

use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::time::Duration;

use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, Runtime, WebviewWindow, WindowEvent,
};

use crate::core::window_state::{
    self, DisplayRect, PendingWindowState, RestorePlan, SavedWindowState,
};
use crate::core::Core;

/// The label of the window declared in `tauri.conf.json`.
pub const MAIN_WINDOW_LABEL: &str = "main";

/// How long the window must be still before its geometry is written.
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

    // The row is read first and the displays are enumerated after, so
    // the arrangement the plan is judged against is as fresh as it can
    // be. Enumerating first would leave the list stale by however long
    // the database took — up to `READ_TIMEOUT` — and a display unplugged
    // inside that window would be a display the check still trusted.
    // The race cannot be closed (a monitor can go between the
    // enumeration and the placement, microseconds later) but it is now
    // as small as this layer can make it.
    let plan = app
        .try_state::<Core>()
        .and_then(|core| read_saved(&core))
        .and_then(|raw| window_state::plan_restore(Some(&raw), &displays(&window)));

    match plan {
        Some(plan) => apply(&window, plan),
        None => tracing::info!(
            target: "gmm::window",
            "opening at the default size: no usable remembered geometry",
        ),
    }

    // The window is declared `visible: false` so the restore above is
    // not visible as a jump. This must happen on every path.
    //
    // A failure here is only logged: there is no second way to show a
    // window, so there is nothing to recover to. It would leave GMM
    // running with nothing on screen, which is why `show()` being
    // reachable is held by a test rather than by review.
    if let Err(e) = window.show() {
        tracing::error!(target: "gmm::window", error = %e, "could not show the main window");
    }

    if let Some(core) = app.try_state::<Core>() {
        track(app.clone(), Core::clone(&core));
    }
}

/// The displays connected right now: physical rectangles and each one's
/// own scale factor.
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
                scale: monitor.scale_factor(),
            }
        })
        .collect()
}

/// Read the remembered row.
///
/// Runs on its own short-lived runtime rather than Tauri's: the SQLite
/// pool was built on the startup runtime, and a current-thread runtime
/// per blocking read is the pattern the rest of GMM's startup already
/// uses. Bounded by [`READ_TIMEOUT`] so a locked database delays the
/// window instead of withholding it.
fn read_saved(core: &Core) -> Option<String> {
    let core = core.clone();
    std::thread::spawn(move || {
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
    // A panicking reader thread must not take the window with it.
    .unwrap_or(None)
}

/// Apply a plan, in physical pixels, converting nothing.
///
/// Position first, then size: the window is on its target display before
/// the size lands, so Tauri's minimum-size enforcement — which is in
/// *logical* pixels, and so depends on that display's scale factor —
/// happens where it was predicted to. That enforcement is the one thing
/// that can still change the applied rectangle, and it can only **grow**
/// it, anchored at the top-left corner this function just set. Growing a
/// rectangle from its top-left can only increase the overlap between its
/// title-bar strip and any display, so a plan that passed
/// [`window_state::is_reachable`] is still reachable afterwards.
///
/// Each step is independent: a refused size must not stop the position
/// from being applied, or the window could end up remembered-size at a
/// default position.
fn apply<R: Runtime>(window: &WebviewWindow<R>, plan: RestorePlan) {
    if let Err(e) = window.set_position(PhysicalPosition::new(plan.x, plan.y)) {
        tracing::warn!(target: "gmm::window", error = %e, "could not restore window position");
    }
    if let Err(e) = window.set_size(PhysicalSize::new(plan.width, plan.height)) {
        tracing::warn!(target: "gmm::window", error = %e, "could not restore window size");
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
/// write happens on a dedicated thread once the window has been still.
/// Close is not involved at any point.
fn track<R: Runtime>(app: AppHandle<R>, core: Core) {
    let pending = Arc::new(PendingWindowState::default());
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return;
    };

    let moved = spawn_writer(core, pending.clone());

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
            // Every accepted move notifies, because each notification is
            // what resets the debounce deadline. A closed channel means
            // the writer thread is gone, which only happens at shutdown.
            let _ = moved.send(());
        }
    });
}

/// The writer thread: wake on a move, wait for the window to be still,
/// write the newest geometry.
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
        // The outer `recv` ends only when every sender is dropped, i.e.
        // at shutdown.
        while rx.recv().is_ok() {
            // Debounce: every further move resets this deadline, so a
            // drag produces one write after it ends rather than one
            // every `WRITE_DEBOUNCE` while it lasts.
            let _ = window_state::wait_until_quiet(&rx, WRITE_DEBOUNCE);
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
/// **Inner size, outer position** — the pair that matches what [`apply`]
/// sets. Tauri's `set_size` takes an *inner* size and `set_position` an
/// *outer* position, so capturing `outer_size` here would write back a
/// size a decoration wider and taller than the one the next launch
/// applies as the inner size, and the window would grow a little every
/// session. The inner size is also what `minWidth`/`minHeight` constrain,
/// so it is the size the 900x600 clamp is about.
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
    let size = window.inner_size().ok()?;
    Some(SavedWindowState {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        scale: window.scale_factor().ok()?,
        maximized: window.is_maximized().unwrap_or(false),
    })
}
