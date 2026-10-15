//! Remembered main-window geometry (#369).
//!
//! GMM reopens where and how the user left it. The numbers themselves are
//! the easy half; the hard half is **refusing** to use them. A saved
//! position is a promise about a display arrangement that may no longer
//! exist — a monitor unplugged, a laptop undocked, the scaling or the
//! resolution changed. A window restored off-screen cannot be dragged
//! back, so the application looks broken through no fault of its own.
//!
//! Everything in this module is pure: it takes the stored string and a
//! list of display rectangles and returns a plan, or [`None`] meaning
//! "open at the default and say nothing". That keeps the decision that
//! protects the user testable on a host with one display, while the Tauri
//! plumbing in `crate::runtime::window` stays a thin shell over it.
//!
//! Absent state and rejected state take the same path by construction:
//! both are [`None`], so a first run and an unusable arrangement are
//! indistinguishable from the outside. That is deliberate — the user is
//! told nothing either way (#369).

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// The window's default logical size. Mirrors `app.windows[0]` in
/// `tauri.conf.json`, which is what the window actually opens at when
/// nothing is restored; `window_state.rs`'s test module pins the two
/// together so they cannot drift.
pub const DEFAULT_WIDTH: f64 = 1100.0;
/// See [`DEFAULT_WIDTH`].
pub const DEFAULT_HEIGHT: f64 = 760.0;

/// The window's minimum logical size (#351). A restored size is clamped
/// up to it, never below: below 900x600 the Settings dialog fills the
/// window and stops reading as a dialog.
pub const MIN_WIDTH: f64 = 900.0;
/// See [`MIN_WIDTH`].
pub const MIN_HEIGHT: f64 = 600.0;

/// How tall the title-bar strip is taken to be, in logical pixels, for
/// the reachability rule below. Windows 11's own caption is 32 epx.
const TITLE_BAR_STRIP: f64 = 32.0;

/// The reachability rule, in logical pixels: how much of the title-bar
/// strip must land on a connected display for the window to count as
/// reachable.
///
/// 120 logical pixels of width is a grab area a user can actually hit
/// with a mouse; 20 of the strip's 32 logical pixels of height stops a
/// sliver peeking over a screen edge from counting. Both are scaled by
/// the saved scale factor before they are compared, so the rule means
/// the same thing at 100% and at 200%.
const REACHABLE_WIDTH: f64 = 120.0;
/// See [`REACHABLE_WIDTH`].
const REACHABLE_HEIGHT: f64 = 20.0;

/// Largest physical pixel extent treated as a real window. Anything
/// beyond this is a corrupt or sentinel value, not a window someone sat
/// in front of — Windows reports a minimised window at -32000,-32000,
/// for instance.
const MAX_PHYSICAL_EXTENT: i64 = 30_000;

/// What GMM persists about the main window.
///
/// Position and size are **physical** pixels, because that is the single
/// coordinate space Windows places windows in: each display has its own
/// scale factor, so there is no coherent logical space spanning them.
/// `scale` is the window's scale factor at the moment of the write, and
/// is what converts the physical size back to the logical size the
/// 900x600 minimum is expressed in.
///
/// The **position is the outer rect's** and the **size is the inner
/// rect's**, which is the pair the Tauri setters take: anything else and
/// the window drifts by a decoration's worth every session. The strip
/// the reachability rule measures therefore starts at the top of the
/// decorations, where the title bar actually is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SavedWindowState {
    /// Physical x of the window's outer rect.
    pub x: i32,
    /// Physical y of the window's outer rect.
    pub y: i32,
    /// Physical width of the window's inner rect.
    pub width: u32,
    /// Physical height of the window's inner rect.
    pub height: u32,
    /// The window's scale factor when this was captured.
    pub scale: f64,
    /// Whether the window was maximised. A maximised window reopens
    /// maximised (#369).
    pub maximized: bool,
}

impl SavedWindowState {
    /// Reject values no live window ever had. A minimised Windows window
    /// reports -32000,-32000; a zero extent, a non-finite scale or an
    /// absurd extent means the row was hand-edited or truncated.
    ///
    /// This runs on the write side as well as the read side, so a
    /// sentinel never reaches the database in the first place.
    pub fn is_plausible(&self) -> bool {
        if !self.scale.is_finite() || self.scale <= 0.0 {
            return false;
        }
        if self.width == 0 || self.height == 0 {
            return false;
        }
        let far = |v: i64| v.abs() > MAX_PHYSICAL_EXTENT;
        if far(self.width as i64) || far(self.height as i64) {
            return false;
        }
        // The rect must sit inside a plausible virtual desktop. Two
        // displays stacked either side of the primary is the widest
        // realistic arrangement, so one `MAX_PHYSICAL_EXTENT` of slack
        // in each direction is generous rather than tight.
        !far(self.x as i64) && !far(self.y as i64)
    }
}

/// A connected display's **physical** rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayRect {
    /// Physical x of the display's top-left corner.
    pub x: i32,
    /// Physical y of the display's top-left corner.
    pub y: i32,
    /// Physical width.
    pub width: u32,
    /// Physical height.
    pub height: u32,
}

/// What the Tauri layer should apply before showing the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RestorePlan {
    /// Logical width to apply.
    pub width: f64,
    /// Logical height to apply.
    pub height: f64,
    /// Physical x to apply.
    pub x: i32,
    /// Physical y to apply.
    pub y: i32,
    /// Whether to maximise after placing the window.
    pub maximized: bool,
}

