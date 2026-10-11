//! Source proof that the production Tauri path-state command offloads inspection.
//! Filesystem outcomes are exercised by game_path_status; this gate does not
//! claim to reproduce an unavailable Windows drive on the test host.

use syn::visit::Visit;

#[derive(Default)]
struct InspectionBoundary {
    inside_blocking: bool,
    blocking_calls: usize,
    offloaded_inspections: usize,
    inline_inspections: usize,
}

impl<'ast> Visit<'ast> for InspectionBoundary {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        let path = match call.func.as_ref() {
            syn::Expr::Path(path) => path
                .path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        if path == ["tokio", "task", "spawn_blocking"] {
            self.blocking_calls += 1;
            let previous = self.inside_blocking;
            self.inside_blocking = true;
            syn::visit::visit_expr_call(self, call);
            self.inside_blocking = previous;
            return;
        }
        if path == ["crate", "core", "game_path", "inspect"] {
            if self.inside_blocking {
                self.offloaded_inspections += 1;
            } else {
                self.inline_inspections += 1;
            }
        }
        syn::visit::visit_expr_call(self, call);
    }
}

#[test]
fn settings_path_status_inspection_runs_inside_spawn_blocking() {
    let source = include_str!("../src/commands.rs");
    let file = syn::parse_file(source).expect("parse production commands");
    let command = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == "get_game_path_status" => {
                Some(function)
            }
            _ => None,
        })
        .expect("production get_game_path_status command");
    let mut boundary = InspectionBoundary::default();
    boundary.visit_block(&command.block);
    assert_eq!(
        boundary.blocking_calls, 1,
        "path check must use one blocking worker"
    );
    assert_eq!(
        boundary.offloaded_inspections, 1,
        "actual path inspection must run in the blocking closure"
    );
    assert_eq!(
        boundary.inline_inspections, 0,
        "no synchronous inspection on async workers"
    );
}
