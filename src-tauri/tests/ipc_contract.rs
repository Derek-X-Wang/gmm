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

use std::collections::BTreeSet;
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

/// Pull the command name out of `invoke("x")` and `invoke<T>("x")`.
fn invoked_commands() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for (_file, text) in frontend_sources() {
        let mut rest = text.as_str();
        while let Some(idx) = rest.find("invoke") {
            rest = &rest[idx + "invoke".len()..];
            // Skip an optional turbofish-style generic argument.
            let after_generic = match rest.strip_prefix('<') {
                Some(tail) => match tail.find('>') {
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
