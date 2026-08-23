//! Acting on unreferenced Library directories (#72).
//!
//! #70 shipped the read-only audit. These tests drive the acting half:
//! reveal, recover-in-place, and delete. They run against real SQLite and
//! a real filesystem on both CI legs, because everything interesting here
//! is filesystem semantics — what moved, what did not, and what is gone.
//!
//! # How an orphan is produced
//!
//! Never by hand. `adopt_folder` copies into the Library *before*
//! inserting the Mod row, and the crash seam from #59 stops it exactly in
//! that window. Fabricating a ULID-named directory would test a shape we
//! invented; crashing a real adopt tests the shape the bug actually
//! leaves behind.
//!
//! # How "copies nothing" is asserted
//!
//! By file identity, not by the Mod appearing. Before recovery a hard
//! link is made to a file inside the orphan, from outside the Library.
//! After recovery the test writes through that link and reads the file
//! back through the recovered Mod's `library_path`. Same bytes means the
//! two names still share one inode — the file was never copied. A
//! copy-then-delete implementation leaves the recovered copy holding the
//! old bytes and fails here.
//!
//! (`std::os::windows::fs::MetadataExt::file_index` would say this
//! directly, but it is unstable. A hard link is the same fact, observable
//! on stable Rust and on NTFS, ext4 and APFS alike.)

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gmm_lib::core::{crash_points, Core, GameCode, Source};
use tempfile::TempDir;
use ulid::Ulid;

async fn fresh_core(tmp: &TempDir) -> Core {
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    Core::new(library_root, &db_url).await.expect("init core")
}

/// Run a real `adopt_folder` and kill it at `crash_points::ADOPT_AFTER_LIBRARY_COPY`.
///
/// The returned `Core` shares the pool with `core`, so the caller sees
/// exactly the database a restart would open: bytes in the Library, no row.
async fn orphan_from_crashed_adopt(
    core: &Core,
    tmp: &TempDir,
    contents: &[(&str, &[u8])],
) -> PathBuf {
    let source = tmp.path().join("interrupted-import");
    for (name, bytes) in contents {
        let path = source.join(name);
        fs::create_dir_all(path.parent().expect("parent")).expect("source dir");
        fs::write(&path, bytes).expect("source file");
    }

    let crashing = core.clone().with_crash_hook(Arc::new(|point| {
        if point == crash_points::ADOPT_AFTER_LIBRARY_COPY {
            panic!("crash point: {point}");
        }
    }));
    let outcome = tokio::spawn(async move {
        crashing
            .adopt_folder(GameCode::Gimi, &source, "Raiden Shogun Alt")
            .await
    })
    .await;
    assert!(
        outcome.is_err(),
        "the adopt must have died at the crash point, not completed",
    );

    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit after the crashed adopt");
    assert_eq!(
        report.unreferenced.len(),
        1,
        "the crashed adopt must leave exactly one orphan: {report:?}",
    );
    report.unreferenced[0].path.clone()
}

/// Hard-link `inside` to a name outside the Library, so the test can later
/// prove the two names are still one file.
fn witness(tmp: &TempDir, inside: &Path, label: &str) -> PathBuf {
    let link = tmp.path().join(label);
    fs::hard_link(inside, &link).expect("hard link the witness file");
    link
}

#[tokio::test]
async fn recovering_a_ulid_named_orphan_reuses_its_id_and_copies_nothing() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let orphan = orphan_from_crashed_adopt(
        &core,
        &tmp,
        &[
            ("merged.ini", b"hash=7\n"),
            ("nested/texture.buf", b"before"),
        ],
    )
    .await;
    let directory_name = orphan
        .file_name()
        .expect("orphan name")
        .to_string_lossy()
        .into_owned();
    assert!(
        Ulid::from_string(&directory_name).is_ok(),
        "precondition: a crashed adopt names its directory with a ULID, got {directory_name:?}",
    );
    let link = witness(&tmp, &orphan.join("nested/texture.buf"), "texture.witness");

    let recovered = core
        .recover_unreferenced_library_dir(GameCode::Gimi, &orphan, "Recovered Raiden")
        .await
        .expect("recover the orphan");

    // The user named it; GMM invented nothing.
    assert_eq!(recovered.name, "Recovered Raiden");
    assert_eq!(recovered.source, Source::Manual);
    assert!(!recovered.enabled);

    // The directory kept its ULID, so the row ID and the Library path's
    // last component still agree — the invariant the rest of the codebase
    // leans on.
    assert_eq!(recovered.id, directory_name);
    assert_eq!(recovered.library_path, orphan);
    assert_eq!(
        recovered.library_path.file_name().expect("last component"),
        std::ffi::OsStr::new(&recovered.id),
    );

    // Nothing was copied: writing through the outside hard link is
    // visible through the recovered Mod's own Library path.
    fs::write(&link, b"written through the witness").expect("write through the link");
    assert_eq!(
        fs::read(recovered.library_path.join("nested/texture.buf")).expect("read back"),
        b"written through the witness",
        "the recovered Mod points at a copy, not at the original bytes",
    );
    assert_eq!(
        fs::read(recovered.library_path.join("merged.ini")).expect("read ini"),
        b"hash=7\n",
    );

    // And it is gone from the report.
    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit after recovery");
    assert!(
        report.unreferenced.is_empty(),
        "a recovered directory is referenced and must leave the report: {report:?}",
    );
}

