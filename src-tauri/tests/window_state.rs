//! Remembered main-window geometry (#369).
//!
//! The round trip is the easy half. What these tests exist for is the
//! refusal: GMM must decline its own saved position when the displays it
//! was saved against are gone, because a window restored off-screen
//! cannot be dragged back and the application then looks broken through
//! no fault of its own.
//!
//! The reachability rule under test is **at least 120 x 20 logical
//! pixels of the window's title-bar strip — its top 32 logical pixels —
//! must overlap some connected display**, measured in physical pixels
//! against that display's own scale factor.
//!
//! The rule is only worth as much as the rectangle it is applied to, so
//! several tests below assert `is_reachable` about the `RestorePlan` that
//! comes back rather than about the row that went in. A check that
//! judges one rectangle while the window gets another is not a check;
//! that was the first version of this feature, and
//! [`the_plan_is_reachable_after_the_scale_factor_changes`] is the case
//! that exposed it.
//!
//! Whether Windows enumerates the displays GMM asks it for, and what it
//! reports across a scaling change, is Windows-only evidence; #326
//! collects it.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use gmm_lib::core::window_state::{
    self, DisplayRect, PendingWindowState, Quiet, RestorePlan, SavedWindowState, DEFAULT_HEIGHT,
    DEFAULT_WIDTH, MIN_HEIGHT, MIN_WIDTH,
};
use gmm_lib::core::Core;
use tempfile::TempDir;

async fn fresh_core(tmp: &TempDir) -> Core {
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    Core::new(library_root, &db_url).await.expect("init core")
}

/// A single 1920x1080 display at the origin, at 100% scaling. The
/// arrangement every one of these tests restores against unless it says
/// otherwise.
fn one_display() -> Vec<DisplayRect> {
    vec![display(0, 0, 1920, 1080, 1.0)]
}

fn display(x: i32, y: i32, width: u32, height: u32, scale: f64) -> DisplayRect {
    DisplayRect {
        x,
        y,
        width,
        height,
        scale,
    }
}

/// A window the user left at 1400x900, 200 pixels in from the corner of
/// that display.
fn settled_window() -> SavedWindowState {
    SavedWindowState {
        x: 200,
        y: 200,
        width: 1400,
        height: 900,
        scale: 1.0,
        maximized: false,
    }
}

/// Can a user grab this rectangle?
///
/// **A second implementation of the rule, written from its statement
/// rather than shared with production.** `window_state` exposes no
/// predicate for a test to borrow, deliberately: asserting a plan with
/// the same function that produced it only proves the function agrees
/// with itself, and a wrong predicate would pass.
///
/// The part this commits to independently is the part production got
/// wrong once: **one** display. A window has one effective DPI, taken
/// from the display it primarily occupies, so the title bar has one
/// height and the grab area one size. Both are resolved here from the
/// greatest-overlap display, computed below, and nothing else is allowed
/// to vouch for the window.
fn grabbable(plan: RestorePlan, displays: &[DisplayRect]) -> bool {
    let area = |display: &DisplayRect| -> i64 {
        let span = |start: i32, extent: u32, d_start: i32, d_extent: u32| {
            ((start as i64 + extent as i64).min(d_start as i64 + d_extent as i64)
                - (start as i64).max(d_start as i64))
            .max(0)
        };
        span(plan.x, plan.width, display.x, display.width)
            * span(plan.y, plan.height, display.y, display.height)
    };
    let Some(target) = displays
        .iter()
        .filter(|display| area(display) > 0)
        .max_by(|a, b| area(a).cmp(&area(b)))
    else {
        return false;
    };

    let scale = target.scale;
    let strip = (32.0 * scale) as i64;
    let on_screen = |start: i32, extent: i64, d_start: i32, d_extent: u32| -> i64 {
        (start as i64 + extent).min(d_start as i64 + d_extent as i64)
            - (start as i64).max(d_start as i64)
    };
    on_screen(plan.x, plan.width as i64, target.x, target.width) >= (120.0 * scale) as i64
        && on_screen(
            plan.y,
            strip.min(plan.height as i64),
            target.y,
            target.height,
        ) >= (20.0 * scale) as i64
}

/// The property that matters: the rectangle GMM is about to apply is
/// one a user can grab on the displays it was judged against.
fn assert_plan_reachable(plan: RestorePlan, displays: &[DisplayRect], context: &str) {
    assert!(
        grabbable(plan, displays),
        "{context}: the rect GMM applies must be the rect it checked, but \
         {plan:?} cannot be grabbed on {displays:?}",
    );
}

// ---- the round trip ----

