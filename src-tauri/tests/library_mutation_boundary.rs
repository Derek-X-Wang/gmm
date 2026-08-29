//! Structural gate for Library-content mutation fence policy.
//!
//! This test parses every checked-in production Rust module under `src/`. It
//! follows syntactically visible Library paths (`library_root`, `library_path`,
//! and values derived from them) through local bindings, call arguments, and
//! method receivers. Passing such a value to a helper is conservatively treated
//! as a possible mutation. Explicit `std::fs` imports and aliases are resolved
//! to their original items before the closed non-mutating call set is applied;
//! glob-imported `std::fs` calls are classified by their local item name.
//! Consequently a new module, helper indirection, different formatting, or a
//! previously unseen `std::fs` API cannot silently add a Library-content
//! mutation without declaring policy.
//!
//! Policy evidence is structural: the containing function must call a known
//! fence/staging acquisition API or accept a `LibraryMutationFence`. The named
//! high-risk call sites enforced by the predecessor gate also retain their
//! specific acquisition, staging-commit, witness, and quarantine contracts. A
//! deliberate exception must instead annotate one direct mutating call with
//! `#[allow(clippy::disallowed_methods, reason = "Library mutation policy exemption: ...")]`;
//! block, function, impl, and module blanket exemptions are rejected.
//!
//! This is deliberately not a type checker or control/dataflow proof. It cannot
//! recognize a Library path whose meaning is hidden behind an unrelated name,
//! an opaque return value, macro expansion, generated source, or a call outside
//! checked-in `src/`, and it does not model Rust name resolution beyond simple
//! file-level `std::fs` imports. Most importantly, seeing a fence acquisition
//! in a function does **not** prove that the fence is held across the mutation;
//! that requires dataflow analysis and remains review's job.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::{self, Visit};

const EXEMPTION_PREFIX: &str = "Library mutation policy exemption:";

const ESTABLISHED_MUTATION_CONTRACTS: &[(&str, &[&str])] = &[
    (
        "finish_interrupted_library_deletes",
        &[
            "begin_library_mutation",
            "LibraryMutation::FinishInterruptedDeletes",
        ],
    ),
    (
        "retry_reinstall_recovery",
        &[
            "begin_library_mutation",
            "LibraryMutation::RetryReinstallRecovery",
        ],
    ),
    (
        "set_library_root",
        &["begin_library_mutation", "LibraryMutation::SetLibraryRoot"],
    ),
    (
        "set_library_path_for_game",
        &[
            "begin_library_mutation",
            "LibraryMutation::SetLibraryPathForGame",
        ],
    ),
    (
        "adopt_folder",
        &[
            "create_staged_library_directory",
            "commit_staged_mod",
            "LibraryMutation::AdoptFolder",
        ],
    ),
    (
        "import_zip",
        &[
            "create_staged_library_directory",
            "commit_staged_mod",
            "LibraryMutation::ImportZip",
        ],
    ),
    (
        "recover_unreferenced_library_dir",
        &[
            "begin_guarded_library_mutation",
            "LibraryMutation::RecoverUnreferencedLibraryDir",
        ],
    ),
    (
        "delete_unreferenced_library_dir",
        &[
            "begin_guarded_library_mutation",
            "LibraryMutation::DeleteUnreferencedLibraryDir",
        ],
    ),
    (
        "resolve_duplicate_mods",
        &[
            "begin_library_mutation",
            "LibraryMutation::ResolveDuplicateMods",
            "withdraw_reinstall_junction",
        ],
    ),
    (
        "reinstall_gamebanana_mod_with_endpoints",
        &[
            "begin_library_mutation",
            "LibraryMutation::ReinstallGamebananaMod",
            "insert_reinstall_swap_witness",
            "quarantine_library_directory_with_token",
        ],
    ),
];

#[test]
fn library_content_mutations_declare_their_fence_policy() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = rust_files_below(&source_root);
    sources.sort();

    let mut parsed = Vec::new();
    let mut policy_functions = HashSet::new();
    let mut non_policy_functions = HashSet::new();
    let mut policy_methods = HashSet::new();
    let mut non_policy_methods = HashSet::new();
    for path in sources {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let syntax = syn::parse_file(&source)
            .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));
        let mut policies = PolicyFunctions {
            policies: &mut policy_functions,
            non_policies: &mut non_policy_functions,
            methods: &mut policy_methods,
            non_methods: &mut non_policy_methods,
        };
        policies.visit_file(&syntax);
        parsed.push((path, syntax));
    }
    policy_functions.retain(|name| !non_policy_functions.contains(name));
    policy_methods.retain(|name| !non_policy_methods.contains(name));

    let mut violations = established_contract_violations(&parsed);
    for (path, syntax) in parsed {
        let aliases = FilesystemAliases::from_file(&syntax);
        let relative = path.strip_prefix(&source_root).unwrap_or(&path);
        let mut boundary = BoundaryVisitor {
            aliases: &aliases,
            policy_functions: &policy_functions,
            policy_methods: &policy_methods,
            relative,
            violations: &mut violations,
        };
        boundary.visit_file(&syntax);
    }

    assert!(
        violations.is_empty(),
        "Library-content mutation has no declared fence policy; acquire or accept the Library fence/staging policy, or annotate only the exceptional call with a non-empty `{EXEMPTION_PREFIX} ...` reason: {violations:?}",
    );
}

