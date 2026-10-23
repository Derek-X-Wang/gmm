use egui::{
    accesskit::{Live, Role},
    vec2,
};
use egui_kittest::{
    Harness, Node,
    kittest::{By, NodeT, Queryable},
};
use gmm_egui_modlist_spike::{model::*, view::ModUpdateBatchAction};

const APPLY: &str = "Apply all Mod updates";
const STOP: &str = "Stop after this Mod";
const WAITING: &str = "Stop requested. Waiting for Current to finish; later Mods will not start.";
const SKIPPED: &str =
    "Later: Skipped — Not started because you stopped the batch. The Library was not touched.";
const CONFLICT_ERROR: &str = "Conflicts unverified: The full Conflict reading is unverified because the batch stopped with Mods not attempted.";
const STOP_ERROR: &str =
    "Stop request unverified: connection lost The batch may continue. Try again.";

fn rows() -> Vec<ModUpdateRow> {
    ["First", "Current", "Later"]
        .into_iter()
        .map(|name| ModUpdateRow {
            item: BatchMod {
                mod_id: name.into(),
                name: name.into(),
            },
            upstream_ahead: true,
        })
        .collect()
}

fn updated(index: usize) -> BatchModResult {
    BatchModResult {
        item: rows()[index].item.clone(),
        status: ModResult::Updated,
    }
}

fn report() -> ModUpdateBatchReport {
    ModUpdateBatchReport {
        stopped: false,
        results: (0..3).map(updated).collect(),
        conflicts: Some(ConflictReport::default()),
        conflict_error: None,
    }
}

fn harness() -> Harness<'static, ModUpdateBatchAction> {
    Harness::builder()
        .with_size(vec2(800.0, 800.0))
        .build_ui_state(
            |ui, view| view.show(ui),
            ModUpdateBatchAction::new("gimi", rows()),
        )
}

fn visible(node: Node<'_>) {
    assert!(!node.accesskit_node().is_hidden());
    assert!(node.rect().is_positive());
    assert!(
        egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 800.0)).intersects(node.rect())
    );
}

fn content(h: &Harness<'_, ModUpdateBatchAction>, role: Role, name: &str) {
    let node = h.get_by_role_and_label(role, name);
    assert_eq!(node.value().as_deref(), Some(name));
    visible(node);
}

fn status(h: &Harness<'_, ModUpdateBatchAction>, name: &str) {
    // Keep the original bare-role uniqueness assertion as well as role + name.
    assert_eq!(h.query_all_by_role(Role::Status).count(), 1);
    content(h, Role::Status, name);
    let node = h.get_by_role_and_label(Role::Status, name);
    assert_eq!(node.accesskit_node().live(), Live::Polite);
    assert!(node.accesskit_node().is_live_atomic());
}

fn alert(h: &Harness<'_, ModUpdateBatchAction>, name: &str) {
    assert_eq!(h.query_all_by_role(Role::Alert).count(), 1);
    content(h, Role::Alert, name);
    assert_eq!(
        h.get_by_role_and_label(Role::Alert, name)
            .accesskit_node()
            .live(),
        Live::Assertive
    );
}

fn no_stop(h: &Harness<'_, ModUpdateBatchAction>) {
    assert!(h.query_by_role_and_label(Role::Button, STOP).is_none());
}

fn no_alert(h: &Harness<'_, ModUpdateBatchAction>, name: &str) {
    assert!(h.query_by_role_and_label(Role::Alert, name).is_none());
    // Original queryByRole(alert) ruled out ALL alerts, not just one name.
    assert_eq!(h.query_all_by_role(Role::Alert).count(), 0);
}

fn click(h: &mut Harness<'_, ModUpdateBatchAction>, name: &str) {
    h.get_by_role_and_label(Role::Button, name).click();
    h.run();
}

fn stops(h: &Harness<'_, ModUpdateBatchAction>) -> Vec<Command> {
    h.state()
        .service
        .calls
        .iter()
        .filter(|c| matches!(c, Command::Stop { .. }))
        .cloned()
        .collect()
}