#[tokio::test]
async fn a_hand_dropped_directory_recovers_under_a_fresh_ulid_and_is_moved_not_copied() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    // Not every directory in the Library root came from a crashed import.
    // A user can drop one there by hand, and its name cannot be a Mod ID.
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game root");
    let dropped = root.join("Raiden Shogun by hand");
    fs::create_dir_all(dropped.join("nested")).expect("dropped tree");
    fs::write(dropped.join("nested/texture.buf"), b"before").expect("dropped file");
    let link = witness(&tmp, &dropped.join("nested/texture.buf"), "dropped.witness");

    let recovered = core
        .recover_unreferenced_library_dir(GameCode::Gimi, &dropped, "Hand Dropped")
        .await
        .expect("recover the hand-dropped folder");

    assert!(
        Ulid::from_string(&recovered.id).is_ok(),
        "a directory whose name is not a ULID must get a fresh one, got {:?}",
        recovered.id,
    );
    assert_eq!(
        recovered.library_path,
        root.join(&recovered.id),
        "the directory must be moved so its name matches the new Mod ID",
    );
    assert!(
        !dropped.exists(),
        "the old name must be gone, not left beside the new one",
    );

    // Moved, not copied: a rename keeps the file, so the outside hard link
    // still names the same bytes the recovered Mod does.
    fs::write(&link, b"written through the witness").expect("write through the link");
    assert_eq!(
        fs::read(recovered.library_path.join("nested/texture.buf")).expect("read back"),
        b"written through the witness",
        "the fallback must rename the directory, not copy and delete it",
    );

    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit after recovery");
    assert!(
        report.unreferenced.is_empty(),
        "nothing should remain unreferenced: {report:?}",
    );
}

/// The report the user clicked can be stale: another window, another
/// import, or a recovery they already performed can have created a Mod row
/// for that very directory in between.
#[tokio::test]
async fn both_actions_refuse_a_directory_a_mod_row_now_references() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let orphan = orphan_from_crashed_adopt(&core, &tmp, &[("merged.ini", b"hash=7\n")]).await;
    // Everything below acts from this now-stale report.
    let stale = orphan.clone();

    let recovered = core
        .recover_unreferenced_library_dir(GameCode::Gimi, &orphan, "Already Recovered")
        .await
        .expect("first recovery wins");
    assert_eq!(recovered.library_path, stale);

    let second = core
        .recover_unreferenced_library_dir(GameCode::Gimi, &stale, "Recovered Twice")
        .await;
    assert!(
        matches!(
            second,
            Err(gmm_lib::core::Error::NotAnUnreferencedLibraryDir { .. })
        ),
        "recovering an already-referenced directory must be refused, got {second:?}",
    );

    let deleted = core
        .delete_unreferenced_library_dir(GameCode::Gimi, &stale)
        .await;
    assert!(
        matches!(
            deleted,
            Err(gmm_lib::core::Error::NotAnUnreferencedLibraryDir { .. })
        ),
        "deleting a directory a Mod now owns must be refused, got {deleted:?}",
    );
    assert_eq!(
        fs::read(stale.join("merged.ini")).expect("the Mod's files survive"),
        b"hash=7\n",
        "a refused delete must not have removed anything",
    );

    assert_eq!(
        core.list_mods(GameCode::Gimi).await.expect("list").len(),
        1,
        "exactly one Mod exists: the first recovery",
    );
}