#[test]
fn reasoned_statement_exemption_is_individual() {
    let violations = fixture_violations(
        r#"
        fn exceptional(library_path: &std::path::Path) -> std::io::Result<()> {
            #[allow(
                clippy::disallowed_methods,
                reason = "Library mutation policy exemption: fixture owns an external fence"
            )]
            std::fs::write(library_path.join("fixture"), b"bytes")
        }
        "#,
    );
    assert!(
        violations.is_empty(),
        "a reasoned exemption on the one mutating statement must be accepted: {violations:?}"
    );
}

#[test]
fn function_level_library_mutation_exemption_is_rejected() {
    let violations = fixture_violations(
        r#"
        #[allow(
            clippy::disallowed_methods,
            reason = "Library mutation policy exemption: fixture tries to hide the whole function"
        )]
        fn blanket(library_path: &std::path::Path) -> std::io::Result<()> {
            std::fs::write(library_path.join("fixture"), b"bytes")
        }
        "#,
    );
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("one statement, not a whole function")),
        "a function-level blanket exemption must be rejected: {violations:?}"
    );
}

#[test]
fn destructive_filesystem_alias_is_classified_by_original_item() {
    let violations = fixture_violations(
        r#"
        use std::fs::remove_file as read;

        fn aliased(library_path: &std::path::Path) -> std::io::Result<()> {
            read(library_path)
        }
        "#,
    );
    assert_eq!(
        violations,
        ["fixture.rs::aliased: call to `std::fs::remove_file`"],
        "a destructive alias must retain the original std::fs identity"
    );
}

#[test]
fn unaliased_destructive_filesystem_control_is_rejected() {
    let violations = fixture_violations(
        r#"
        fn unaliased(library_path: &std::path::Path) -> std::io::Result<()> {
            std::fs::remove_file(library_path)
        }
        "#,
    );
    assert_eq!(
        violations,
        ["fixture.rs::unaliased: call to `std::fs::remove_file`"],
        "the unaliased control must remain rejected"
    );
}

#[test]
fn library_path_method_receiver_is_inspected() {
    let violations = fixture_violations(
        r#"
        fn receiver(library_path: &std::path::Path) -> std::io::Result<()> {
            library_path.some_destructive_method()
        }
        "#,
    );
    assert_eq!(
        violations,
        ["fixture.rs::receiver: call to `some_destructive_method`"],
        "a Library path passed as the receiver must be treated like an argument"
    );
}

#[test]
fn unused_library_mutation_variant_is_not_policy_evidence() {
    let violations = fixture_violations(
        r#"
        use super::library_mutation::LibraryMutation;

        fn unused_policy(library_path: &std::path::Path) -> std::io::Result<()> {
            let _policy = LibraryMutation::AdoptFolder;
            std::fs::remove_dir_all(library_path)
        }
        "#,
    );
    assert_eq!(
        violations,
        ["fixture.rs::unused_policy: call to `std::fs::remove_dir_all`"],
        "naming a policy without acquiring a fence must not satisfy the gate"
    );
}

#[test]
fn unawaited_fence_future_is_not_policy_evidence() {
    let violations = fixture_violations(
        r#"
        async fn unused_future(library_path: &std::path::Path) -> std::io::Result<()> {
            let _future = begin_library_mutation();
            std::fs::remove_dir_all(library_path)
        }
        "#,
    );
    assert_eq!(
        violations,
        ["fixture.rs::unused_future: call to `std::fs::remove_dir_all`"],
        "constructing but not awaiting the fence future must not satisfy the gate"
    );
}

#[test]
fn awaited_fence_acquisition_is_policy_evidence() {
    let violations = fixture_violations(
        r#"
        async fn acquired(library_path: &std::path::Path) -> std::io::Result<()> {
            let _fence = begin_library_mutation().await?;
            std::fs::remove_dir_all(library_path)
        }
        "#,
    );
    assert!(
        violations.is_empty(),
        "an awaited fence acquisition must satisfy the structural policy check: {violations:?}"
    );
}

