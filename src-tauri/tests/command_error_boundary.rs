//! Structural and behavioral guards for the Tauri command-error boundary.
//!
//! The source gate deliberately covers literal `#[tauri::command]` item
//! functions under `src/` and `crates/cli/src/`. It exactly matches that
//! attribute and lexically resolves direct `crate::command_error::CommandResult` imports (including
//! import aliases). Conditional imports are rejected as binding proof because
//! another target can supply a shadow with the same name. The gate cannot
//! inspect macro expansion, aliased attributes, commands outside those roots, or
//! arbitrary type re-exports; review of the Tauri registration list remains
//! the backstop for those shapes.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use gmm_lib::command_error::CommandError;
use gmm_lib::core::error::SurfaceFailureKind;
use gmm_lib::core::Error;
use syn::visit::Visit;
use syn::{Item, ReturnType, Type, UseTree};

fn rust_sources_below(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read Rust source directory") {
        let path = entry.expect("read Rust source entry").path();
        if path.is_dir() {
            rust_sources_below(&path, found);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            found.push(path);
        }
    }
}

fn is_tauri_command(function: &syn::ItemFn) -> bool {
    function.attrs.iter().any(|attribute| {
        attribute
            .path()
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .eq(["tauri", "command"])
    })
}

fn collect_use_bindings(
    tree: &UseTree,
    prefix: &mut Vec<String>,
    shared_bindings: &mut HashSet<String>,
) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            collect_use_bindings(&path.tree, prefix, shared_bindings);
            prefix.pop();
        }
        UseTree::Name(name) => {
            let mut full_path = prefix.clone();
            full_path.push(name.ident.to_string());
            if full_path == ["crate", "command_error", "CommandResult"] {
                shared_bindings.insert(name.ident.to_string());
            }
        }
        UseTree::Rename(rename) => {
            let mut full_path = prefix.clone();
            full_path.push(rename.ident.to_string());
            if full_path == ["crate", "command_error", "CommandResult"] {
                shared_bindings.insert(rename.rename.to_string());
            }
        }
        UseTree::Group(group) => {
            for item in &group.items {
                collect_use_bindings(item, prefix, shared_bindings);
            }
        }
        UseTree::Glob(_) => {}
    }
}

fn shared_command_result_bindings(items: &[Item]) -> HashSet<String> {
    let mut bindings = HashSet::new();
    for item in items {
        if let Item::Use(import) = item {
            // A conditional import cannot establish what a one-segment alias
            // names on every target. Fail closed: commands may use an
            // unconditional shared import or the fully-qualified alias.
            if import.attrs.iter().any(|attribute| {
                matches!(
                    attribute
                        .path()
                        .segments
                        .last()
                        .map(|segment| segment.ident.to_string())
                        .as_deref(),
                    Some("cfg" | "cfg_attr")
                )
            }) {
                continue;
            }
            collect_use_bindings(&import.tree, &mut Vec::new(), &mut bindings);
        }
    }
    bindings
}

#[test]
fn cfg_gated_shared_alias_does_not_authorize_an_active_shadow() {
    let file = syn::parse_file(
        r#"
        #[cfg(not(windows))]
        use crate::command_error::CommandResult;
        #[cfg(windows)]
        type CommandResult<T> = Result<T, String>;

        #[tauri::command]
        fn windows_only() -> CommandResult<()> { todo!() }
        "#,
    )
    .expect("parse cfg-shadow fixture");

    let bindings = shared_command_result_bindings(&file.items);
    assert!(
        !bindings.contains("CommandResult"),
        "a cfg-gated import cannot prove that a one-segment command result names the shared envelope on every target"
    );
}

fn returns_shared_command_result(
    function: &syn::ItemFn,
    shared_bindings: &HashSet<String>,
) -> bool {
    let ReturnType::Type(_, result) = &function.sig.output else {
        return false;
    };
    let Type::Path(result) = result.as_ref() else {
        return false;
    };
    let segments = result
        .path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();
    segments == ["crate", "command_error", "CommandResult"]
        || (segments.len() == 1 && shared_bindings.contains(&segments[0]))
}