/// Windows path spelling is not directory identity. NTFS resolves the
/// lowercase spelling below to the same directory, so neither destructive
/// action may treat it as an orphan merely because SQLite stores uppercase
/// ULIDs. This test also runs on the Windows CI leg; the dynamic skip only
/// keeps case-sensitive Unix development hosts useful.
#[tokio::test]
async fn alternate_case_spelling_of_a_referenced_directory_is_not_an_orphan() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let source = tmp.path().join("case-alias-source");
    fs::create_dir_all(&source).expect("source dir");
    fs::write(source.join("merged.ini"), b"hash=case\n").expect("source file");
    let adopted = core
        .adopt_folder(GameCode::Gimi, &source, "Case Alias")
        .await
        .expect("adopt");

    let lowercase_name = adopted.id.to_ascii_lowercase();
    assert_ne!(lowercase_name, adopted.id, "ULID fixture needs a letter");
    let alias = adopted
        .library_path
        .parent()
        .expect("Library root")
        .join(lowercase_name);
    if !alias.is_dir() {
        #[cfg(windows)]
        panic!("Windows must resolve an alternate-case spelling of an NTFS directory");
        #[cfg(not(windows))]
        {
            eprintln!("skipping case-alias behavior on a case-sensitive filesystem");
            return;
        }
    }

    let deleted = core
        .delete_unreferenced_library_dir(GameCode::Gimi, &alias)
        .await;
    assert!(
        matches!(
            deleted,
            Err(gmm_lib::core::Error::NotAnUnreferencedLibraryDir { .. })
        ),
        "delete must recognize that the alias is the adopted Mod, got {deleted:?}",
    );
    assert_eq!(
        fs::read(adopted.library_path.join("merged.ini")).expect("Mod bytes survive"),
        b"hash=case\n",
    );

    let recovered = core
        .recover_unreferenced_library_dir(GameCode::Gimi, &alias, "Duplicate Case Alias")
        .await;
    assert!(
        matches!(
            recovered,
            Err(gmm_lib::core::Error::NotAnUnreferencedLibraryDir { .. })
        ),
        "recover must not create a second row over the same directory, got {recovered:?}",
    );
    assert_eq!(
        core.list_mods(GameCode::Gimi).await.expect("list").len(),
        1,
        "case-insensitive ULID identity must leave exactly the adopted row",
    );

    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit referenced directory");
    assert!(
        report.unreferenced.is_empty(),
        "the audit must use the same identity rule as the guard: {report:?}",
    );

    // Reopen the same Library through alternate casing. `read_dir` builds
    // reported child paths from this spelling while the row retains the
    // original one, so textual audit identity would falsely report the Mod.
    let alternate_library_root = tmp.path().join("LiBrArY");
    assert!(
        alternate_library_root.is_dir(),
        "precondition: the alternate root spelling resolves"
    );
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let reopened = Core::new(alternate_library_root, &db_url)
        .await
        .expect("reopen through alternate Library spelling");
    assert!(
        reopened
            .audit_library(GameCode::Gimi)
            .await
            .expect("audit through alternate root spelling")
            .unreferenced
            .is_empty(),
        "the audit and destructive guard must agree on filesystem identity",
    );
}

#[tokio::test]
async fn recover_refuses_an_existing_mod_id_ignoring_ulid_case() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let source = tmp.path().join("ulid-case-source");
    fs::create_dir_all(&source).expect("source dir");
    fs::write(source.join("merged.ini"), b"hash=id-case\n").expect("source file");
    let adopted = core
        .adopt_folder(GameCode::Gimi, &source, "ULID Case")
        .await
        .expect("adopt");
    let case_variant = adopted
        .library_path
        .parent()
        .expect("Library root")
        .join(adopted.id.to_ascii_lowercase());

    if !case_variant.is_dir() {
        // On a case-sensitive host this is a second directory, which proves
        // ID uniqueness independently of the filesystem-alias guard.
        fs::create_dir_all(&case_variant).expect("case-variant orphan");
        fs::write(case_variant.join("merged.ini"), b"hash=other\n").expect("orphan file");
    }
    let recovered = core
        .recover_unreferenced_library_dir(GameCode::Gimi, &case_variant, "Duplicate ULID")
        .await;
    assert!(
        matches!(
            recovered,
            Err(gmm_lib::core::Error::NotAnUnreferencedLibraryDir { .. })
        ),
        "recover must refuse an existing ULID under different ASCII case, got {recovered:?}",
    );
    assert_eq!(core.list_mods(GameCode::Gimi).await.expect("list").len(), 1);
}