#[test]
fn block_library_mutation_exemption_is_rejected() {
    let violations = fixture_violations(
        r#"
        fn blanket(library_path: &std::path::Path) -> std::io::Result<()> {
            #[allow(
                clippy::disallowed_methods,
                reason = "Library mutation policy exemption: fixture tries to hide a block"
            )]
            {
                std::fs::write(library_path.join("one"), b"one")?;
                std::fs::rename(library_path.join("one"), library_path.join("two"))?;
                std::fs::remove_dir_all(library_path)?;
            }
            Ok(())
        }
        "#,
    );
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("one direct mutating call, not a block")),
        "a block-level exemption must be rejected: {violations:?}"
    );
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("call to `std::fs::write`")),
        "nested mutations must remain visible after rejecting the block: {violations:?}"
    );
}

#[test]
fn aliased_file_open_is_non_mutating() {
    let violations = fixture_violations(
        r#"
        use std::fs::File as FsFile;

        fn open_for_read(library_path: &std::path::Path) -> std::io::Result<FsFile> {
            FsFile::open(library_path)
        }
        "#,
    );
    assert!(
        violations.is_empty(),
        "File::open must stay non-mutating after alias resolution: {violations:?}"
    );
}

fn fixture_violations(source: &str) -> Vec<String> {
    let syntax = syn::parse_file(source).expect("parse structural gate fixture");
    let aliases = FilesystemAliases::from_file(&syntax);
    let mut policies = HashSet::new();
    let mut non_policies = HashSet::new();
    let mut methods = HashSet::new();
    let mut non_methods = HashSet::new();
    let mut collector = PolicyFunctions {
        policies: &mut policies,
        non_policies: &mut non_policies,
        methods: &mut methods,
        non_methods: &mut non_methods,
    };
    collector.visit_file(&syntax);
    policies.retain(|name| !non_policies.contains(name));
    methods.retain(|name| !non_methods.contains(name));
    let mut violations = Vec::new();
    let mut boundary = BoundaryVisitor {
        aliases: &aliases,
        policy_functions: &policies,
        policy_methods: &methods,
        relative: Path::new("fixture.rs"),
        violations: &mut violations,
    };
    boundary.visit_file(&syntax);
    violations
}

fn established_contract_violations(parsed: &[(PathBuf, syn::File)]) -> Vec<String> {
    #[derive(Default)]
    struct FunctionShape {
        calls: HashSet<String>,
        awaited_calls: HashSet<String>,
        paths: HashSet<String>,
    }

    impl FunctionShape {
        fn contains(&self, marker: &str) -> bool {
            if marker.contains("::") {
                self.paths.iter().any(|path| {
                    path == marker
                        || path
                            .strip_suffix(marker)
                            .is_some_and(|prefix| prefix.ends_with("::"))
                })
            } else {
                if is_policy_acquisition_name(marker) {
                    self.awaited_calls.contains(marker)
                } else {
                    self.calls.contains(marker)
                }
            }
        }
    }

    impl<'ast> Visit<'ast> for FunctionShape {
        fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
            if let syn::Expr::Path(path) = peel_expression(&call.func) {
                if let Some(name) = path.path.segments.last() {
                    self.calls.insert(name.ident.to_string());
                }
            }
            visit::visit_expr_call(self, call);
        }

        fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
            self.calls.insert(call.method.to_string());
            visit::visit_expr_method_call(self, call);
        }

        fn visit_expr_await(&mut self, awaited: &'ast syn::ExprAwait) {
            if let Some(name) = call_name(&awaited.base) {
                self.awaited_calls.insert(name);
            }
            visit::visit_expr_await(self, awaited);
        }

        fn visit_path(&mut self, path: &'ast syn::Path) {
            self.paths.insert(path_segments(path).join("::"));
            visit::visit_path(self, path);
        }
    }

    struct Functions<'a> {
        shapes: &'a mut HashMap<String, Vec<FunctionShape>>,
    }

    impl Functions<'_> {
        fn inspect(&mut self, name: &syn::Ident, block: &syn::Block) {
            if ESTABLISHED_MUTATION_CONTRACTS
                .iter()
                .any(|(expected, _)| name == expected)
            {
                let mut shape = FunctionShape::default();
                shape.visit_block(block);
                self.shapes.entry(name.to_string()).or_default().push(shape);
            }
        }
    }

    impl<'ast> Visit<'ast> for Functions<'_> {
        fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
            self.inspect(&function.sig.ident, &function.block);
            visit::visit_item_fn(self, function);
        }

        fn visit_impl_item_fn(&mut self, function: &'ast syn::ImplItemFn) {
            self.inspect(&function.sig.ident, &function.block);
            visit::visit_impl_item_fn(self, function);
        }
    }

    let mut shapes = HashMap::new();
    for (_, syntax) in parsed {
        Functions {
            shapes: &mut shapes,
        }
        .visit_file(syntax);
    }

    let mut violations = Vec::new();
    for (function, markers) in ESTABLISHED_MUTATION_CONTRACTS {
        let Some(candidates) = shapes.get(*function) else {
            violations.push(format!(
                "established Library mutation contract function `{function}` is missing"
            ));
            continue;
        };
        for marker in *markers {
            if !candidates.iter().any(|shape| shape.contains(marker)) {
                violations.push(format!(
                    "{function}: established Library mutation contract is missing `{marker}`"
                ));
            }
        }
    }
    violations
}

