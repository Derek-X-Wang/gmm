use egui::{
    accesskit::{Live, Role},
    vec2,
};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use gmm_egui_modlist_spike::view::text;

#[test]
fn accessible_name_is_independent_of_visible_button_label() {
    let h = Harness::new_ui(|ui| {
        ui.button("×").accessible_name("Close Mod details");
    });
    let node = h.get_by_role_and_label(Role::Button, "Close Mod details");
    assert!(!node.accesskit_node().is_disabled());
    assert!(h.query_by_role_and_label(Role::Button, "×").is_none());
}

#[test]
fn plain_label_is_not_status_but_explicit_status_and_alert_are_readable() {
    let h = Harness::new_ui(|ui| {
        ui.label("Checking Conflicts…");
    });
    assert!(h.query_by_role(Role::Status).is_none());
    let h = Harness::new_ui(|ui| {
        text(ui, Role::Status, "Checking Conflicts… 3 of 3 completed.");
        text(ui, Role::Alert, "Stop request unverified");
    });
    let status = h.get_by_role_and_label(Role::Status, "Checking Conflicts… 3 of 3 completed.");
    assert_eq!(
        status.value().as_deref(),
        Some("Checking Conflicts… 3 of 3 completed.")
    );
    assert_eq!(status.accesskit_node().live(), Live::Polite);
    assert!(status.accesskit_node().is_live_atomic());
    assert_eq!(
        h.get_by_role_and_label(Role::Alert, "Stop request unverified")
            .accesskit_node()
            .live(),
        Live::Assertive
    );
}

#[test]
fn basic_text_input_accepts_text_through_named_accesskit_node() {
    let mut h = Harness::new_ui_state(
        |ui, value| {
            ui.text_edit_singleline(value).accessible_name("Mod filter");
        },
        String::new(),
    );
    h.get_by_role_and_label(Role::TextInput, "Mod filter")
        .focus();
    h.run();
    h.get_by_role_and_label(Role::TextInput, "Mod filter")
        .type_text("First Mod");
    h.run();
    assert_eq!(h.state(), "First Mod");
    assert_eq!(
        h.get_by_role_and_label(Role::TextInput, "Mod filter")
            .value()
            .as_deref(),
        Some("First Mod")
    );
}

#[test]
fn show_rows_only_exposes_the_rendered_slice_to_accesskit() {
    let mut h = Harness::builder()
        .with_size(vec2(300.0, 120.0))
        .build_ui(|ui| {
            egui::ScrollArea::vertical().show_rows(ui, 20.0, 10_000, |ui, range| {
                for i in range {
                    let _ = ui.button(format!("Mod {i}"));
                }
            });
        });
    h.run();
    h.get_by_role_and_label(Role::Button, "Mod 0");
    assert!(
        h.query_by_role_and_label(Role::Button, "Mod 9999")
            .is_none()
    );
    assert!(h.query_all_by_role(Role::Button).count() < 20);
}
