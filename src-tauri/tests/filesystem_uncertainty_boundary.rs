//! Structural gate for filesystem uncertainty inside `core`.
//!
//! Rust's lint configuration can deny named methods, but the unsafe behavior
//! is a shape: a fallible filesystem observation becomes a boolean or `Option`
//! without classifying its error. This test parses every production module in
//! `core` and rejects that shape across direct calls, local aliases, method
//! chains, UFCS calls, `match`/`if let`, and `matches!`. It also rejects direct
//! follow-up filesystem lookups on entries yielded by `read_dir` unless the
//! lookup is inside `resolve_enumerated_entry`.
//!
//! This is deliberately not a Rust type checker. It recognizes standard-library
//! filesystem calls and values that remain syntactically connected through a
//! local binding or ordinary `Result` combinator. A helper that hides I/O behind
//! an unrelated function name, runtime-generated source, procedural macro
//! output, or a value passed across a function boundary still requires review.
//! Core cannot make `std::fs` unconstructible: any module in the crate can name
//! it directly. The structural gate is therefore the enforceable boundary; the
//! safe helpers in `core::filesystem` remain the preferred production route.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::TokenStream;
use syn::visit::{self, Visit};

const CORE: &str = "src/core";

#[test]
fn core_filesystem_uncertainty_is_never_collapsed() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let core = crate_root.join(CORE);
    let mut files = rust_files_below(&core);
    files.sort();

    let mut violations = Vec::new();
    for path in files {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let syntax = syn::parse_file(&source)
            .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));
        let aliases = FilesystemAliases::from_file(&syntax);
        let relative = path
            .strip_prefix(&core)
            .expect("core source is below core root")
            .to_path_buf();
        let mut boundary = BoundaryVisitor {
            aliases: &aliases,
            relative: &relative,
            violations: &mut violations,
        };
        boundary.visit_file(&syntax);
    }

    assert!(
        violations.is_empty(),
        "filesystem uncertainty escaped core::filesystem; propagate the error, classify NotFound explicitly, or annotate an intentional best-effort probe with #[allow(clippy::disallowed_methods, reason = \"...\")]: {violations:?}",
    );
}

fn rust_files_below(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).expect("read core source directory") {
            let path = entry.expect("read core source entry").path();
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
    modules: HashSet<String>,
    functions: HashSet<String>,
    types: HashSet<String>,
    path_types: HashSet<String>,
    glob_import: bool,
}

impl FilesystemAliases {
    fn from_file(file: &syn::File) -> Self {
        let mut aliases = Self::default();
        for item in &file.items {
            if let syn::Item::Use(item) = item {
                collect_use_tree(Vec::new(), &item.tree, &mut aliases);
            }
        }
        aliases
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
            if prefix == ["std", "fs"] {
                if ident == "self" {
                    aliases.modules.insert("fs".to_string());
                } else if is_filesystem_type(&ident) {
                    aliases.types.insert(ident);
                } else {
                    aliases.functions.insert(ident);
                }
            } else if prefix == ["std"] && ident == "fs" {
                aliases.modules.insert(ident);
            } else if prefix == ["std", "path"] && ident == "Path" {
                aliases.path_types.insert(ident);
            }
        }
        syn::UseTree::Rename(rename) => {
            let ident = rename.ident.to_string();
            let renamed = rename.rename.to_string();
            if prefix == ["std"] && ident == "fs" {
                aliases.modules.insert(renamed);
            } else if prefix == ["std", "fs"] {
                if ident == "self" {
                    aliases.modules.insert(renamed);
                } else if is_filesystem_type(&ident) {
                    aliases.types.insert(renamed);
                } else {
                    aliases.functions.insert(renamed);
                }
            } else if prefix == ["std", "path"] && ident == "Path" {
                aliases.path_types.insert(renamed);
            }
        }
        syn::UseTree::Group(group) => {
            for item in &group.items {
                collect_use_tree(prefix.clone(), item, aliases);
            }
        }
        syn::UseTree::Glob(_) => {
            if prefix == ["std", "fs"] {
                aliases.glob_import = true;
            }
        }
    }
}

struct BoundaryVisitor<'a> {
    aliases: &'a FilesystemAliases,
    relative: &'a Path,
    violations: &'a mut Vec<String>,
}