fn rust_files_below(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).expect("read production source directory") {
            let path = entry.expect("read production source entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }
    files
}

#[derive(Default)]
struct FilesystemAliases {
    explicit: HashMap<String, Vec<String>>,
    glob: bool,
}

impl FilesystemAliases {
    fn from_file(file: &syn::File) -> Self {
        struct Collector {
            aliases: FilesystemAliases,
        }
        impl<'ast> Visit<'ast> for Collector {
            fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
                collect_use_tree(Vec::new(), &item.tree, &mut self.aliases);
            }
        }
        let mut collector = Collector {
            aliases: Self::default(),
        };
        collector.visit_file(file);
        collector.aliases
    }
}

fn collect_use_tree(mut prefix: Vec<String>, tree: &syn::UseTree, aliases: &mut FilesystemAliases) {
    match tree {
        syn::UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            collect_use_tree(prefix, &path.tree, aliases);
        }
        syn::UseTree::Name(name) => {
            let ident = name.ident.to_string();
            if prefix == ["std"] && ident == "fs" {
                aliases
                    .explicit
                    .insert(ident, vec!["std".to_string(), "fs".to_string()]);
            } else if prefix == ["std", "fs"] {
                let local = if ident == "self" {
                    "fs".to_string()
                } else {
                    ident.clone()
                };
                let mut canonical = prefix;
                if ident != "self" {
                    canonical.push(ident);
                }
                aliases.explicit.insert(local, canonical);
            }
        }
        syn::UseTree::Rename(rename) => {
            let ident = rename.ident.to_string();
            let renamed = rename.rename.to_string();
            if prefix == ["std"] && ident == "fs" {
                aliases
                    .explicit
                    .insert(renamed, vec!["std".to_string(), "fs".to_string()]);
            } else if prefix == ["std", "fs"] {
                let mut canonical = prefix;
                if ident != "self" {
                    canonical.push(ident);
                }
                aliases.explicit.insert(renamed, canonical);
            }
        }
        syn::UseTree::Group(group) => {
            for item in &group.items {
                collect_use_tree(prefix.clone(), item, aliases);
            }
        }
        syn::UseTree::Glob(_) if prefix == ["std", "fs"] => {
            aliases.glob = true;
        }
        syn::UseTree::Glob(_) => {}
    }
}

struct BoundaryVisitor<'a> {
    aliases: &'a FilesystemAliases,
    policy_functions: &'a HashSet<String>,
    policy_methods: &'a HashSet<String>,
    relative: &'a Path,
    violations: &'a mut Vec<String>,
}

impl BoundaryVisitor<'_> {
    fn inspect_function(
        &mut self,
        name: &syn::Ident,
        attrs: &[syn::Attribute],
        inputs: &syn::punctuated::Punctuated<syn::FnArg, syn::Token![,]>,
        block: &syn::Block,
    ) {
        if is_test_only(attrs) {
            return;
        }
        if has_library_mutation_exemption(attrs) {
            self.violations.push(format!(
                "{}::{name}: a Library mutation exemption must annotate one statement, not a whole function",
                self.relative.display(),
            ));
        }

        let mut tainted = library_path_parameters(inputs);
        loop {
            let before = tainted.len();
            let mut locals = TaintedLocals {
                tainted: &mut tainted,
            };
            locals.visit_block(block);
            if tainted.len() == before {
                break;
            }
        }

        let mut policy = PolicyEvidence::default();
        policy.visit_block(block);
        let mut delegated_policy = KnownPolicyCalls {
            functions: self.policy_functions,
            methods: self.policy_methods,
            found: false,
        };
        delegated_policy.visit_block(block);
        policy.found |= delegated_policy.found;
        if inputs.iter().any(|input| match input {
            syn::FnArg::Typed(typed) => type_mentions(&typed.ty, "LibraryMutationFence"),
            syn::FnArg::Receiver(_) => false,
        }) {
            policy.found = true;
        }

        let mut mutations = MutationVisitor {
            aliases: self.aliases,
            policy_functions: self.policy_functions,
            policy_methods: self.policy_methods,
            tainted: &tainted,
            exempt_mutation_budget: 0,
            found: Vec::new(),
            invalid_exemptions: Vec::new(),
        };
        mutations.visit_block(block);
        self.violations.extend(
            mutations
                .invalid_exemptions
                .into_iter()
                .map(|violation| format!("{}::{name}: {violation}", self.relative.display())),
        );
        if !policy.found {
            self.violations.extend(
                mutations
                    .found
                    .into_iter()
                    .map(|call| format!("{}::{name}: {call}", self.relative.display())),
            );
        }
    }
}

