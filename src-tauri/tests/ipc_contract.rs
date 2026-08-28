//! The frontend↔backend command contract.
//!
//! Every IPC call is a string literal on one side (`invoke("foo")`) and
//! an identifier on the other (`generate_handler![foo]`). Nothing in the
//! compiler connects the two: a typo, a command added to `commands.rs`
//! but never registered, or one renamed on only one side all build
//! cleanly, pass every other test, and fail at runtime the first time a
//! user clicks the button.
//!
//! These tests close that gap by parsing all three artefacts and
//! cross-checking them. They are deliberately host-runnable — this is a
//! source-consistency property, not a Windows one, so it should fail on
//! the fast Linux matrix entry rather than waiting for Windows.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Every `.ts` / `.tsx` file under `src/`, excluding test files (a mock
/// in a test may reference a command that legitimately doesn't exist).
fn frontend_sources() -> Vec<(String, String)> {
    let dir = repo_root().join("src");
    let mut out = Vec::new();
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let is_source = name.ends_with(".ts") || name.ends_with(".tsx");
            let is_test = name.contains(".test.");
            if is_source && !is_test {
                if let Ok(text) = std::fs::read_to_string(&p) {
                    out.push((name, text));
                }
            }
        }
    }
    assert!(!out.is_empty(), "found no frontend sources under src/");
    out
}

/// Index of the `>` closing a generic argument list whose opening `<` has
/// already been consumed, honouring nesting.
fn end_of_generic(tail: &str) -> Option<usize> {
    let mut depth = 1usize;
    for (index, character) in tail.char_indices() {
        match character {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            // A generic argument list never spans a statement.
            ';' | '{' => return None,
            _ => {}
        }
    }
    None
}

/// Pull the command name out of `invoke("x")` and `invoke<T>("x")`.
fn invoked_commands() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for (_file, text) in frontend_sources() {
        let mut rest = text.as_str();
        while let Some(idx) = rest.find("invoke") {
            rest = &rest[idx + "invoke".len()..];
            // Skip an optional turbofish-style generic argument. The
            // argument can itself be generic (`invoke<Partial<Foo>>`), so
            // match angle brackets by depth rather than taking the first
            // `>` — stopping early leaves `>(` and silently drops the call,
            // which would make an invoked command look unreachable.
            let after_generic = match rest.strip_prefix('<') {
                Some(tail) => match end_of_generic(tail) {
                    Some(end) => &tail[end + 1..],
                    None => continue,
                },
                None => rest,
            };
            // Then expect `("command_name"`.
            let Some(tail) = after_generic.strip_prefix('(') else {
                continue;
            };
            let tail = tail.trim_start();
            let Some(tail) = tail.strip_prefix('"') else {
                continue;
            };
            let Some(end) = tail.find('"') else { continue };
            let name = &tail[..end];
            // Command names are snake_case identifiers; anything else is
            // an `invoke` that isn't a Tauri command call.
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                found.insert(name.to_string());
            }
        }
    }
    assert!(
        !found.is_empty(),
        "parsed zero invoke() calls — the parser is broken, not the code",
    );
    found
}