fn start(h: &mut Harness<'_, ModUpdateBatchAction>) {
    assert!(
        !h.get_by_role_and_label(Role::Button, APPLY)
            .accesskit_node()
            .is_disabled()
    ); // TS L28
    click(h, APPLY);
    assert!(h.state().service.on_progress.is_some()); // L30: the type is FnMut at compile time
    h.state_mut().service.emit(ModUpdateProgress::Started(
        rows().into_iter().map(|r| r.item).collect(),
    ));
    h.state_mut()
        .service
        .emit(ModUpdateProgress::Completed(updated(0)));
    h.state_mut()
        .service
        .emit(ModUpdateProgress::Applying(rows()[1].item.clone()));
    h.run();
}

#[test]
fn explicit_boundary_stop_names_waiting_mod_and_sends_only_once() {
    let mut h = harness();
    start(&mut h);
    assert!(
        !h.get_by_role_and_label(Role::Button, STOP)
            .accesskit_node()
            .is_disabled()
    ); // L43
    click(&mut h, STOP);
    click(&mut h, STOP);
    // Also exercise same-batch request reentry directly; the UI fence alone is
    // insufficient evidence that the service only receives one stop request.
    let id = h.state().batch_id("gimi");
    h.state_mut().request_stop("gimi", id);
    h.run();
    assert!(
        h.get_by_role_and_label(Role::Button, STOP)
            .accesskit_node()
            .is_disabled()
    ); // L46
    content(&h, Role::Paragraph, WAITING); // L47
    let apply_id = h
        .state()
        .service
        .calls
        .iter()
        .find_map(|call| match call {
            Command::Apply { game, batch_id } if game == "gimi" => Some(*batch_id),
            _ => None,
        })
        .unwrap();
    assert_eq!(stops(&h), vec![Command::Stop { batch_id: apply_id }]); // L48
    content(&h, Role::ListItem, "First: Updated"); // L49
    h.state_mut()
        .progress("gimi", ModUpdateProgress::CheckingConflicts);
    h.run();
    no_stop(&h); // L51
    let mut result = report();
    result.stopped = true;
    result.results[2].status = ModResult::Skipped(
        "Not started because you stopped the batch. The Library was not touched.".into(),
    );
    result.conflict_error = Some("The full Conflict reading is unverified because the batch stopped with Mods not attempted.".into());
    h.state_mut().finish("gimi", result.clone());
    h.run();
    status(&h, "Batch stopped."); // L59
    content(&h, Role::ListItem, "Current: Updated"); // L60
    content(&h, Role::ListItem, SKIPPED); // L61: full string is stronger than prefix + substring
    assert_eq!(
        h.query_all(By::new().predicate(|node| {
            // TextRun is the inline text fragment of the same widget, not a
            // second element (React getByText also excludes ancestor duplicates).
            node.role() != Role::TextRun
                && node
                    .value()
                    .is_some_and(|value| value.starts_with("Later: Skipped"))
        }))
        .count(),
        1
    );
    alert(&h, CONFLICT_ERROR); // L62
    assert!(
        h.query_by_role_and_label(Role::Paragraph, "0 Conflicts found after the batch.")
            .is_none()
    ); // L63
    assert_eq!(
        h.query_all(By::new().predicate(|node| {
            node.value().as_deref() == Some("0 Conflicts found after the batch.")
                || node.label().as_deref() == Some("0 Conflicts found after the batch.")
        }))
        .count(),
        0
    );
    assert!(h.state().service.calls.contains(&Command::Invalidate(vec![
        "conflicts".into(),
        "gimi".into()
    ]))); // L64
    assert!(!h.state().service.calls.contains(&Command::SetConflicts {
        game: "gimi".into(),
        report: result.conflicts.unwrap()
    })); // L65
}