impl<'ast> Visit<'ast> for BoundaryVisitor<'_> {
    fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
        self.inspect_function(
            &function.sig.ident,
            &function.attrs,
            &function.sig.inputs,
            &function.block,
        );
    }

    fn visit_impl_item_fn(&mut self, function: &'ast syn::ImplItemFn) {
        self.inspect_function(
            &function.sig.ident,
            &function.attrs,
            &function.sig.inputs,
            &function.block,
        );
    }

    fn visit_item_impl(&mut self, implementation: &'ast syn::ItemImpl) {
        if has_library_mutation_exemption(&implementation.attrs) {
            self.violations.push(format!(
                "{}: a Library mutation exemption must annotate one statement, not a whole impl",
                self.relative.display(),
            ));
        }
        visit::visit_item_impl(self, implementation);
    }

    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        if is_test_only(&module.attrs) {
            return;
        }
        if has_library_mutation_exemption(&module.attrs) {
            self.violations.push(format!(
                "{}::{}: a Library mutation exemption must annotate one statement, not a whole module",
                self.relative.display(),
                module.ident,
            ));
        }
        visit::visit_item_mod(self, module);
    }
}

struct TaintedLocals<'a> {
    tainted: &'a mut HashSet<String>,
}

impl<'ast> Visit<'ast> for TaintedLocals<'_> {
    fn visit_local(&mut self, local: &'ast syn::Local) {
        if local
            .init
            .as_ref()
            .is_some_and(|init| expression_mentions_library_path(&init.expr, self.tainted))
        {
            collect_bindings(&local.pat, self.tainted);
        }
        visit::visit_local(self, local);
    }
}

#[derive(Default)]
struct PolicyEvidence {
    found: bool,
}

struct PolicyFunctions<'a> {
    policies: &'a mut HashSet<String>,
    non_policies: &'a mut HashSet<String>,
    methods: &'a mut HashSet<String>,
    non_methods: &'a mut HashSet<String>,
}

impl PolicyFunctions<'_> {
    fn inspect(
        &mut self,
        name: &syn::Ident,
        attrs: &[syn::Attribute],
        inputs: &syn::punctuated::Punctuated<syn::FnArg, syn::Token![,]>,
        block: &syn::Block,
        method: bool,
    ) {
        if is_test_only(attrs) {
            return;
        }
        let mut evidence = PolicyEvidence::default();
        evidence.visit_block(block);
        let name = name.to_string();
        let declared = evidence.found
            || inputs.iter().any(|input| match input {
                syn::FnArg::Typed(typed) => type_mentions(&typed.ty, "LibraryMutationFence"),
                syn::FnArg::Receiver(_) => false,
            });
        if (method, declared) == (true, true) {
            self.methods.insert(name);
        } else if method {
            self.non_methods.insert(name);
        } else if declared {
            self.policies.insert(name);
        } else {
            self.non_policies.insert(name);
        }
    }
}

impl<'ast> Visit<'ast> for PolicyFunctions<'_> {
    fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
        self.inspect(
            &function.sig.ident,
            &function.attrs,
            &function.sig.inputs,
            &function.block,
            false,
        );
    }

    fn visit_impl_item_fn(&mut self, function: &'ast syn::ImplItemFn) {
        self.inspect(
            &function.sig.ident,
            &function.attrs,
            &function.sig.inputs,
            &function.block,
            true,
        );
    }

    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        if !is_test_only(&module.attrs) {
            visit::visit_item_mod(self, module);
        }
    }
}

impl<'ast> Visit<'ast> for PolicyEvidence {
    fn visit_expr_await(&mut self, awaited: &'ast syn::ExprAwait) {
        self.found |=
            call_name(&awaited.base).is_some_and(|name| is_policy_acquisition_name(&name));
        visit::visit_expr_await(self, awaited);
    }
}

fn is_policy_acquisition_name(name: &str) -> bool {
    matches!(
        name,
        "begin_library_mutation"
            | "begin_guarded_library_mutation"
            | "create_staged_library_directory"
            | "revalidate_library_root_for_mutation"
    )
}

struct KnownPolicyCalls<'a> {
    functions: &'a HashSet<String>,
    methods: &'a HashSet<String>,
    found: bool,
}

impl<'ast> Visit<'ast> for KnownPolicyCalls<'_> {
    fn visit_expr_await(&mut self, awaited: &'ast syn::ExprAwait) {
        if let Some(name) = call_name(&awaited.base) {
            self.found |= self.functions.contains(&name) || self.methods.contains(&name);
        }
        visit::visit_expr_await(self, awaited);
    }
}

