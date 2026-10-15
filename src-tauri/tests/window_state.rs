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
//! must overlap some connected display**. Whether Windows actually
//! enumerates the displays GMM asks it for, and what it reports across a
//! scaling change, is Windows-only evidence; #326 collects it.

use gmm_lib::core::window_state::{
    self, DisplayRect, PendingWindowState, SavedWindowState, DEFAULT_HEIGHT, DEFAULT_WIDTH,
    MIN_HEIGHT, MIN_WIDTH,
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
    vec![DisplayRect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    }]
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
        DisplayRect {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
        },
        DisplayRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        },
    ];
    let plan = window_state::plan_restore(Some(&raw), &displays).expect("restored");
    assert_eq!(
        (plan.x, plan.y, plan.maximized),
        (-1920, 0, true),
        "a window closed maximised on the left display reopens maximised on the left display",
    );
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
        (plan.width, plan.height, plan.x, plan.y, plan.maximized),
        (1400.0, 900.0, 200, 200, false),
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
        (plan.width, plan.height, plan.maximized),
        (DEFAULT_WIDTH, DEFAULT_HEIGHT, true),
        "maximise onto the remembered display, but restore down to the default size",
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
        (MIN_WIDTH, MIN_HEIGHT),
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
    // minimum, and must be clamped. The same physical numbers at 100%
    // are comfortably over it and must be left alone. Comparing physical
    // pixels against a logical minimum would get one of these wrong.
    let at_200 = SavedWindowState {
        x: 0,
        y: 0,
        width: 1600,
        height: 1100,
        scale: 2.0,
        maximized: false,
    };
    let raw = window_state::serialize(&at_200).expect("serialise");
    let plan = window_state::plan_restore(
        Some(&raw),
        &[DisplayRect {
            x: 0,
            y: 0,
            width: 3840,
            height: 2160,
        }],
    )
    .expect("restored");
    assert_eq!((plan.width, plan.height), (MIN_WIDTH, MIN_HEIGHT));

    let at_100 = SavedWindowState {
        scale: 1.0,
        ..at_200
    };
    let raw = window_state::serialize(&at_100).expect("serialise");
    let plan = window_state::plan_restore(Some(&raw), &one_display()).expect("restored");
    assert_eq!((plan.width, plan.height), (1600.0, 1100.0));
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
        DisplayRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        },
        DisplayRect {
            x: 1920,
            y: 0,
            width: 1920,
            height: 1080,
        },
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
fn the_reachability_rule_means_the_same_thing_at_200_percent_scaling() {
    // The rule is in logical pixels, so the threshold has to move with
    // the scale factor. At 200% the same 120-logical-pixel grab area is
    // 240 physical pixels, so a 150-physical-pixel overhang that passes
    // at 100% must fail at 200%.
    let display_4k = vec![DisplayRect {
        x: 0,
        y: 0,
        width: 3840,
        height: 2160,
    }];
    let overhang = SavedWindowState {
        x: 3690,
        y: 100,
        width: 2400,
        height: 1600,
        scale: 2.0,
        maximized: false,
    };
    let raw = window_state::serialize(&overhang).expect("serialise");
    assert_eq!(
        window_state::plan_restore(Some(&raw), &display_4k),
        None,
        "150 physical pixels is only 75 logical pixels at 200%: not a grab area",
    );

    let at_100 = SavedWindowState {
        scale: 1.0,
        ..overhang
    };
    let raw = window_state::serialize(&at_100).expect("serialise");
    assert!(
        window_state::plan_restore(Some(&raw), &display_4k).is_some(),
        "the same 150 physical pixels at 100% is a grab area",
    );
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
        (MIN_WIDTH, MIN_HEIGHT, -100),
        "the clamped window is reachable, so it is restored",
    );
}

#[test]
fn geometry_judged_against_no_displays_at_all_is_rejected() {
    // An empty display list means GMM could not enumerate the monitors,
    // not that the window is vacuously reachable on all of them.
    let raw = window_state::serialize(&settled_window()).expect("serialise");
    assert_eq!(window_state::plan_restore(Some(&raw), &[]), None);
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
fn a_drag_across_a_desk_coalesces_into_one_write() {
    // The write trigger is a debounce after the last move or resize, not
    // a write per event. Nothing is written on close, which is how
    // "writing never blocks window close" is structural rather than a
    // promise about how fast a write is.
    let pending = PendingWindowState::default();

    assert!(
        pending.record(settled_window()),
        "the first move must schedule a flush",
    );
    for x in 201..400 {
        assert!(
            !pending.record(SavedWindowState {
                x,
                ..settled_window()
            }),
            "a move while a flush is pending must not schedule a second one",
        );
    }

    assert_eq!(
        pending.take().map(|state| state.x),
        Some(399),
        "the one write carries where the drag ended, not where it started",
    );
    assert_eq!(pending.take(), None, "taking twice must not re-write");
    assert!(
        pending.record(settled_window()),
        "a move after the flush schedules the next one",
    );
}

#[test]
fn a_minimised_windows_sentinel_is_dropped_without_scheduling_a_write() {
    let pending = PendingWindowState::default();
    assert!(!pending.record(SavedWindowState {
        x: -32000,
        y: -32000,
        ..settled_window()
    }));
    assert_eq!(pending.take(), None);
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
    let source = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/runtime/window.rs"),
    )
    .expect("read src/runtime/window.rs");

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
    let source = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/runtime/window.rs"),
    )
    .expect("read src/runtime/window.rs");

    assert!(
        source.contains("window.inner_size()") && !source.contains("window.outer_size()"),
        "the remembered size must be the inner size `set_size` applies",
    );
    assert!(
        source.contains("window.outer_position()") && !source.contains("window.inner_position()"),
        "the remembered position must be the outer position `set_position` applies",
    );
}