#[tokio::test]
async fn actions_refuse_relative_and_parent_traversal_spellings() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game root");
    let orphan = root.join("01DIRECTCHILD");
    fs::create_dir_all(&orphan).expect("orphan");
    fs::write(orphan.join("keep.buf"), b"keep").expect("orphan bytes");
    let traversal = root.join("unused").join("..").join("01DIRECTCHILD");
    let relative = PathBuf::from("01DIRECTCHILD");

    for candidate in [&traversal, &relative] {
        let deleted = core
            .delete_unreferenced_library_dir(GameCode::Gimi, candidate)
            .await;
        assert!(
            matches!(
                deleted,
                Err(gmm_lib::core::Error::NotAnUnreferencedLibraryDir { .. })
            ),
            "delete must refuse {candidate:?}, got {deleted:?}",
        );
        let recovered = core
            .recover_unreferenced_library_dir(GameCode::Gimi, candidate, "No Traversal")
            .await;
        assert!(
            matches!(
                recovered,
                Err(gmm_lib::core::Error::NotAnUnreferencedLibraryDir { .. })
            ),
            "recover must refuse {candidate:?}, got {recovered:?}",
        );
    }
    assert_eq!(
        fs::read(orphan.join("keep.buf")).expect("bytes survive"),
        b"keep"
    );
}

#[tokio::test]
async fn a_delete_like_name_without_gmms_identity_intent_is_never_auto_purged() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game root");
    let user_dir = root.join(format!(".gmm-delete-{}", Ulid::new()));
    fs::create_dir_all(&user_dir).expect("user directory");
    fs::write(user_dir.join("precious.buf"), b"not GMM's quarantine").expect("user bytes");
    drop(core);

    let restarted = fresh_core(&tmp).await;
    assert_eq!(
        fs::read(user_dir.join("precious.buf")).expect("user bytes survive startup"),
        b"not GMM's quarantine",
        "a reserved-looking name alone is not proof that GMM owns a delete",
    );
    assert!(
        restarted
            .audit_library(GameCode::Gimi)
            .await
            .expect("audit user directory")
            .unreferenced
            .iter()
            .any(|entry| entry.path == user_dir),
        "an unowned reserved-looking directory stays visible for user action",
    );
}

/// Both actions resolve the Library root *now*, so nothing outside it can
/// be reached by handing GMM a path from somewhere else.
#[tokio::test]
async fn both_actions_refuse_anything_that_is_not_a_direct_child_of_the_game_root() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game root");
    fs::create_dir_all(&root).expect("game root exists");

    // A grandchild, another game's root, and a directory outside the
    // Library entirely. None of these is an orphan of this game.
    let grandchild = root.join("01ORPHAN").join("nested");
    fs::create_dir_all(&grandchild).expect("grandchild");
    let other_game = core
        .resolved_library_root_for(GameCode::Srmi)
        .await
        .expect("other game root")
        .join("01OTHERGAME");
    fs::create_dir_all(&other_game).expect("other game dir");
    let outside = tmp.path().join("precious");
    fs::create_dir_all(&outside).expect("outside dir");
    fs::write(outside.join("keepsake.bin"), b"not GMM's").expect("outside file");

    for candidate in [&grandchild, &other_game, &outside] {
        let deleted = core
            .delete_unreferenced_library_dir(GameCode::Gimi, candidate)
            .await;
        assert!(
            matches!(
                deleted,
                Err(gmm_lib::core::Error::NotAnUnreferencedLibraryDir { .. })
            ),
            "delete must refuse {candidate:?}, got {deleted:?}",
        );
        let recovered = core
            .recover_unreferenced_library_dir(GameCode::Gimi, candidate, "Nope")
            .await;
        assert!(
            matches!(
                recovered,
                Err(gmm_lib::core::Error::NotAnUnreferencedLibraryDir { .. })
            ),
            "recover must refuse {candidate:?}, got {recovered:?}",
        );
        assert!(candidate.is_dir(), "{candidate:?} must still be there");
    }
    assert_eq!(
        fs::read(outside.join("keepsake.bin")).expect("outside file survives"),
        b"not GMM's",
    );
}