struct MutationVisitor<'a> {
    aliases: &'a FilesystemAliases,
    policy_functions: &'a HashSet<String>,
    policy_methods: &'a HashSet<String>,
    tainted: &'a HashSet<String>,
    exempt_mutation_budget: usize,
    found: Vec<String>,
    invalid_exemptions: Vec<String>,
}

impl MutationVisitor<'_> {
    fn inspect_call(
        &mut self,
        callee: &syn::Path,
        arguments: impl Iterator<Item = syn::Expr>,
        method: bool,
    ) {
        let segments = path_segments(callee);
        let resolved = resolve_filesystem_path(&segments, self.aliases);
        let classified = resolved.as_deref().unwrap_or(&segments);
        let Some(name) = classified.last() else {
            return;
        };
        let arguments = arguments.collect::<Vec<_>>();
        if !arguments
            .iter()
            .any(|argument| expression_mentions_library_path(argument, self.tainted))
        {
            return;
        }
        let filesystem = resolved.is_some();
        let policy_declared = if method {
            self.policy_methods.contains(name)
        } else {
            self.policy_functions.contains(name)
        };
        if !is_non_mutating_call(classified)
            && !policy_declared
            && (filesystem || !is_path_transform(name))
        {
            if self.exempt_mutation_budget > 0 {
                self.exempt_mutation_budget = self.exempt_mutation_budget.saturating_sub(1);
            } else {
                self.found
                    .push(format!("call to `{}`", classified.join("::")));
            }
        }
    }
}

impl<'ast> Visit<'ast> for MutationVisitor<'_> {
    fn visit_stmt(&mut self, statement: &'ast syn::Stmt) {
        let exemption = statement_exemption(statement);
        if exemption.invalid {
            self.invalid_exemptions
                .push("Library mutation exemption has no non-empty reason".to_string());
        }
        if exemption.valid {
            let expression = match statement {
                syn::Stmt::Local(local) => local.init.as_ref().map(|init| init.expr.as_ref()),
                syn::Stmt::Expr(expression, _) => Some(expression),
                syn::Stmt::Item(_) | syn::Stmt::Macro(_) => None,
            };
            if expression.is_some_and(is_direct_call_expression) {
                self.exempt_mutation_budget = 1;
                visit::visit_stmt(self, statement);
                if self.exempt_mutation_budget != 0 {
                    self.invalid_exemptions.push(
                        "a Library mutation exemption must annotate one direct mutating call"
                            .to_string(),
                    );
                }
                self.exempt_mutation_budget = 0;
                return;
            }
            self.invalid_exemptions.push(
                "a Library mutation exemption must annotate one direct mutating call, not a block"
                    .to_string(),
            );
        }
        visit::visit_stmt(self, statement);
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = peel_expression(&call.func) {
            self.inspect_call(&path.path, call.args.iter().cloned(), false);
        }
        visit::visit_expr_call(self, call);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        let path = syn::Path::from(call.method.clone());
        self.inspect_call(
            &path,
            std::iter::once((*call.receiver).clone()).chain(call.args.iter().cloned()),
            true,
        );
        visit::visit_expr_method_call(self, call);
    }
}

fn library_path_parameters(
    inputs: &syn::punctuated::Punctuated<syn::FnArg, syn::Token![,]>,
) -> HashSet<String> {
    let mut tainted = HashSet::new();
    for input in inputs {
        if let syn::FnArg::Typed(typed) = input {
            let mut bindings = HashSet::new();
            collect_bindings(&typed.pat, &mut bindings);
            tainted.extend(
                bindings
                    .into_iter()
                    .filter(|binding| is_library_path_name(binding)),
            );
        }
    }
    tainted
}

fn expression_mentions_library_path(expression: &syn::Expr, tainted: &HashSet<String>) -> bool {
    struct Finder<'a> {
        tainted: &'a HashSet<String>,
        found: bool,
    }
    impl<'ast> Visit<'ast> for Finder<'_> {
        fn visit_ident(&mut self, ident: &'ast syn::Ident) {
            let name = ident.to_string();
            if self.tainted.contains(&name) || is_library_path_name(&name) {
                self.found = true;
            }
        }
    }
    let mut finder = Finder {
        tainted,
        found: false,
    };
    finder.visit_expr(expression);
    finder.found
}

fn is_library_path_name(name: &str) -> bool {
    matches!(
        name,
        "library_path" | "library_root" | "default_library_root" | "resolved_library_root"
    ) || name.ends_with("_library_path")
        || name.ends_with("_library_root")
}

fn resolve_filesystem_path(
    segments: &[String],
    aliases: &FilesystemAliases,
) -> Option<Vec<String>> {
    if segments.windows(2).any(|pair| pair == ["std", "fs"]) {
        return Some(segments.to_vec());
    }
    if let Some(canonical) = segments
        .first()
        .and_then(|first| aliases.explicit.get(first))
    {
        let mut resolved = canonical.clone();
        resolved.extend_from_slice(&segments[1..]);
        return Some(resolved);
    }
    (segments.len() == 1 && aliases.glob).then(|| segments.to_vec())
}