/// Parse a stored value, rejecting anything implausible.
///
/// Corrupt input is not an error to report; it is simply absent state.
pub fn parse(raw: &str) -> Option<SavedWindowState> {
    let state: SavedWindowState = serde_json::from_str(raw).ok()?;
    state.is_plausible().then_some(state)
}

/// Serialise for the `settings` row.
pub fn serialize(state: &SavedWindowState) -> Option<String> {
    state
        .is_plausible()
        .then(|| serde_json::to_string(state).ok())
        .flatten()
}

/// Decide what to restore, given the stored value and the displays that
/// are connected **now**.
///
/// Returns [`None`] when nothing should be restored — absent state,
/// unreadable state, or geometry that would strand the window. The
/// caller then leaves the window at its configured default and says
/// nothing to the user.
pub fn plan_restore(raw: Option<&str>, displays: &[DisplayRect]) -> Option<RestorePlan> {
    let saved = parse(raw?)?;

    // Clamp the restored size up to the minimum (#351). The clamp runs
    // before the reachability check on purpose: the check must judge the
    // rect that will actually exist, not the one that was stored.
    let logical_width = (saved.width as f64 / saved.scale).max(MIN_WIDTH);
    let logical_height = (saved.height as f64 / saved.scale).max(MIN_HEIGHT);
    let physical_width = (logical_width * saved.scale).round().max(1.0) as u32;
    let physical_height = (logical_height * saved.scale).round().max(1.0) as u32;

    if !is_reachable(
        saved.x,
        saved.y,
        physical_width,
        physical_height,
        saved.scale,
        displays,
    ) {
        return None;
    }

    // A maximised window's saved size *is* its maximised rect, so
    // applying it would make Restore Down a no-op: the window would
    // un-maximise to the same pixels it already covers. The pre-maximise
    // size is deliberately not remembered (#369), so the default is what
    // Restore Down returns to.
    let (width, height) = if saved.maximized {
        (DEFAULT_WIDTH, DEFAULT_HEIGHT)
    } else {
        (logical_width, logical_height)
    };

    Some(RestorePlan {
        width,
        height,
        x: saved.x,
        y: saved.y,
        maximized: saved.maximized,
    })
}

/// The reachability rule: **at least 120 x 20 logical pixels of the
/// window's title-bar strip — its top 32 logical pixels — must overlap
/// some connected display.**
///
/// That is the testable form of "the user can grab the window and drag
/// it somewhere else". It rejects the cases that strand a window: a
/// position on a monitor that is no longer connected, a position below
/// or to the right of a desktop that shrank, and a title bar pushed
/// above the top of every display — the one offset a user cannot
/// recover from, because the thing you drag with is the thing that is
/// gone.
///
/// An empty display list is unreachable, not vacuously reachable.
fn is_reachable(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale: f64,
    displays: &[DisplayRect],
) -> bool {
    let strip_height = (TITLE_BAR_STRIP * scale).round() as i64;
    let needed_width = (REACHABLE_WIDTH * scale).round() as i64;
    let needed_height = (REACHABLE_HEIGHT * scale).round() as i64;

    let left = x as i64;
    let right = left + width as i64;
    let top = y as i64;
    // A window shorter than the strip (impossible after the clamp, but
    // the rule should not depend on that) contributes only its height.
    let bottom = top + strip_height.min(height as i64);

    displays.iter().any(|display| {
        let d_left = display.x as i64;
        let d_right = d_left + display.width as i64;
        let d_top = display.y as i64;
        let d_bottom = d_top + display.height as i64;

        let overlap_width = right.min(d_right) - left.max(d_left);
        let overlap_height = bottom.min(d_bottom) - top.max(d_top);

        overlap_width >= needed_width && overlap_height >= needed_height
    })
}

/// The newest geometry seen, waiting to be written.
///
/// **Write trigger**: geometry is written a short debounce after the
/// last move or resize, never on close. Close therefore has nothing to
/// flush and cannot be delayed by a write — the guarantee is structural,
/// not a promise about how fast a write is. The cost is that a process
/// killed within the debounce window loses its last move; by the time a
/// user has reached for the close button, the state is already durable.
///
/// [`record`](Self::record) reports whether the caller must schedule a
/// flush. While one is scheduled, further moves only replace the pending
/// value, so dragging a window across a desk produces one write rather
/// than hundreds.
#[derive(Debug, Default)]
pub struct PendingWindowState {
    inner: Mutex<Pending>,
}

#[derive(Debug, Default)]
struct Pending {
    latest: Option<SavedWindowState>,
    flush_scheduled: bool,
}

impl PendingWindowState {
    /// Record the newest geometry. Returns `true` when the caller must
    /// schedule a flush; `false` means one is already scheduled and will
    /// pick this value up instead.
    ///
    /// Implausible geometry is dropped here, so a minimised window's
    /// sentinel position never reaches the database.
    pub fn record(&self, state: SavedWindowState) -> bool {
        if !state.is_plausible() {
            return false;
        }
        let mut pending = self.lock();
        pending.latest = Some(state);
        if pending.flush_scheduled {
            false
        } else {
            pending.flush_scheduled = true;
            true
        }
    }

    /// Take the geometry to write and clear the scheduled flag, so the
    /// next move schedules a fresh flush.
    pub fn take(&self) -> Option<SavedWindowState> {
        let mut pending = self.lock();
        pending.flush_scheduled = false;
        pending.latest.take()
    }

    /// A poisoned lock here is not worth failing a window move over: the
    /// data is one geometry struct, and the worst case is one write of a
    /// slightly stale rect.
    fn lock(&self) -> std::sync::MutexGuard<'_, Pending> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}
