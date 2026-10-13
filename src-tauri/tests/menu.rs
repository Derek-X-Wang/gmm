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
//!
//! That "without adding anything of its own" is a property, not a promise:
//! an item appended straight into the builder would paint on Windows while
//! the layout test below went on pinning a declaration the real menu no
//! longer matched. Since no behavioural test can reach `build_menu`, the two
//! guards at the end of this file read its source instead, as
//! `ipc_contract.rs` reads both sides of the IPC boundary.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gmm_lib::menu::{
    self, about_metadata, forward_menu_event, menu_action, menu_spec, Entry, MenuSpec,
    MENU_ACTION_EVENT,
};
use proc_macro2::{TokenStream, TokenTree};
use serde_json::Value;
use syn::visit::{self, Visit};
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

// ---- the builder may not add anything the declaration does not describe ----

/// Menu constructors that may appear in `build_menu` *outside* the `match`
/// over [`Entry`]. `Menu::new` takes no text at all, and `Submenu::new` takes
/// its label from the spec being iterated, which
/// [`only_app_bools_and_spec_fields`] checks. Every other constructor would
/// put an item in the menu that the declaration does not describe.
const CONSTRUCTORS_ALLOWED_OUTSIDE_THE_MATCH: [&str; 2] = ["Menu::new", "Submenu::new"];

/// Types whose constructors put something into a menu.
const MENU_TYPES: [&str; 9] = [
    "Menu",
    "Submenu",
    "MenuItem",
    "PredefinedMenuItem",
    "CheckMenuItem",
    "IconMenuItem",
    "MenuBuilder",
    "SubmenuBuilder",
    "MenuItemBuilder",
];

fn build_menu_body() -> syn::Block {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/menu.rs");
    let source =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let parsed = syn::parse_file(&source).expect("src/menu.rs must be parseable Rust");
    let function = parsed
        .items
        .into_iter()
        .find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == "build_menu" => Some(function),
            _ => None,
        })
        .expect("src/menu.rs must define `build_menu`");
    // The body only: the function's own doc comment is a string literal too.
    *function.block
}

/// `A::b` for a call like `A::b(..)`, or `None` when the callee is not a path.
fn callee_path(call: &syn::ExprCall) -> Option<String> {
    let syn::Expr::Path(path) = &*call.func else {
        return None;
    };
    Some(
        path.path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>()
            .join("::"),
    )
}

fn is_menu_construction(path: &str) -> bool {
    path.split("::")
        .next()
        .is_some_and(|ty| MENU_TYPES.contains(&ty))
}

/// `true` when this `match` dispatches on the loop's `Entry` value.
fn is_entry_match(node: &syn::ExprMatch) -> bool {
    matches!(&*node.expr, syn::Expr::Path(path) if path.path.is_ident("entry"))
}

/// Every string literal in a token stream, macro bodies included, so a label
/// cannot be smuggled in through `concat!` or `stringify!`.
fn literals_in_tokens(tokens: TokenStream, found: &mut Vec<String>) {
    for token in tokens {
        match token {
            TokenTree::Group(group) => literals_in_tokens(group.stream(), found),
            TokenTree::Literal(literal) => {
                let text = literal.to_string();
                if text.starts_with('"') || text.starts_with("r\"") || text.starts_with("r#") {
                    found.push(text);
                }
            }
            _ => {}
        }
    }
}

/// What `build_menu` does, treating the `Entry` match as a closed box:
/// constructions inside it answer an `Entry` the declaration lists, and
/// `file_and_help_carry_exactly_the_decided_layout` already pins that list.
#[derive(Default)]
struct BuilderAudit {
    /// Every string literal in the body, match arms and macros included.
    literals: Vec<String>,
    /// Menu constructions reached without passing through the `Entry` match.
    outside_constructions: Vec<String>,
    /// Arguments to those constructions that are neither `app`, a bool, nor a
    /// field of the spec being iterated.
    foreign_arguments: Vec<String>,
    entry_matches: usize,
    menu_spec_calls: usize,
}

impl<'ast> Visit<'ast> for BuilderAudit {
    fn visit_lit_str(&mut self, node: &'ast syn::LitStr) {
        self.literals.push(node.value());
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        literals_in_tokens(node.tokens.clone(), &mut self.literals);
        visit::visit_macro(self, node);
    }

    fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
        if !is_entry_match(node) {
            visit::visit_expr_match(self, node);
            return;
        }
        self.entry_matches += 1;
        // Descend for literals only. A label typed into an arm is still a
        // label `menu_spec` never declared.
        let mut inner = LiteralsOnly::default();
        inner.visit_expr_match(node);
        self.literals.append(&mut inner.literals);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let Some(path) = callee_path(node) {
            if path == "menu_spec" {
                self.menu_spec_calls += 1;
            }
            if is_menu_construction(&path) {
                if CONSTRUCTORS_ALLOWED_OUTSIDE_THE_MATCH.contains(&path.as_str()) {
                    self.foreign_arguments.extend(
                        node.args
                            .iter()
                            .filter(|argument| !only_app_bools_and_spec_fields(argument))
                            .map(|argument| format!("{path} got {}", describe(argument))),
                    );
                } else {
                    self.outside_constructions.push(path);
                }
            }
        }
        visit::visit_expr_call(self, node);
    }
}

#[derive(Default)]
struct LiteralsOnly {
    literals: Vec<String>,
}

impl<'ast> Visit<'ast> for LiteralsOnly {
    fn visit_lit_str(&mut self, node: &'ast syn::LitStr) {
        self.literals.push(node.value());
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        literals_in_tokens(node.tokens.clone(), &mut self.literals);
        visit::visit_macro(self, node);
    }
}

/// `app`, a bool, or `spec.<field>` — the only things a constructor outside
/// the match may be handed, so any text it shows came from the declaration.
fn only_app_bools_and_spec_fields(argument: &syn::Expr) -> bool {
    match argument {
        syn::Expr::Reference(reference) => only_app_bools_and_spec_fields(&reference.expr),
        syn::Expr::Lit(literal) => matches!(literal.lit, syn::Lit::Bool(_)),
        syn::Expr::Path(path) => path.path.is_ident("app"),
        syn::Expr::Field(field) => {
            matches!(&*field.base, syn::Expr::Path(base) if base.path.is_ident("spec"))
        }
        _ => false,
    }
}

fn describe(expr: &syn::Expr) -> String {
    match expr {
        syn::Expr::Lit(_) => "a literal".to_string(),
        syn::Expr::Path(path) => format!(
            "the path `{}`",
            path.path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>()
                .join("::"),
        ),
        syn::Expr::Call(call) => format!(
            "a call to `{}`",
            callee_path(call).unwrap_or_else(|| "?".to_string()),
        ),
        syn::Expr::MethodCall(call) => format!("a call to `.{}()`", call.method),
        syn::Expr::Macro(node) => format!(
            "the macro `{}!`",
            node.mac
                .path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>()
                .join("::"),
        ),
        syn::Expr::Field(field) => match &field.member {
            syn::Member::Named(name) => format!("a field `.{name}` of something other than `spec`"),
            syn::Member::Unnamed(index) => format!("a field `.{}`", index.index),
        },
        _ => "an expression this guard does not recognise".to_string(),
    }
}

/// `build_menu` never runs in a test — every `muda` constructor goes through
/// `run_main_thread!`, which the mock runtime does not service — so no
/// behavioural test can see an item appended straight into the builder.
/// `file_and_help_carry_exactly_the_decided_layout` would keep passing beside
/// such an item: the decided layout would be pinned while the real menu on
/// Windows showed something else. This reads the builder's source instead, as
/// `ipc_contract.rs` reads both sides of the IPC boundary and
/// `witness_trust_boundary.rs` reads SQL ownership.
///
/// It constrains the builder, not the menu: changing `menu_spec` is still a
/// one-place edit. What it does not claim to prove is text reaching a
/// constructor *inside* the match through a helper function or a runtime
/// value; the arms destructure `Entry`, so that would need review at this
/// file's boundary.
#[test]
fn build_menu_can_only_render_what_menu_spec_declares() {
    let mut audit = BuilderAudit::default();
    audit.visit_block(&build_menu_body());

    assert_eq!(
        audit.entry_matches, 1,
        "`build_menu` must dispatch on exactly one `match entry`. Without it \
         items would be built inline and `menu_spec` would describe nothing",
    );
    assert_eq!(
        audit.menu_spec_calls, 1,
        "`build_menu` must take its entries from exactly one `menu_spec()` call",
    );
    assert_eq!(
        audit.literals,
        Vec::<String>::new(),
        "`build_menu` must contain no string literal. A label or id written \
         here would paint in the real menu while `menu_spec` — the list the \
         maintainer decided, and the only list the layout test pins — never \
         mentions it. This includes text inside a macro, so a log line \
         belongs outside this function",
    );
    assert_eq!(
        audit.outside_constructions,
        Vec::<String>::new(),
        "every menu item must be constructed inside the `match entry`, so \
         each one answers an `Entry` the declaration lists. Allowed outside: \
         {CONSTRUCTORS_ALLOWED_OUTSIDE_THE_MATCH:?}",
    );
    assert_eq!(
        audit.foreign_arguments,
        Vec::<String>::new(),
        "a constructor outside the `match entry` may only be handed `app`, a \
         bool, or a field of the spec being iterated. Anything else is text \
         from outside the declaration",
    );
}

