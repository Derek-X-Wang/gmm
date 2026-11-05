//! Issue #446 — a Mod's name travels with its identifier in user-facing errors.
//!
//! Twelve `core::Error` variants print a Mod's ULID, which means nothing to
//! the person reading the message. The command boundary resolves the name
//! (`SurfaceErrors::surface`) and writes `"<name>" (<id>)` in place of the
//! bare identifier; `Error::mod_ids()` tells it which identifiers to look up.
//!
//! The risk this file guards is silent decay: `mod_ids()` ends in a wildcard
//! arm, so a variant added later that carries a `mod_id` compiles, passes
//! every behavioral test written before it existed, and ships a bare ULID.
//! `every_variant_that_carries_a_mod_id_is_listed_in_mod_ids` closes that by
//! parsing the enum itself. It is paired with a fixture test that feeds the
//! same checker an unlisted variant, so a checker that stops finding anything
//! cannot go green on its own.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gmm_lib::command_error::{CommandError, SurfaceErrors};
use gmm_lib::core::{Core, Error};
use sqlx::SqlitePool;
use syn::visit::Visit;
use tempfile::TempDir;
use tracing_subscriber::fmt::MakeWriter;

const MOD_ID: &str = "01J0000000000000000000MOD1";
const SURVIVOR_ID: &str = "01J0000000000000000000MOD2";
const MOD_NAME: &str = "Sparkle Dress";
const SURVIVOR_NAME: &str = "Plain Dress";

// ---- The exhaustiveness gate -------------------------------------------

/// Variants of `Error` with a field naming a Mod identifier, as declared.
fn variants_declaring_a_mod_id(error_source: &str) -> Vec<String> {
    let file = syn::parse_file(error_source).expect("parse the Error source");
    let declaration = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Enum(declaration) if declaration.ident == "Error" => Some(declaration),
            _ => None,
        })
        .expect("find `enum Error`");
    declaration
        .variants
        .iter()
        .filter(|variant| {
            variant.fields.iter().any(|field| {
                field
                    .ident
                    .as_ref()
                    .is_some_and(|name| name == "mod_id" || name == "surviving_mod_id")
            })
        })
        .map(|variant| variant.ident.to_string())
        .collect()
}

/// Variants that `Error::mod_ids` names in one of its match arms.
fn variants_matched_by_mod_ids(error_source: &str) -> Vec<String> {
    #[derive(Default)]
    struct ArmPatterns(Vec<String>);
    impl ArmPatterns {
        fn record(&mut self, path: &syn::Path) {
            let mut segments = path.segments.iter();
            if segments.next().is_some_and(|first| first.ident == "Self") {
                if let (Some(variant), None) = (segments.next(), segments.next()) {
                    self.0.push(variant.ident.to_string());
                }
            }
        }
    }
    impl<'ast> Visit<'ast> for ArmPatterns {
        fn visit_pat_struct(&mut self, pattern: &'ast syn::PatStruct) {
            self.record(&pattern.path);
            syn::visit::visit_pat_struct(self, pattern);
        }
        fn visit_pat_tuple_struct(&mut self, pattern: &'ast syn::PatTupleStruct) {
            self.record(&pattern.path);
            syn::visit::visit_pat_tuple_struct(self, pattern);
        }
        // A unit variant in pattern position parses as a path expression.
        fn visit_expr_path(&mut self, path: &'ast syn::ExprPath) {
            self.record(&path.path);
            syn::visit::visit_expr_path(self, path);
        }
    }

    let file = syn::parse_file(error_source).expect("parse the Error source");
    let accessor = file
        .items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Impl(block) => Some(block),
            _ => None,
        })
        .flat_map(|block| &block.items)
        .find_map(|item| match item {
            syn::ImplItem::Fn(function) if function.sig.ident == "mod_ids" => Some(function),
            _ => None,
        })
        .expect("find `Error::mod_ids`");
    let mut arms = ArmPatterns::default();
    arms.visit_block(&accessor.block);
    arms.0
}

