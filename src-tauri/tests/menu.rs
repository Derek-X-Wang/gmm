//! Guards on the native File/Help menu and the window it lives in (#351).
//!
//! `build_menu` itself cannot be exercised here: every `muda` constructor
//! runs through `run_main_thread!`, and `tauri::test::mock_app` never runs
//! an event loop for that hop to land on (on macOS `muda::Menu::new` also
//! panics outright off the main thread). `src/menu.rs` therefore keeps the
//! layout in a declaration, `menu_spec`, and builds from it without adding
//! a label, id or ordering of its own — so the declaration plus the click
//! routing is the whole observable contract, and that is what these tests
//! pin. Whether Windows actually paints this menu is Windows-only evidence.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gmm_lib::menu::{
    self, about_metadata, forward_menu_event, menu_action, menu_spec, Entry, MenuSpec,
    MENU_ACTION_EVENT,
};
use serde_json::Value;
use tauri::test::{mock_app, MockRuntime};
use tauri::{App, Listener};

/// The context the shipped binary is built with. `generate_context!` may be
/// expanded only once per crate, so every test goes through this.
fn compiled_context() -> tauri::Context<MockRuntime> {
    tauri::generate_context!()
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn tauri_conf() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
    let raw =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&raw).expect("tauri.conf.json must be valid JSON")
}

fn main_window() -> Value {
    tauri_conf()["app"]["windows"][0].clone()
}

/// Records every payload emitted on the menu channel, in order.
#[derive(Clone, Default)]
struct ActionLog {
    actions: Arc<Mutex<Vec<String>>>,
}

impl ActionLog {
    fn attach(app: &App<MockRuntime>) -> Self {
        let log = Self::default();
        let sink = log.actions.clone();
        app.listen(MENU_ACTION_EVENT, move |event| {
            // Payloads arrive as JSON; a plain action id is a JSON string.
            let payload: String =
                serde_json::from_str(event.payload()).expect("menu payload must be a JSON string");
            sink.lock().expect("action log poisoned").push(payload);
        });
        log
    }

    fn actions(&self) -> Vec<String> {
        self.actions.lock().expect("action log poisoned").clone()
    }
}

/// Every action id the menu declares, in declaration order.
fn declared_actions() -> Vec<&'static str> {
    menu_spec()
        .into_iter()
        .flat_map(|spec| spec.entries)
        .filter_map(|entry| match entry {
            Entry::Action { id, .. } => Some(id),
            _ => None,
        })
        .collect()
}

#[test]
fn file_and_help_carry_exactly_the_decided_layout() {
    // The maintainer decided this layout. It is reproduced literally so a
    // renamed, reordered, added or dropped item fails here and has to be
    // re-decided rather than drifting.
    assert_eq!(
        menu_spec(),
        vec![
            MenuSpec {
                label: "&File",
                entries: vec![
                    Entry::Action {
                        id: "settings",
                        label: "Settings…",
                    },
                    Entry::Separator,
                    Entry::Quit { label: "Exit" },
                ],
            },
            MenuSpec {
                label: "&Help",
                entries: vec![
                    Entry::Action {
                        id: "check-updates",
                        label: "Check for updates…",
                    },
                    Entry::Action {
                        id: "run-setup",
                        label: "Run setup again",
                    },
                    Entry::Action {
                        id: "diagnostics",
                        label: "Diagnostics…",
                    },
                    Entry::Separator,
                    Entry::About { label: "About GMM" },
                ],
            },
        ],
    );
}

#[test]
fn every_menu_label_carries_an_alt_mnemonic_and_no_accelerator() {
    // Alt mnemonics and accelerators are both unreliable on Windows under
    // Tauri 2 (wry#451). The decision was: keep `&` in the top-level labels
    // so Windows underlines a letter, and declare no accelerator at all.
    for spec in menu_spec() {
        assert!(
            spec.label.contains('&'),
            "top-level menu {:?} must carry an `&` mnemonic",
            spec.label,
        );
    }
    let source = std::fs::read_to_string(repo_root().join("src-tauri/src/menu.rs"))
        .expect("read src/menu.rs");
    assert!(
        !source.contains("CmdOrControl") && !source.contains("Alt+"),
        "no accelerator may be declared: WebView2 swallows the key press \
         before the host window sees it, so a declared shortcut would be a \
         label for behaviour that does not happen",
    );
}

#[test]
fn every_declared_action_has_a_route_to_the_frontend() {
    let actions = declared_actions();
    assert!(
        !actions.is_empty(),
        "the menu must declare at least one action, or nothing reaches React",
    );
    assert_eq!(
        actions.iter().collect::<BTreeSet<_>>().len(),
        actions.len(),
        "two menu items sharing an id would make one of them unroutable: {actions:?}",
    );
    for id in &actions {
        assert_eq!(
            menu_action(id),
            Some(*id),
            "menu item {id:?} is declared but `menu_action` does not route it, \
             so clicking it would do nothing",
        );
    }
}

