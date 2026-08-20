//! Offline shape validator for `manifest/recommended-importers.json`.
//!
//! ADR 0005 fetches that file at runtime on every released build, so a
//! bad commit reconfigures every install within minutes. The review gate
//! on `main` and this validator are the only things in the way — which
//! is why a malformed manifest must be *unmergeable*, and why every
//! failure names the offending game key or field.
//!
//! Deliberately **offline**. Checking that a recommended origin still
//! resolves belongs on a schedule, not on a pull request: putting a
//! third-party host on the critical path of merging makes the manifest
//! unmergeable whenever the GitHub API is rate-limited or an upstream
//! repository is briefly down (#94).
//!
//! It validates through the same [`gmm_lib::core::recommended_importers`]
//! type the app parses with, so a manifest this accepts is by
//! construction one the app can read.
//!
//! ```text
//! cargo run --bin validate-manifest              # the committed file
//! cargo run --bin validate-manifest -- <path>    # any file
//! ```
//!
//! Exit status is 0 on success and 1 on any failure.

use std::path::PathBuf;
use std::process::ExitCode;

use gmm_lib::core::recommended_importers::{self, MANIFEST_PATH};

fn main() -> ExitCode {
    let path = match std::env::args().nth(1) {
        Some(arg) => PathBuf::from(arg),
        // Default to the committed manifest, resolved from this
        // crate's location so the command works from any directory.
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root")
            .join(MANIFEST_PATH),
    };

    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) => {
            // Naming the path matters: a validator that cannot find the
            // file and exits 0 is worse than no validator at all,
            // because CI goes green on an unchecked manifest.
            eprintln!("could not read manifest at {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };

    match recommended_importers::validate(&raw) {
        Ok(manifest) => {
            println!(
                "{} is valid (schemaVersion {})",
                path.display(),
                manifest.schema_version(),
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{} is invalid: {e}", path.display());
            ExitCode::FAILURE
        }
    }
}