fn unlisted_mod_id_variants(error_source: &str) -> Vec<String> {
    let listed = variants_matched_by_mod_ids(error_source);
    variants_declaring_a_mod_id(error_source)
        .into_iter()
        .filter(|variant| !listed.contains(variant))
        .collect()
}

#[test]
fn every_variant_that_carries_a_mod_id_is_listed_in_mod_ids() {
    let source =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/core/error.rs"))
            .expect("read the Error source");

    // The twelve variants named by the issue plus `InvalidActiveVariant`,
    // which carries a `mod_id` but whose message never prints it. If the
    // parse stopped finding variants, the assertion below would hold for the
    // empty set; this floor is what makes it mean something.
    let declaring = variants_declaring_a_mod_id(&source);
    assert!(
        declaring.len() >= 13,
        "expected at least thirteen variants declaring a Mod identifier, found {declaring:?}"
    );

    let unlisted = unlisted_mod_id_variants(&source);
    assert!(
        unlisted.is_empty(),
        "these Error variants carry a Mod identifier but have no arm in Error::mod_ids(), \
         so their message would reach the user with a bare ULID: {unlisted:?}. \
         Add an arm; return an empty Vec only if the message never prints the identifier."
    );
}

#[test]
fn the_exhaustiveness_gate_reports_a_variant_missing_from_mod_ids() {
    let fixture = r#"
        pub enum Error {
            Listed { mod_id: String },
            Forgotten { mod_id: String, reason: String },
            Unrelated { game: String },
        }
        impl Error {
            pub fn mod_ids(&self) -> Vec<&str> {
                match self {
                    Self::Listed { mod_id } => vec![mod_id],
                    _ => Vec::new(),
                }
            }
        }
    "#;

    assert_eq!(unlisted_mod_id_variants(fixture), ["Forgotten"]);
}

// ---- Behavior at the command boundary ----------------------------------

async fn fixture() -> (TempDir, Core, SqlitePool) {
    let tmp = TempDir::new().unwrap();
    let url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(tmp.path().join("library"), &url).await.unwrap();
    let pool = SqlitePool::connect(&url).await.unwrap();
    (tmp, core, pool)
}

async fn seed_mod(pool: &SqlitePool, id: &str, name: &str) {
    sqlx::query(
        "INSERT INTO mods (id, game_code, name, source, library_path, enabled, created_at, junction_dir_name) \
         VALUES (?, 'gimi', ?, 'manual', ?, 0, '2026-11-01T00:00:00Z', ?)",
    )
    .bind(id)
    .bind(name)
    .bind(format!("library/gimi/{id}"))
    .bind(name)
    .execute(pool)
    .await
    .unwrap();
}

/// One instance of each of the twelve variants whose message names a Mod.
fn mod_naming_errors(id: &str, survivor: &str) -> Vec<(&'static str, Error)> {
    let reason = || "the disk said no".to_string();
    let path = || PathBuf::from("Mods/Example");
    vec![
        (
            "EnabledTransitionWitnessCorrupt",
            Error::EnabledTransitionWitnessCorrupt {
                mod_id: id.into(),
                reason: reason(),
            },
        ),
        (
            "EnabledTransitionPending",
            Error::EnabledTransitionPending { mod_id: id.into() },
        ),
        (
            "DuplicateModResolutionBlockedByReinstall",
            Error::DuplicateModResolutionBlockedByReinstall { mod_id: id.into() },
        ),
        (
            "DuplicateModInstallPathMissing",
            Error::DuplicateModInstallPathMissing {
                mod_id: id.into(),
                game: "gimi".into(),
            },
        ),
        (
            "DuplicateModJunctionConflict",
            Error::DuplicateModJunctionConflict {
                mod_id: id.into(),
                path: path(),
            },
        ),
        (
            "DuplicateModJunctionClaimedBySurvivor",
            Error::DuplicateModJunctionClaimedBySurvivor {
                mod_id: id.into(),
                surviving_mod_id: survivor.into(),
                path: path(),
            },
        ),
        (
            "DuplicateModJunctionStillPresent",
            Error::DuplicateModJunctionStillPresent {
                mod_id: id.into(),
                path: path(),
            },
        ),
        (
            "LibraryRelocationBlockedByReinstall",
            Error::LibraryRelocationBlockedByReinstall { mod_id: id.into() },
        ),
        (
            "ReinstallRecoveryUncertain",
            Error::ReinstallRecoveryUncertain {
                mod_id: id.into(),
                reason: reason(),
            },
        ),
        (
            "ReinstallWitnessCorrupt",
            Error::ReinstallWitnessCorrupt {
                mod_id: id.into(),
                reason: reason(),
            },
        ),
        (
            "ReinstallRecoveryQuarantined",
            Error::ReinstallRecoveryQuarantined { mod_id: id.into() },
        ),
        (
            "ReinstallRecoveryDeploymentUnverified",
            Error::ReinstallRecoveryDeploymentUnverified {
                mod_id: id.into(),
                reason: reason(),
            },
        ),
    ]
}

