//! Structural and behavioral guards for the Tauri command-error boundary.

use std::fs;
use std::path::{Path, PathBuf};

use gmm_lib::command_error::CommandError;
use gmm_lib::core::error::SurfaceFailureKind;
use gmm_lib::core::Error;
use syn::visit::Visit;
use syn::{ReturnType, Type};

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
            .last()
            .is_some_and(|segment| segment.ident == "command")
    })
}

fn returns_command_result(function: &syn::ItemFn) -> bool {
    let ReturnType::Type(_, result) = &function.sig.output else {
        return false;
    };
    let Type::Path(result) = result.as_ref() else {
        return false;
    };
    result
        .path
        .segments
        .last()
        .is_some_and(|segment| segment.ident == "CommandResult")
}

struct CommandVisitor<'a> {
    source_path: &'a Path,
    source_root: &'a Path,
    command_count: usize,
    violations: Vec<String>,
}

impl<'ast> Visit<'ast> for CommandVisitor<'_> {
    fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
        if is_tauri_command(function) {
            self.command_count += 1;
            if !returns_command_result(function) {
                self.violations.push(format!(
                    "{}::{}",
                    self.source_path
                        .strip_prefix(self.source_root)
                        .unwrap_or(self.source_path)
                        .display(),
                    function.sig.ident
                ));
            }
        }
        syn::visit::visit_item_fn(self, function);
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
        let mut visitor = CommandVisitor {
            source_path: &path,
            source_root: &source_root,
            command_count: 0,
            violations: Vec::new(),
        };
        visitor.visit_file(&file);
        command_count += visitor.command_count;
        violations.extend(visitor.violations);
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
