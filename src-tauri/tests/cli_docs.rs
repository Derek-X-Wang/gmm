use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use syn::visit::Visit;

#[derive(Default)]
struct CommandCandidates(BTreeSet<String>);

impl<'ast> Visit<'ast> for CommandCandidates {
    fn visit_lit_str(&mut self, literal: &'ast syn::LitStr) {
        let value = literal.value();
        if !value.is_empty() && !value.starts_with('-') && !value.contains(char::is_whitespace) {
            self.0.insert(value);
        }
    }
}

fn collect_candidates(directory: &Path, candidates: &mut CommandCandidates) {
    for entry in std::fs::read_dir(directory).expect("read CLI source directory") {
        let path = entry.expect("read CLI source entry").path();
        if path.is_dir() {
            collect_candidates(&path, candidates);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let source = std::fs::read_to_string(path).expect("read CLI source");
            candidates.visit_file(&syn::parse_file(&source).expect("parse CLI source"));
        }
    }
}

#[test]
fn documented_command_table_matches_binary() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let doc = std::fs::read_to_string(manifest.parent().unwrap().join("docs/cli.md"))
        .expect("read CLI guide");
    let header = doc
        .lines()
        .position(|line| line == "| Command | Required options | Result |")
        .expect("CLI guide must contain the command table");
    let documented: BTreeSet<String> = doc
        .lines()
        .skip(header + 2)
        .take_while(|line| line.starts_with('|'))
        .map(|line| {
            line.split('|')
                .nth(1)
                .unwrap()
                .trim()
                .strip_prefix('`')
                .and_then(|cell| cell.strip_suffix('`'))
                .expect("command table entries must be backtick-quoted command names")
                .to_owned()
        })
        .collect();
    assert!(!documented.is_empty(), "command table must not be empty");

    // Source literals supply candidates, not the expected command set. Probe
    // the built binary so only commands its real parser recognises count.
    let mut candidates = CommandCandidates::default();
    collect_candidates(&manifest.join("crates/cli/src"), &mut candidates);
    candidates.0.extend(documented.iter().cloned());
    let test_exe = std::env::current_exe().expect("locate test executable");
    let binary = test_exe
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(format!("gmm-cli{}", std::env::consts::EXE_SUFFIX));
    let data = tempfile::tempdir().unwrap();
    let mut actual = BTreeSet::new();
    for candidate in candidates.0 {
        // An unsupported option keeps even no-argument commands in usage
        // validation, before the instance lock or any state is opened.
        let output = Command::new(&binary)
            .arg(&candidate)
            .args(["--gmm-doc-table-probe", "invalid", "--data-dir"])
            .arg(data.path())
            .output()
            .expect("run gmm-cli; build the workspace before testing");
        assert_eq!(output.status.code(), Some(1), "usage probe for {candidate}");
        let outcome: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("CLI usage response must be JSON");
        let message = outcome["error"]["message"].as_str().unwrap();
        if message != format!("unknown command {candidate}") {
            actual.insert(candidate);
        }
    }
    assert!(!actual.is_empty(), "binary must recognise commands");
    for command in &actual {
        assert!(
            documented.contains(command),
            "CLI command {command:?} is missing from docs/cli.md command table"
        );
    }
    for command in &documented {
        assert!(
            actual.contains(command),
            "Documented command {command:?} does not exist in the CLI binary"
        );
    }
}
