//! The signature half of the updater round trip (issue #56).
//!
//! `tests/updater_config.rs` asserts the shipped `tauri.conf.json` has a
//! well-formed pubkey and HTTPS endpoints. That is configuration shape:
//! it cannot tell whether an artifact the release pipeline produces is
//! one the shipped app would actually accept, nor whether a tampered one
//! would be refused.
//!
//! These tests close that by running the real toolchain end to end on a
//! **throwaway keypair** — `tauri signer generate` makes the key,
//! `tauri signer sign` produces the `.sig`, and the assertion re-runs
//! the exact verification `tauri-plugin-updater` performs:
//!
//! ```ignore
//! let pub_key = PublicKey::decode(&base64_decode(pubkey)?)?;
//! let sig = Signature::decode(&base64_decode(release_signature)?)?;
//! pub_key.verify(data, &sig, true)?;
//! ```
//!
//! (`tauri-plugin-updater-2.10.1/src/updater.rs::verify_signature`.)
//!
//! The real signing key is a release-only secret and is never touched
//! here; nothing in this file reads `TAURI_SIGNING_PRIVATE_KEY`.
//!
//! The install half — a signed artifact actually upgrading an installed
//! GMM with `%APPDATA%\GMM` intact — needs a real MSI and lives in
//! `.github/scripts/updater-e2e.ps1`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use base64::Engine as _;
use minisign_verify::{PublicKey, Signature};
use tempfile::TempDir;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .to_path_buf()
}

/// Run the Tauri CLI the same way the release workflow does. `pnpm`
/// rather than a vendored binary so the test exercises the version the
/// repo actually pins.
///
/// On Windows pnpm is a `.cmd` shim, and `Command` only ever appends
/// `.exe` when resolving a bare name — so it has to go through the shell
/// there or the call fails with "program not found".
fn tauri_cli(args: &[&str]) {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "pnpm"]);
        c
    } else {
        Command::new("pnpm")
    };
    let out = cmd
        .arg("--silent")
        .arg("tauri")
        .args(args)
        .current_dir(repo_root())
        .output()
        .unwrap_or_else(|e| panic!("run `pnpm tauri {}`: {e}", args.join(" ")));
    assert!(
        out.status.success(),
        "`pnpm tauri {}` failed:\n{}\n{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// A throwaway keypair plus a signed stand-in artifact. Generating a key
/// and signing costs a couple of seconds, so the whole fixture is built
/// once and shared; every test copies what it needs before mutating.
struct Signed {
    _tmp: TempDir,
    pubkey_b64: String,
    /// A second, unrelated public key — the "signed by someone else"
    /// case.
    other_pubkey_b64: String,
    artifact: Vec<u8>,
    signature_b64: String,
    artifact_path: PathBuf,
}

fn fixture() -> &'static Signed {
    static FIXTURE: OnceLock<Signed> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let tmp = TempDir::new().expect("tmp");
        let key = tmp.path().join("key");
        let other_key = tmp.path().join("other");
        // `--ci` skips the interactive prompts; the empty password keeps
        // the signing call non-interactive too.
        tauri_cli(&["signer", "generate", "--ci", "-p", "", "-w", path(&key)]);
        tauri_cli(&[
            "signer",
            "generate",
            "--ci",
            "-p",
            "",
            "-w",
            path(&other_key),
        ]);

        // Stand-in for the `.msi.zip` the bundler produces: the updater
        // signs and verifies opaque bytes, so the content only has to be
        // stable and non-trivial.
        let artifact_path = tmp.path().join("GMM_0.1.1_x64_en-US.msi.zip");
        let artifact: Vec<u8> = (0u16..4096).map(|i| (i % 251) as u8).collect();
        std::fs::write(&artifact_path, &artifact).expect("write artifact");

        tauri_cli(&[
            "signer",
            "sign",
            "-f",
            path(&key),
            "-p",
            "",
            path(&artifact_path),
        ]);

        let sig_path = artifact_path.with_extension("zip.sig");
        Signed {
            pubkey_b64: read_trimmed(&key.with_extension("pub")),
            other_pubkey_b64: read_trimmed(&other_key.with_extension("pub")),
            signature_b64: read_trimmed(&sig_path),
            artifact,
            artifact_path,
            _tmp: tmp,
        }
    })
}

fn path(p: &Path) -> &str {
    p.to_str().expect("utf-8 path")
}

fn read_trimmed(p: &Path) -> String {
    std::fs::read_to_string(p)
        .unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
        .trim()
        .to_string()
}

/// Byte-for-byte what `tauri-plugin-updater` does before it will install
/// anything. Kept as its own function so the tests below differ only in
/// what they corrupt.
fn updater_would_accept(data: &[u8], signature_b64: &str, pubkey_b64: &str) -> Result<(), String> {
    let decode = |s: &str, what: &str| -> Result<String, String> {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(s)
            .map_err(|e| format!("{what} is not base64: {e}"))?;
        String::from_utf8(raw).map_err(|e| format!("{what} is not utf-8: {e}"))
    };

    let public_key = PublicKey::decode(&decode(pubkey_b64, "pubkey")?)
        .map_err(|e| format!("decode pubkey: {e}"))?;
    let signature = Signature::decode(&decode(signature_b64, "signature")?)
        .map_err(|e| format!("decode signature: {e}"))?;
    public_key
        .verify(data, &signature, true)
        .map_err(|e| format!("verify: {e}"))
}

