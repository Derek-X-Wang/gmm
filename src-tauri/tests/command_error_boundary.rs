//! Structural and behavioral guards for the Tauri command-error boundary.
//!
//! The source gate deliberately covers literal `#[tauri::command]` item
//! functions under `src/`. It exactly matches that attribute and lexically
//! resolves direct `crate::command_error::CommandResult` imports (including
//! import aliases). It cannot inspect macro expansion, aliased attributes,
//! commands outside `src/`, or arbitrary type re-exports; review of the Tauri
//! registration list remains the backstop for those shapes.

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
            collect_use_bindings(&import.tree, &mut Vec::new(), &mut bindings);
        }
    }
    bindings
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