#[test]
fn predefined_items_and_unknown_ids_are_not_forwarded() {
    // Exit and About are handled by the OS. Forwarding them would make React
    // open Settings on a Quit click.
    for entry in menu_spec().into_iter().flat_map(|spec| spec.entries) {
        let label = match entry {
            Entry::Quit { label } | Entry::About { label } => label,
            _ => continue,
        };
        assert_eq!(
            menu_action(label),
            None,
            "the OS-handled item {label:?} must not become a frontend action",
        );
    }
    assert_eq!(menu_action("settings-"), None);
    assert_eq!(menu_action(""), None);
    assert_eq!(menu_action("reset-library"), None);
}

#[test]
fn clicking_an_action_puts_its_id_on_the_menu_channel() {
    let app = mock_app();
    let log = ActionLog::attach(&app);
    let handle = app.handle().clone();

    let declared = declared_actions();
    for id in &declared {
        forward_menu_event(&handle, id);
    }
    assert_eq!(
        log.actions(),
        declared.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
        "every declared menu item must reach the frontend as its own id",
    );

    // An id from outside the menu must not put anything on the channel.
    forward_menu_event(&handle, "not-a-menu-item");
    assert_eq!(
        log.actions().len(),
        declared.len(),
        "an unknown menu id must not be forwarded",
    );
}

#[test]
fn the_frontend_listens_on_the_same_channel_and_ids() {
    // Nothing in either language checks the other, so a rename on one side
    // would leave a menu item that clicks into nowhere.
    let api = std::fs::read_to_string(repo_root().join("src/api.ts")).expect("read src/api.ts");
    assert!(
        api.contains(&format!("\"{MENU_ACTION_EVENT}\"")),
        "src/api.ts must declare the {MENU_ACTION_EVENT:?} channel",
    );
    for id in declared_actions() {
        assert!(
            api.contains(&format!("\"{id}\"")),
            "src/api.ts must declare the {id:?} menu action",
        );
    }
    // And no extra TypeScript action that Rust never emits.
    let listed = api
        .split("export const MENU_ACTIONS")
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .expect("src/api.ts must export a MENU_ACTIONS list");
    assert_eq!(
        listed.matches('"').count() / 2,
        declared_actions().len(),
        "MENU_ACTIONS lists a different number of actions than the menu \
         declares; one side would handle an action nothing sends: {listed}",
    );
}

#[test]
fn about_reports_the_running_compiled_version() {
    let app = tauri::test::mock_builder()
        .build(compiled_context())
        .expect("app with the compiled context");
    let expected = app.package_info().version.to_string();
    let metadata = about_metadata(app.handle());
    assert_eq!(
        metadata.version.as_deref(),
        Some(expected.as_str()),
        "About must report the running build's version",
    );
    assert_eq!(
        metadata.name.as_deref(),
        Some(app.package_info().name.as_str())
    );

    // A different build must produce a different About version. This is the
    // assertion a baked-in literal fails: it would keep reporting `expected`.
    let mut context = compiled_context();
    context.package_info_mut().version.patch += 1;
    let changed = tauri::test::mock_builder()
        .build(context)
        .expect("changed build");
    let changed_version = changed.package_info().version.to_string();
    assert_ne!(changed_version, expected, "the fixture must differ");
    assert_eq!(
        about_metadata(changed.handle()).version.as_deref(),
        Some(changed_version.as_str()),
        "About must follow the compiled package version, not a literal",
    );

    // Windows ignores both of these; setting them would claim content the
    // dialog never shows.
    assert!(metadata.icon.is_none());
    assert!(metadata.credits.is_none());
}

#[test]
fn the_menu_channel_is_namespaced_so_it_cannot_collide_with_a_domain_event() {
    assert_eq!(menu::MENU_ACTION_EVENT, "menu://action");
}

#[test]
fn the_window_opens_at_the_decided_size_and_refuses_to_shrink_below_the_minimum() {
    let window = main_window();
    assert_eq!(
        (window["width"].as_u64(), window["height"].as_u64()),
        (Some(1100), Some(760)),
        "the decided default leaves room for the Settings dialog with the \
         main screen still visible behind it",
    );
    assert_eq!(
        (window["minWidth"].as_u64(), window["minHeight"].as_u64()),
        (Some(900), Some(600)),
        "without a minimum the user can shrink the window until the dialog \
         fills it and stops reading as a dialog",
    );
    let (width, height) = (
        window["width"].as_u64().expect("width"),
        window["height"].as_u64().expect("height"),
    );
    let (min_width, min_height) = (
        window["minWidth"].as_u64().expect("minWidth"),
        window["minHeight"].as_u64().expect("minHeight"),
    );
    assert!(
        min_width <= width && min_height <= height,
        "a minimum larger than the default would make the window open \
         already resized",
    );
}
