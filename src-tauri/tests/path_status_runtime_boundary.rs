//! The command delegates to the same Core path exercised by game_path_status.
//! Keep that small path fail-closed: only the named database read, worker spawn,
//! inspection and join-error mapping are permitted calls. Unknown helpers and
//! aliases need an explicit review here; a retained spawn cannot hide an extra
//! inline inspection. This is source proof, not a physical drive experiment.

use syn::visit::Visit;

fn path_name(expression: &syn::Expr) -> Vec<String> {
    match expression {
        syn::Expr::Path(path) => path
            .path
            .segments
            .iter()
            .map(|part| part.ident.to_string())
            .collect(),
        _ => Vec::new(),
    }
}

#[derive(Default)]
struct InspectionBoundary {
    inside_blocking: bool,
    blocking_calls: usize,
    offloaded_inspections: usize,
    database_reads: usize,
    errors: Vec<String>,
}

impl<'ast> Visit<'ast> for InspectionBoundary {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        let path = path_name(&call.func);
        if path == ["tokio", "task", "spawn_blocking"] {
            self.blocking_calls += 1;
            if call.args.len() != 1 || !matches!(call.args.first(), Some(syn::Expr::Closure(_))) {
                self.errors
                    .push("blocking worker must take one explicit closure".into());
                return;
            }
            let previous = self.inside_blocking;
            self.inside_blocking = true;
            self.visit_expr(call.args.first().unwrap());
            self.inside_blocking = previous;
            return;
        }
        if path == ["game_path", "inspect"] {
            if self.inside_blocking {
                self.offloaded_inspections += 1;
            } else {
                self.errors.push("inline inspection on async worker".into());
            }
        } else if path == ["updates", "importer_install_record"]
            || path == ["updates", "importer_pinned"]
        {
            self.database_reads += 1;
        } else if path != ["Error", "PathInspectionTask"] {
            self.errors
                .push(format!("unreviewed call: {}", path.join("::")));
        }
        syn::visit::visit_expr_call(self, call);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        match call.method.to_string().as_str() {
            "game_install_path" if path_name(&call.receiver) == ["self"] => {
                self.database_reads += 1
            }
            "map_err" => {}
            name => self.errors.push(format!("unreviewed method: {name}")),
        }
        syn::visit::visit_expr_method_call(self, call);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        // Only the fixed join-error message is allowed. An arbitrary macro can
        // otherwise conceal filesystem calls from a source visitor.
        let expected: syn::ExprMacro =
            syn::parse_quote!(format!("path inspection task join error: {error}"));
        if mac
            .path
            .segments
            .iter()
            .map(|part| part.ident.to_string())
            .collect::<Vec<_>>()
            != ["format"]
            || mac.tokens.to_string() != expected.mac.tokens.to_string()
        {
            self.errors
                .push("unreviewed macro in path-state boundary".into());
        }
    }
}

fn inspect_boundary(block: &syn::Block) -> InspectionBoundary {
    let mut boundary = InspectionBoundary::default();
    boundary.visit_block(block);
    boundary
}

#[test]
fn settings_path_status_inspection_runs_inside_spawn_blocking() {
    let file = syn::parse_file(include_str!("../src/core/mod.rs")).expect("parse Core");
    let methods = file
        .items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Impl(implementation)
                if implementation.trait_.is_none()
                    && matches!(implementation.self_ty.as_ref(), syn::Type::Path(path)
                        if path.qself.is_none() && path.path.is_ident("Core")) =>
            {
                Some(implementation)
            }
            _ => None,
        })
        .flat_map(|implementation| &implementation.items)
        .filter_map(|item| match item {
            syn::ImplItem::Fn(method) if method.sig.ident == "game_path_status" => Some(method),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        methods.len(),
        1,
        "one unambiguous inherent Core::game_path_status"
    );
    let method = methods[0];
    let boundary = inspect_boundary(&method.block);
    assert!(
        boundary.errors.is_empty(),
        "unsafe path-state boundary: {:?}",
        boundary.errors
    );
    assert_eq!(
        boundary.database_reads, 3,
        "read the saved path, local receipt and pin once each"
    );
    assert_eq!(
        boundary.blocking_calls, 1,
        "path check must use one blocking worker"
    );
    assert_eq!(
        boundary.offloaded_inspections, 1,
        "actual inspection must run inside the blocking closure"
    );
}

#[test]
fn command_delegates_once_to_the_tested_core_boundary() {
    let file = syn::parse_file(include_str!("../src/commands.rs")).expect("parse commands");
    let command = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == "get_game_path_status" => {
                Some(function)
            }
            _ => None,
        })
        .expect("get_game_path_status command");
    // The command is deliberately only a typed delegation. This checks its AST,
    // not formatting, and fails on inline helpers, aliases or extra method calls.
    assert_eq!(
        command.block.stmts.len(),
        1,
        "command must contain only delegation"
    );
    let Some(syn::Stmt::Expr(syn::Expr::MethodCall(map_error), None)) = command.block.stmts.first()
    else {
        panic!("command must return mapped Core result");
    };
    assert_eq!(map_error.method, "map_err");
    assert_eq!(map_error.args.len(), 1);
    assert_eq!(
        path_name(map_error.args.first().unwrap()),
        ["CommandError", "from"]
    );
    let syn::Expr::Await(awaited) = map_error.receiver.as_ref() else {
        panic!("await Core result");
    };
    let syn::Expr::MethodCall(delegation) = awaited.base.as_ref() else {
        panic!("delegate to Core");
    };
    assert_eq!(delegation.method, "game_path_status");
    assert_eq!(path_name(&delegation.receiver), ["core"]);
    assert_eq!(delegation.args.len(), 1);
    assert_eq!(path_name(delegation.args.first().unwrap()), ["game"]);
}

