//! Issue #124 — a persisted Importer Origin that cannot be read back is
//! distinguishable from one that was never set.
//!
//! Two reads on the origin path parsed stored JSON with `.ok()` and
//! swallowed the failure:
//!
//! - an unreadable **user override** became "no override set", so the
//!   user's highest-precedence choice was discarded and the game
//!   silently dropped to the manifest or the compiled-in default — which
//!   can be the very package they moved away from;
//! - an unreadable **installed origin** became `Unknown`, which is a
//!   real and load-bearing state (#99) meaning "hand-installed, GMM
//!   never recorded this". Collapsing a corrupt record into it makes GMM
//!   claim it never performed an install it did perform.
//!
//! Neither logged anything, while the cached-manifest path on the same
//! feature records its parse failures — so this was an inconsistency,
//! not a house style. The stored shape is simple enough today that this
//! is hard to reach; it becomes reachable the moment `ImporterOrigin`
//! gains the local-zip variant ADR 0005 already specifies, or when a
//! user downgrades past a build that wrote a newer serialisation.

use gmm_lib::core::importer_origin::{
    ImporterOrigin, InstalledOrigin, OriginLayer, OriginResolution, StoredOverride,
};
use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

fn db_url(tmp: &TempDir) -> String {
    format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display())
}

async fn fresh_core(tmp: &TempDir) -> Core {
    Core::new(tmp.path().join("library"), &db_url(tmp))
        .await
        .expect("init")
}

/// Write a raw settings value behind `Core`'s back, over a second
/// connection to the test's own database.
///
/// The only way to produce this state without adding a corruption seam
/// to shipped code — and a downgraded GMM, which is how a real user
/// reaches it, would arrive at exactly the same row.
async fn corrupt_setting(tmp: &TempDir, key: &str, value: &str) {
    let pool = sqlx::SqlitePool::connect(&db_url(tmp))
        .await
        .expect("open db");
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(&pool)
    .await
    .expect("write corrupt setting");
    pool.close().await;
}

/// A serialisation a future GMM might write and this build cannot read:
/// a variant tag that does not exist yet.
const FROM_A_NEWER_BUILD: &str = r#"{"kind":"localZip","path":"C:\\pkgs\\gimi.zip"}"#;

fn a_different_origin() -> ImporterOrigin {
    ImporterOrigin::github("someone", "GIMI-Fork", r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip")
}

#[tokio::test]
async fn an_unreadable_override_is_not_the_same_as_no_override() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    assert!(
        matches!(
            core.importer_origin_override(GameCode::Gimi)
                .await
                .expect("read"),
            StoredOverride::NotSet,
        ),
        "a game with nothing stored has no override",
    );

    corrupt_setting(&tmp, "importer.gimi.origin_override", FROM_A_NEWER_BUILD).await;

    match core
        .importer_origin_override(GameCode::Gimi)
        .await
        .expect("read")
    {
        StoredOverride::Unreadable { raw, .. } => assert!(
            raw.contains("localZip"),
            "the unreadable value is carried so a maintainer can see what was stored",
        ),
        other => panic!("a corrupt override must not read back as absence; got {other:?}"),
    }
}

#[tokio::test]
async fn an_unreadable_override_does_not_silently_demote_the_game_to_a_lower_layer() {
    // The whole point of layer 1 is that the user's own choice outranks
    // GMM's opinion. Falling through on a read failure hands the game
    // back to the manifest or to the compiled-in default — which, for
    // anyone who set an override *because* the default went bad, is
    // GMM quietly reinstating the thing they moved away from.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    corrupt_setting(&tmp, "importer.gimi.origin_override", FROM_A_NEWER_BUILD).await;

    match core
        .resolve_importer_origin(GameCode::Gimi)
        .await
        .expect("resolve")
    {
        OriginResolution::NoneInEffect { reason } => {
            let reason = reason.expect("the user has to be told why nothing is in effect");
            assert!(
                reason.to_lowercase().contains("origin"),
                "the reason must name what could not be read: {reason}",
            );
        }
        OriginResolution::InEffect { origin, layer } => panic!(
            "a corrupt override must not resolve to a lower layer; got {origin:?} from {layer:?}",
        ),
    }
}

#[tokio::test]
async fn a_healthy_override_still_wins_outright() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let mine = a_different_origin();

    core.set_importer_origin_override(GameCode::Gimi, Some(&mine))
        .await
        .expect("set override");

    match core
        .resolve_importer_origin(GameCode::Gimi)
        .await
        .expect("resolve")
    {
        OriginResolution::InEffect { origin, layer } => {
            assert_eq!(origin, mine);
            assert_eq!(layer, OriginLayer::UserOverride);
        }
        other => panic!("a readable override is layer 1; got {other:?}"),
    }
    assert!(matches!(
        core.importer_origin_override(GameCode::Gimi)
            .await
            .expect("read"),
        StoredOverride::Set(_),
    ));
}

#[tokio::test]
async fn an_unreadable_installed_origin_is_not_a_genuine_unknown_origin() {
    // `Unknown` means "hand-installed; GMM never recorded where this
    // came from" — a state #99 makes first-class and that later
    // decisions read. A corrupt record is the opposite claim: GMM *did*
    // install this and can no longer say from where. Collapsing them
    // makes GMM assert something false about the user's machine.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    assert_eq!(
        core.installed_importer_origin(GameCode::Gimi)
            .await
            .expect("read"),
        InstalledOrigin::Unknown,
        "nothing recorded is a genuine unknown origin",
    );

    corrupt_setting(&tmp, "importer.gimi.installed_origin", FROM_A_NEWER_BUILD).await;

    match core
        .installed_importer_origin(GameCode::Gimi)
        .await
        .expect("read")
    {
        InstalledOrigin::Unreadable { raw, .. } => assert!(raw.contains("localZip")),
        other => panic!("a corrupt install record must not read back as Unknown; got {other:?}"),
    }
}

#[tokio::test]
async fn a_healthy_installed_origin_still_reads_back_exactly() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let mine = a_different_origin();

    core.record_importer_install(GameCode::Gimi, "v1.4.4", &mine)
        .await
        .expect("record");

    assert_eq!(
        core.installed_importer_origin(GameCode::Gimi)
            .await
            .expect("read"),
        InstalledOrigin::Known(mine),
    );
}

#[tokio::test]
async fn both_read_failures_are_recorded_where_a_maintainer_can_find_them() {
    // Silence is what let this sit unnoticed: the cached-manifest path
    // on this same feature logs its parse failures, and these two did
    // not. The value itself is the record — a maintainer holding an
    // `Unreadable` has the raw text and the parser's complaint.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    corrupt_setting(&tmp, "importer.gimi.origin_override", "{ not json").await;
    corrupt_setting(&tmp, "importer.gimi.installed_origin", "{ not json").await;

    match core
        .importer_origin_override(GameCode::Gimi)
        .await
        .expect("read")
    {
        StoredOverride::Unreadable { error, .. } => assert!(
            !error.is_empty(),
            "the parser's complaint has to survive to the call site",
        ),
        other => panic!("expected an unreadable override; got {other:?}"),
    }
    match core
        .installed_importer_origin(GameCode::Gimi)
        .await
        .expect("read")
    {
        InstalledOrigin::Unreadable { error, .. } => assert!(!error.is_empty()),
        other => panic!("expected an unreadable install record; got {other:?}"),
    }
}
