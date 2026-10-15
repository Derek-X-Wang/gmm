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
//! **One rectangle, one coordinate space.** [`RestorePlan`] is physical
//! pixels, and it is the *same four integers* [`is_reachable`] judged.
//! Nothing between the check and the window converts them, because a
//! conversion is where the two rectangles come apart: a `LogicalSize`
//! handed to Tauri is converted by the scale factor of whatever display
//! the window happens to be on at that moment, which is not necessarily
//! the scale factor the geometry was saved at. Checking one rectangle and
//! applying a different one is exactly the failure this module exists to
//! prevent.
//!
//! Absent state and rejected state take the same path by construction:
//! both are [`None`], so a first run and an unusable arrangement are
//! indistinguishable from the outside. That is deliberate — the user is
//! told nothing either way (#369).

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The window's default logical size. Mirrors `app.windows[0]` in
/// `tauri.conf.json`, which is what the window actually opens at when
/// nothing is restored; the `window_state` test target pins the two
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
/// sliver peeking over a screen edge from counting. Both are converted
/// to physical pixels using **the scale factor of the display being
/// tested**, because that display's scaling is what decides how large
/// the title bar and the grab area physically are once the window is
/// sitting on it.
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
///
/// `scale` is the window's scale factor at the moment of the write. It
/// has exactly one job: converting the physical size to the logical size
/// the 900x600 minimum is expressed in, so the clamp means the same thing
/// at 100% and at 200%. It is deliberately *not* used to place the
/// window or to size the reachability thresholds — the first needs no
/// conversion at all, and the second belongs to the display being tested.
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

/// A connected display: its **physical** rectangle and its own scale
/// factor.
///
/// The scale factor is per display, not per desktop. A docked laptop
/// commonly runs its built-in panel at 200% beside an external monitor at
/// 100%, and the reachability rule has to be measured against whichever
/// one the window would land on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayRect {
    /// Physical x of the display's top-left corner.
    pub x: i32,
    /// Physical y of the display's top-left corner.
    pub y: i32,
    /// Physical width.
    pub width: u32,
    /// Physical height.
    pub height: u32,
    /// This display's scale factor.
    pub scale: f64,
}

impl DisplayRect {
    /// A nonsense scale factor from the platform must not make GMM
    /// refuse to restore; treat it as 100%.
    fn usable_scale(&self) -> f64 {
        if self.scale.is_finite() && self.scale > 0.0 {
            self.scale
        } else {
            1.0
        }
    }

    fn contains(&self, x: i32, y: i32) -> bool {
        let inside = |v: i32, start: i32, extent: u32| {
            v >= start && (v as i64) < start as i64 + extent as i64
        };
        inside(x, self.x, self.width) && inside(y, self.y, self.height)
    }

    /// Physical area of this display's intersection with a rectangle.
    fn overlap_area(&self, x: i32, y: i32, width: u32, height: u32) -> i64 {
        let overlap = |start: i32, extent: u32, d_start: i32, d_extent: u32| {
            let end = start as i64 + extent as i64;
            let d_end = d_start as i64 + d_extent as i64;
            (end.min(d_end) - (start as i64).max(d_start as i64)).max(0)
        };
        overlap(x, width, self.x, self.width) * overlap(y, height, self.y, self.height)
    }
}

/// What the Tauri layer should apply before showing the window.
///
/// **Physical pixels, and the same rectangle [`is_reachable`] accepted.**
/// The caller applies it with `PhysicalSize` and `PhysicalPosition` and
/// performs no conversion of its own, so there is no second rectangle to
/// get wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestorePlan {
    /// Physical width to apply.
    pub width: u32,
    /// Physical height to apply.
    pub height: u32,
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
    // rect that will actually exist, not the one that was stored, and
    // clamping only ever grows the rect from its top-left anchor — so
    // checking the stored rect first could throw away a window that
    // reopens perfectly reachable.
    //
    // The physical size is left byte-identical to the stored one unless
    // the clamp actually bites, so the ordinary case round-trips exactly
    // rather than through a float.
    let logical_width = saved.width as f64 / saved.scale;
    let logical_height = saved.height as f64 / saved.scale;
    let physical_width = if logical_width < MIN_WIDTH {
        (MIN_WIDTH * saved.scale).round().max(1.0) as u32
    } else {
        saved.width
    };
    let physical_height = if logical_height < MIN_HEIGHT {
        (MIN_HEIGHT * saved.scale).round().max(1.0) as u32
    } else {
        saved.height
    };

    if !is_reachable(saved.x, saved.y, physical_width, physical_height, displays) {
        return None;
    }

    if !saved.maximized {
        return Some(RestorePlan {
            width: physical_width,
            height: physical_height,
            x: saved.x,
            y: saved.y,
            maximized: false,
        });
    }

    // A maximised window's saved size *is* its maximised rect, so
    // applying it would make Restore Down a no-op: the window would
    // un-maximise onto the same pixels it already covers. The
    // pre-maximise size is deliberately not remembered (#369), so the
    // default is what Restore Down returns to.
    //
    // That rect is then placed **wholly inside the display the window was
    // maximised on**, because Windows maximises onto the display holding
    // the largest part of the window: a default-sized rect left hanging
    // over a neighbour could be handed to the wrong screen. Containment
    // makes the choice unambiguous instead of probable, and it also makes
    // the maximised plan reachable by construction — every pixel of its
    // title bar is on a connected display.
    let home = home_display(&saved, physical_width, physical_height, displays)?;
    let width = ((DEFAULT_WIDTH * home.usable_scale()).round().max(1.0) as u32).min(home.width);
    let height = ((DEFAULT_HEIGHT * home.usable_scale()).round().max(1.0) as u32).min(home.height);
    let contained = RestorePlan {
        width,
        height,
        // `width <= home.width` and `height <= home.height`, so the
        // clamp ranges below are never inverted.
        x: saved
            .x
            .clamp(home.x, home.x + home.width as i32 - width as i32),
        y: saved
            .y
            .clamp(home.y, home.y + home.height as i32 - height as i32),
        maximized: true,
    };

    // This branch substitutes a rectangle the check above never saw, so
    // it is checked too — the postcondition is that *whatever* comes back
    // is reachable, not just the remembered rect. The only way a
    // contained rect fails is a display narrower than the grab area
    // itself, and opening at the default is the right answer for that.
    is_reachable(
        contained.x,
        contained.y,
        contained.width,
        contained.height,
        displays,
    )
    .then_some(contained)
}