#[tokio::test]
async fn size_position_and_maximised_state_survive_a_close_and_reopen() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let left_maximised = SavedWindowState {
        // A second display to the left of the primary: negative x is the
        // ordinary Windows arrangement, not an error.
        x: -1920,
        y: 0,
        width: 1920,
        height: 1080,
        scale: 1.0,
        maximized: true,
    };
    core.remember_window_state(&left_maximised)
        .await
        .expect("remember");

    // Reopening reads the row back exactly, with the display still there.
    let raw = core
        .remembered_window_state()
        .await
        .expect("read")
        .expect("a row was written");
    assert_eq!(
        window_state::parse(&raw),
        Some(left_maximised),
        "every field must survive the round trip, maximised included",
    );

    let displays = vec![
        display(-1920, 0, 1920, 1080, 1.0),
        display(0, 0, 1920, 1080, 1.0),
    ];
    let plan = window_state::plan_restore(Some(&raw), &displays).expect("restored");
    assert_eq!(
        (plan.x, plan.y, plan.maximized),
        (-1920, 0, true),
        "a window closed maximised on the left display reopens maximised on the left display",
    );
    assert_plan_reachable(plan, &displays, "maximised on the left display");
}

#[tokio::test]
async fn a_remembered_size_and_position_come_back_as_a_plan_to_apply() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.remember_window_state(&settled_window())
        .await
        .expect("remember");
    let raw = core.remembered_window_state().await.expect("read");

    let plan = window_state::plan_restore(raw.as_deref(), &one_display()).expect("restored");
    assert_eq!(
        plan,
        RestorePlan {
            width: 1400,
            height: 900,
            x: 200,
            y: 200,
            maximized: false,
        },
        "the window reopens at the size and position it was left at",
    );
}

#[tokio::test]
async fn remembering_twice_keeps_the_newer_geometry() {
    // The write trigger coalesces moves, so the row is overwritten
    // repeatedly during one session. The last write must win.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.remember_window_state(&settled_window())
        .await
        .expect("first");
    let moved = SavedWindowState {
        x: 40,
        y: 60,
        ..settled_window()
    };
    core.remember_window_state(&moved).await.expect("second");

    let raw = core.remembered_window_state().await.expect("read");
    let plan = window_state::plan_restore(raw.as_deref(), &one_display()).expect("restored");
    assert_eq!((plan.x, plan.y), (40, 60));
}

// ---- one rectangle, one coordinate space ----

#[test]
fn the_plan_is_reachable_after_the_scale_factor_changes() {
    // The case that exposed a checked rectangle and an applied rectangle
    // coming apart. The user had GMM at 1100x750 logical on a 200%
    // display to the left of the primary — 2200x1500 physical. Then they
    // undocked, leaving one 1920x1080 display at 100%.
    //
    // About 300 physical pixels of that remembered rect reach the
    // remaining display, so the geometry is restorable. What must not
    // happen is GMM checking the 2200-pixel-wide rect and then applying
    // a 1100-pixel-wide one: its right edge would land near -800 and the
    // whole window would be off-screen, unreachable, with no way back —
    // the exact failure this feature exists to prevent.
    let undocked = vec![display(0, 0, 1920, 1080, 1.0)];
    let saved_at_200 = SavedWindowState {
        x: -1900,
        y: 100,
        width: 2200,
        height: 1500,
        scale: 2.0,
        maximized: false,
    };
    let raw = window_state::serialize(&saved_at_200).expect("serialise");
    let plan = window_state::plan_restore(Some(&raw), &undocked).expect("restored");

    assert_plan_reachable(plan, &undocked, "the saved scale no longer applies");
    assert_eq!(
        (plan.width, plan.height, plan.x, plan.y),
        (2200, 1500, -1900, 100),
        "the plan is physical pixels — the same numbers the check judged, \
         not a logical size something downstream will reinterpret",
    );
    assert!(
        plan.x + plan.width as i32 > 0,
        "the restored window must still reach the remaining display",
    );
}

#[test]
fn every_restored_plan_is_reachable_across_a_spread_of_arrangements() {
    // The property, swept rather than sampled: whatever `plan_restore`
    // returns is reachable on the displays it was given. A clamp, a
    // maximised containment or a scale change that produced an
    // unreachable plan would be caught here even without its own named
    // test above.
    let arrangements: [Vec<DisplayRect>; 5] = [
        one_display(),
        vec![display(0, 0, 3840, 2160, 2.0)],
        vec![
            display(-1920, 0, 1920, 1080, 1.0),
            display(0, 0, 2560, 1440, 1.25),
        ],
        vec![
            display(0, 0, 1280, 720, 1.0),
            display(1280, 0, 3840, 2160, 2.0),
        ],
        // A 1x panel beside a 4x monitor: the arrangement in which a
        // per-display rule lets the 4x neighbour vouch for a title bar
        // the window does not have.
        vec![
            display(0, 0, 2560, 1440, 1.0),
            display(2560, 0, 3840, 2160, 4.0),
        ],
    ];

    let mut restored = 0usize;
    let mut rejected = 0usize;
    for displays in &arrangements {
        for x in [-2200, -1900, -100, 0, 300, 1800, 1870, 2400, 3690] {
            for y in [-44, -40, -12, 0, 100, 1000] {
                for (width, height, scale) in [(200, 150, 1.0), (1400, 900, 1.0), (2200, 1500, 2.0)]
                {
                    for maximized in [false, true] {
                        let saved = SavedWindowState {
                            x,
                            y,
                            width,
                            height,
                            scale,
                            maximized,
                        };
                        let raw = window_state::serialize(&saved).expect("serialise");
                        match window_state::plan_restore(Some(&raw), displays) {
                            Some(plan) => {
                                restored += 1;
                                assert_plan_reachable(plan, displays, "swept arrangement");
                                assert!(
                                    plan.width >= (MIN_WIDTH * 1.0) as u32
                                        && plan.height >= (MIN_HEIGHT * 1.0) as u32,
                                    "a restored plan may never be below the minimum in any \
                                     coordinate space, and {plan:?} is below it even at 100%",
                                );
                            }
                            None => rejected += 1,
                        }
                    }
                }
            }
        }
    }

    // A predicate that rejected everything would satisfy the assertions
    // above by vacuity, so the sweep has to show it exercises both
    // answers. These are floors, not pinned counts: adding a case must
    // not force this test to be retuned.
    assert!(
        restored > 100,
        "only {restored} of {} cases restored — the sweep is asserting almost nothing",
        restored + rejected,
    );
    assert!(
        rejected > 100,
        "only {rejected} of {} cases were rejected — the sweep is not reaching the refusal",
        restored + rejected,
    );
}