impl BoundaryVisitor<'_> {
    fn analyze_function(&mut self, attrs: &[syn::Attribute], block: &syn::Block) {
        if is_test_only(attrs) || has_deliberate_collapse_allow(attrs) {
            return;
        }
        let mut analyzer = FunctionAnalyzer {
            aliases: self.aliases,
            relative: self.relative,
            violations: self.violations,
            filesystem_results: HashSet::new(),
            directory_iterators: HashSet::new(),
            enumerated_entries: HashSet::new(),
            enumerated_paths: HashSet::new(),
            inside_enumerated_entry_resolver: 0,
        };
        analyzer.visit_block(block);
    }
}

impl<'ast> Visit<'ast> for BoundaryVisitor<'_> {
    fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
        self.analyze_function(&function.attrs, &function.block);
    }

    fn visit_impl_item_fn(&mut self, function: &'ast syn::ImplItemFn) {
        self.analyze_function(&function.attrs, &function.block);
    }

    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        if !is_test_only(&module.attrs) {
            visit::visit_item_mod(self, module);
        }
    }
}

struct FunctionAnalyzer<'a> {
    aliases: &'a FilesystemAliases,
    relative: &'a Path,
    violations: &'a mut Vec<String>,
    filesystem_results: HashSet<String>,
    directory_iterators: HashSet<String>,
    enumerated_entries: HashSet<String>,
    enumerated_paths: HashSet<String>,
    inside_enumerated_entry_resolver: usize,
}

impl FunctionAnalyzer<'_> {
    fn report(&mut self, shape: impl Into<String>) {
        self.violations
            .push(format!("{}: {}", self.relative.display(), shape.into()));
    }

    fn is_filesystem_result(&self, expression: &syn::Expr) -> bool {
        match peel_expression(expression) {
            syn::Expr::Path(path) => single_ident(&path.path)
                .is_some_and(|ident| self.filesystem_results.contains(&ident)),
            syn::Expr::Call(call) => {
                call_path(&call.func).is_some_and(|path| is_filesystem_call(path, self.aliases))
            }
            syn::Expr::MethodCall(call) => {
                let method = call.method.to_string();
                is_filesystem_result_method(&method)
                    || (preserves_result(&method) && self.is_filesystem_result(&call.receiver))
            }
            syn::Expr::Await(awaited) => self.is_filesystem_result(&awaited.base),
            syn::Expr::Block(block) => block
                .block
                .stmts
                .last()
                .and_then(statement_expression)
                .is_some_and(|expression| self.is_filesystem_result(expression)),
            _ => false,
        }
    }

    fn is_enumerated_value(&self, expression: &syn::Expr) -> bool {
        match peel_expression(expression) {
            syn::Expr::Path(path) => single_ident(&path.path).is_some_and(|ident| {
                self.enumerated_entries.contains(&ident) || self.enumerated_paths.contains(&ident)
            }),
            syn::Expr::MethodCall(call) => self.is_enumerated_value(&call.receiver),
            _ => false,
        }
    }

    fn is_unwrapped_enumerated_lookup(&self, expression: &syn::Expr) -> bool {
        match peel_expression(expression) {
            syn::Expr::MethodCall(call) => {
                let method = call.method.to_string();
                matches!(
                    method.as_str(),
                    "file_type" | "metadata" | "symlink_metadata" | "try_exists"
                ) && self.is_enumerated_value(&call.receiver)
            }
            syn::Expr::Call(call) => {
                let Some(path) = call_path(&call.func) else {
                    return false;
                };
                is_filesystem_call(path, self.aliases)
                    && call
                        .args
                        .iter()
                        .any(|argument| self.is_enumerated_value(argument))
            }
            _ => false,
        }
    }
}

