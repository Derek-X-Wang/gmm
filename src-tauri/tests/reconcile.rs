//! Slice 1c: Library/junction reconciliation.
//!
//! These tests exercise the Core directly so they run on macOS dev hosts;
//! the junction layer is realised as a unix directory symlink there.
//! Reconcile/Rebuild are filesystem-only operations so behaviour matches
//! on both targets.

use std::fs;

use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

async fn fresh_core(tmp: &TempDir) -> (Core, std::path::PathBuf, std::path::PathBuf) {
    let library_root = tmp.path().join("library");
    let game_mods = tmp.path().join("Genshin/Mods");
    fs::create_dir_all(&game_mods).expect("game mods dir");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root.clone(), &db_url)
        .await
        .expect("init core");
    (core, library_root, game_mods)
}

async fn adopt_and_enable(
    core: &Core,
    game_mods: &std::path::Path,
    fixture_root: &std::path::Path,
    name: &str,
) -> gmm_lib::core::Mod {
    fs::create_dir_all(fixture_root).expect("fixture dir");
    fs::write(
        fixture_root.join("merged.ini"),
        b"[TextureOverride]\nhash=42\n" as &[u8],
    )
    .expect("fixture ini");
    let m = core
        .adopt_folder(GameCode::Gimi, fixture_root, name)
        .await
        .expect("adopt");
    core.set_enabled(&m.id, true, game_mods)
        .await
        .expect("enable");
    m
}

#[tokio::test]
async fn reconcile_recreates_missing_junction_for_enabled_mod() {
    let tmp = TempDir::new().expect("tmp");
    let (core, _, game_mods) = fresh_core(&tmp).await;

    let fixture = tmp.path().join("fixture/Mod1");
    let m = adopt_and_enable(&core, &game_mods, &fixture, "Reconcile Mod").await;

    let link = game_mods.join("Reconcile Mod");
    assert!(link.exists(), "precondition: junction exists after enable");

    // Simulate the user nuking the junction by hand. Use the crate's
    // own junction module so this works on Windows (where fs::remove_file
    // on a junction returns "Access is denied") as well as macOS/Linux.
    gmm_lib::core::junction::remove(&link).expect("remove junction");
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "precondition: junction is gone",
    );

    let result = core
        .reconcile_junctions(GameCode::Gimi, &game_mods)
        .await
        .expect("reconcile");

    assert_eq!(
        result.recreated.as_slice(),
        std::slice::from_ref(&m.id),
        "the missing junction must be recreated, got {result:?}",
    );
    assert!(link.exists(), "junction recreated");
    assert!(
        link.join("merged.ini").exists(),
        "junction resolves into Library"
    );
}

#[tokio::test]
async fn reconcile_marks_unexpected_target_as_conflicting_without_overwriting() {
    let tmp = TempDir::new().expect("tmp");
    let (core, _, game_mods) = fresh_core(&tmp).await;

    let fixture = tmp.path().join("fixture/Mod1");
    let m = adopt_and_enable(&core, &game_mods, &fixture, "Conflict Mod").await;

    let link = game_mods.join("Conflict Mod");
    // Replace the link with one that points somewhere unrelated. Use the
    // crate's own junction module so this works on Windows (NTFS junction)
    // and macOS/Linux (directory symlink) alike.
    gmm_lib::core::junction::remove(&link).expect("remove original");
    let bogus = tmp.path().join("not_the_library");
    fs::create_dir_all(&bogus).expect("bogus dir");
    gmm_lib::core::junction::create(&link, &bogus).expect("plant bogus link");

    let result = core
        .reconcile_junctions(GameCode::Gimi, &game_mods)
        .await
        .expect("reconcile");

    assert!(
        result.recreated.is_empty(),
        "reconcile must NOT silently overwrite a drifted junction: {result:?}",
    );
    assert_eq!(
        result.conflicting.len(),
        1,
        "should record one conflict: {result:?}"
    );
    assert_eq!(result.conflicting[0].mod_id, m.id);
}