// ---- the clamp ----

#[test]
fn a_restored_size_is_clamped_up_to_the_minimum() {
    // 900x600 is the size the Settings dialog needs to still read as a
    // dialog (#351). A row holding anything smaller — written by an
    // older build, or by a Windows that reported a collapsed window —
    // must not reopen below it.
    let too_small = SavedWindowState {
        x: 100,
        y: 100,
        width: 640,
        height: 400,
        scale: 1.0,
        maximized: false,
    };
    let raw = window_state::serialize(&too_small).expect("serialise");
    let plan = window_state::plan_restore(Some(&raw), &one_display()).expect("restored");
    assert_eq!(
        (plan.width, plan.height),
        (MIN_WIDTH as u32, MIN_HEIGHT as u32),
        "a remembered size below the minimum is clamped up to it, never applied",
    );
    assert_eq!(
        (plan.x, plan.y),
        (100, 100),
        "clamping the size must not move the window",
    );
}

#[test]
fn the_clamp_is_in_logical_pixels_so_high_dpi_is_not_clamped_away() {
    // 1600x1100 physical at 200% is 800x550 logical: genuinely under the
    // minimum, and must be clamped — to 1800x1200 physical, because the
    // minimum is logical and the window was saved at 200%. The same
    // physical numbers at 100% are comfortably over the minimum and must
    // be left alone. Comparing physical pixels against a logical minimum
    // would get one of these wrong.
    let at_200 = SavedWindowState {
        x: 0,
        y: 0,
        width: 1600,
        height: 1100,
        scale: 2.0,
        maximized: false,
    };
    let raw = window_state::serialize(&at_200).expect("serialise");
    let plan = window_state::plan_restore(Some(&raw), &[display(0, 0, 3840, 2160, 2.0)])
        .expect("restored");
    assert_eq!(
        (plan.width, plan.height),
        ((MIN_WIDTH * 2.0) as u32, (MIN_HEIGHT * 2.0) as u32),
        "the clamp is logical, so at 200% it lands at 1800x1200 physical",
    );

    let at_100 = SavedWindowState {
        scale: 1.0,
        ..at_200
    };
    let raw = window_state::serialize(&at_100).expect("serialise");
    let plan = window_state::plan_restore(Some(&raw), &one_display()).expect("restored");
    assert_eq!((plan.width, plan.height), (1600, 1100));
}

#[test]
fn the_clamp_follows_the_current_displays_scaling_not_the_saved_rows() {
    // The gap the same-scale tests could not see. A 1400x900 window saved
    // on a 1x display is 700x450 logical on a 2x one — below the 900x600
    // the Settings dialog needs — so the clamp has to be measured against
    // the display the window will live on, not the scale factor the row
    // was written at.
    //
    // Nothing downstream rescues this: tao 0.35.2's Windows size setter
    // forwards a programmatic size without clamping, and its
    // `WM_WINDOWPOSCHANGING` handler returns zero rather than letting
    // default processing validate the minimum. Treat `minWidth`/
    // `minHeight` as unenforced for a size GMM sets itself.
    let saved_at_100 = SavedWindowState {
        x: 100,
        y: 100,
        width: 1400,
        height: 900,
        scale: 1.0,
        maximized: false,
    };
    let raw = window_state::serialize(&saved_at_100).expect("serialise");

    let plan = window_state::plan_restore(Some(&raw), &[display(0, 0, 3840, 2160, 2.0)])
        .expect("restored");
    assert_eq!(
        (plan.width, plan.height),
        ((MIN_WIDTH * 2.0) as u32, (MIN_HEIGHT * 2.0) as u32),
        "700x450 logical on a 2x display must be clamped up to 900x600 \
         logical, which is 1800x1200 physical",
    );

    // The other direction: the same row on a 0.5x-equivalent reading is
    // comfortably over the minimum and must not be touched.
    let plan = window_state::plan_restore(Some(&raw), &one_display()).expect("restored");
    assert_eq!(
        (plan.width, plan.height),
        (1400, 900),
        "on the display it was saved on, nothing is clamped",
    );
}