impl<'ast> Visit<'ast> for FunctionAnalyzer<'_> {
    fn visit_local(&mut self, local: &'ast syn::Local) {
        if let Some(init) = &local.init {
            visit::visit_expr(self, &init.expr);
            let mut bindings = Vec::new();
            collect_bindings(&local.pat, &mut bindings);
            if self.is_filesystem_result(&init.expr) {
                self.filesystem_results.extend(bindings.iter().cloned());
            }
            if contains_read_dir(&init.expr, self.aliases) {
                self.directory_iterators.extend(bindings.iter().cloned());
            }
            if is_entry_path(&init.expr, &self.enumerated_entries) {
                self.enumerated_paths.extend(bindings);
            }
            if self.is_filesystem_result(&init.expr)
                && init
                    .diverge
                    .as_ref()
                    .is_some_and(|(_, diverge)| collapses_to_bool_or_option(diverge))
            {
                self.report("let-else collapses a filesystem Result");
            }
        }
    }

    fn visit_expr_for_loop(&mut self, loop_expression: &'ast syn::ExprForLoop) {
        visit::visit_expr(self, &loop_expression.expr);
        let enumerates_directory = contains_read_dir(&loop_expression.expr, self.aliases)
            || path_ident(&loop_expression.expr)
                .is_some_and(|ident| self.directory_iterators.contains(&ident));
        // Different enumeration loops have different contracts. Conflict and
        // diagnostics scans deliberately skip an immediate child that vanishes;
        // copy and mutation loops deliberately propagate the same race. Once a
        // loop establishes the skip-on-NotFound policy through the resolver,
        // every direct follow-up lookup in that loop must use it too.
        if !enumerates_directory || !block_contains_resolver(&loop_expression.body) {
            visit::visit_block(self, &loop_expression.body);
            return;
        }

        let mut bindings = Vec::new();
        collect_bindings(&loop_expression.pat, &mut bindings);
        let previous_entries = self.enumerated_entries.clone();
        let previous_paths = self.enumerated_paths.clone();
        self.enumerated_entries.extend(bindings);
        visit::visit_block(self, &loop_expression.body);
        self.enumerated_entries = previous_entries;
        self.enumerated_paths = previous_paths;
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        let method = call.method.to_string();
        let explicitly_classifies_not_found =
            method == "is_err_and" && call.args.iter().any(expression_mentions_not_found);
        if collapses_result(&method)
            && !explicitly_classifies_not_found
            && self.is_filesystem_result(&call.receiver)
        {
            self.report(format!(".{method}() collapses a filesystem Result"));
        }
        if self.inside_enumerated_entry_resolver == 0
            && self.is_unwrapped_enumerated_lookup(&syn::Expr::MethodCall(call.clone()))
        {
            self.report(format!(
                ".{method}() follows read_dir without resolve_enumerated_entry"
            ));
        }
        visit::visit_expr_method_call(self, call);
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        let function = call_path(&call.func).and_then(|path| path.segments.last());
        let function_name = function.map(|segment| segment.ident.to_string());
        if function_name.as_deref() == Some("resolve_enumerated_entry") {
            self.inside_enumerated_entry_resolver += 1;
            visit::visit_expr_call(self, call);
            self.inside_enumerated_entry_resolver -= 1;
            return;
        }
        let explicitly_classifies_not_found = function_name.as_deref() == Some("is_err_and")
            && call.args.iter().skip(1).any(expression_mentions_not_found);
        if function_name.as_deref().is_some_and(collapses_result)
            && !explicitly_classifies_not_found
            && call
                .args
                .first()
                .is_some_and(|argument| self.is_filesystem_result(argument))
        {
            self.report(format!(
                "{}(...) collapses a filesystem Result",
                function_name.expect("collapse function name")
            ));
        }
        if self.inside_enumerated_entry_resolver == 0
            && self.is_unwrapped_enumerated_lookup(&syn::Expr::Call(call.clone()))
        {
            self.report("filesystem call follows read_dir without resolve_enumerated_entry");
        }
        visit::visit_expr_call(self, call);
    }

    fn visit_expr_match(&mut self, expression: &'ast syn::ExprMatch) {
        if self.is_filesystem_result(&expression.expr)
            && expression.arms.iter().any(|arm| {
                pattern_contains_err(&arm.pat)
                    && !arm
                        .guard
                        .as_ref()
                        .is_some_and(|(_, guard)| expression_mentions_not_found(guard))
                    && collapses_to_bool_or_option(&arm.body)
            })
        {
            self.report("match arm collapses a filesystem error to bool or Option");
        }
        visit::visit_expr_match(self, expression);
    }

    fn visit_expr_if(&mut self, expression: &'ast syn::ExprIf) {
        if let syn::Expr::Let(condition) = peel_expression(&expression.cond) {
            if pattern_contains_err(&condition.pat)
                && self.is_filesystem_result(&condition.expr)
                && (block_collapses(&expression.then_branch)
                    || expression
                        .else_branch
                        .as_ref()
                        .is_some_and(|(_, branch)| collapses_to_bool_or_option(branch)))
            {
                self.report("if-let collapses a filesystem error to bool or Option");
            }
        }
        visit::visit_expr_if(self, expression);
    }

    fn visit_macro(&mut self, invocation: &'ast syn::Macro) {
        if invocation.path.is_ident("matches")
            && macro_mentions_filesystem(&invocation.tokens, self.aliases)
            && !invocation.tokens.to_string().contains("NotFound")
        {
            self.report("matches! collapses a filesystem Result to bool");
        }
        visit::visit_macro(self, invocation);
    }
}

