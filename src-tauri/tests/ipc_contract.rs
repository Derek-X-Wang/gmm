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
    use gmm_lib::core::recommended_importers::MANIFEST_URL_OVERRIDE_ENV;

    #[test]
    fn installer_smoke_counts_the_same_marker_for_this_launch() {
        let script = read(".github/scripts/installer-smoke.ps1");
        assert!(
            script.contains(MANIFEST_REFRESH_STARTED_MARKER),
            "installer-smoke.ps1 must count the marker literal \
             '{MANIFEST_REFRESH_STARTED_MARKER}'",
        );
        assert!(
            script.contains("Get-DiagnosticMarkerCount"),
            "installer-smoke.ps1 must compare marker counts so an old log line \
             cannot satisfy the current launch",
        );
        assert!(
            script.contains(MANIFEST_URL_OVERRIDE_ENV),
            "installer-smoke.ps1 must set {MANIFEST_URL_OVERRIDE_ENV} to the \
             endpoint it deliberately holds open",
        );
        assert!(
            script.contains("Get-DiagnosticEventTimestamp"),
            "installer-smoke.ps1 must compare structured log timestamps rather \
             than impose a machine-speed deadline",
        );
        assert!(
            script.contains("$ipcReadyAt -ge $manifestRefreshFinishedAt"),
            "installer-smoke.ps1 must require IPC readiness before the held-open \
             refresh reaches its own terminal event",
        );
        assert!(
            !script.contains("$manifestRequestAcceptedAt.AddSeconds("),
            "the startup guard must not depend on an arbitrary wall-clock margin",
        );
        assert!(
            script.contains("startup work blocked Tauri past the deadline"),
            "the ordinary readiness timeout must name blocking startup work as a \
             possible cause rather than diagnosing only frontend/IPC failures",
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