#[test]
fn an_unclamped_size_round_trips_byte_for_byte() {
    // A fractional scale factor is the common Windows case (125%, 150%).
    // Dividing to logical and multiplying back would drift the remembered
    // size by a pixel a session, so the ordinary path must not convert at
    // all.
    let at_125 = SavedWindowState {
        x: 10,
        y: 10,
        width: 1375,
        height: 950,
        scale: 1.25,
        maximized: false,
    };
    let raw = window_state::serialize(&at_125).expect("serialise");
    let plan = window_state::plan_restore(Some(&raw), &[display(0, 0, 2560, 1440, 1.25)])
        .expect("restored");
    assert_eq!((plan.width, plan.height), (1375, 950));
}

#[test]
fn the_clamp_runs_before_the_reachability_check() {
    // The check has to judge the rect that will actually exist, not the
    // one that was stored. This 200-pixel-wide remembered window hangs
    // off the left edge with only 100 pixels on screen — not a grab area
    // as stored, but the clamp grows it to 900x600 and 800 pixels of its
    // title bar land on the display. Checking the stored rect first
    // would throw away a window that reopens perfectly reachable.
    let tiny_off_the_left_edge = SavedWindowState {
        x: -100,
        y: 0,
        width: 200,
        height: 150,
        scale: 1.0,
        maximized: false,
    };
    let raw = window_state::serialize(&tiny_off_the_left_edge).expect("serialise");
    let plan = window_state::plan_restore(Some(&raw), &one_display()).expect("restored");
    assert_eq!(
        (plan.width, plan.height, plan.x),
        (MIN_WIDTH as u32, MIN_HEIGHT as u32, -100),
        "the clamped window is reachable, so it is restored",
    );
}

// ---- the refusal ----

#[test]
fn geometry_on_a_display_that_is_no_longer_connected_is_rejected() {
    // The laptop left its dock. The second display was to the right; the
    // window was on it, and every pixel of it is now past the right edge
    // of the only display left.
    let on_the_second_display = SavedWindowState {
        x: 2400,
        y: 300,
        width: 1200,
        height: 800,
        scale: 1.0,
        maximized: false,
    };
    let raw = window_state::serialize(&on_the_second_display).expect("serialise");
    assert_eq!(
        window_state::plan_restore(Some(&raw), &one_display()),
        None,
        "a window restored onto a display that is gone cannot be dragged back",
    );

    // Plug the display back in and the very same row is usable again.
    let docked = vec![
        display(0, 0, 1920, 1080, 1.0),
        display(1920, 0, 1920, 1080, 1.0),
    ];
    assert!(
        window_state::plan_restore(Some(&raw), &docked).is_some(),
        "rejection is about the arrangement, not about the row",
    );
}

#[test]
fn a_title_bar_above_every_display_is_rejected_even_when_the_body_is_visible() {
    // The one offset a user cannot recover from: the window's body is on
    // screen, but the strip you drag it with is above the top edge.
    let title_bar_off_the_top = SavedWindowState {
        x: 300,
        y: -40,
        width: 1200,
        height: 800,
        scale: 1.0,
        maximized: false,
    };
    let raw = window_state::serialize(&title_bar_off_the_top).expect("serialise");
    assert_eq!(
        window_state::plan_restore(Some(&raw), &one_display()),
        None,
        "visible body, unreachable title bar: there is nothing to grab",
    );

    // 20 of the strip's 32 logical pixels is the stated threshold, so
    // -12 is the last position that still counts as reachable.
    let just_reachable = SavedWindowState {
        y: -12,
        ..title_bar_off_the_top
    };
    let raw = window_state::serialize(&just_reachable).expect("serialise");
    assert!(
        window_state::plan_restore(Some(&raw), &one_display()).is_some(),
        "20 logical pixels of the strip is the rule; -12 leaves exactly that",
    );
}

#[test]
fn a_sliver_of_the_title_bar_at_a_screen_edge_is_not_enough_to_grab() {
    // 60 physical pixels of the strip hanging onto the right edge of the
    // display. Something is visible, but not a grab area: the rule asks
    // for 120 logical pixels of width.
    let sliver = SavedWindowState {
        x: 1860,
        y: 100,
        width: 1200,
        height: 800,
        scale: 1.0,
        maximized: false,
    };
    let raw = window_state::serialize(&sliver).expect("serialise");
    assert_eq!(
        window_state::plan_restore(Some(&raw), &one_display()),
        None,
        "60 pixels of title bar is visible but not reachable",
    );

    let grabbable = SavedWindowState { x: 1800, ..sliver };
    let raw = window_state::serialize(&grabbable).expect("serialise");
    assert!(
        window_state::plan_restore(Some(&raw), &one_display()).is_some(),
        "120 pixels of title bar is the rule, and 1800 leaves exactly that",
    );
}

