//! The CI-invocation contract: every test CI claims to run must exist.
//!
//! Some checks are too slow, too networked or too Windows-bound to run on
//! every pull request, so they are `#[ignore]`d and invoked by name from a
//! workflow or a CI script:
//!
//! ```text
//! cargo test --test importer_sources -- --ignored --exact some_test_name
//! ```
//!
//! Nothing connects that string to the Rust identifier. `libtest` treats
//! `--exact <name>` as a *filter*: a name that matches nothing filters
//! every test out, prints `running 0 tests`, and **exits 0**. A renamed or
//! mistyped test therefore turns the job green while asserting nothing —
//! the job still appears in the workflow list, still takes minutes, still
//! reports success, and covers exactly nothing.
//!
//! That is not hypothetical. The scheduled `upstream importers` workflow
//! named `every_importer_profile_resolves_to_a_live_release_with_a_matching_asset`,
//! which has never existed; its production runs logged
//! `test result: ok. 0 passed; 0 failed; 3 filtered out` and went green.
//! The live Model Importer release contract was unguarded the entire time
//! it looked guarded.
//!
//! These tests close that gap the way `ipc_contract.rs` closes the
//! frontend↔backend one: by parsing both artefacts and cross-checking
//! them. Host-runnable on purpose — this is a source-consistency property,
//! so it should fail on the fast Linux matrix entry rather than a week
//! later on a cron.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .to_path_buf()
}

/// One `cargo test …` command found in a CI artefact.
#[derive(Debug)]
struct Invocation {
    /// The file it was found in, relative to the repository root.
    origin: String,
    /// `--test <target>`, when the invocation names one.
    target: Option<String>,
    /// Every `--exact <name>` the invocation passes.
    exact: Vec<String>,
}

/// Everything under `.github/` that could invoke `cargo test`: workflow
/// YAML, PowerShell, shell. Recursive, so a new subdirectory is covered
/// without editing this list.
fn ci_files() -> Vec<(String, String)> {
    let root = repo_root().join(".github");
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let entries =
            std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(ext, "yml" | "yaml" | "ps1" | "sh" | "bash") {
                continue;
            }
            let rel = path
                .strip_prefix(repo_root())
                .expect("under repo root")
                .to_string_lossy()
                .to_string();
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            out.push((rel, text));
        }
    }
    assert!(
        !out.is_empty(),
        "found no CI files under .github/ — this test cannot be vacuous",
    );
    out.sort();
    out
}

/// Split a CI file on the literal `cargo test` and read the flags out of
/// each invocation.
///
/// Deliberately a tokeniser rather than a YAML/PowerShell parser: the same
/// command is written as a YAML folded scalar in one file and a
/// backtick-continued PowerShell line in another, and both must be
/// covered. Whitespace tokenising with the continuation characters dropped
/// handles every form the repository uses, and a form it does not handle
/// yields *no* invocation rather than a wrong one — which would show up as
/// a failure of `every_ignored_test_is_invoked_by_something`, not as a
/// silent pass.
fn invocations() -> Vec<Invocation> {
    let mut out = Vec::new();
    for (origin, text) in ci_files() {
        for chunk in text.split("cargo test").skip(1) {
            // The invocation ends at the first blank line, a YAML key, or
            // the start of the next step — approximated by the first line
            // that is not a continuation of the command.
            let mut tokens: Vec<&str> = Vec::new();
            for raw_line in chunk.lines() {
                let line = raw_line.trim();
                // A comment or an empty line ends the command, unless the
                // previous line explicitly continued it.
                let continued = tokens
                    .last()
                    .is_some_and(|t| *t == "--" || t.starts_with('-'))
                    || tokens.is_empty();
                if line.is_empty() || line.starts_with('#') {
                    if !continued {
                        break;
                    }
                    continue;
                }
                tokens.extend(
                    line.split_whitespace()
                        // PowerShell backtick and shell backslash line
                        // continuations, and YAML's folded-scalar noise.
                        .filter(|t| !matches!(*t, "`" | "\\" | ">-" | "|")),
                );
                if !line.ends_with('`') && !line.ends_with('\\') && !line.ends_with("--") {
                    // Heuristic end-of-command: a line that neither
                    // continues nor ends in a bare `--`.
                    if !tokens.last().is_some_and(|t| t.starts_with('-')) {
                        break;
                    }
                }
            }

            let value_after = |flag: &str| -> Option<String> {
                tokens
                    .iter()
                    .position(|t| *t == flag)
                    .and_then(|i| tokens.get(i + 1))
                    .map(|s| s.trim_matches(|c| c == '"' || c == '\'').to_string())
            };

            let exact = tokens
                .iter()
                .enumerate()
                .filter(|(_, t)| **t == "--exact")
                .filter_map(|(i, _)| tokens.get(i + 1))
                .map(|s| s.trim_matches(|c| c == '"' || c == '\'').to_string())
                .collect();

            out.push(Invocation {
                origin: origin.clone(),
                target: value_after("--test"),
                exact,
            });
        }
    }
    out
}