/// Names inside `tauri::generate_handler![...]` in `lib.rs`.
fn registered_handlers() -> BTreeSet<String> {
    let lib = read("src-tauri/src/lib.rs");
    let start = lib
        .find("generate_handler!")
        .expect("lib.rs must register commands via generate_handler!");
    let after = &lib[start..];
    let open = after.find('[').expect("generate_handler! opening bracket");
    let close = after.find(']').expect("generate_handler! closing bracket");
    let body = &after[open + 1..close];

    body.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            // Entries are written as `commands::foo`.
            s.rsplit("::").next().unwrap_or(s).trim().to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// `#[tauri::command]`-annotated function names in `commands.rs`.
fn defined_commands() -> BTreeSet<String> {
    let src = read("src-tauri/src/commands.rs");
    let mut out = BTreeSet::new();
    let mut lines = src.lines().peekable();
    while let Some(line) = lines.next() {
        if !line.trim().starts_with("#[tauri::command]") {
            continue;
        }
        // The fn signature is on one of the next couple of lines.
        for candidate in lines.clone().take(3) {
            let t = candidate.trim();
            let sig = t
                .strip_prefix("pub async fn ")
                .or_else(|| t.strip_prefix("pub fn "));
            if let Some(sig) = sig {
                if let Some(name) = sig.split('(').next() {
                    out.insert(name.trim().to_string());
                }
                break;
            }
        }
    }
    assert!(!out.is_empty(), "parsed zero #[tauri::command] functions");
    out
}

/// Find the matching closing delimiter, starting immediately after an opening
/// delimiter. The command signatures and invocation objects we inspect may
/// contain nested generic/argument objects, so taking the first `)` or `}`
/// would silently truncate the contract.
fn matching_delimiter(text: &str, opening: char, closing: char) -> Option<usize> {
    let mut depth = 1usize;
    for (index, character) in text.char_indices() {
        match character {
            c if c == opening => depth += 1,
            c if c == closing => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// Split at commas that are not inside a nested Rust type or object literal.
fn top_level_fields(text: &str) -> Vec<&str> {
    let mut fields = Vec::new();
    let mut start = 0usize;
    let mut angle_depth = 0usize;
    let mut paren_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut bracket_depth = 0usize;

    for (index, character) in text.char_indices() {
        match character {
            '<' => angle_depth += 1,
            '>' => angle_depth = angle_depth.saturating_sub(1),
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            ',' if angle_depth == 0
                && paren_depth == 0
                && brace_depth == 0
                && bracket_depth == 0 =>
            {
                fields.push(text[start..index].trim());
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    fields.push(text[start..].trim());
    fields
        .into_iter()
        .filter(|field| !field.is_empty())
        .collect()
}

fn contains_identifier(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(start, _)| {
        let before = text[..start].chars().next_back();
        let after = text[start + name.len()..].chars().next();
        let is_identifier = |character: char| character.is_ascii_alphanumeric() || character == '_';
        before.is_none_or(|character| !is_identifier(character))
            && after.is_none_or(|character| !is_identifier(character))
    })
}

fn argument_type_names(parameter: &str) -> Vec<String> {
    let Some((_, parameter_type)) = parameter.split_once(':') else {
        return parameter
            .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            .filter(|identifier| identifier.ends_with("Args"))
            .map(str::to_string)
            .collect();
    };

    parameter_type
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .filter(|identifier| identifier.ends_with("Args"))
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Inventory every one-line `struct` declaration whose name ends in `Args`,
/// independently of the exact public form the contract parser supports. This
/// is the completeness sentinel: changing `pub struct ProxyArgs` to
/// `pub(crate) struct ProxyArgs` must make the parser fail loudly, not make the
/// type disappear from its input set.
fn declared_argument_types(src: &str) -> BTreeMap<String, String> {
    let mut declarations = BTreeMap::new();

    for (index, line) in src.lines().enumerate() {
        let code = line.split_once("//").map_or(line, |(code, _)| code);
        let Some(struct_start) = code.find("struct") else {
            continue;
        };
        let before = code[..struct_start].chars().next_back();
        let after_keyword = &code[struct_start + "struct".len()..];
        let after = after_keyword.chars().next();
        if before.is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
            || after.is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            continue;
        }

        let name: String = after_keyword
            .trim_start()
            .chars()
            .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
            .collect();
        assert!(
            !name.is_empty(),
            "unparsed struct declaration at commands.rs:{}: {:?}",
            index + 1,
            line.trim(),
        );
        if !name.ends_with("Args") {
            continue;
        }

        let expected = format!("pub struct {name} {{");
        assert_eq!(
            code.trim(),
            expected,
            "unparsed *Args declaration for {name} at commands.rs:{}: {:?}; expected the supported `pub struct {name} {{` form",
            index + 1,
            line.trim(),
        );
        assert!(
            declarations
                .insert(
                    name.clone(),
                    format!("commands.rs:{}: {}", index + 1, line.trim())
                )
                .is_none(),
            "duplicate *Args declaration for {name}",
        );
    }

    assert!(
        !declarations.is_empty(),
        "found no *Args struct declarations in commands.rs — the declaration inventory is broken",
    );
    declarations
}

/// Derive every struct-argument command and its real outer parameter name from
/// `commands.rs`. Command parameters are inventoried independently of the
/// declarations above so moving an `*Args` type to another module cannot make
/// its command silently disappear. The supported syntax stays deliberately
/// narrow: unsupported forms fail by name instead of narrowing coverage.
fn backend_struct_argument_names() -> BTreeMap<String, (String, String)> {
    const ATTRIBUTE: &str = "#[tauri::command]";

    let src = read("src-tauri/src/commands.rs");
    let argument_types = declared_argument_types(&src);
    let mut matches: BTreeMap<String, Vec<(String, String)>> = argument_types
        .keys()
        .map(|name| (name.clone(), Vec::new()))
        .collect();

    let mut rest = src.as_str();
    while let Some(attribute) = rest.find(ATTRIBUTE) {
        let after_attribute = &rest[attribute + ATTRIBUTE.len()..];
        let block_end = after_attribute
            .find(ATTRIBUTE)
            .unwrap_or(after_attribute.len());
        let block = &after_attribute[..block_end];
        rest = &after_attribute[block_end..];

        let declaration = block.trim_start();
        let function = declaration.find("fn ").unwrap_or_else(|| {
            panic!(
                "unparsed #[tauri::command] function declaration: {:?}",
                declaration.lines().next().unwrap_or_default(),
            )
        });
        let discovered_signature = &declaration[function + "fn ".len()..];
        let discovered_open = discovered_signature.find('(').unwrap_or_else(|| {
            panic!(
                "unparsed #[tauri::command] function declaration: {:?}",
                declaration.lines().next().unwrap_or_default(),
            )
        });
        let discovered_command = discovered_signature[..discovered_open].trim();
        let discovered_tail = &discovered_signature[discovered_open + 1..];
        let discovered_close = matching_delimiter(discovered_tail, '(', ')').unwrap_or_else(|| {
            panic!("unterminated signature for {discovered_command}: {declaration:?}")
        });
        for parameter in top_level_fields(&discovered_tail[..discovered_close]) {
            for argument_type in argument_type_names(parameter) {
                assert!(
                    argument_types.contains_key(&argument_type),
                    "{discovered_command} parameter {parameter:?} uses {argument_type}, but that type is missing from the *Args declaration inventory built from src-tauri/src/commands.rs; move {argument_type} back into commands.rs or teach declared_argument_types where to scan before moving it",
                );
            }
        }

        let Some(signature) = declaration.strip_prefix("pub async fn ") else {
            let mentioned: Vec<_> = argument_types
                .keys()
                .filter(|name| contains_identifier(block, name))
                .cloned()
                .collect();
            assert!(
                mentioned.is_empty(),
                "unparsed #[tauri::command] declaration using {mentioned:?}: {:?}; expected the supported `pub async fn` form",
                declaration.lines().next().unwrap_or_default(),
            );
            continue;
        };
        let open = signature.find('(').unwrap_or_else(|| {
            panic!(
                "unparsed #[tauri::command] function declaration: {:?}",
                declaration.lines().next().unwrap_or_default(),
            )
        });
        let command = signature[..open].trim();
        let tail = &signature[open + 1..];
        let close = matching_delimiter(tail, '(', ')')
            .unwrap_or_else(|| panic!("unterminated signature for {command}: {declaration:?}"));
        let parameters = &tail[..close];

        for parameter in top_level_fields(parameters) {
            let mentioned = argument_type_names(parameter);
            if mentioned.is_empty() {
                continue;
            }
            assert_eq!(
                mentioned.len(),
                1,
                "unparsed *Args parameter in {command}: {parameter:?}; mentioned types = {mentioned:?}",
            );
            let argument_type = &mentioned[0];
            let (parameter_name, parameter_type) = parameter.split_once(':').unwrap_or_else(|| {
                panic!(
                    "unparsed {argument_type} parameter in {command}: {parameter:?}; expected `name: {argument_type}`",
                )
            });
            assert_eq!(
                parameter_type.trim(),
                argument_type,
                "unparsed {argument_type} parameter type in {command}: {parameter:?}; expected the unqualified type `{argument_type}`",
            );
            matches
                .get_mut(argument_type)
                .expect("declared argument type has a match bucket")
                .push((command.to_string(), parameter_name.trim().to_string()));
        }
    }

    let mut found = BTreeMap::new();
    for (argument_type, declaration) in argument_types {
        let type_matches = matches
            .remove(&argument_type)
            .expect("declared argument type has a match bucket");
        assert_eq!(
            type_matches.len(),
            1,
            "{argument_type} declared at {declaration} must match exactly one supported #[tauri::command] parameter, found {type_matches:?}",
        );
        let (command, parameter_name) = type_matches
            .into_iter()
            .next()
            .expect("exactly one match was asserted");
        assert!(
            found
                .insert(command.clone(), (argument_type.clone(), parameter_name))
                .is_none(),
            "multiple *Args types matched the same command {command}",
        );
    }
    found
}

#[derive(Debug)]
struct FrontendInvocation {
    callsite: String,
    outer_names: Result<Vec<String>, String>,
}

/// Derive outer object keys from every actual frontend `invoke` callsite. This
/// is intentionally not an expected-name table: the test below compares every
/// real call directly with the real Rust parameter identifier.
fn frontend_invocation_outer_names() -> BTreeMap<String, Vec<FrontendInvocation>> {
    let mut found: BTreeMap<String, Vec<FrontendInvocation>> = BTreeMap::new();
    for (file, text) in frontend_sources() {
        let mut cursor = 0usize;
        while let Some(relative_invoke) = text[cursor..].find("invoke") {
            let invoke = cursor + relative_invoke;
            cursor = invoke + "invoke".len();
            let rest = &text[cursor..];
            let after_generic = match rest.strip_prefix('<') {
                Some(tail) => match end_of_generic(tail) {
                    Some(end) => &tail[end + 1..],
                    None => continue,
                },
                None => rest,
            };
            let Some(tail) = after_generic.strip_prefix('(') else {
                continue;
            };
            let tail = tail.trim_start();
            let Some(tail) = tail.strip_prefix('"') else {
                continue;
            };
            let Some(name_end) = tail.find('"') else {
                continue;
            };
            let command = &tail[..name_end];
            let callsite = format!("{file}:{}", text[..invoke].lines().count());
            let after_name = tail[name_end + 1..].trim_start();
            let outer_names = match after_name.strip_prefix(',') {
                None => Err("has no argument object".to_string()),
                Some(after_comma) => match after_comma.trim_start().strip_prefix('{') {
                    None => Err(format!(
                        "has an unsupported non-object argument: {:?}; inline the argument envelope as an object at this callsite, or extend frontend_invocation_outer_names to support this form",
                        after_comma.trim_start().lines().next().unwrap_or_default(),
                    )),
                    Some(object) => {
                        let close = matching_delimiter(object, '{', '}').unwrap_or_else(|| {
                            panic!(
                                "unterminated invoke argument object for {command} at {callsite}"
                            )
                        });
                        Ok(top_level_fields(&object[..close])
                            .into_iter()
                            .map(|field| {
                                field
                                    .split_once(':')
                                    .map_or(field, |(key, _)| key)
                                    .trim()
                                    .to_string()
                            })
                            .collect())
                    }
                },
            };
            found
                .entry(command.to_string())
                .or_default()
                .push(FrontendInvocation {
                    callsite,
                    outer_names,
                });
        }
    }
    found
}

#[test]
fn every_invoked_command_is_registered() {
    let invoked = invoked_commands();
    let registered = registered_handlers();

    let missing: Vec<_> = invoked.difference(&registered).cloned().collect();
    assert!(
        missing.is_empty(),
        "the frontend invokes commands that are not in generate_handler![]: {missing:?}\n\
         These compile fine and fail only at runtime, when the user clicks the thing.\n\
         registered = {registered:?}",
    );
}

#[test]
fn every_registered_handler_is_a_real_command() {
    let registered = registered_handlers();
    let defined = defined_commands();

    let missing: Vec<_> = registered.difference(&defined).cloned().collect();
    assert!(
        missing.is_empty(),
        "generate_handler![] names functions that aren't #[tauri::command] in commands.rs: {missing:?}",
    );
}

#[test]
fn struct_argument_outer_names_match_across_the_real_boundary_sources() {
    let backend = backend_struct_argument_names();
    let frontend = frontend_invocation_outer_names();

    for (command, (argument_type, rust_name)) in backend {
        let invocations = frontend.get(&command).unwrap_or_else(|| {
            panic!("{argument_type} maps to {command}, but no frontend invoke callsite was found")
        });
        assert!(
            !invocations.is_empty(),
            "{command} has no frontend callsites"
        );
        for invocation in invocations {
            let frontend_names = invocation.outer_names.as_ref().unwrap_or_else(|problem| {
                panic!(
                    "{argument_type} maps to {command}, but its frontend invoke at {} {problem}",
                    invocation.callsite,
                )
            });
            assert_eq!(
                frontend_names,
                &[rust_name.as_str()],
                "struct-argument IPC outer-name mismatch for {command} at {}: Rust parameter is \
                 {rust_name:?}, frontend invoke keys are {frontend_names:?}",
                invocation.callsite,
            );
        }
    }
}

/// A command defined and registered but never invoked is either dead
/// code or a frontend wiring bug. This is a warning-shaped test: it
/// keeps a known-unused allowlist so intentional cases are explicit
/// rather than silently tolerated.
#[test]
fn no_unreachable_commands() {
    // Commands intentionally not called from the frontend today.
    // Add to this list *with a reason* rather than deleting the test.
    const KNOWN_UNUSED: &[&str] = &[];

    let invoked = invoked_commands();
    let registered = registered_handlers();

    let unused: Vec<_> = registered
        .difference(&invoked)
        .filter(|c| !KNOWN_UNUSED.contains(&c.as_str()))
        .cloned()
        .collect();

    assert!(
        unused.is_empty(),
        "these commands are registered but never invoked from the frontend: {unused:?}\n\
         Either wire them up, delete them, or add them to KNOWN_UNUSED with a reason.",
    );
}

/// The scanner above is the only thing standing between a renamed or
/// unwired command and a silent break, so its own blind spots matter.
/// A generic argument that is itself generic used to stop the scan at the
/// inner `>`, dropping the call entirely — the command then looked
/// unreachable, and an unregistered one would have looked fine.
#[test]
fn the_scanner_reads_a_nested_generic_invoke() {
    assert_eq!(end_of_generic("Partial<MoveReport>>(\"x\")"), Some(19));
    assert_eq!(end_of_generic("MoveReport>(\"x\")"), Some(10));
    // An unterminated generic must not swallow the rest of the file.
    assert_eq!(end_of_generic("Broken(\"x\");\nnext()"), None);
}

/// The IPC readiness marker (issue #54) is only worth anything while
/// three things stay true: the frontend still invokes the command that
/// emits it, that command is still registered, and the installer smoke
/// still looks for the same string. Each can drift independently, and
/// every drift turns the smoke back into "the Rust side started",
/// silently.
mod ipc_readiness_marker {
    use super::{invoked_commands, read, registered_handlers};
    use gmm_lib::core::diagnostics::{IPC_READY_COMMAND, IPC_READY_MARKER};

    #[test]
    fn the_marker_command_is_registered_and_invoked_by_the_frontend() {
        assert!(
            registered_handlers().contains(IPC_READY_COMMAND),
            "{IPC_READY_COMMAND} carries the IPC readiness marker but is not in \
             generate_handler![] — the installer smoke would fail on every build",
        );
        assert!(
            invoked_commands().contains(IPC_READY_COMMAND),
            "{IPC_READY_COMMAND} carries the IPC readiness marker but the frontend \
             never invokes it — nothing would ever emit the marker",
        );
    }

    #[test]
    fn the_marker_command_emits_the_marker() {
        let commands = read("src-tauri/src/commands.rs");
        let start = commands
            .find(&format!("pub async fn {IPC_READY_COMMAND}("))
            .or_else(|| commands.find(&format!("pub fn {IPC_READY_COMMAND}(")))
            .unwrap_or_else(|| panic!("{IPC_READY_COMMAND} not found in commands.rs"));
        // The body ends at the next `#[tauri::command]`, or EOF.
        let rest = &commands[start..];
        let body_end = rest.find("#[tauri::command]").unwrap_or(rest.len());
        assert!(
            rest[..body_end].contains("record_ipc_ready"),
            "{IPC_READY_COMMAND} must call diagnostics::record_ipc_ready — that call \
             is the only thing that writes the marker the installer smoke waits for",
        );
    }

    #[test]
    fn every_windows_script_waits_for_the_same_marker() {
        // Three scripts now gate on this literal, not one. Each is the
        // only place its own layer proves the WebView actually reached
        // the backend, and none of them can be checked from a
        // non-Windows host — so a renamed constant would drop the check
        // silently and nothing would notice until a broken bundle
        // shipped.
        for script_path in [
            ".github/scripts/installer-smoke.ps1",
            ".github/scripts/updater-e2e.ps1",
            ".github/scripts/installer-lifecycle.ps1",
        ] {
            let script = read(script_path);
            assert!(
                script.contains(IPC_READY_MARKER),
                "{script_path} must grep for the marker literal \
                 '{IPC_READY_MARKER}' — otherwise renaming the constant \
                 quietly drops the check",
            );
        }
    }

    #[test]
    fn the_startup_checks_require_a_marker_from_this_launch() {
        // The subtle version of the same failure. GMM's logs are not
        // cleared between launches, so "does the marker appear anywhere
        // in the logs" is satisfied instantly by the *previous*
        // launch's line — an app that crashed on startup would pass.
        //
        // Both multi-launch scripts must therefore compare a count
        // taken before starting the process against one taken after,
        // rather than testing for mere presence. `installer-smoke.ps1`
        // is exempt: it launches exactly once, from a data directory it
        // has just deleted.
        for script_path in [
            ".github/scripts/updater-e2e.ps1",
            ".github/scripts/installer-lifecycle.ps1",
        ] {
            let script = read(script_path);
            assert!(
                script.contains("Get-IpcMarkerCount"),
                "{script_path} launches the app more than once, so it must \
                 require a *new* marker rather than any marker — see \
                 Get-IpcMarkerCount",
            );
        }
    }
}

/// The packaged startup smoke holds the manifest endpoint open and proves
/// both that this launch started the refresh and that IPC became ready
/// without waiting for the response. Keep the Rust marker and the script's
/// literal/counting contract in lockstep.
mod manifest_refresh_started_marker {
    use super::read;
    use gmm_lib::core::diagnostics::MANIFEST_REFRESH_STARTED_MARKER;
    use gmm_lib::core::recommended_importers::{
        MANIFEST_URL_OVERRIDE_ENV, PACKAGED_SMOKE_FETCH_TIMEOUT,
    };
    use std::time::Duration;

    // This is intentionally a narrow structural scanner, not a PowerShell
    // parser. The pinned fixture functions contain no braces in strings or
    // comments, so balanced braces identify their bodies without pretending
    // to validate arbitrary PowerShell syntax.
    fn powershell_function_body<'a>(script: &'a str, name: &str) -> &'a str {
        let declaration = format!("function {name}");
        let declaration_start = script
            .find(&declaration)
            .unwrap_or_else(|| panic!("PowerShell function {name} is not declared"));
        let body_start = script[declaration_start..]
            .find('{')
            .map(|offset| declaration_start + offset)
            .unwrap_or_else(|| panic!("PowerShell function {name} has no body"));

        let mut depth = 0usize;
        for (offset, character) in script[body_start..].char_indices() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &script[body_start + 1..body_start + offset];
                    }
                }
                _ => {}
            }
        }
        panic!("PowerShell function {name} has an unterminated body");
    }

    fn command_positions(body: &str, command: &str) -> Vec<usize> {
        let mut positions = Vec::new();
        let mut line_start = 0usize;
        for line in body.split_inclusive('\n') {
            let indentation = line.len() - line.trim_start().len();
            let trimmed = line.trim();
            if trimmed == command
                || trimmed
                    .strip_prefix(command)
                    .is_some_and(|tail| tail.starts_with(char::is_whitespace))
            {
                positions.push(line_start + indentation);
            }
            line_start += line.len();
        }
        positions
    }

    fn position(body: &str, needle: &str) -> usize {
        body.find(needle)
            .unwrap_or_else(|| panic!("expected PowerShell structure not found: {needle}"))
    }

    #[test]
    fn installer_smoke_counts_the_same_marker_for_this_launch() {
        let script = read(".github/scripts/installer-smoke.ps1");
        let startup = powershell_function_body(&script, "Invoke-StartupSmoke");

        // These literal assertions pin cross-language names and diagnostic
        // wording only. They do not prove the corresponding commands execute;
        // the ordered invocation checks in the tests below provide that proof.
        assert!(
            script.contains(MANIFEST_REFRESH_STARTED_MARKER),
            "installer-smoke.ps1 must retain the marker literal \
             '{MANIFEST_REFRESH_STARTED_MARKER}' for the cross-language contract",
        );
        assert!(script.contains(
            "manifest refresh client closed its held request before the fixture released its response"
        ));
        assert!(startup
            .contains("IPC readiness did not occur while the manifest request remained held open"));
        assert!(startup.contains("startup work blocked Tauri past the deadline"));

        let count_before = position(
            startup,
            "$manifestRefreshBefore = Get-DiagnosticMarkerCount $ManifestRefreshStartedMarker",
        );
        let launch = position(startup, "$script:AppProc = Start-Process $exe -PassThru");
        let count_after = position(
            startup,
            "(Get-DiagnosticMarkerCount $ManifestRefreshStartedMarker) -gt",
        );
        assert!(
            count_before < launch && launch < count_after,
            "installer-smoke.ps1 must capture the manifest marker count before launch \
             and compare it after launch",
        );

        let set_override = position(startup, "[System.Environment]::SetEnvironmentVariable(");
        let restore_override = startup[launch..]
            .find("[System.Environment]::SetEnvironmentVariable(")
            .map(|offset| launch + offset)
            .expect("installer smoke must restore the manifest URL override after launch");
        assert!(
            set_override < launch && launch < restore_override,
            "installer-smoke.ps1 must set {MANIFEST_URL_OVERRIDE_ENV} before launch \
             and restore it afterward",
        );
        assert!(startup[set_override..launch].contains("$ManifestUrlOverrideEnv"));
        assert!(startup[set_override..launch].contains("$manifestUrl"));
        assert!(startup[restore_override..].contains("$previousManifestUrl"));
    }

    #[test]
    fn installer_smoke_invokes_accept_health_checks_on_the_live_path() {
        let script = read(".github/scripts/installer-smoke.ps1");
        let accept_guard = powershell_function_body(&script, "Assert-ManifestFixtureAcceptHealthy");
        let faulted = position(accept_guard, "if ($acceptTask.IsFaulted)");
        let fault_class = accept_guard[faulted..]
            .find("$script:FailureClass = \"INFRASTRUCTURE\"")
            .map(|offset| faulted + offset)
            .expect("faulted accept must be classified INFRASTRUCTURE");
        let fault_throw = accept_guard[fault_class..]
            .find("throw \"manifest fixture accept faulted")
            .map(|offset| fault_class + offset)
            .expect("faulted accept must stop the smoke");
        let canceled = position(accept_guard, "if ($acceptTask.IsCanceled)");
        let cancel_class = accept_guard[canceled..]
            .find("$script:FailureClass = \"INFRASTRUCTURE\"")
            .map(|offset| canceled + offset)
            .expect("canceled accept must be classified INFRASTRUCTURE");
        let cancel_throw = accept_guard[cancel_class..]
            .find("throw \"manifest fixture accept was canceled")
            .map(|offset| cancel_class + offset)
            .expect("canceled accept must stop the smoke");
        assert!(faulted < fault_class && fault_class < fault_throw);
        assert!(canceled < cancel_class && cancel_class < cancel_throw);

        let startup = powershell_function_body(&script, "Invoke-StartupSmoke");
        let calls = command_positions(startup, "Assert-ManifestFixtureAcceptHealthy");
        assert_eq!(
            calls.len(),
            2,
            "Invoke-StartupSmoke must invoke the accept-health guard in the polling \
             loop and again after the loop before classifying startup"
        );
        let polling_loop = position(startup, "while ((Get-Date) -lt $deadline)");
        let first_observation = position(startup, "if (-not $dbSeen -and");
        let after_loop = position(startup, "if (-not $dbSeen) { throw");
        let product_classification = position(
            startup,
            "if (-not $ipcSeen -and $manifestRefreshSeen -and $manifestRequestSeen)",
        );
        assert!(polling_loop < calls[0] && calls[0] < first_observation);
        assert!(after_loop < calls[1] && calls[1] < product_classification);

        let launch = position(startup, "$script:AppProc = Start-Process $exe -PassThru");
        let fault_injection = position(
            startup,
            "if ($FixtureMode -eq \"unavailable-after-launch\")",
        );
        assert!(
            launch < fault_injection && fault_injection < polling_loop,
            "the post-launch accept-fault proof seam must run after launch and before polling"
        );
    }

    #[test]
    fn installer_smoke_invokes_peer_checks_through_fixture_release() {
        let script = read(".github/scripts/installer-smoke.ps1");
        let peer_guard = powershell_function_body(&script, "Assert-ManifestFixturePeerConnected");
        let faulted = position(peer_guard, "if ($script:ManifestPeerReadTask.IsFaulted)");
        let canceled = position(peer_guard, "if ($script:ManifestPeerReadTask.IsCanceled)");
        let read_result = position(peer_guard, "$bytesRead =");
        let eof = position(peer_guard, "if ($bytesRead -eq 0)");
        let next_read = peer_guard
            .rfind("$script:ManifestPeerReadTask = $stream.ReadAsync(")
            .expect("peer monitor must leave another read pending after request bytes");
        assert!(faulted < canceled && canceled < read_result && read_result < eof);
        assert!(eof < next_read);
        assert_eq!(
            peer_guard
                .matches("$script:FailureClass = \"PRODUCT\"")
                .count(),
            3,
            "faulted, canceled, and zero-byte reads must each classify the close as PRODUCT"
        );
        assert_eq!(
            peer_guard
                .matches("throw $ManifestPeerClosedMessage")
                .count(),
            2,
            "canceled and zero-byte reads must each stop the smoke"
        );

        let startup = powershell_function_body(&script, "Invoke-StartupSmoke");
        let startup_calls = command_positions(startup, "Assert-ManifestFixturePeerConnected");
        assert_eq!(
            startup_calls.len(),
            2,
            "Invoke-StartupSmoke must invoke the peer guard while polling and again \
             after the loop before declaring the held request healthy"
        );
        let monitor_start = position(startup, "Start-ManifestFixturePeerMonitor");
        let premature_finish = position(startup, "if ($manifestRefreshFinishedSeen)");
        let request_required = position(startup, "if (-not $manifestRequestSeen)");
        let direct_assertion = position(
            startup,
            "IPC readiness observed while manifest request was held open and unanswered",
        );
        assert!(monitor_start < startup_calls[0] && startup_calls[0] < premature_finish);
        assert!(request_required < startup_calls[1] && startup_calls[1] < direct_assertion);

        let release = powershell_function_body(&script, "Complete-ManifestFixtureRequest");
        let release_calls = command_positions(release, "Assert-ManifestFixturePeerConnected");
        assert_eq!(
            release_calls.len(),
            2,
            "Complete-ManifestFixtureRequest must invoke the peer guard immediately \
             before writing and again after flushing before discarding the read task"
        );
        let get_stream = position(
            release,
            "$stream = $script:HeldManifestConnection.GetStream()",
        );
        let close_window_proof = position(
            release,
            "if ($FixtureMode -eq \"pause-after-prewrite-peer-check\")",
        );
        let writes: Vec<_> = release
            .match_indices("$stream.Write(")
            .map(|(i, _)| i)
            .collect();
        let flushes: Vec<_> = release
            .match_indices("$stream.Flush()")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            writes.len(),
            2,
            "fixture release must split prefix and final-byte writes"
        );
        assert_eq!(
            flushes.len(),
            2,
            "both fixture response writes must be flushed"
        );
        let dispose = position(release, "$script:HeldManifestConnection.Dispose()");
        assert!(get_stream < release_calls[0] && release_calls[0] < close_window_proof);
        assert!(close_window_proof < writes[0] && writes[0] < flushes[0]);
        assert!(flushes[0] < release_calls[1] && release_calls[1] < writes[1]);
        assert!(writes[1] < flushes[1] && flushes[1] < dispose);
    }

    #[test]
    fn installer_smoke_releases_only_after_direct_startup_proof() {
        let script = read(".github/scripts/installer-smoke.ps1");
        let startup = powershell_function_body(&script, "Invoke-StartupSmoke");
        let direct_assertion = position(
            startup,
            "IPC readiness observed while manifest request was held open and unanswered",
        );
        let fixture_release = command_positions(startup, "Complete-ManifestFixtureRequest")
            .into_iter()
            .next()
            .expect("installer smoke must release the fixture response after its assertion");
        assert!(
            direct_assertion < fixture_release,
            "the held response must be released only after IPC readiness is proven",
        );
        assert!(
            PACKAGED_SMOKE_FETCH_TIMEOUT > Duration::from_secs(90),
            "the loopback client's timeout must outlast the smoke startup deadline",
        );
        assert!(
            !script.contains("foreach ($attempt in 1..2)"),
            "a product assertion must not be discarded by retrying the launch",
        );
        assert!(
            !script.contains("$manifestRequestAcceptedAt.AddSeconds("),
            "the startup guard must not depend on an arbitrary wall-clock margin",
        );
    }

    #[test]
    fn startup_reads_the_override_only_through_the_loopback_guard() {
        let lib = read("src-tauri/src/lib.rs");
        assert!(
            lib.contains("recommended_importers::loopback_manifest_url_override();"),
            "startup must obtain the test URL through the no-argument accessor that \
             reads and validates the environment value together",
        );
        assert!(
            !lib.contains("MANIFEST_URL_OVERRIDE_ENV"),
            "lib.rs must not name the manifest override environment variable \
             directly, where the loopback-only validator could be bypassed",
        );
        assert!(
            lib.contains("refresh_recommended_importers_from_loopback_override(&url)"),
            "the validated URL must use the refresh path that refuses redirects, \
             not the shipped URL path that deliberately follows them",
        );
    }

    #[test]
    fn startup_refresh_emits_the_marker_at_the_kickoff_site() {
        let lib = read("src-tauri/src/lib.rs");
        let marker_call = "diagnostics::record_manifest_refresh_started();";
        let marker_end = lib
            .find(marker_call)
            .map(|start| start + marker_call.len())
            .expect("startup refresh marker call not found in lib.rs");
        let refresh_start = lib[marker_end..]
            .find("let refresh = match")
            .map(|offset| marker_end + offset)
            .expect("startup refresh selection not found after its marker call");
        assert!(
            lib[marker_end..refresh_start].trim().is_empty(),
            "the startup refresh must emit its diagnostic marker immediately \
             before choosing and polling the refresh future",
        );
    }
}