#[test]
fn the_grab_area_is_measured_against_the_scaling_of_the_display_it_lands_on() {
    // The rule is in logical pixels, so the physical threshold belongs to
    // the display being tested, not to the saved row: a title bar on a
    // 200% display is physically twice as large, and so is the grab area
    // the user needs. The same 150-physical-pixel overhang is a grab area
    // on a 100% display and is not one on a 200% display.
    let overhang = SavedWindowState {
        x: 3690,
        y: 100,
        width: 2400,
        height: 1600,
        scale: 1.0,
        maximized: false,
    };
    let raw = window_state::serialize(&overhang).expect("serialise");
    assert_eq!(
        window_state::plan_restore(Some(&raw), &[display(0, 0, 3840, 2160, 2.0)]),
        None,
        "150 physical pixels is only 75 logical pixels on a 200% display",
    );
    assert!(
        window_state::plan_restore(Some(&raw), &[display(0, 0, 3840, 2160, 1.0)]).is_some(),
        "the same 150 physical pixels on a 100% display is a grab area",
    );
}

#[test]
fn geometry_judged_against_no_displays_at_all_is_rejected() {
    // An empty display list means GMM could not enumerate the monitors,
    // not that the window is vacuously reachable on all of them.
    let raw = window_state::serialize(&settled_window()).expect("serialise");
    assert_eq!(window_state::plan_restore(Some(&raw), &[]), None);
}

// ---- maximising onto the right display ----

#[test]
fn a_maximised_window_restores_down_to_the_default_not_to_its_maximised_rect() {
    // The pre-maximise size is deliberately not remembered (#369).
    // Applying the maximised rect as the restore-down size would make
    // Restore Down a no-op: the window would un-maximise onto the same
    // pixels it already covers.
    let maximised = SavedWindowState {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
        scale: 1.0,
        maximized: true,
    };
    let raw = window_state::serialize(&maximised).expect("serialise");
    let plan = window_state::plan_restore(Some(&raw), &one_display()).expect("restored");
    assert_eq!(
        plan,
        RestorePlan {
            width: DEFAULT_WIDTH as u32,
            height: DEFAULT_HEIGHT as u32,
            x: 0,
            y: 0,
            maximized: true,
        },
        "maximise onto the remembered display, but restore down to the default size",
    );
}

#[test]
fn the_pre_maximise_rect_is_placed_wholly_inside_the_remembered_display() {
    // Windows maximises onto the display holding the largest part of the
    // window. A default-sized rect left hanging over a neighbour could
    // therefore be handed to the wrong screen, so it is contained first.
    //
    // Here the remembered display is smaller than the default rect in
    // both directions and sits beside a much larger one, which is the
    // arrangement where a hanging rect would overlap the neighbour most.
    let small = display(0, 0, 1024, 720, 1.0);
    let large = display(1024, 0, 3840, 2160, 2.0);
    let maximised_on_the_small_one = SavedWindowState {
        x: 0,
        y: 0,
        width: 1024,
        height: 720,
        scale: 1.0,
        maximized: true,
    };
    let raw = window_state::serialize(&maximised_on_the_small_one).expect("serialise");
    let plan = window_state::plan_restore(Some(&raw), &[small, large]).expect("restored");

    assert_eq!(
        plan,
        RestorePlan {
            width: 1024,
            height: 720,
            x: 0,
            y: 0,
            maximized: true,
        },
        "the default rect is cut to the display rather than hung over its neighbour",
    );
    assert!(
        plan.x >= small.x
            && plan.y >= small.y
            && plan.x + plan.width as i32 <= small.x + small.width as i32
            && plan.y + plan.height as i32 <= small.y + small.height as i32,
        "the pre-maximise rect must lie wholly inside the remembered display, \
         or Windows may maximise onto the neighbour",
    );
}

#[test]
fn the_pre_maximise_rect_follows_the_remembered_displays_scaling() {
    // Restore Down should give a default-sized window on the display it
    // lands on, which means the default's logical size scaled by *that*
    // display's scale factor.
    let hidpi = display(0, 0, 3840, 2160, 2.0);
    let maximised = SavedWindowState {
        x: 0,
        y: 0,
        width: 3840,
        height: 2160,
        scale: 2.0,
        maximized: true,
    };
    let raw = window_state::serialize(&maximised).expect("serialise");
    let plan = window_state::plan_restore(Some(&raw), &[hidpi]).expect("restored");
    assert_eq!(
        (plan.width, plan.height),
        ((DEFAULT_WIDTH * 2.0) as u32, (DEFAULT_HEIGHT * 2.0) as u32),
    );
}