/// Functions in `src/menu.rs` that are allowed to contain text.
///
/// `menu_spec` is the declaration itself. `forward_menu_event` logs a failed
/// emit. `about_metadata` reads the crate manifest through `env!`. Nothing
/// else in the file has any business holding a string: a label or id typed
/// into a helper would reach the menu without passing through the
/// declaration, and [`build_menu_can_only_render_what_menu_spec_declares`]
/// only inspects `build_menu` itself.
const FUNCTIONS_ALLOWED_TO_HOLD_TEXT: [&str; 3] =
    ["menu_spec", "forward_menu_event", "about_metadata"];

/// Every `fn` body in the file, paired with its name. Bodies only — a doc
/// comment is a string literal too, and those are documentation, not labels.
fn function_bodies(file: &syn::File) -> Vec<(String, syn::Block)> {
    let mut out = Vec::new();
    let mut items: Vec<syn::Item> = file.items.clone();
    while let Some(item) = items.pop() {
        match item {
            syn::Item::Fn(function) => {
                out.push((function.sig.ident.to_string(), *function.block));
            }
            syn::Item::Impl(block) => {
                for inner in block.items {
                    if let syn::ImplItem::Fn(function) = inner {
                        out.push((function.sig.ident.to_string(), function.block));
                    }
                }
            }
            syn::Item::Mod(module) => {
                if let Some((_, inner)) = module.content {
                    items.extend(inner);
                }
            }
            _ => {}
        }
    }
    out
}

/// The companion to the builder guard: text may only live where the
/// declaration lives. Without this, `build_menu` could stay literal-free and
/// still append `&smuggled_item(app)?`, with the label sitting in a helper
/// the other guard never reads.
///
/// What this still does not claim to prove is a label assembled from runtime
/// values rather than written down anywhere. That would need review at this
/// file's boundary, as `witness_trust_boundary.rs` says of its own limits.
#[test]
fn no_menu_text_lives_outside_the_declaration() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/menu.rs");
    let source =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let file = syn::parse_file(&source).expect("src/menu.rs must be parseable Rust");

    let bodies = function_bodies(&file);
    for name in FUNCTIONS_ALLOWED_TO_HOLD_TEXT {
        assert!(
            bodies.iter().any(|(found, _)| found == name),
            "`{name}` is on the text allowlist but no longer exists; drop it \
             from FUNCTIONS_ALLOWED_TO_HOLD_TEXT rather than leaving the \
             allowlist describing a function that is gone",
        );
    }

    let mut offenders: Vec<String> = Vec::new();
    for (name, body) in &bodies {
        if FUNCTIONS_ALLOWED_TO_HOLD_TEXT.contains(&name.as_str()) {
            continue;
        }
        let mut literals = LiteralsOnly::default();
        literals.visit_block(body);
        offenders.extend(
            literals
                .literals
                .into_iter()
                .map(|literal| format!("{name}: {literal:?}")),
        );
    }
    assert_eq!(
        offenders,
        Vec::<String>::new(),
        "only {FUNCTIONS_ALLOWED_TO_HOLD_TEXT:?} may hold text in \
         src/menu.rs. A label or id anywhere else could reach the menu \
         without appearing in `menu_spec`, which is the list the maintainer \
         decided and the only list the layout test pins",
    );
}
