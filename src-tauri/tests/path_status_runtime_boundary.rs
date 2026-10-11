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
    let method = file
        .items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Impl(implementation) => Some(implementation),
            _ => None,
        })
        .flat_map(|implementation| &implementation.items)
        .find_map(|item| match item {
            syn::ImplItem::Fn(method) if method.sig.ident == "game_path_status" => Some(method),
            _ => None,
        })
        .expect("production Core::game_path_status");
    let boundary = inspect_boundary(&method.block);
    assert!(
        boundary.errors.is_empty(),
        "unsafe path-state boundary: {:?}",
        boundary.errors
    );
    assert_eq!(boundary.database_reads, 1, "read the saved path once");
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