#[test]
fn the_maximise_target_is_the_display_the_window_most_overlaps_not_the_first_that_overlaps() {
    // Two displays, both overlapped, and the one the window mostly sits
    // on is **second** in the list — so "pick the first display that
    // overlaps" fails this and greatest-overlap passes it. The remembered
    // origin is also above both displays, so the rect's own overlap is
    // the only thing that can decide.
    let lesser = display(0, 0, 1920, 1080, 1.0);
    let greater = display(1920, 0, 1920, 1080, 1.0);
    let maximised_mostly_on_the_right = SavedWindowState {
        x: 1500,
        y: -12,
        width: 1600,
        height: 1000,
        scale: 1.0,
        maximized: true,
    };
    let raw = window_state::serialize(&maximised_mostly_on_the_right).expect("serialise");
    let displays = [lesser, greater];
    let plan = window_state::plan_restore(Some(&raw), &displays).expect("restored");

    // 420 of the window's 1600 columns fall on `lesser` and 1180 on
    // `greater`, so `greater` is the target and the pre-maximise rect is
    // contained inside it — x nudged from 1500 up to 1920.
    assert_eq!(
        plan,
        RestorePlan {
            width: DEFAULT_WIDTH as u32,
            height: DEFAULT_HEIGHT as u32,
            x: 1920,
            y: 0,
            maximized: true,
        },
        "the pre-maximise rect must be contained inside the display the \
         window most overlaps, not the first one it touches",
    );
    assert_plan_reachable(plan, &displays, "maximised across two displays");
}

// ---- one display, one DPI ----

#[test]
fn a_window_is_judged_on_the_display_it_primarily_occupies() {
    // A 1x panel beside a 4x monitor. Most of this window sits on the 1x
    // panel, so that is where Windows would give it its DPI and its
    // 32-pixel title bar — which ends at y=-12, above both screens. The
    // window is unreachable and must be refused.
    //
    // Judging each display on its own terms accepts it instead: the 4x
    // neighbour measures a 128-pixel strip the window does not have, sees
    // 640x84 physical pixels of it against its own 480x80 threshold, and
    // vouches for a window living at 1x.
    let displays = [
        display(0, 0, 2560, 1440, 1.0),
        display(2560, 0, 3840, 2160, 4.0),
    ];
    let title_bar_above_both_screens = SavedWindowState {
        x: 1800,
        y: -44,
        width: 1400,
        height: 900,
        scale: 1.0,
        maximized: false,
    };
    let raw = window_state::serialize(&title_bar_above_both_screens).expect("serialise");
    assert_eq!(
        window_state::plan_restore(Some(&raw), &displays),
        None,
        "a high-DPI neighbour must not vouch for a title bar the window \
         does not have",
    );

    // Nothing about the 4x display is being rejected for its own sake:
    // drop the window 32 pixels and the same arrangement restores it.
    let reachable = SavedWindowState {
        y: -12,
        ..title_bar_above_both_screens
    };
    let raw = window_state::serialize(&reachable).expect("serialise");
    let plan = window_state::plan_restore(Some(&raw), &displays).expect("restored");
    assert_plan_reachable(plan, &displays, "mostly on the 1x panel");
}

// ---- absent and unreadable state ----

#[tokio::test]
async fn a_first_run_has_nothing_remembered_and_does_not_fail() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let raw = core.remembered_window_state().await.expect("read");
    assert_eq!(raw, None, "a fresh install remembers nothing");
    assert_eq!(
        window_state::plan_restore(raw.as_deref(), &one_display()),
        None,
        "absent state opens at the default",
    );
}

#[tokio::test]
async fn corrupt_remembered_state_opens_at_the_default_and_does_not_fail() {
    // Absent state and rejected state take the same path on purpose:
    // from the outside a first run and an unusable row are
    // indistinguishable, and the user is told nothing either way.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let pool = sqlx::SqlitePool::connect(&db_url).await.expect("open db");

    for corrupt in [
        "",
        "not json at all",
        "{",
        "[]",
        "null",
        // Valid JSON, wrong shape: an older or hand-edited row.
        r#"{"x":10,"y":10}"#,
        // Plausible JSON, impossible values.
        r#"{"x":10,"y":10,"width":0,"height":600,"scale":1.0,"maximized":false}"#,
        r#"{"x":10,"y":10,"width":1100,"height":760,"scale":0.0,"maximized":false}"#,
        r#"{"x":10,"y":10,"width":1100,"height":760,"scale":-2.0,"maximized":false}"#,
        // Windows reports a minimised window here. Restoring to it would
        // reopen GMM somewhere the user cannot reach.
        r#"{"x":-32000,"y":-32000,"width":1100,"height":760,"scale":1.0,"maximized":false}"#,
    ] {
        gmm_lib::core::settings::put(&pool, "window.mainGeometry", Some(corrupt))
            .await
            .expect("seed corrupt geometry");
        let raw = core
            .remembered_window_state()
            .await
            .expect("reading a corrupt row must not be an error");
        assert_eq!(
            window_state::plan_restore(raw.as_deref(), &one_display()),
            None,
            "{corrupt:?} must open at the default",
        );
    }
    pool.close().await;
}

#[test]
fn a_display_reporting_a_nonsense_scale_factor_does_not_block_a_restore() {
    // A platform that hands back a zero or NaN scale factor must not make
    // GMM refuse to reopen where the user left it. The display is read at
    // 100% instead.
    let raw = window_state::serialize(&settled_window()).expect("serialise");
    for nonsense in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(
            window_state::plan_restore(Some(&raw), &[display(0, 0, 1920, 1080, nonsense)])
                .is_some(),
            "a scale factor of {nonsense} must be read as 100%, not as a refusal",
        );
    }
}