fn inspect_commands(
    items: &[Item],
    source_path: &Path,
    source_root: &Path,
    module_path: &mut Vec<String>,
    command_count: &mut usize,
    violations: &mut Vec<String>,
) {
    let shared_bindings = shared_command_result_bindings(items);
    for item in items {
        match item {
            Item::Fn(function) if is_tauri_command(function) => {
                *command_count += 1;
                if !returns_shared_command_result(function, &shared_bindings) {
                    let module = if module_path.is_empty() {
                        String::new()
                    } else {
                        format!("{}::", module_path.join("::"))
                    };
                    violations.push(format!(
                        "{}::{module}{}",
                        source_path
                            .strip_prefix(source_root)
                            .unwrap_or(source_path)
                            .display(),
                        function.sig.ident
                    ));
                }
            }
            Item::Mod(module) => {
                if let Some((_, nested)) = &module.content {
                    module_path.push(module.ident.to_string());
                    inspect_commands(
                        nested,
                        source_path,
                        source_root,
                        module_path,
                        command_count,
                        violations,
                    );
                    module_path.pop();
                }
            }
            _ => {}
        }
    }
}

#[test]
fn every_tauri_command_uses_structured_command_result() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    rust_sources_below(&source_root, &mut sources);
    rust_sources_below(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/cli/src"),
        &mut sources,
    );

    let mut command_count = 0;
    let mut violations = Vec::new();
    for path in sources {
        let source = fs::read_to_string(&path).expect("read Rust source");
        let file = syn::parse_file(&source).expect("parse Rust source");
        inspect_commands(
            &file.items,
            &path,
            &source_root,
            &mut Vec::new(),
            &mut command_count,
            &mut violations,
        );
    }

    assert!(
        command_count > 0,
        "the command boundary gate found no Tauri commands"
    );
    assert!(
        violations.is_empty(),
        "every #[tauri::command] must return CommandResult<T> so failures cannot silently discard their classification; violations: {}",
        violations.join(", ")
    );
}

/// Names of `#[tauri::command]` functions that convert a core failure without
/// `SurfaceErrors`, so a Mod's ULID reaches the user unaccompanied by its name
/// (#446). Two shapes are recognised:
///
/// - `CommandError::from`, called directly or passed to `map_err`.
/// - A `?` whose receiver chain starts at the identifier `core` and contains
///   no `.surface(..)`. `?` invokes the same `From<Error> for CommandError`,
///   so `core.work().await?` bypasses the resolver as surely as the first
///   shape. A `?` on a helper that already returns `CommandResult`, such as
///   `refuse_during_session(&core).await?`, does not start at `core` and
///   stays legal.
///
/// This is syntax, not type inference, and deliberately does not try to be
/// general. It cannot see `.map_err(Into::into)`, a closure that calls
/// `e.into()`, or a `?` on a chain that does not start at `core` (for example
/// a value first bound to another name). Those remain review
/// responsibilities, as the module doc lists for the `CommandResult` gate.
fn commands_bypassing_the_resolver(items: &[Item]) -> Vec<String> {
    #[derive(Default)]
    struct Bypass(bool);
    impl<'ast> Visit<'ast> for Bypass {
        fn visit_expr_path(&mut self, expression: &'ast syn::ExprPath) {
            let mut segments = expression
                .path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .rev();
            if segments.next().as_deref() == Some("from")
                && segments.next().as_deref() == Some("CommandError")
            {
                self.0 = true;
            }
            syn::visit::visit_expr_path(self, expression);
        }

        fn visit_expr_try(&mut self, expression: &'ast syn::ExprTry) {
            let mut resolved = false;
            let mut link = expression.expr.as_ref();
            let starts_at_core = loop {
                link = match link {
                    syn::Expr::MethodCall(call) => {
                        resolved |= call.method == "surface";
                        &call.receiver
                    }
                    syn::Expr::Await(awaited) => &awaited.base,
                    syn::Expr::Try(inner) => &inner.expr,
                    syn::Expr::Field(field) => &field.base,
                    syn::Expr::Paren(paren) => &paren.expr,
                    syn::Expr::Path(path) => break path.path.is_ident("core"),
                    _ => break false,
                };
            };
            if starts_at_core && !resolved {
                self.0 = true;
            }
            syn::visit::visit_expr_try(self, expression);
        }
    }

    let mut bypassing = Vec::new();
    for item in items {
        match item {
            Item::Fn(function) if is_tauri_command(function) => {
                let mut conversion = Bypass::default();
                conversion.visit_block(&function.block);
                if conversion.0 {
                    bypassing.push(function.sig.ident.to_string());
                }
            }
            Item::Mod(module) => {
                if let Some((_, nested)) = &module.content {
                    bypassing.extend(commands_bypassing_the_resolver(nested));
                }
            }
            _ => {}
        }
    }
    bypassing
}