#[test]
fn withdraws_stop_during_final_scan_and_restores_it_for_fresh_batch() {
    let mut h = harness();
    start(&mut h);
    h.state_mut()
        .progress("gimi", ModUpdateProgress::Completed(updated(1)));
    h.state_mut()
        .progress("gimi", ModUpdateProgress::Applying(rows()[2].item.clone()));
    h.state_mut()
        .progress("gimi", ModUpdateProgress::Completed(updated(2)));
    h.state_mut()
        .progress("gimi", ModUpdateProgress::CheckingConflicts);
    h.run();
    status(&h, "Checking Conflicts… 3 of 3 completed."); // L76
    no_stop(&h); // L77
    assert_eq!(stops(&h).len(), 0); // L78
    let old_id = h.state().batch_id("gimi");
    h.state_mut().finish("gimi", report());
    h.run();
    assert!(
        !h.get_by_role_and_label(Role::Button, APPLY)
            .accesskit_node()
            .is_disabled()
    ); // L81
    click(&mut h, APPLY);
    assert_eq!(
        h.state()
            .service
            .calls
            .iter()
            .filter(|c| matches!(c, Command::Apply { .. }))
            .count(),
        2
    ); // L83
    assert_ne!(old_id, h.state().batch_id("gimi"));
    assert!(
        !h.get_by_role_and_label(Role::Button, STOP)
            .accesskit_node()
            .is_disabled()
    ); // L84
    status(&h, "Starting updates… 0 of 3 completed."); // L85
}

#[test]
fn stop_delivery_failure_stays_visible_and_retry_is_possible() {
    let mut h = harness();
    start(&mut h);
    click(&mut h, STOP);
    let id = h.state().batch_id("gimi");
    h.state_mut().stop_failed("gimi", id, "connection lost");
    h.run();
    alert(&h, STOP_ERROR); // L92
    assert!(
        !h.get_by_role_and_label(Role::Button, STOP)
            .accesskit_node()
            .is_disabled()
    ); // L94
    click(&mut h, STOP);
    assert!(
        h.get_by_role_and_label(Role::Button, STOP)
            .accesskit_node()
            .is_disabled()
    ); // L96
    no_alert(&h, STOP_ERROR); // L97
    assert_eq!(
        stops(&h),
        vec![
            Command::Stop { batch_id: id },
            Command::Stop { batch_id: id }
        ]
    );
}

#[test]
fn another_game_has_no_stale_token_and_late_stop_does_not_change_finished_result() {
    let mut h = harness();
    start(&mut h);
    h.state_mut().game = "srmi".into();
    h.state_mut().rows.clear();
    h.run();
    no_stop(&h); // L103
    h.state_mut().game = "gimi".into();
    h.state_mut().rows = rows();
    h.run();
    click(&mut h, STOP);
    h.state_mut().finish("gimi", report());
    h.run();
    status(&h, "Batch finished."); // L107
    no_stop(&h); // L108
}

#[test]
fn delayed_stop_error_is_ignored_after_batch_finishes() {
    let mut h = harness();
    start(&mut h);
    click(&mut h, STOP);
    let id = h.state().batch_id("gimi");
    h.state_mut().finish("gimi", report());
    h.run();
    h.state_mut()
        .stop_failed("gimi", id, "late connection failure");
    h.run();
    status(&h, "Batch finished."); // L119
    no_alert(
        &h,
        "Stop request unverified: late connection failure The batch may continue. Try again.",
    ); // L120
    no_stop(&h); // L121
}

#[test]
fn apply_gates_and_pending_name_are_exposed_in_accesskit() {
    for gate in 0..5 {
        let mut h = harness();
        match gate {
            0 => h.state_mut().checking = true,
            1 => h.state_mut().verified = false,
            2 => h
                .state_mut()
                .rows
                .iter_mut()
                .for_each(|r| r.upstream_ahead = false),
            3 => h.state_mut().session = None,
            4 => h.state_mut().session = Some(true),
            _ => unreachable!(),
        }
        h.run();
        assert!(
            h.get_by_role_and_label(Role::Button, APPLY)
                .accesskit_node()
                .is_disabled()
        );
        click(&mut h, APPLY);
        assert!(h.state().service.calls.is_empty());
    }
    let mut h = harness();
    click(&mut h, APPLY);
    assert!(
        h.get_by_role_and_label(Role::Button, "Applying Mod updates…")
            .accesskit_node()
            .is_disabled()
    );
    assert!(
        !h.get_by_role_and_label(Role::Button, STOP)
            .accesskit_node()
            .is_disabled()
    );
}