#[tokio::test]
async fn implausible_geometry_is_never_written_in_the_first_place() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let minimised = SavedWindowState {
        x: -32000,
        y: -32000,
        width: 1100,
        height: 760,
        scale: 1.0,
        maximized: false,
    };
    assert_eq!(window_state::serialize(&minimised), None);
    core.remember_window_state(&minimised)
        .await
        .expect("dropping an implausible write is not an error");
    assert_eq!(
        core.remembered_window_state().await.expect("read"),
        None,
        "a sentinel position must not reach the database",
    );
}

// ---- the write trigger ----

#[test]
fn only_the_newest_geometry_is_kept_to_write() {
    let pending = PendingWindowState::default();
    assert!(pending.record(settled_window()));
    for x in 201..400 {
        assert!(pending.record(SavedWindowState {
            x,
            ..settled_window()
        }));
    }
    assert_eq!(
        pending.take().map(|state| state.x),
        Some(399),
        "the one write carries where the drag ended, not where it started",
    );
    assert_eq!(pending.take(), None, "taking twice must not re-write");
}

#[test]
fn a_minimised_windows_sentinel_is_dropped_and_does_not_wake_the_writer() {
    let pending = PendingWindowState::default();
    assert!(!pending.record(SavedWindowState {
        x: -32000,
        y: -32000,
        ..settled_window()
    }));
    assert_eq!(pending.take(), None);
}

/// The write trigger is a **debounce**, not a throttle: every move resets
/// the deadline, so a drag produces one write after it ends rather than
/// one every window while it lasts. That was the stated reason for having
/// a delay at all, so it is worth testing the timing rather than only the
/// coalescing.
#[test]
fn each_move_resets_the_debounce_deadline() {
    let quiet = Duration::from_millis(300);
    let (tx, rx) = mpsc::channel::<()>();

    // Four moves at a fifth of the quiet window apart, so the last lands
    // at about one whole quiet window in. A throttle returns one quiet
    // window after the *first* notification; a debounce waits a whole one
    // out after the *last*, so it cannot return before 2x.
    let notifier_tx = tx.clone();
    let notifier = std::thread::spawn(move || {
        for _ in 0..4 {
            std::thread::sleep(quiet / 5);
            if notifier_tx.send(()).is_err() {
                return;
            }
        }
    });

    let started = Instant::now();
    window_state::wait_until_quiet(&rx, quiet);
    let waited = started.elapsed();
    notifier.join().expect("notifier");
    drop(tx);

    assert!(
        waited >= quiet + quiet / 2,
        "a throttle would have returned after about one quiet window ({quiet:?}); \
         a debounce must wait out the last move, and this returned after {waited:?}",
    );
}

#[test]
fn a_still_window_settles_after_one_quiet_window() {
    let quiet = Duration::from_millis(80);
    let (tx, rx) = mpsc::channel::<()>();
    tx.send(()).expect("one move");
    // Drain the move the writer thread's outer `recv` would have taken.
    rx.recv().expect("the move");

    let started = Instant::now();
    assert_eq!(window_state::wait_until_quiet(&rx, quiet), Quiet::Settled);
    assert!(
        started.elapsed() >= quiet,
        "the write must not land before the window has been still",
    );
    drop(tx);
}