fn resolved_form(name: &str, id: &str) -> String {
    format!("\"{name}\" ({id})")
}

#[tokio::test]
async fn every_mod_naming_error_shows_the_mods_name_beside_its_identifier() {
    let (_tmp, core, pool) = fixture().await;
    seed_mod(&pool, MOD_ID, MOD_NAME).await;
    seed_mod(&pool, SURVIVOR_ID, SURVIVOR_NAME).await;

    let errors = mod_naming_errors(MOD_ID, SURVIVOR_ID);
    assert_eq!(errors.len(), 12, "the issue names twelve variants");
    for (variant, error) in errors {
        let original = error.to_string();
        let surfaced = Err::<(), _>(error).surface(&core).await.unwrap_err();

        assert!(
            surfaced.message.contains(&resolved_form(MOD_NAME, MOD_ID)),
            "{variant} did not name the Mod: {}",
            surfaced.message
        );
        // The change is purely additive: undoing the annotation must give
        // back today's text exactly, so no existing assertion can break.
        let mut undone = surfaced
            .message
            .replace(&resolved_form(MOD_NAME, MOD_ID), MOD_ID);
        if variant == "DuplicateModJunctionClaimedBySurvivor" {
            undone = undone.replace(&resolved_form(SURVIVOR_NAME, SURVIVOR_ID), SURVIVOR_ID);
        }
        assert_eq!(
            undone, original,
            "{variant} changed more than the identifier"
        );
    }
}

#[tokio::test]
async fn the_survivor_variant_names_both_of_its_mods() {
    let (_tmp, core, pool) = fixture().await;
    seed_mod(&pool, MOD_ID, MOD_NAME).await;
    seed_mod(&pool, SURVIVOR_ID, SURVIVOR_NAME).await;

    let error = Error::DuplicateModJunctionClaimedBySurvivor {
        mod_id: MOD_ID.into(),
        surviving_mod_id: SURVIVOR_ID.into(),
        path: PathBuf::from("Mods/Example"),
    };
    assert_eq!(error.mod_ids(), [MOD_ID, SURVIVOR_ID]);

    let surfaced = Err::<(), _>(error).surface(&core).await.unwrap_err();

    assert!(surfaced.message.contains(&format!(
        "Rejected Mod ID: {}",
        resolved_form(MOD_NAME, MOD_ID)
    )));
    assert!(surfaced.message.contains(&format!(
        "surviving Mod ID: {}",
        resolved_form(SURVIVOR_NAME, SURVIVOR_ID)
    )));
}

#[tokio::test]
async fn an_identifier_with_no_row_leaves_the_message_exactly_as_it_was() {
    let (_tmp, core, pool) = fixture().await;
    // Only the survivor exists: the rejected Mod's row is gone.
    seed_mod(&pool, SURVIVOR_ID, SURVIVOR_NAME).await;

    for (variant, error) in
        mod_naming_errors("01J0000000000000000000GONE", "01J000000000000000000GONE2")
    {
        let original = error.to_string();
        let surfaced = Err::<(), _>(error).surface(&core).await.unwrap_err();
        assert_eq!(
            surfaced.message, original,
            "{variant} altered an unknown id"
        );
    }
}