/// The display a maximised window should be handed back to: the one
/// holding its remembered origin, or failing that the one its remembered
/// rect overlaps most.
///
/// Reachability has already passed when this is called, so some display
/// overlaps the rect and the fallback always finds one.
fn home_display<'a>(
    saved: &SavedWindowState,
    width: u32,
    height: u32,
    displays: &'a [DisplayRect],
) -> Option<&'a DisplayRect> {
    if let Some(display) = displays
        .iter()
        .find(|display| display.contains(saved.x, saved.y))
    {
        return Some(display);
    }
    displays
        .iter()
        .filter(|display| display.overlap_area(saved.x, saved.y, width, height) > 0)
        .max_by_key(|display| display.overlap_area(saved.x, saved.y, width, height))
}

/// The reachability rule: **at least 120 x 20 logical pixels of the
/// window's title-bar strip — its top 32 logical pixels — must overlap
/// some connected display**, measured in physical pixels against that
/// display's own scale factor.
///
/// That is the testable form of "the user can grab the window and drag
/// it somewhere else". It rejects the cases that strand a window: a
/// position on a monitor that is no longer connected, a position below
/// or to the right of a desktop that shrank, and a title bar pushed
/// above the top of every display — the one offset a user cannot
/// recover from, because the thing you drag with is the thing that is
/// gone.
///
/// Public because it is a property of a *rectangle*, not only of a
/// stored row: a test can assert it about the [`RestorePlan`] that comes
/// back, which is the guarantee that matters.
///
/// An empty display list is unreachable, not vacuously reachable.
pub fn is_reachable(x: i32, y: i32, width: u32, height: u32, displays: &[DisplayRect]) -> bool {
    displays.iter().any(|display| {
        let scale = display.usable_scale();
        let strip_height = (TITLE_BAR_STRIP * scale).round() as i64;
        let needed_width = (REACHABLE_WIDTH * scale).round() as i64;
        let needed_height = (REACHABLE_HEIGHT * scale).round() as i64;

        let left = x as i64;
        let right = left + width as i64;
        let top = y as i64;
        // A window shorter than the strip (impossible after the clamp,
        // but the rule should not depend on that) contributes only its
        // own height.
        let bottom = top + strip_height.min(height as i64);

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
/// Holds one snapshot, not a queue: a move that arrives before the
/// pending one has been written replaces it, because only the latest
/// rectangle is worth remembering.
#[derive(Debug, Default)]
pub struct PendingWindowState {
    latest: Mutex<Option<SavedWindowState>>,
}

impl PendingWindowState {
    /// Record the newest geometry, returning whether it was accepted.
    ///
    /// Implausible geometry is dropped here, so a minimised window's
    /// sentinel position never reaches the database and never wakes the
    /// writer.
    pub fn record(&self, state: SavedWindowState) -> bool {
        if !state.is_plausible() {
            return false;
        }
        *self.lock() = Some(state);
        true
    }

    /// Take the geometry to write.
    ///
    /// The snapshot is removed rather than kept: a write that then fails
    /// is logged and discarded, with no retry. The next move will record
    /// again, and the cost of losing one is reopening at a slightly older
    /// rectangle — so a scheduled write is not a durable one.
    pub fn take(&self) -> Option<SavedWindowState> {
        self.lock().take()
    }

    /// A poisoned lock here is not worth failing a window move over: the
    /// data is one geometry struct, and the worst case is one write of a
    /// slightly stale rect.
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<SavedWindowState>> {
        match self.latest.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// Why a debounce wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quiet {
    /// The geometry stopped changing for the whole quiet window.
    Settled,
    /// Every notifier went away — the window is gone.
    Disconnected,
}

/// Wait until `notifications` has been silent for `quiet`.
///
/// **Each notification resets the deadline.** That is what makes this a
/// debounce rather than a throttle, and it is the whole reason the write
/// trigger exists: dragging a window across a desk reports a move every
/// frame, and a throttle would write every `quiet` for the length of the
/// drag. One write, `quiet` after the drag ends, is the intent.
///
/// The trade-off is the other direction: a drag that never pauses defers
/// the write for as long as it lasts, and a process killed in the
/// meantime reopens at the rectangle before the drag. That is acceptable
/// because the alternative — writing during close — is what must never
/// happen; close has nothing to flush and so cannot be delayed by a
/// write at all.
pub fn wait_until_quiet(notifications: &Receiver<()>, quiet: Duration) -> Quiet {
    loop {
        match notifications.recv_timeout(quiet) {
            Ok(()) => continue,
            Err(RecvTimeoutError::Timeout) => return Quiet::Settled,
            Err(RecvTimeoutError::Disconnected) => return Quiet::Disconnected,
        }
    }
}