/// Every `#[test]` / `#[tokio::test]` function in the workspace's
/// integration-test targets, as `target name -> [test fn names]`.
///
/// A "target" is a `.rs` file directly inside a `tests/` directory, which
/// is what `--test <name>` selects.
fn test_targets() -> BTreeMap<String, Vec<String>> {
    fn collect(dir: &Path, out: &mut BTreeMap<String, Vec<String>>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                if path.file_name().is_some_and(|n| n == "tests") {
                    let files = std::fs::read_dir(&path)
                        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
                    for file in files.flatten() {
                        let file = file.path();
                        if file.extension().and_then(|e| e.to_str()) != Some("rs") {
                            continue;
                        }
                        let name = file
                            .file_stem()
                            .expect("file stem")
                            .to_string_lossy()
                            .to_string();
                        let text = std::fs::read_to_string(&file)
                            .unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
                        out.entry(name).or_default().extend(test_fns(&text));
                    }
                }
                collect(&path, out);
            }
        }
    }

    let mut out = BTreeMap::new();
    collect(&repo_root().join("src-tauri"), &mut out);
    assert!(
        !out.is_empty(),
        "found no integration-test targets — this test cannot be vacuous",
    );
    out
}

/// The name of every test-attributed function in one source file.
fn test_fns(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for marker in ["#[test]", "#[tokio::test]"] {
        let mut from = 0usize;
        while let Some(at) = text[from..].find(marker) {
            let start = from + at + marker.len();
            // The signature follows within a few lines — other attributes
            // such as `#[ignore = "…"]` sit between.
            let window = &text[start..text.len().min(start + 400)];
            if let Some(sig) = window
                .find("fn ")
                .map(|i| &window[i + 3..])
                .and_then(|rest| rest.split('(').next())
            {
                let name = sig.trim();
                if !name.is_empty() && !name.contains(char::is_whitespace) {
                    out.push(name.to_string());
                }
            }
            from = start;
        }
    }
    out
}

/// The bug this file exists for: a CI job that names a test which does not
/// exist runs nothing and reports success.
#[test]
fn every_test_ci_names_by_hand_actually_exists() {
    let targets = test_targets();
    let mut problems = Vec::new();

    let mut checked = 0usize;
    for inv in invocations() {
        for name in &inv.exact {
            checked += 1;
            let found_in: Vec<&String> = targets
                .iter()
                .filter(|(_, fns)| fns.contains(name))
                .map(|(target, _)| target)
                .collect();

            if found_in.is_empty() {
                problems.push(format!(
                    "{}: `--exact {name}` names no test in the workspace, so the \
                     invocation filters every test out, prints `running 0 tests` \
                     and exits 0",
                    inv.origin,
                ));
                continue;
            }
            if let Some(target) = &inv.target {
                if !found_in.contains(&target) {
                    problems.push(format!(
                        "{}: `--exact {name}` is not in `--test {target}` (it lives \
                         in {found_in:?}), so the invocation runs 0 tests and exits 0",
                        inv.origin,
                    ));
                }
            }
        }
    }

    assert!(
        checked > 0,
        "no `--exact` test selection found under .github/ — either the \
         tokeniser stopped understanding how CI invokes cargo test, or the \
         invocations moved; either way this test has stopped guarding anything",
    );
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The other half: an `#[ignore]`d test that nothing invokes is dead
/// coverage. It compiles, it looks deliberate, and it never runs.
///
/// Two exemptions, both by intent rather than omission: a fixture
/// generator is meant to be run by a human, and the one-line reason on the
/// attribute says which case a test is.
#[test]
fn every_ignored_test_is_invoked_by_something() {
    let invoked: Vec<String> = invocations()
        .into_iter()
        .flat_map(|inv| inv.exact)
        .collect();

    let mut orphans = Vec::new();
    let mut stack = vec![repo_root().join("src-tauri")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            // Attribute position, not substring: an attribute always opens
            // its own line, whereas `#[ignore` inside a string literal or a
            // doc comment does not — and this file contains both.
            let lines: Vec<&str> = text.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                if !line.trim_start().starts_with("#[ignore") {
                    continue;
                }
                // `#[ignore = "regenerates …"]` — a generator a human runs.
                let reason = line.to_lowercase();
                if reason.contains("regenerat") || reason.contains("run deliberately") {
                    continue;
                }
                // The signature follows within a few lines; other
                // attributes may sit between.
                let Some(name) = lines[i + 1..]
                    .iter()
                    .take(6)
                    .find_map(|l| l.trim_start().split_once("fn "))
                    .and_then(|(_, rest)| rest.split('(').next())
                    .map(str::trim)
                else {
                    continue;
                };
                if !invoked.iter().any(|i| i == name) {
                    orphans.push(format!(
                        "{}: `{name}` is #[ignore]d and nothing under .github/ \
                         invokes it — it never runs anywhere",
                        path.strip_prefix(repo_root())
                            .unwrap_or(&path)
                            .to_string_lossy(),
                    ));
                }
            }
        }
    }

    orphans.sort();
    assert!(orphans.is_empty(), "{}", orphans.join("\n"));
}