fn is_non_mutating_call(segments: &[String]) -> bool {
    let Some(name) = segments.last().map(String::as_str) else {
        return false;
    };
    if name == "open" {
        return segments
            .iter()
            .any(|segment| segment == "IdentifiedDirectory" || segment == "File");
    }
    if name == "new" {
        return segments
            .iter()
            .any(|segment| segment == "Path" || segment == "Core");
    }
    matches!(
        name,
        "Err"
            | "None"
            | "Ok"
            | "Some"
            | "bind"
            | "and_then"
            | "as_ref"
            | "canonicalize"
            | "collect"
            | "contains"
            | "exists"
            | "execute"
            | "expect"
            | "extract_hashes_from_dir"
            | "file_type"
            | "from"
            | "get_setting"
            | "insert"
            | "junction_target_for"
            | "map"
            | "map_err"
            | "metadata"
            | "metadata_if_exists"
            | "new_inner"
            | "path_within"
            | "push"
            | "read"
            | "read_dir"
            | "read_link"
            | "read_to_string"
            | "require_ntfs_pair"
            | "same_path"
            | "symlink_metadata"
            | "try_exists"
            | "unwrap_or_else"
            | "validate_paths"
    )
}

fn is_path_transform(name: &str) -> bool {
    matches!(
        name,
        "as_os_str"
            | "as_path"
            | "canonicalize"
            | "clone"
            | "display"
            | "file_name"
            | "join"
            | "parent"
            | "starts_with"
            | "strip_prefix"
            | "to_path_buf"
            | "to_string_lossy"
    )
}

fn has_library_mutation_exemption(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(valid_library_mutation_exemption)
}

fn valid_library_mutation_exemption(attribute: &syn::Attribute) -> bool {
    if !attribute.path().is_ident("allow") {
        return false;
    }
    let mut lint = false;
    let mut reason = false;
    let parsed = attribute.parse_nested_meta(|meta| {
        if path_ends_with(&meta.path, &["clippy", "disallowed_methods"]) {
            lint = true;
        } else if meta.path.is_ident("reason") {
            let value = meta.value()?;
            let literal: syn::LitStr = value.parse()?;
            reason = literal
                .value()
                .strip_prefix(EXEMPTION_PREFIX)
                .is_some_and(|explanation| !explanation.trim().is_empty());
        }
        Ok(())
    });
    parsed.is_ok() && lint && reason
}

fn attribute_contains_exemption_prefix(attribute: &syn::Attribute) -> bool {
    if !attribute.path().is_ident("allow") {
        return false;
    }
    let mut found = false;
    let _ = attribute.parse_nested_meta(|meta| {
        if meta.path.is_ident("reason") {
            let value = meta.value()?;
            let literal: syn::LitStr = value.parse()?;
            found = literal.value().starts_with(EXEMPTION_PREFIX);
        }
        Ok(())
    });
    found
}

#[derive(Default)]
struct StatementExemption {
    valid: bool,
    invalid: bool,
}

fn statement_exemption(statement: &syn::Stmt) -> StatementExemption {
    let attrs = match statement {
        syn::Stmt::Local(local) => local.attrs.as_slice(),
        syn::Stmt::Item(_) => &[],
        syn::Stmt::Expr(expression, _) => expression_attrs(expression),
        syn::Stmt::Macro(macro_statement) => macro_statement.attrs.as_slice(),
    };
    StatementExemption {
        valid: attrs.iter().any(valid_library_mutation_exemption),
        invalid: attrs.iter().any(|attribute| {
            attribute_contains_exemption_prefix(attribute)
                && !valid_library_mutation_exemption(attribute)
        }),
    }
}