#[tokio::test]
async fn rebuild_recreates_every_enabled_junction_against_current_library() {
    let tmp = TempDir::new().expect("tmp");
    let (core, _, game_mods) = fresh_core(&tmp).await;

    let fixture1 = tmp.path().join("fixture/Mod1");
    let fixture2 = tmp.path().join("fixture/Mod2");
    let _m1 = adopt_and_enable(&core, &game_mods, &fixture1, "Mod One").await;
    let _m2 = adopt_and_enable(&core, &game_mods, &fixture2, "Mod Two").await;

    // Simulate a library-was-moved-by-user scenario by nuking the Mods
    // dir altogether — Rebuild should reconstruct both junctions.
    fs::remove_dir_all(&game_mods).expect("nuke Mods dir");

    let result = core
        .rebuild_junctions(GameCode::Gimi, &game_mods)
        .await
        .expect("rebuild");

    assert_eq!(
        result.recreated.len(),
        2,
        "both junctions recreated: {result:?}"
    );
    assert!(game_mods.join("Mod One").exists());
    assert!(game_mods.join("Mod Two").exists());
    assert!(game_mods.join("Mod One/merged.ini").exists());
    assert!(game_mods.join("Mod Two/merged.ini").exists());
}

#[tokio::test]
async fn rebuild_skips_disabled_mods() {
    let tmp = TempDir::new().expect("tmp");
    let (core, _, game_mods) = fresh_core(&tmp).await;

    // Adopt without enabling.
    let fixture = tmp.path().join("fixture/Disabled");
    fs::create_dir_all(&fixture).expect("fixture dir");
    fs::write(fixture.join("merged.ini"), b"hash=1\n").expect("ini");
    let disabled = core
        .adopt_folder(GameCode::Gimi, &fixture, "Disabled Mod")
        .await
        .expect("adopt");

    let result = core
        .rebuild_junctions(GameCode::Gimi, &game_mods)
        .await
        .expect("rebuild");

    assert_eq!(result.recreated.len(), 0);
    assert_eq!(result.skipped.as_slice(), &[disabled.id]);
    assert!(!game_mods.join("Disabled Mod").exists());
}

/// A junction that still points where the DB says, but whose target
/// directory has been deleted, is **not** healthy.
///
/// Found by `tests/ntfs_windows.rs` on the Windows runner: reconcile
/// compared the junction's recorded target against the DB and stopped
/// there, so a Library copy deleted by the user (or moved by a sync
/// tool, or on an unmounted drive) still reported healthy. The UI would
/// show the mod enabled while the game loaded nothing, with no
/// diagnostic anywhere.
///
/// Kept here as well as the Windows suite because the classification
/// logic is platform-independent — this fails in seconds on Linux
/// rather than minutes into the Windows matrix.
#[tokio::test]
async fn a_junction_whose_target_was_deleted_is_not_healthy() {
    let tmp = TempDir::new().expect("tmp");
    let (core, _, game_mods) = fresh_core(&tmp).await;

    let fixture = tmp.path().join("fixture/Vanishing");
    let m = adopt_and_enable(&core, &game_mods, &fixture, "Vanishing Mod").await;

    // Precondition: a clean reconcile calls it healthy.
    let before = core
        .reconcile_junctions(GameCode::Gimi, &game_mods)
        .await
        .expect("reconcile before");
    assert!(
        before.healthy.contains(&m.id),
        "precondition: mod should start healthy, got {before:?}",
    );

    // The user deletes the Library copy by hand.
    fs::remove_dir_all(&m.library_path).expect("remove library copy");

    let after = core
        .reconcile_junctions(GameCode::Gimi, &game_mods)
        .await
        .expect("reconcile after");

    assert!(
        !after.healthy.contains(&m.id),
        "a mod whose Library copy is gone must not be healthy, got {after:?}",
    );
    assert!(
        after.conflicting.iter().any(|c| c.mod_id == m.id),
        "it should be surfaced as conflicting so the UI can prompt, got {after:?}",
    );
}