#[test]
fn a_tauri_signed_artifact_is_one_the_updater_accepts() {
    let f = fixture();
    updater_would_accept(&f.artifact, &f.signature_b64, &f.pubkey_b64)
        .expect("the toolchain that signs releases must produce signatures the app accepts");
}

#[test]
fn the_signature_file_the_bundler_writes_sits_next_to_the_artifact() {
    // The release workflow uploads `<artifact>` and `<artifact>.sig` as a
    // pair, and `latest.json` carries the `.sig` contents inline. If the
    // CLI ever stops writing the sibling file, the pipeline silently
    // publishes an unsigned update.
    let f = fixture();
    let sig = f.artifact_path.with_extension("zip.sig");
    assert!(
        sig.exists(),
        "`tauri signer sign` must leave {} next to the artifact",
        sig.display(),
    );
    assert!(
        !f.signature_b64.is_empty(),
        "the signature file must not be empty",
    );
}

#[test]
fn an_artifact_tampered_after_signing_is_refused() {
    let f = fixture();
    // One byte, in the middle — the cheapest possible supply-chain edit.
    let mut tampered = f.artifact.clone();
    tampered[2048] ^= 0xff;

    let err = updater_would_accept(&tampered, &f.signature_b64, &f.pubkey_b64)
        .expect_err("a modified artifact must never install");
    assert!(
        err.starts_with("verify:"),
        "rejection should come from signature verification, got: {err}",
    );
}

#[test]
fn a_tampered_signature_is_refused() {
    let f = fixture();
    // Flip a byte inside the base64 payload rather than mangling the
    // envelope, so this exercises verification and not just decoding.
    let raw = base64::engine::general_purpose::STANDARD
        .decode(&f.signature_b64)
        .expect("decode signature");
    let mut text = String::from_utf8(raw).expect("signature is utf-8");
    let line = text
        .lines()
        .nth(1)
        .expect("minisign signature line")
        .to_string();
    let mut bytes = line.into_bytes();
    let last = bytes.len() - 2;
    bytes[last] = if bytes[last] == b'A' { b'B' } else { b'A' };
    let swapped = String::from_utf8(bytes).expect("still utf-8");
    text = text
        .lines()
        .enumerate()
        .map(|(i, l)| {
            if i == 1 {
                swapped.clone()
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let tampered_sig = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());

    let err = updater_would_accept(&f.artifact, &tampered_sig, &f.pubkey_b64)
        .expect_err("a modified signature must never install");
    assert!(
        err.starts_with("verify:") || err.starts_with("decode signature:"),
        "rejection should come from the signature itself, got: {err}",
    );
}

#[test]
fn an_artifact_signed_by_a_different_key_is_refused() {
    let f = fixture();
    // The attack the pubkey pin exists for: a correctly-formed, validly
    // signed update from a key that isn't ours.
    let err = updater_would_accept(&f.artifact, &f.signature_b64, &f.other_pubkey_b64)
        .expect_err("only the pinned key may ship updates");
    assert!(
        err.starts_with("verify:"),
        "rejection should come from signature verification, got: {err}",
    );
}

/// Verify a **real** bundler artifact, driven by
/// `.github/scripts/updater-e2e.ps1`.
///
/// The tests above sign a stand-in file; this one is handed the actual
/// `.msi.zip` the Windows build produced, the signature out of the
/// served `latest.json`, and the throwaway pubkey — so the assertion
/// covers the whole pipeline (bundle → sign → publish → download)
/// rather than the signer alone. Ignored by default because it needs
/// that build; the script sets:
///
/// ```text
/// GMM_UPDATER_ARTIFACT   path to the downloaded .msi.zip
/// GMM_UPDATER_SIGNATURE  the base64 signature from latest.json
/// GMM_UPDATER_PUBKEY     the throwaway public key
/// ```
#[test]
#[ignore = "driven by .github/scripts/updater-e2e.ps1 against a real build"]
fn the_bundled_artifact_verifies_against_the_key_that_signed_it() {
    let artifact_path = std::env::var("GMM_UPDATER_ARTIFACT").expect("GMM_UPDATER_ARTIFACT");
    let signature = std::env::var("GMM_UPDATER_SIGNATURE").expect("GMM_UPDATER_SIGNATURE");
    let pubkey = std::env::var("GMM_UPDATER_PUBKEY").expect("GMM_UPDATER_PUBKEY");

    let artifact =
        std::fs::read(&artifact_path).unwrap_or_else(|e| panic!("read {artifact_path}: {e}"));
    assert!(
        artifact.len() > 1024,
        "a real MSI zip should not be {} bytes — did the download fail?",
        artifact.len(),
    );

    updater_would_accept(&artifact, &signature, &pubkey)
        .expect("the artifact served by the update endpoint must verify against the pinned key");

    // And the same artifact, one byte different, must not.
    let mut tampered = artifact.clone();
    let mid = tampered.len() / 2;
    tampered[mid] ^= 0xff;
    updater_would_accept(&tampered, &signature, &pubkey)
        .expect_err("a tampered download must be refused before anything is installed");
}