fn expression_attrs(expression: &syn::Expr) -> &[syn::Attribute] {
    match expression {
        syn::Expr::Array(value) => &value.attrs,
        syn::Expr::Assign(value) => &value.attrs,
        syn::Expr::Async(value) => &value.attrs,
        syn::Expr::Await(value) => &value.attrs,
        syn::Expr::Binary(value) => &value.attrs,
        syn::Expr::Block(value) => &value.attrs,
        syn::Expr::Break(value) => &value.attrs,
        syn::Expr::Call(value) => &value.attrs,
        syn::Expr::Cast(value) => &value.attrs,
        syn::Expr::Closure(value) => &value.attrs,
        syn::Expr::Const(value) => &value.attrs,
        syn::Expr::Continue(value) => &value.attrs,
        syn::Expr::Field(value) => &value.attrs,
        syn::Expr::ForLoop(value) => &value.attrs,
        syn::Expr::Group(value) => &value.attrs,
        syn::Expr::If(value) => &value.attrs,
        syn::Expr::Index(value) => &value.attrs,
        syn::Expr::Infer(value) => &value.attrs,
        syn::Expr::Let(value) => &value.attrs,
        syn::Expr::Lit(value) => &value.attrs,
        syn::Expr::Loop(value) => &value.attrs,
        syn::Expr::Macro(value) => &value.attrs,
        syn::Expr::Match(value) => &value.attrs,
        syn::Expr::MethodCall(value) => &value.attrs,
        syn::Expr::Paren(value) => &value.attrs,
        syn::Expr::Path(value) => &value.attrs,
        syn::Expr::Range(value) => &value.attrs,
        syn::Expr::RawAddr(value) => &value.attrs,
        syn::Expr::Reference(value) => &value.attrs,
        syn::Expr::Repeat(value) => &value.attrs,
        syn::Expr::Return(value) => &value.attrs,
        syn::Expr::Struct(value) => &value.attrs,
        syn::Expr::Try(value) => &value.attrs,
        syn::Expr::TryBlock(value) => &value.attrs,
        syn::Expr::Tuple(value) => &value.attrs,
        syn::Expr::Unary(value) => &value.attrs,
        syn::Expr::Unsafe(value) => &value.attrs,
        syn::Expr::While(value) => &value.attrs,
        syn::Expr::Yield(value) => &value.attrs,
        syn::Expr::Verbatim(_) => &[],
        _ => &[],
    }
}

fn is_test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attribute| {
        attribute.path().is_ident("test")
            || path_ends_with(attribute.path(), &["tokio", "test"])
            || attribute.path().is_ident("cfg")
            && matches!(&attribute.meta, syn::Meta::List(list) if list.tokens.to_string() == "test")
    })
}

fn type_mentions(ty: &syn::Type, expected: &str) -> bool {
    struct Finder<'a> {
        expected: &'a str,
        found: bool,
    }
    impl<'ast> Visit<'ast> for Finder<'_> {
        fn visit_path(&mut self, path: &'ast syn::Path) {
            if path
                .segments
                .iter()
                .any(|segment| segment.ident == self.expected)
            {
                self.found = true;
            }
            visit::visit_path(self, path);
        }
    }
    let mut finder = Finder {
        expected,
        found: false,
    };
    finder.visit_type(ty);
    finder.found
}

fn collect_bindings(pattern: &syn::Pat, bindings: &mut HashSet<String>) {
    match pattern {
        syn::Pat::Ident(ident) => {
            bindings.insert(ident.ident.to_string());
        }
        syn::Pat::Tuple(tuple) => {
            for item in &tuple.elems {
                collect_bindings(item, bindings);
            }
        }
        syn::Pat::TupleStruct(tuple) => {
            for item in &tuple.elems {
                collect_bindings(item, bindings);
            }
        }
        syn::Pat::Reference(reference) => collect_bindings(&reference.pat, bindings),
        syn::Pat::Type(typed) => collect_bindings(&typed.pat, bindings),
        syn::Pat::Paren(paren) => collect_bindings(&paren.pat, bindings),
        _ => {}
    }
}

fn path_segments(path: &syn::Path) -> Vec<String> {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect()
}

fn call_name(expression: &syn::Expr) -> Option<String> {
    match peel_expression(expression) {
        syn::Expr::Call(call) => match peel_expression(&call.func) {
            syn::Expr::Path(path) => path
                .path
                .segments
                .last()
                .map(|segment| segment.ident.to_string()),
            _ => None,
        },
        syn::Expr::MethodCall(call) => Some(call.method.to_string()),
        _ => None,
    }
}

fn path_ends_with(path: &syn::Path, expected: &[&str]) -> bool {
    let actual = path_segments(path);
    actual.len() >= expected.len()
        && actual[actual.len() - expected.len()..]
            .iter()
            .map(String::as_str)
            .eq(expected.iter().copied())
}

fn peel_expression(mut expression: &syn::Expr) -> &syn::Expr {
    loop {
        expression = match expression {
            syn::Expr::Group(group) => &group.expr,
            syn::Expr::Paren(paren) => &paren.expr,
            syn::Expr::Reference(reference) => &reference.expr,
            _ => return expression,
        };
    }
}

fn is_direct_call_expression(expression: &syn::Expr) -> bool {
    matches!(
        peel_exemption_expression(expression),
        syn::Expr::Call(_) | syn::Expr::MethodCall(_)
    )
}

fn peel_exemption_expression(expression: &syn::Expr) -> &syn::Expr {
    match expression {
        syn::Expr::Await(awaited) => peel_exemption_expression(&awaited.base),
        syn::Expr::Group(group) => peel_exemption_expression(&group.expr),
        syn::Expr::Paren(paren) => peel_exemption_expression(&paren.expr),
        syn::Expr::Try(tried) => peel_exemption_expression(&tried.expr),
        _ => expression,
    }
}