#[test]
fn a_closed_window_stops_the_wait_instead_of_holding_the_writer() {
    // Every notifier dropped means the window is gone. The wait must end
    // at once rather than sit out another quiet window: nothing is
    // waiting on this thread, but a thread that lingers after shutdown is
    // still a thread that could be mistaken for one that blocks close.
    let (tx, rx) = mpsc::channel::<()>();
    drop(tx);
    let started = Instant::now();
    assert_eq!(
        window_state::wait_until_quiet(&rx, Duration::from_secs(30)),
        Quiet::Disconnected,
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

// ---- the default the fallback returns to ----

#[test]
fn the_declared_window_matches_the_constants_the_fallback_uses() {
    // `plan_restore` returning `None` leaves the window at whatever
    // `tauri.conf.json` declares, and the clamp and the maximised
    // restore-down size are expressed against these constants. If the
    // two drift, the fallback stops being 1100x760 while every test here
    // goes on claiming it is.
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
    let raw = std::fs::read_to_string(&path).expect("read tauri.conf.json");
    let conf: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON");
    let window = &conf["app"]["windows"][0];

    assert_eq!(
        (
            window["width"].as_f64(),
            window["height"].as_f64(),
            window["minWidth"].as_f64(),
            window["minHeight"].as_f64(),
        ),
        (
            Some(DEFAULT_WIDTH),
            Some(DEFAULT_HEIGHT),
            Some(MIN_WIDTH),
            Some(MIN_HEIGHT),
        ),
    );
    assert_eq!(
        window["label"].as_str(),
        Some(gmm_lib::runtime::window::MAIN_WINDOW_LABEL),
        "the restore targets the window by label",
    );
    assert_eq!(
        window["visible"].as_bool(),
        Some(false),
        "the window is hidden until the geometry is applied, so the restore \
         is not seen as a jump from the default size to the remembered one",
    );
}

/// The window is declared `visible: false`, so exactly one thing makes
/// it appear. No behavioural test can reach `restore_and_track` — it
/// needs a real event loop and real monitors — so this reads its source
/// instead, as `menu.rs` reads `build_menu`'s.
///
/// Without this guard, an edit that returns early on an unreadable row
/// or a failed placement would ship a GMM that never draws a window at
/// all, and every test above would still pass.
#[test]
fn the_restore_shows_the_window_on_every_path() {
    let source = runtime_window_source();

    let body = source
        .split_once("pub fn restore_and_track")
        .expect("restore_and_track must exist")
        .1;
    let body = body
        .split_once("\nfn displays")
        .expect("restore_and_track is followed by `displays`")
        .0;

    assert!(
        body.contains("window.show()"),
        "restore_and_track must show the window it restored",
    );
    // One `return`: the early exit taken when there is no window to show
    // in the first place.
    assert_eq!(
        body.matches("return").count(),
        1,
        "a second early return in restore_and_track could skip `show()` and \
         ship a GMM with no visible window",
    );
    assert!(
        !body.contains(".expect(") && !body.contains(".unwrap()"),
        "a panic in restore_and_track would leave the window hidden forever",
    );
}

/// What is captured must pair with what is applied.
///
/// Tauri's `set_size` takes an **inner** size and `set_position` an
/// **outer** position. Capturing the outer size instead would write back
/// a size a decoration wider and taller than the next launch applies as
/// the inner size, and the window would creep larger every session — a
/// drift no single-session test can see, and one a user would read as
/// GMM slowly losing its mind rather than as a bug.
#[test]
fn the_captured_size_pairs_with_the_setter_that_applies_it() {
    let source = runtime_window_source();
    assert!(
        source.contains("window.inner_size()") && !source.contains("window.outer_size()"),
        "the remembered size must be the inner size `set_size` applies",
    );
    assert!(
        source.contains("window.outer_position()") && !source.contains("window.inner_position()"),
        "the remembered position must be the outer position `set_position` applies",
    );
}

/// The plan is physical pixels and must be applied as physical pixels.
///
/// A `LogicalSize` here would be converted by the scale factor of
/// whichever display the window is on at that instant — which is not
/// necessarily the one the geometry was saved at, and not necessarily the
/// one the check used. That mismatch is the P1 this feature shipped with
/// in its first draft: a 2200-pixel-wide rect was checked and an
/// 1100-pixel-wide one applied, putting the whole window off-screen. The
/// pure core can hold the rule but not the conversion, so this guard
/// does.
#[test]
fn the_plan_is_applied_in_the_coordinate_space_it_was_checked_in() {
    let source = runtime_window_source();
    assert!(
        source.contains("PhysicalSize::new(plan.width, plan.height)")
            && source.contains("PhysicalPosition::new(plan.x, plan.y)"),
        "the plan must go out as physical pixels, unconverted",
    );
    assert!(
        !source.contains("LogicalSize") && !source.contains("LogicalPosition"),
        "a logical size or position would be reinterpreted by the current \
         scale factor, so the window would not get the rect that was checked",
    );
}

/// `src/runtime/window.rs` with its comments removed.
///
/// The guards above assert what the module *does*, and this module
/// explains at length what it must not do — so a guard that searched the
/// raw file would trip over the very prose warning against the mistake.
/// No string literal in that file contains `//`, so dropping from the
/// first `//` on each line is enough.
/// The writer must wait before it writes.
///
/// `each_move_resets_the_debounce_deadline` tests `wait_until_quiet`
/// itself, but nothing makes the writer thread *call* it: delete the wait
/// and the geometry is written on the first move, which is the throttle
/// this feature already shipped once. `spawn_writer` needs a real `Core`
/// and a runtime, so it is guarded at the source like `show()` is.
#[test]
fn the_writer_waits_for_the_window_to_be_still_before_it_writes() {
    let source = runtime_window_source();
    let body = source
        .split_once("fn spawn_writer")
        .expect("spawn_writer must exist")
        .1;
    let body = body
        .split_once("\nfn capture")
        .expect("spawn_writer is followed by `capture`")
        .0;

    let wait = body
        .find("window_state::wait_until_quiet(&rx, WRITE_DEBOUNCE)")
        .expect("the writer must debounce before writing, not write on the first move");
    let take = body
        .find("pending.take()")
        .expect("the writer must take the pending geometry");
    let write = body
        .find("core.remember_window_state(")
        .expect("the writer must write the geometry");
    assert!(
        wait < take && take < write,
        "the wait must come before the geometry is taken and written, or \
         a drag writes every intermediate rectangle",
    );
}

fn runtime_window_source() -> String {
    let raw = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/runtime/window.rs"),
    )
    .expect("read src/runtime/window.rs");
    raw.lines()
        .map(|line| match line.split_once("//") {
            Some((code, _)) => code,
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