#[test]
fn no_tauri_command_converts_a_core_error_without_the_resolver() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    rust_sources_below(&source_root, &mut sources);
    rust_sources_below(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/cli/src"),
        &mut sources,
    );

    // A gate that parses nothing passes trivially, so it must also see the
    // commands that do route through `.surface(..)`.
    let mut command_count = 0;
    let mut bypassing = Vec::new();
    for path in sources {
        let source = fs::read_to_string(&path).expect("read Rust source");
        let file = syn::parse_file(&source).expect("parse Rust source");
        let mut violations = Vec::new();
        inspect_commands(
            &file.items,
            &path,
            &source_root,
            &mut Vec::new(),
            &mut command_count,
            &mut violations,
        );
        bypassing.extend(
            commands_bypassing_the_resolver(&file.items)
                .into_iter()
                .map(|name| format!("{}::{name}", path.display())),
        );
    }

    assert!(
        command_count > 0,
        "the command boundary gate found no Tauri commands"
    );
    assert!(
        bypassing.is_empty(),
        "a #[tauri::command] body must convert core errors with `.surface(&core).await` so a \
         Mod's name is resolved beside its identifier; CommandError::from, or a bare `?` on a `core` call, skips that: {}",
        bypassing.join(", ")
    );
}

#[test]
fn the_resolver_gate_flags_a_command_that_converts_directly() {
    let file = syn::parse_file(
        r#"
        #[tauri::command]
        async fn resolves(core: State<'_, Core>) -> CommandResult<()> {
            core.work().await.surface(&core).await
        }

        #[tauri::command]
        async fn bypasses_through_map_err(core: State<'_, Core>) -> CommandResult<()> {
            core.work().await.map_err(CommandError::from)
        }

        mod nested {
            #[tauri::command]
            async fn bypasses_by_call(core: State<'_, Core>) -> CommandResult<()> {
                match core.work().await {
                    Ok(value) => Ok(value),
                    Err(error) => Err(crate::command_error::CommandError::from(error)),
                }
            }
        }

        // Not a command body: helpers such as the launch orchestration are
        // outside this rule.
        async fn helper(core: &Core) -> CommandResult<()> {
            core.work().await.map_err(CommandError::from)
        }
        "#,
    )
    .expect("parse resolver-gate fixture");

    assert_eq!(
        commands_bypassing_the_resolver(&file.items),
        ["bypasses_through_map_err", "bypasses_by_call"]
    );
}

#[test]
fn the_resolver_gate_flags_a_question_mark_on_an_unresolved_core_chain() {
    let file = syn::parse_file(
        r#"
        #[tauri::command]
        async fn resolves_before_the_question_mark(core: State<'_, Core>) -> CommandResult<Thing> {
            let value = core.work().await.surface(&core).await?;
            Ok(transform(value))
        }

        #[tauri::command]
        async fn question_mark_on_core(core: State<'_, Core>) -> CommandResult<Thing> {
            let value = core.work().await?;
            Ok(transform(value))
        }

        #[tauri::command]
        async fn resolved_early_in_a_longer_chain(core: State<'_, Core>) -> CommandResult<Thing> {
            let value = core.first().await.surface(&core).await?.second().await?;
            Ok(value)
        }

        #[tauri::command]
        async fn question_mark_deep_in_an_unresolved_chain(
            core: State<'_, Core>,
        ) -> CommandResult<Thing> {
            let value = core.first().await?.second().await?;
            Ok(value)
        }

        #[tauri::command]
        async fn question_mark_on_a_helper_that_returns_command_result(
            core: State<'_, Core>,
        ) -> CommandResult<Thing> {
            refuse_during_session(&core).await?;
            let value = core.work().await.surface(&core).await?;
            Ok(value)
        }
        "#,
    )
    .expect("parse question-mark fixture");

    assert_eq!(
        commands_bypassing_the_resolver(&file.items),
        [
            "question_mark_on_core",
            "question_mark_deep_in_an_unresolved_chain"
        ]
    );
}