#[test]
fn gate_rejects_inline_method_and_aliased_inspection_beside_a_worker() {
    let inline_method: syn::Block = syn::parse_quote!({
        tokio::task::spawn_blocking(move || game_path::inspect(game, saved_path)).await;
        core.game_path_status(game).await;
    });
    let aliased: syn::Block = syn::parse_quote!({
        use game_path::inspect as check;
        tokio::task::spawn_blocking(move || game_path::inspect(game, saved_path)).await;
        check(game, saved_path);
    });
    for block in [&inline_method, &aliased] {
        assert!(
            !inspect_boundary(block).errors.is_empty(),
            "extra inline inspection must fail closed"
        );
    }
}

/// Count direct calls and calls through explicit imports of the private helper.
/// Source aliases are collected conservatively per file, including cfg branches.
/// This does not resolve arbitrary re-exports or generated Rust; review still
/// owns those forms. The Core boundary itself rejects unreviewed macros/calls.
#[derive(Default)]
struct InspectorCalls {
    functions: std::collections::HashSet<String>,
    modules: std::collections::HashSet<String>,
    calls: usize,
}

impl InspectorCalls {
    fn import(&mut self, tree: &syn::UseTree, prefix: Vec<String>) {
        match tree {
            syn::UseTree::Path(path) => {
                let mut prefix = prefix;
                prefix.push(path.ident.to_string());
                self.import(&path.tree, prefix);
            }
            syn::UseTree::Group(group) => {
                for item in &group.items {
                    self.import(item, prefix.clone());
                }
            }
            syn::UseTree::Name(name) => {
                self.binding(prefix, name.ident.to_string(), name.ident.to_string())
            }
            syn::UseTree::Rename(rename) => {
                self.binding(prefix, rename.ident.to_string(), rename.rename.to_string())
            }
            syn::UseTree::Glob(_) => {
                assert!(
                    !prefix.ends_with(&["game_path".into()]),
                    "inspector wildcard import needs review"
                );
            }
        }
    }

    fn binding(&mut self, mut prefix: Vec<String>, name: String, local: String) {
        if name != "self" {
            prefix.push(name);
        }
        if prefix.ends_with(&["game_path".into(), "inspect".into()]) {
            self.functions.insert(local);
        } else if prefix.last().is_some_and(|part| part == "game_path") {
            self.modules.insert(local);
        }
    }
}

struct InspectorImports<'a>(&'a mut InspectorCalls);
impl<'ast> Visit<'ast> for InspectorImports<'_> {
    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        self.0.import(&item.tree, Vec::new());
    }
}

impl<'ast> Visit<'ast> for InspectorCalls {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        let path = path_name(&call.func);
        let direct = path.ends_with(&["game_path".into(), "inspect".into()]);
        let imported = path.len() == 1 && self.functions.contains(&path[0]);
        let module_alias = path.len() >= 2
            && path.last().is_some_and(|part| part == "inspect")
            && self.modules.contains(&path[path.len() - 2]);
        if direct || imported || module_alias {
            self.calls += 1;
        }
        syn::visit::visit_expr_call(self, call);
    }
}

#[test]
fn private_path_inspector_has_one_source_call_site_crate_wide() {
    fn count_tree(directory: &std::path::Path) -> usize {
        std::fs::read_dir(directory)
            .expect("read production source directory")
            .map(|entry| {
                let path = entry.expect("source entry").path();
                if path.is_dir() {
                    return count_tree(&path);
                }
                if path.extension().is_none_or(|extension| extension != "rs") {
                    return 0;
                }
                let source = std::fs::read_to_string(&path).expect("read Rust source");
                let file = syn::parse_file(&source).expect("parse Rust source");
                let mut inspector = InspectorCalls::default();
                InspectorImports(&mut inspector).visit_file(&file);
                inspector.visit_file(&file);
                inspector.calls
            })
            .sum()
    }
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    assert_eq!(
        count_tree(&source),
        1,
        "private inspection belongs only in Core's blocking worker"
    );
}

#[test]
fn install_receipt_reader_is_only_a_local_sqlite_read_and_decode() {
    struct ReceiptBoundary;
    impl<'ast> Visit<'ast> for ReceiptBoundary {
        fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
            let path = path_name(&call.func).join("::");
            assert!(
                [
                    "sqlx::query_as",
                    "keys::importer_installed",
                    "super::importer_origin::keys::installed_origin",
                    "super::importer_origin::InstalledOrigin::decode",
                    "Ok",
                    "Some",
                ]
                .contains(&path.as_str()),
                "unreviewed receipt call: {path}"
            );
            if path == "sqlx::query_as" {
                let Some(syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(sql),
                    ..
                })) = call.args.first()
                else {
                    panic!("receipt query must be a fixed read");
                };
                assert_eq!(sql.value().split_whitespace().collect::<Vec<_>>().join(" "),
                    "SELECT (SELECT value FROM settings WHERE key = ?), (SELECT value FROM settings WHERE key = ?)");
            }
            syn::visit::visit_expr_call(self, call);
        }
        fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
            assert!(
                ["bind", "fetch_one", "is_none"].contains(&call.method.to_string().as_str()),
                "unreviewed receipt method: {}",
                call.method
            );
            syn::visit::visit_expr_method_call(self, call);
        }
        fn visit_macro(&mut self, _: &'ast syn::Macro) {
            panic!("unreviewed receipt macro");
        }
    }
    let file = syn::parse_file(include_str!("../src/core/updates.rs")).unwrap();
    let reader = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == "importer_install_record" => {
                Some(function)
            }
            _ => None,
        })
        .unwrap();
    ReceiptBoundary.visit_block(&reader.block);
}