/// The Library root is overridable globally *and* per game. A user who
/// relocated one game's Library must have that game's actions act on the
/// relocated directory — and must not have the default location, which may
/// still hold the same directory name, touched instead.
#[tokio::test]
async fn actions_follow_a_per_game_library_root_override() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let relocated = tmp.path().join("D_drive/GMM Library/genshin");
    fs::create_dir_all(&relocated).expect("relocated root");
    core.set_library_path_for_game(GameCode::Gimi, Some(&relocated))
        .await
        .expect("relocate the Gimi Library");
    assert_eq!(
        core.resolved_library_root_for(GameCode::Gimi)
            .await
            .expect("resolved root"),
        relocated,
    );

    let orphan = orphan_from_crashed_adopt(&core, &tmp, &[("merged.ini", b"relocated\n")]).await;
    assert_eq!(
        orphan.parent(),
        Some(relocated.as_path()),
        "the crashed adopt writes under the override, so the audit finds it there",
    );

    // A decoy of the same name under the *default* root. Resolving the
    // global root instead of the per-game one would act on this one.
    let default_root = tmp.path().join("library/gimi");
    let decoy = default_root.join(orphan.file_name().expect("name"));
    fs::create_dir_all(&decoy).expect("decoy dir");
    fs::write(decoy.join("merged.ini"), b"decoy\n").expect("decoy file");

    let recovered = core
        .recover_unreferenced_library_dir(GameCode::Gimi, &orphan, "Relocated Mod")
        .await
        .expect("recover under the override");
    assert_eq!(recovered.library_path, orphan);
    assert_eq!(
        fs::read(decoy.join("merged.ini")).expect("decoy survives"),
        b"decoy\n",
        "the default Library root must be untouched by a relocated game's recovery",
    );

    // And delete, on a second orphan under the same override.
    let second = relocated.join("01SECONDORPHANBYHAND");
    fs::create_dir_all(&second).expect("second orphan");
    fs::write(second.join("mod.ini"), vec![b'x'; 40]).expect("second file");
    let deleted = core
        .delete_unreferenced_library_dir(GameCode::Gimi, &second)
        .await
        .expect("delete under the override");
    assert_eq!(deleted.path, second);
    assert_eq!(deleted.size_bytes, Some(40));
    assert!(!second.exists());
    assert!(decoy.is_dir(), "delete must not have reached the decoy");
}

#[tokio::test]
async fn delete_removes_exactly_the_chosen_folder_and_reports_what_it_freed() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let doomed = orphan_from_crashed_adopt(&core, &tmp, &[("merged.ini", b"1234567890")]).await;
    let spared = doomed
        .parent()
        .expect("Library root")
        .join("01SPAREDORPHANBYHAND");
    fs::create_dir_all(spared.join("nested")).expect("spared tree");
    fs::write(spared.join("nested/keep.buf"), b"keep me").expect("spared file");

    let deleted = core
        .delete_unreferenced_library_dir(GameCode::Gimi, &doomed)
        .await
        .expect("delete the chosen orphan");

    assert_eq!(deleted.path, doomed);
    assert_eq!(
        deleted.directory_name,
        doomed.file_name().expect("name").to_string_lossy(),
    );
    assert_eq!(
        deleted.size_bytes,
        Some(10),
        "the freed size is measured at the moment of deletion",
    );
    assert!(!doomed.exists(), "the chosen folder is gone");
    assert_eq!(
        fs::read(spared.join("nested/keep.buf")).expect("the other orphan survives"),
        b"keep me",
        "delete acts on one explicitly chosen folder, never on the report",
    );

    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit after delete");
    assert_eq!(
        report.unreferenced.len(),
        1,
        "only the deleted folder leaves the report: {report:?}",
    );
    assert_eq!(report.unreferenced[0].path, spared);
}