/// The inverse tear: the DB says a Mod is *disabled* but a Junction for
/// it is still sitting in `<Game>/Mods/`.
///
/// This is strictly worse than the missing-Junction direction the tests
/// above cover. A missing Junction means a Mod the user turned on isn't
/// loading — visible, annoying, self-evident. A stranded Junction means
/// the Model Importer keeps loading a Mod that GMM's UI says is off, and
/// nothing in the app will ever tell the user why their game looks wrong.
///
/// Found by `tests/concurrency.rs` (issue #58): a concurrent
/// enable/disable can produce it, as can a crash between `set_enabled`'s
/// filesystem step and its DB step (issue #59). Reconcile has to be able
/// to repair both directions or "run reconcile" is not a real answer.
#[tokio::test]
async fn reconcile_removes_a_stranded_junction_for_a_disabled_mod() {
    let tmp = TempDir::new().expect("tmp");
    let (core, _, game_mods) = fresh_core(&tmp).await;

    let fixture = tmp.path().join("fixture/Stranded");
    let m = adopt_and_enable(&core, &game_mods, &fixture, "Stranded Mod").await;

    let link = game_mods.join("Stranded Mod");
    let target = fs::canonicalize(&link).expect("resolve the enabled junction");

    // Disable through the public API, then put the Junction back by hand
    // — the same end state a torn write leaves, reached without reaching
    // into the DB.
    core.set_enabled(&m.id, false, &game_mods)
        .await
        .expect("disable");
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "precondition: disable removed the junction",
    );
    gmm_lib::core::junction::create(&link, &target).expect("strand a junction");
    assert!(link.exists(), "precondition: junction is stranded");

    let result = core
        .reconcile_junctions(GameCode::Gimi, &game_mods)
        .await
        .expect("reconcile");

    assert_eq!(
        result.removed.as_slice(),
        std::slice::from_ref(&m.id),
        "the stranded junction must be reported as removed, got {result:?}",
    );
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "the stranded junction must actually be gone",
    );
    assert!(
        fixture.join("merged.ini").exists() || target.join("merged.ini").exists(),
        "removing a junction must never touch the Library copy (ADR 0003)",
    );
}

/// ...but only when it points where we put it. A junction the user aimed
/// somewhere else is theirs, and clobbering it would lose data we did not
/// own. Same reasoning as the enabled-but-drifted case: surface it, don't
/// auto-fix it.
#[tokio::test]
async fn reconcile_leaves_a_disabled_mods_junction_alone_when_it_points_elsewhere() {
    let tmp = TempDir::new().expect("tmp");
    let (core, _, game_mods) = fresh_core(&tmp).await;

    let fixture = tmp.path().join("fixture/Elsewhere");
    let m = adopt_and_enable(&core, &game_mods, &fixture, "Elsewhere Mod").await;
    core.set_enabled(&m.id, false, &game_mods)
        .await
        .expect("disable");

    let somewhere_else = tmp.path().join("not-the-library");
    fs::create_dir_all(&somewhere_else).expect("other dir");
    let link = game_mods.join("Elsewhere Mod");
    gmm_lib::core::junction::create(&link, &somewhere_else).expect("user's own junction");

    let result = core
        .reconcile_junctions(GameCode::Gimi, &game_mods)
        .await
        .expect("reconcile");

    assert!(
        result.removed.is_empty(),
        "a junction pointing outside the Library is not ours to delete, got {result:?}",
    );
    assert!(
        result.conflicting.iter().any(|c| c.mod_id == m.id),
        "it should be surfaced as conflicting instead, got {result:?}",
    );
    assert!(link.exists(), "the user's junction survives");
}