fn is_test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attribute| {
        attribute.path().is_ident("cfg")
            && matches!(&attribute.meta, syn::Meta::List(list) if list.tokens.to_string() == "test")
    })
}

fn has_deliberate_collapse_allow(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attribute| {
        if !attribute.path().is_ident("allow") {
            return false;
        }
        let mut disallowed_methods = false;
        let mut reason = false;
        let parsed = attribute.parse_nested_meta(|meta| {
            if path_ends_with(&meta.path, &["clippy", "disallowed_methods"]) {
                disallowed_methods = true;
            } else if meta.path.is_ident("reason") {
                let value = meta.value()?;
                let literal: syn::LitStr = value.parse()?;
                reason = !literal.value().trim().is_empty();
            }
            Ok(())
        });
        parsed.is_ok() && disallowed_methods && reason
    })
}

fn is_filesystem_call(path: &syn::Path, aliases: &FilesystemAliases) -> bool {
    let segments: Vec<String> = path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect();
    let Some(last) = segments.last() else {
        return false;
    };
    if segments.len() == 1 {
        return aliases.functions.contains(last)
            || (aliases.glob_import && is_known_filesystem_function(last));
    }
    if is_filesystem_result_method(last)
        && segments[..segments.len() - 1].last().is_some_and(|owner| {
            aliases.path_types.contains(owner)
                || aliases.types.contains(owner)
                || matches!(owner.as_str(), "Path" | "DirEntry")
        })
    {
        return true;
    }
    if segments.len() == 2
        && (aliases.modules.contains(&segments[0]) || aliases.types.contains(&segments[0]))
    {
        return true;
    }
    segments.windows(2).any(|pair| pair == ["std", "fs"])
        || segments.windows(2).any(|pair| pair == ["tokio", "fs"])
}

fn is_filesystem_type(ident: &str) -> bool {
    matches!(
        ident,
        "File" | "OpenOptions" | "DirEntry" | "ReadDir" | "Metadata" | "FileType"
    )
}

fn is_known_filesystem_function(ident: &str) -> bool {
    matches!(
        ident,
        "canonicalize"
            | "copy"
            | "create_dir"
            | "create_dir_all"
            | "exists"
            | "hard_link"
            | "metadata"
            | "read"
            | "read_dir"
            | "read_link"
            | "read_to_string"
            | "remove_dir"
            | "remove_dir_all"
            | "remove_file"
            | "rename"
            | "set_permissions"
            | "symlink_metadata"
            | "write"
    )
}

fn is_filesystem_result_method(method: &str) -> bool {
    matches!(
        method,
        "file_type" | "metadata" | "symlink_metadata" | "read_dir" | "try_exists"
    )
}

fn preserves_result(method: &str) -> bool {
    matches!(
        method,
        "map" | "map_err" | "and_then" | "inspect" | "inspect_err" | "as_ref" | "as_mut"
    )
}

fn collapses_result(method: &str) -> bool {
    matches!(
        method,
        "ok" | "is_ok"
            | "is_err"
            | "is_ok_and"
            | "is_err_and"
            | "unwrap_or"
            | "unwrap_or_else"
            | "unwrap_or_default"
            | "map_or"
            | "map_or_else"
            | "or_else"
    )
}

fn contains_read_dir(expression: &syn::Expr, aliases: &FilesystemAliases) -> bool {
    struct Finder<'a> {
        aliases: &'a FilesystemAliases,
        found: bool,
    }
    impl<'ast> Visit<'ast> for Finder<'_> {
        fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
            if call_path(&call.func).is_some_and(|path| {
                path.segments
                    .last()
                    .is_some_and(|segment| segment.ident == "read_dir")
                    && is_filesystem_call(path, self.aliases)
            }) {
                self.found = true;
                return;
            }
            visit::visit_expr_call(self, call);
        }

        fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
            if call.method == "read_dir" {
                self.found = true;
                return;
            }
            visit::visit_expr_method_call(self, call);
        }
    }
    let mut finder = Finder {
        aliases,
        found: false,
    };
    finder.visit_expr(expression);
    finder.found
}