/// Every other Library mutation refuses during a Game Session. These two
/// are the destructive end of the same surface, so they refuse as well —
/// and a refusal must leave both the bytes and the database alone.
#[tokio::test]
async fn neither_action_runs_during_a_game_session() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let orphan = orphan_from_crashed_adopt(&core, &tmp, &[("merged.ini", b"hash=7\n")]).await;

    core.start_session(&gmm_lib::core::SessionInfo {
        game: GameCode::Gimi,
        pid: std::process::id(),
        started_at: chrono::Utc::now(),
    })
    .await
    .expect("start a session");

    let recovered = core
        .recover_unreferenced_library_dir(GameCode::Gimi, &orphan, "Mid Session")
        .await;
    assert!(
        matches!(recovered, Err(gmm_lib::core::Error::SessionActive { .. })),
        "recover must refuse during a Game Session, got {recovered:?}",
    );
    let deleted = core
        .delete_unreferenced_library_dir(GameCode::Gimi, &orphan)
        .await;
    assert!(
        matches!(deleted, Err(gmm_lib::core::Error::SessionActive { .. })),
        "delete must refuse during a Game Session, got {deleted:?}",
    );

    assert_eq!(
        fs::read(orphan.join("merged.ini")).expect("the orphan is untouched"),
        b"hash=7\n",
    );
    assert!(
        core.list_mods(GameCode::Gimi)
            .await
            .expect("list")
            .is_empty(),
        "a refused recovery must not have written a Mod row",
    );

    // Revealing changes nothing, so it stays available while the game runs.
    core.end_session().await.expect("end session");
}

/// Inspect is the safety valve for the other two: a user who can look
/// inside a folder has no reason to guess. It reads nothing and changes
/// nothing, but it still validates, so a stale report cannot make GMM open
/// an arbitrary directory.
#[tokio::test]
async fn reveal_returns_the_validated_directory_and_refuses_anything_else() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let orphan = orphan_from_crashed_adopt(&core, &tmp, &[("merged.ini", b"hash=7\n")]).await;
    assert_eq!(
        core.unreferenced_library_dir_for_reveal(GameCode::Gimi, &orphan)
            .await
            .expect("reveal the orphan"),
        orphan,
    );

    let outside = tmp.path().join("elsewhere");
    fs::create_dir_all(&outside).expect("outside dir");
    let refused = core
        .unreferenced_library_dir_for_reveal(GameCode::Gimi, &outside)
        .await;
    assert!(
        matches!(
            refused,
            Err(gmm_lib::core::Error::NotAnUnreferencedLibraryDir { .. })
        ),
        "reveal must refuse a path outside the Library root, got {refused:?}",
    );
}

/// A Junction (a directory symlink on the test host, a real NTFS junction
/// on Windows) sitting in the Library root is the worst thing either action
/// could be pointed at: recovering it would make a Mod of somebody else's
/// directory, and deleting it could take the target's contents with it.
///
/// The audit already refuses to report reparse points, so this cannot arrive
/// through the UI — which is exactly why it is asserted here rather than
/// assumed. `cargo test --workspace` runs this on the windows-latest CI leg
/// against a genuine junction, where the reparse-point semantics differ from
/// the Unix symlink used on the other leg.
#[tokio::test]
async fn neither_action_touches_a_link_planted_in_the_library_root() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game root");
    fs::create_dir_all(&root).expect("game root exists");

    let target = tmp.path().join("somebody-elses-mods");
    fs::create_dir_all(&target).expect("target dir");
    fs::write(target.join("precious.buf"), b"not GMM's to delete").expect("target file");
    let planted = root.join("01LINKEDINTOTHELIBRARY");
    gmm_lib::core::junction::create(&planted, &target).expect("plant the link");

    // The audit does not report it, so the UI can never offer it.
    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit with a link in the root");
    assert!(
        report.unreferenced.is_empty(),
        "a link is not an orphaned import: {report:?}",
    );

    let deleted = core
        .delete_unreferenced_library_dir(GameCode::Gimi, &planted)
        .await;
    assert!(
        matches!(
            deleted,
            Err(gmm_lib::core::Error::NotAnUnreferencedLibraryDir { .. })
        ),
        "delete must refuse a link, got {deleted:?}",
    );
    let recovered = core
        .recover_unreferenced_library_dir(GameCode::Gimi, &planted, "Not Mine")
        .await;
    assert!(
        matches!(
            recovered,
            Err(gmm_lib::core::Error::NotAnUnreferencedLibraryDir { .. })
        ),
        "recover must refuse a link, got {recovered:?}",
    );

    assert_eq!(
        fs::read(target.join("precious.buf")).expect("the link target survives"),
        b"not GMM's to delete",
    );
}