#[test]
fn command_error_preserves_classification_and_message() {
    let source = Error::InvalidActiveVariant {
        mod_id: "01INTERNAL".into(),
        mod_name: "Broken Outfit".into(),
        variant_id: "01MISSING".into(),
    };
    let expected_message = source.to_string();

    let command_error = CommandError::from(source);

    assert_eq!(
        command_error.kind,
        SurfaceFailureKind::InvalidActiveVariant,
        "the command envelope must retain the core failure classification"
    );
    assert_eq!(
        command_error.message, expected_message,
        "the command envelope must retain the existing user-facing message"
    );
    assert_eq!(
        serde_json::to_value(&command_error).expect("serialize command error"),
        serde_json::json!({
            "kind": "invalidActiveVariant",
            "message": expected_message,
        }),
        "Tauri must reject the command with the frontend's structured envelope"
    );
}

#[test]
fn command_error_message_transform_preserves_classification() {
    let command_error = CommandError::from(Error::InvalidActiveVariant {
        mod_id: "01INTERNAL".into(),
        mod_name: "Broken Outfit".into(),
        variant_id: "01MISSING".into(),
    })
    .map_message(|message| format!("launch context: {message}"));

    assert_eq!(
        command_error.kind,
        SurfaceFailureKind::InvalidActiveVariant,
        "presentation wrappers must not reclassify an already-typed command failure"
    );
    assert!(
        command_error.message.starts_with("launch context: "),
        "the presentation wrapper must still transform the command failure message"
    );
}

/// This source guard proves that both launch layers return the shared alias and
/// that the thin Tauri shell does not call `CommandError::other`. The behavioral
/// `command_error_message_transform_preserves_classification` test separately
/// proves that the launch message wrapper retains an existing kind. It does not
/// prove every core call inside `runtime::launch`; none of today's launch core
/// calls can produce the only non-`Other` kind, so that requires review until a
/// classified launch failure has a real behavioral seam.
#[test]
fn launch_game_keeps_launch_failures_structured() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let launch_source = fs::read_to_string(source_root.join("runtime/launch.rs"))
        .expect("read launch orchestration source");
    let launch_file = syn::parse_file(&launch_source).expect("parse launch orchestration source");
    let launch_bindings = shared_command_result_bindings(&launch_file.items);
    let launch = launch_file
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(function) if function.sig.ident == "launch" => Some(function),
            _ => None,
        })
        .expect("find launch orchestration function");
    assert!(
        returns_shared_command_result(launch, &launch_bindings),
        "runtime::launch::launch must retain typed CommandError values until the Tauri shell"
    );

    let commands_source =
        fs::read_to_string(source_root.join("commands.rs")).expect("read commands source");
    let commands_file = syn::parse_file(&commands_source).expect("parse commands source");
    let launch_command = commands_file
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(function) if function.sig.ident == "launch_game" => Some(function),
            _ => None,
        })
        .expect("find launch_game command");
    #[derive(Default)]
    struct ReclassificationVisitor {
        found_command_error_other: bool,
    }
    impl<'ast> Visit<'ast> for ReclassificationVisitor {
        fn visit_expr_path(&mut self, expression: &'ast syn::ExprPath) {
            let segments = expression
                .path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>();
            if segments == ["CommandError", "other"] {
                self.found_command_error_other = true;
            }
            syn::visit::visit_expr_path(self, expression);
        }
    }
    let mut reclassification = ReclassificationVisitor::default();
    reclassification.visit_block(&launch_command.block);
    assert!(
        !reclassification.found_command_error_other,
        "commands.rs::launch_game must forward the structured launch failure instead of reclassifying it as Other"
    );
}