fn block_contains_resolver(block: &syn::Block) -> bool {
    struct Finder {
        found: bool,
    }
    impl<'ast> Visit<'ast> for Finder {
        fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
            if call_path(&call.func).is_some_and(|path| {
                path.segments
                    .last()
                    .is_some_and(|segment| segment.ident == "resolve_enumerated_entry")
            }) {
                self.found = true;
                return;
            }
            visit::visit_expr_call(self, call);
        }
    }
    let mut finder = Finder { found: false };
    finder.visit_block(block);
    finder.found
}

fn macro_mentions_filesystem(tokens: &TokenStream, aliases: &FilesystemAliases) -> bool {
    let text = tokens.to_string();
    text.contains("std :: fs ::")
        || text.contains("tokio :: fs ::")
        || aliases
            .modules
            .iter()
            .any(|module| text.contains(&format!("{module} ::")))
        || aliases.functions.iter().any(|function| {
            text.split(|character: char| !character.is_alphanumeric() && character != '_')
                .any(|token| token == function)
        })
}

fn is_entry_path(expression: &syn::Expr, entries: &HashSet<String>) -> bool {
    let syn::Expr::MethodCall(call) = peel_expression(expression) else {
        return false;
    };
    call.method == "path"
        && path_ident(&call.receiver).is_some_and(|ident| entries.contains(&ident))
}

fn collapses_to_bool_or_option(expression: &syn::Expr) -> bool {
    match peel_expression(expression) {
        syn::Expr::Lit(literal) => matches!(literal.lit, syn::Lit::Bool(_)),
        syn::Expr::Path(path) => path.path.is_ident("None"),
        syn::Expr::Call(call) => {
            call_path(&call.func)
                .and_then(|path| path.segments.last())
                .is_some_and(|segment| matches!(segment.ident.to_string().as_str(), "Some" | "Ok"))
                && call.args.iter().any(collapses_to_bool_or_option)
        }
        syn::Expr::Return(returned) => returned
            .expr
            .as_ref()
            .is_some_and(|expression| collapses_to_bool_or_option(expression)),
        syn::Expr::Block(block) => block_collapses(&block.block),
        _ => false,
    }
}

fn expression_mentions_not_found(expression: &syn::Expr) -> bool {
    struct Finder {
        found: bool,
    }
    impl<'ast> Visit<'ast> for Finder {
        fn visit_path(&mut self, path: &'ast syn::Path) {
            if path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "NotFound")
            {
                self.found = true;
                return;
            }
            visit::visit_path(self, path);
        }
    }
    let mut finder = Finder { found: false };
    finder.visit_expr(expression);
    finder.found
}

fn block_collapses(block: &syn::Block) -> bool {
    block
        .stmts
        .iter()
        .filter_map(statement_expression)
        .any(collapses_to_bool_or_option)
}

fn pattern_contains_err(pattern: &syn::Pat) -> bool {
    match pattern {
        syn::Pat::TupleStruct(tuple) => {
            tuple
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "Err")
                || tuple.elems.iter().any(pattern_contains_err)
        }
        syn::Pat::Or(or) => or.cases.iter().any(pattern_contains_err),
        syn::Pat::Paren(paren) => pattern_contains_err(&paren.pat),
        syn::Pat::Reference(reference) => pattern_contains_err(&reference.pat),
        _ => false,
    }
}

fn collect_bindings(pattern: &syn::Pat, bindings: &mut Vec<String>) {
    match pattern {
        syn::Pat::Ident(ident) => bindings.push(ident.ident.to_string()),
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

fn call_path(expression: &syn::Expr) -> Option<&syn::Path> {
    let syn::Expr::Path(path) = peel_expression(expression) else {
        return None;
    };
    Some(&path.path)
}

fn path_ident(expression: &syn::Expr) -> Option<String> {
    let syn::Expr::Path(path) = peel_expression(expression) else {
        return None;
    };
    single_ident(&path.path)
}

fn single_ident(path: &syn::Path) -> Option<String> {
    (path.leading_colon.is_none() && path.segments.len() == 1).then(|| {
        path.segments
            .first()
            .expect("one path segment")
            .ident
            .to_string()
    })
}

fn path_ends_with(path: &syn::Path, expected: &[&str]) -> bool {
    let actual: Vec<String> = path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect();
    actual.len() >= expected.len()
        && actual[actual.len() - expected.len()..]
            .iter()
            .map(String::as_str)
            .eq(expected.iter().copied())
}

fn statement_expression(statement: &syn::Stmt) -> Option<&syn::Expr> {
    match statement {
        syn::Stmt::Expr(expression, _) => Some(expression),
        _ => None,
    }
}