#[tokio::test]
async fn a_path_that_contains_the_identifier_is_not_rewritten() {
    let (_tmp, core, pool) = fixture().await;
    seed_mod(&pool, MOD_ID, MOD_NAME).await;
    // The Library path's last component is the Mod ID, and a recovery reason
    // quotes that path. The user may copy it, so it must stay a usable path.
    let reason = format!("could not read /library/gimi/{MOD_ID}/mod.ini");

    let surfaced = Err::<(), _>(Error::ReinstallRecoveryUncertain {
        mod_id: MOD_ID.into(),
        reason: reason.clone(),
    })
    .surface(&core)
    .await
    .unwrap_err();

    assert!(surfaced.message.contains(&reason));
    assert!(surfaced
        .message
        .contains(&format!("Mod {} that", resolved_form(MOD_NAME, MOD_ID))));
}

#[tokio::test]
async fn classification_survives_resolution() {
    let (_tmp, core, pool) = fixture().await;
    seed_mod(&pool, MOD_ID, MOD_NAME).await;
    seed_mod(&pool, SURVIVOR_ID, SURVIVOR_NAME).await;

    for (variant, error) in mod_naming_errors(MOD_ID, SURVIVOR_ID) {
        let kind = error.surface_failure_kind();
        let surfaced = Err::<(), _>(error).surface(&core).await.unwrap_err();
        assert_eq!(surfaced.kind, kind, "{variant} was reclassified");
    }
}

fn errors_without_a_mod_identifier() -> Vec<Error> {
    vec![
        // Carries a `mod_id`, but its message prints the name instead.
        Error::InvalidActiveVariant {
            mod_id: MOD_ID.into(),
            mod_name: MOD_NAME.into(),
            variant_id: "01MISSING".into(),
        },
        Error::LibraryRelocationBlockedByStaging,
        Error::SessionActive {
            game: "gimi".into(),
            since: "now".into(),
        },
    ]
}

#[tokio::test]
async fn an_error_without_a_mod_identifier_is_what_command_error_from_produces() {
    let (_tmp, core, _pool) = fixture().await;
    for (error, same_error) in errors_without_a_mod_identifier()
        .into_iter()
        .zip(errors_without_a_mod_identifier())
    {
        assert!(error.mod_ids().is_empty());
        let surfaced = Err::<(), _>(error).surface(&core).await.unwrap_err();
        assert_eq!(surfaced, CommandError::from(same_error));
    }
}

// ---- The lookup is never on the common path, and never masks the error -

#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CapturedLogs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for CapturedLogs {
    type Writer = CapturedLogs;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl CapturedLogs {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

/// A closed pool makes any query fail, and a failed lookup logs a warning.
/// That turns "was the database touched" into something a test can observe.
#[tokio::test]
async fn a_lookup_happens_only_when_the_error_names_a_mod_and_its_failure_never_masks_it() {
    let (_tmp, core, _pool) = fixture().await;
    core.close().await;
    let logs = CapturedLogs::default();
    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .finish(),
    );

    let plain = Error::LibraryRelocationBlockedByStaging;
    let expected = CommandError::from(Error::LibraryRelocationBlockedByStaging);
    let surfaced = Err::<(), _>(plain).surface(&core).await.unwrap_err();
    assert_eq!(surfaced, expected);
    assert!(
        logs.text().is_empty(),
        "an error with no Mod identifier must not query the database: {}",
        logs.text()
    );

    let naming = Error::EnabledTransitionPending {
        mod_id: MOD_ID.into(),
    };
    let original = naming.to_string();
    let surfaced = Err::<(), _>(naming).surface(&core).await.unwrap_err();
    assert!(
        logs.text().contains("could not look up Mod names"),
        "the control case must reach the database, or the assertion above proves nothing"
    );
    assert_eq!(
        surfaced.message, original,
        "a failed lookup must leave today's message, not replace it"
    );
}
