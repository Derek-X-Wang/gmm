//! Antivirus / SmartScreen launch-error guidance (slice NEW-AV / #13).
//!
//! GMM is unsigned and behaves like a DLL injector (because that is
//! exactly what loading a 3dmigoto-derived Model Importer requires —
//! see ADR 0001). Generic Defender + SmartScreen heuristics flag this
//! shape; the in-app launch flow therefore needs to recognise AV-style
//! failures and surface actionable exclusion guidance inline instead
//! of the raw OS error string.
//!
//! The structured guidance lives in `docs/antivirus-and-smartscreen.md`
//! and is included verbatim here via [`include_str!`] so the doc, the
//! README, the launch-error component, and the onboarding wizard
//! cannot drift. Tests in `tests/av.rs` lock in that single source of
//! truth.

use serde::Serialize;

/// Single source of truth for the AV / SmartScreen guidance text.
///
/// Included at compile time so the in-app surface and the on-disk doc
/// cannot drift (one of #13's acceptance criteria; the onboarding
/// wizard in #24 will load its copy through the same module).
pub const AV_GUIDANCE_DOC: &str = include_str!("../../../docs/antivirus-and-smartscreen.md");

/// Sentinel the in-app launch error component looks for. When
/// `launch_game` (or any future Tauri command we wire through this
/// helper) classifies an error as AV-pattern, the wire message is
/// prefixed with this string so the React side can swap to the
/// structured `<AvGuidance>` component instead of the raw error text.
pub const AV_PATTERN_SENTINEL: &str = "AV-PATTERN: ";

/// Short inline headline shown above the structured guidance.
pub const AV_HEADLINE: &str = "Antivirus or SmartScreen may have blocked the launch.";

/// One-paragraph body that survives even when the user does not expand
/// the full guidance. Mirrors the *Why GMM looks suspicious* section
/// in the canonical doc.
pub const AV_BODY: &str =
    "GMM loads a 3dmigoto-derived Model Importer DLL into your game's process \
     so mods can take effect. Generic Defender + SmartScreen heuristics flag \
     that shape, even though the binary is doing exactly what it is supposed \
     to do.";

/// The exclusion steps shown inline in the launch error component.
/// Every entry must appear verbatim somewhere in the canonical doc — the
/// `guidance_payload_matches_doc_single_source_of_truth` test enforces
/// that.
pub const AV_EXCLUSION_STEPS: &[&str] = &[
    "Open Windows Security",
    "Add an exclusion",
    "Restart GMM after adding the exclusion",
    "Restore from quarantine before re-adding the exclusion",
];

/// Why we picked these patterns:
///
/// - `contains a virus` — the canonical English text Defender returns
///   alongside `ERROR_VIRUS_INFECTED` (`0x800700E1` / `225`).
/// - `error_virus_infected` — the Win32 macro name; some wrappers
///   propagate it.
/// - `0x800700e1` — the hex form Win32 surfaces when the human text
///   is suppressed.
/// - `os error 225` — Rust's `io::Error` display for the same code on
///   Windows.
/// - `smartscreen` / `smart screen` — both forms appear in the wild
///   depending on Defender's locale.
/// - `blocked by your system administrator` — Windows surfaces this
///   when SmartScreen is forced to Block by policy.
///
/// Matching is case-insensitive (we lowercase the haystack once per
/// call, see [`classify_launch_error`]).
const AV_NEEDLES: &[&str] = &[
    "contains a virus",
    "error_virus_infected",
    "0x800700e1",
    "os error 225",
    "smartscreen",
    "smart screen",
    "blocked by your system administrator",
];

/// Why a launch error classifies as AV-related. We only have one
/// variant today; the type keeps the door open for finer-grained
/// branching (e.g. *quarantined* vs *SmartScreen Block-by-policy*)
/// without changing the public API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvPattern {
    /// A generic AV or SmartScreen signal was matched in the error
    /// string. The structured guidance covers all of them.
    Generic,
}

/// Return `Some(_)` if the message text matches a known AV /
/// SmartScreen pattern, otherwise `None`.
pub fn classify_launch_error(message: &str) -> Option<AvPattern> {
    let lower = message.to_lowercase();
    if AV_NEEDLES.iter().any(|needle| lower.contains(needle)) {
        Some(AvPattern::Generic)
    } else {
        None
    }
}

/// Wrap an error string with the [`AV_PATTERN_SENTINEL`] prefix when
/// the message classifies as AV. Non-matching errors round-trip
/// unchanged so existing callers keep their wire shape.
pub fn wrap_launch_error<S: Into<String>>(message: S) -> String {
    let msg = message.into();
    if classify_launch_error(&msg).is_some() {
        format!("{AV_PATTERN_SENTINEL}{msg}")
    } else {
        msg
    }
}

/// Structured payload returned by the `av_guidance` Tauri command so
/// the launch-error component and the onboarding wizard can render the
/// same copy without each maintaining its own string table.
///
/// The doc / Rust / React layers all read from this struct via the
/// `av_guidance` command, which in turn pulls its values from the
/// constants above. The constants in turn must appear verbatim in
/// `docs/antivirus-and-smartscreen.md` — locked in by the tests in
/// `tests/av.rs`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvGuidance {
    /// Short banner shown above the inline guidance.
    pub headline: String,
    /// One-paragraph explanation of why GMM trips AV heuristics.
    pub body: String,
    /// Distinct steps the user can take, rendered as a list. Every
    /// entry appears verbatim in the canonical doc.
    pub exclusion_steps: Vec<String>,
    /// Repo-relative path to the canonical long-form doc; the React
    /// layer turns this into an in-app *Read more* link.
    pub doc_path: String,
    /// Sentinel the React side uses to detect AV-pattern errors. The
    /// frontend strips this prefix before showing the raw OS message,
    /// then renders the structured guidance.
    pub sentinel: String,
}

/// Build the canonical [`AvGuidance`] payload. Cheap to construct (a
/// handful of `String` clones); the call site does not need to cache
/// it.
pub fn guidance() -> AvGuidance {
    AvGuidance {
        headline: AV_HEADLINE.to_string(),
        body: AV_BODY.to_string(),
        exclusion_steps: AV_EXCLUSION_STEPS
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
        doc_path: "docs/antivirus-and-smartscreen.md".to_string(),
        sentinel: AV_PATTERN_SENTINEL.to_string(),
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn sentinel_round_trips() {
        let wrapped = wrap_launch_error("os error 225 — contains a virus");
        assert!(wrapped.starts_with(AV_PATTERN_SENTINEL));
    }

    #[test]
    fn doc_includes_headline() {
        assert!(
            AV_GUIDANCE_DOC.contains(AV_HEADLINE.trim_end_matches('.'))
                || AV_GUIDANCE_DOC.to_lowercase().contains("antivirus"),
            "include_str! produced an empty or stale doc payload",
        );
    }
}
