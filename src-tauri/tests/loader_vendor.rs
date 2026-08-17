//! Contract for the Loader bytes committed into GMM.
//!
//! Windows CI proves the FFI semantics. This platform-independent test keeps
//! the pinned metadata, manifest, and binary bytes from drifting apart.

use std::fs;
use std::path::PathBuf;

use gmm_lib::core::importer::sha256_of_file;

const LOADER_VERSION: &str = "0.9.2";
const LOADER_SHA256: &str = "4ca7425c18881e9ebbce13ae22e7a3ca3843e526b9aa901d14f97953ca87f38b";
const LOADER_SIGNATURE: &str = "MGYCMQD/Pjt1mE7SbB3T+MPpeMRiIC5nNb0IkExnp/TQjFT6eVKW5XBTW5R3SfRwlb6QQroCMQDxUlyilQWeJpyEIUyt+N1PnwjXjrNmaTzn8wxV3bhbzpNeER/u70p9t9u9fNmM9X8=";
const LOADER_SIZE: u64 = 20_480;
const RELEASE_DATE: &str = "2026-06-23";

fn vendor_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri has a repository root")
        .join("vendor/3dmloader")
}

#[test]
fn vendored_loader_matches_the_pinned_stable_release() {
    let vendor = vendor_dir();
    let dll = vendor.join("3dmloader.dll");
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(vendor.join("Manifest.json")).expect("read vendor manifest"),
    )
    .expect("parse vendor manifest");
    let readme = fs::read_to_string(vendor.join("README.md")).expect("read vendor README");

    assert_eq!(
        fs::metadata(&dll).expect("stat vendored Loader").len(),
        LOADER_SIZE,
    );
    assert_eq!(
        sha256_of_file(&dll).expect("hash vendored Loader"),
        LOADER_SHA256
    );
    assert_eq!(manifest["version"], LOADER_VERSION);
    assert_eq!(
        manifest["signatures"]["3dmloader.dll"], LOADER_SIGNATURE,
        "release manifest must carry the v0.9.2 Loader signature",
    );

    let expected_metadata = [
        format!("`v{LOADER_VERSION}`"),
        LOADER_SHA256.to_string(),
        "20 480 bytes".to_string(),
        RELEASE_DATE.to_string(),
    ];
    for expected in expected_metadata {
        assert!(
            readme.contains(&expected),
            "vendor README is missing pinned metadata {expected:?}",
        );
    }
}
