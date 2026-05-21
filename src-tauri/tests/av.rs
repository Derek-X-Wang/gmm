//! Slice NEW-AV (#13): antivirus / SmartScreen launch-error guidance.
//!
//! The Core module under test is `gmm_lib::core::av`. The acceptance
//! contract has three moving parts and one tests them all:
//!
//! 1. `classify_launch_error` recognises Defender's canonical
//!    `ERROR_VIRUS_INFECTED` text (and its 0x800700E1 / OS error 225
//!    surface forms), plus SmartScreen prompt text. Unrelated errors
//!    must NOT classify as AV — the in-app guidance is loud, so a false
//!    positive is worse than a false negative.
//! 2. `wrap_launch_error` prefixes an AV-pattern error with the
//!    `AV-PATTERN: ` sentinel used by the frontend to swap to the
//!    structured AV component. Non-matching errors round-trip
//!    unchanged.
//! 3. The structured `AvGuidance` payload returned by
//!    `core::av::guidance()` matches the canonical doc at
//!    `docs/antivirus-and-smartscreen.md` (single source of truth).
//!    Tests assert the docs file contains every headline and every
//!    `exclusion_step` summary that the in-app component renders, so
//!    the two cannot silently drift.

use gmm_lib::core::av;

#[test]
fn classify_recognises_defender_virus_infected_text() {
    // The English Defender / Win32 canonical string for
    // ERROR_VIRUS_INFECTED. Rust's `io::Error::to_string()` includes
    // this when the OS surfaces error 225.
    let msg = "spawn GenshinImpact.exe: Operation did not complete \
               successfully because the file contains a virus or \
               potentially unwanted software. (os error 225)";
    assert!(
        av::classify_launch_error(msg).is_some(),
        "Defender ERROR_VIRUS_INFECTED text must classify as AV",
    );
}

#[test]
fn classify_recognises_raw_os_error_225() {
    // When Rust's IO error display chain drops the human string but
    // keeps the raw code (some locales / wrappers do this), we still
    // want the structured guidance to fire.
    let msg = "load loader: (os error 225)";
    assert!(av::classify_launch_error(msg).is_some());
}

#[test]
fn classify_recognises_hex_error_code() {
    // Win32 surfaces 0x800700E1 with the human text suppressed when
    // the failure happens deep inside a Win32 API call we wrap.
    let msg = "install hook: HRESULT 0x800700E1";
    assert!(av::classify_launch_error(msg).is_some());
}

#[test]
fn classify_recognises_smartscreen_text() {
    let msg = "Windows SmartScreen prevented an unrecognised app from starting";
    assert!(av::classify_launch_error(msg).is_some());
}

#[test]
fn classify_ignores_unrelated_errors() {
    let cases = [
        "Set the game install path in Settings before launching.",
        "Model Importer DLL not found at C:\\Genshin\\d3d11.dll. \
         Install the importer for this game first.",
        "spawn GenshinImpact.exe: The system cannot find the file specified. (os error 2)",
        "wait_for_injection: timed out",
    ];
    for msg in cases {
        assert!(
            av::classify_launch_error(msg).is_none(),
            "non-AV error must not classify: {msg}",
        );
    }
}

#[test]
fn wrap_prefixes_av_pattern_with_sentinel() {
    let original = "spawn GenshinImpact.exe: Operation did not complete \
                    successfully because the file contains a virus";
    let wrapped = av::wrap_launch_error(original);
    assert!(
        wrapped.starts_with(av::AV_PATTERN_SENTINEL),
        "wrap_launch_error must prefix the sentinel, got: {wrapped}",
    );
    assert!(
        wrapped.contains(original),
        "wrap_launch_error must preserve original message text",
    );
}

#[test]
fn wrap_passes_through_non_av_errors_unchanged() {
    let original =
        "spawn GenshinImpact.exe: The system cannot find the file specified. (os error 2)";
    let wrapped = av::wrap_launch_error(original);
    assert_eq!(
        wrapped, original,
        "non-AV errors must round-trip without the sentinel",
    );
}

/// Normalise whitespace runs (newline+indent etc.) to a single space
/// so multi-line markdown paragraphs can be compared against the Rust
/// constants without forcing the doc to be single-line.
fn squish(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn guidance_payload_matches_doc_single_source_of_truth() {
    // Reads the canonical file at runtime via Cargo's manifest dir so
    // the assertion runs against the same bytes that `include_str!`
    // pulls into the module.
    let doc_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .join("docs/antivirus-and-smartscreen.md");
    let doc = std::fs::read_to_string(&doc_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", doc_path.display()));
    let doc_squished = squish(&doc);

    let g = av::guidance();

    // Headline + body + read-more link all live in the doc.
    assert!(
        doc_squished.contains(&squish(&g.headline)),
        "doc must contain headline: {:?}",
        g.headline,
    );
    assert!(
        doc_squished.contains(&squish(&g.body)),
        "doc must contain body paragraph (single source of truth)",
    );
    for step in &g.exclusion_steps {
        assert!(
            doc_squished.contains(&squish(step)),
            "doc must contain exclusion step text: {step:?}",
        );
    }
    assert!(
        g.doc_path.ends_with("antivirus-and-smartscreen.md"),
        "doc_path must point at the canonical doc, got {:?}",
        g.doc_path,
    );
}

#[test]
fn readme_references_canonical_doc() {
    let readme_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .join("README.md");
    let readme = std::fs::read_to_string(&readme_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", readme_path.display()));
    assert!(
        readme.contains("## Antivirus and SmartScreen"),
        "README must have an 'Antivirus and SmartScreen' section",
    );
    assert!(
        readme.contains("docs/antivirus-and-smartscreen.md"),
        "README must link to the canonical AV doc",
    );
    // AC bullets must all be touched on by the README copy. We check for
    // distinctive phrases the doc and the README both carry.
    for needle in [
        // 1) what behaviours look suspicious
        "DLL",
        // 2) why unsigned
        "unsigned",
        // 3) how to exclude in Defender / common AVs
        "exclusion",
        // 4) auto-quarantine recovery
        "quarantine",
    ] {
        assert!(
            readme.to_lowercase().contains(&needle.to_lowercase()),
            "README must mention {needle:?} so all AC bullets are covered",
        );
    }
}
