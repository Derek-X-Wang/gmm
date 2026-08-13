//! Guards on the shipped updater configuration.
//!
//! The self-update path (slice 13a / #2) is the one subsystem whose
//! failure mode is *silent and permanent*: if `tauri.conf.json` ships a
//! malformed public key, a wrong endpoint, or updater artifacts get
//! switched off, every installed copy quietly stops receiving updates
//! and the only recovery is asking users to reinstall by hand.
//!
//! Nothing here needs a network or a Windows host — these assert on the
//! config that gets compiled into the binary, so they run everywhere and
//! fail fast in CI the moment someone edits the file carelessly.

use std::path::PathBuf;

use serde_json::Value;

fn tauri_conf() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
    let raw =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&raw).expect("tauri.conf.json must be valid JSON")
}

fn updater() -> Value {
    tauri_conf()
        .get("plugins")
        .and_then(|p| p.get("updater"))
        .cloned()
        .expect("plugins.updater must be configured — self-update depends on it")
}

#[test]
fn updater_public_key_is_present_and_well_formed() {
    let pubkey = updater()
        .get("pubkey")
        .and_then(Value::as_str)
        .expect("plugins.updater.pubkey must be set")
        .to_string();

    assert!(
        !pubkey.trim().is_empty(),
        "an empty pubkey disables signature verification entirely",
    );
    assert!(
        !pubkey.contains('\n') && !pubkey.contains('\r'),
        "pubkey must be a single line — a stray newline from copying the \
         .pub file makes tauri reject every update at runtime",
    );

    // The value is base64 of a minisign public-key file. Decode it and
    // check the inner text really is a minisign key, so a truncated or
    // wrong-file paste is caught here rather than in the field.
    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(pubkey.as_bytes())
        .expect("pubkey must be valid base64");
    let text = String::from_utf8(decoded).expect("decoded pubkey must be UTF-8");
    assert!(
        text.contains("minisign public key"),
        "decoded pubkey should be a minisign public key file, got:\n{text}",
    );
    assert!(
        text.lines().count() >= 2,
        "a minisign public key file is a comment line plus the key line, got:\n{text}",
    );
}

#[test]
fn updater_endpoint_points_at_this_repo_over_https() {
    let endpoints = updater()
        .get("endpoints")
        .and_then(Value::as_array)
        .cloned()
        .expect("plugins.updater.endpoints must be a non-empty array");

    assert!(!endpoints.is_empty(), "at least one endpoint is required");

    for ep in &endpoints {
        let url = ep.as_str().expect("each endpoint must be a string");
        assert!(
            url.starts_with("https://"),
            "updater endpoints must be HTTPS (got {url}) — plain HTTP would \
             let a network attacker feed us a manifest",
        );
        assert!(
            url.contains("Derek-X-Wang/gmm"),
            "endpoint should point at this repo's releases, got {url}",
        );
        assert!(
            url.ends_with("latest.json"),
            "endpoint must name the manifest file tauri-action publishes, got {url}",
        );
    }
}

#[test]
fn updater_artifacts_are_enabled_so_releases_ship_a_manifest() {
    let create = tauri_conf()
        .get("bundle")
        .and_then(|b| b.get("createUpdaterArtifacts"))
        .and_then(Value::as_bool);

    assert_eq!(
        create,
        Some(true),
        "bundle.createUpdaterArtifacts must stay true — without it the \
         release build produces installers but no signed latest.json, and \
         self-update silently never fires (this shipped broken once)",
    );
}

#[test]
fn bundle_identifier_and_product_name_are_stable() {
    let conf = tauri_conf();

    // The identifier keys the install location and the updater's notion
    // of "same app". Changing it strands every existing install.
    assert_eq!(
        conf.get("identifier").and_then(Value::as_str),
        Some("com.derekxwang.gmm"),
        "changing the bundle identifier orphans existing installs",
    );
    assert_eq!(
        conf.get("productName").and_then(Value::as_str),
        Some("GMM"),
        "productName feeds the installed exe name that installer-smoke.ps1 looks for",
    );
}

#[test]
fn updater_and_process_permissions_are_granted_to_the_main_window() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("capabilities/default.json");
    let raw = std::fs::read_to_string(&path).expect("read capabilities/default.json");
    let caps: Value = serde_json::from_str(&raw).expect("capabilities must be valid JSON");

    let perms: Vec<&str> = caps
        .get("permissions")
        .and_then(Value::as_array)
        .expect("permissions array")
        .iter()
        .filter_map(Value::as_str)
        .collect();

    // Without these the frontend's check()/relaunch() calls fail at
    // runtime with an ACL error that no compile-time check would catch.
    for required in ["updater:default", "process:default"] {
        assert!(
            perms.contains(&required),
            "capability {required} missing — the updater UI would fail at runtime; got {perms:?}",
        );
    }
}
