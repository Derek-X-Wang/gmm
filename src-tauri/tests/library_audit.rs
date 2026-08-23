//! Library consistency audit (#70).
//!
//! These tests drive the public Core seam against real SQLite and real
//! filesystem state. The audit is deliberately read-only: an unreferenced
//! directory may hold the user's only copy of an interrupted import.

use std::fs;

use gmm_lib::core::{junction, Core, GameCode};
use tempfile::TempDir;

async fn fresh_core(tmp: &TempDir) -> Core {
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    Core::new(library_root, &db_url).await.expect("init core")
}

#[tokio::test]
async fn audit_reports_only_unreferenced_directories_without_changing_them() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let source = tmp.path().join("source");
    fs::create_dir_all(&source).expect("source dir");
    fs::write(source.join("merged.ini"), b"referenced").expect("source file");
    let referenced = core
        .adopt_folder(GameCode::Gimi, &source, "Referenced Mod")
        .await
        .expect("adopt referenced mod");

    let game_root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game root");
    let orphan = game_root.join("01ORPHANEDIMPORT");
    fs::create_dir_all(orphan.join("nested")).expect("orphan tree");
    fs::write(orphan.join("mod.ini"), b"12345").expect("orphan ini");
    fs::write(orphan.join("nested/texture.buf"), b"1234567").expect("orphan buffer");

    // A loose file is not a Mod directory and must not become an orphan row.
    fs::write(game_root.join("README.txt"), b"ignore me").expect("root file");

    // The audit is scoped to one game even when another game's root contains
    // the same orphan shape.
    let other_game_orphan = core
        .resolved_library_root_for(GameCode::Srmi)
        .await
        .expect("other game root")
        .join("01OTHERGAMEORPHAN");
    fs::create_dir_all(&other_game_orphan).expect("other game orphan");
    fs::write(other_game_orphan.join("other.ini"), b"other").expect("other file");

    // Following this link would inflate the reported size from 12 to 4,108
    // bytes. The target is outside the Library, so traversing it would also
    // let the audit wander arbitrarily far from the requested game root.
    // `junction::create` uses a real NTFS junction on Windows and a directory
    // symlink on Unix, so both production and test-host link shapes are covered.
    let outside = tmp.path().join("outside");
    fs::create_dir_all(&outside).expect("outside dir");
    fs::write(outside.join("outside.bin"), vec![0u8; 4096]).expect("outside file");
    junction::create(&orphan.join("outside-link"), &outside).expect("outside link");

    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit library");

    assert_eq!(report.game, GameCode::Gimi);
    assert_eq!(report.unreferenced.len(), 1);
    assert_eq!(report.unreferenced[0].directory_name, "01ORPHANEDIMPORT");
    assert_eq!(report.unreferenced[0].path, orphan);
    assert_eq!(report.unreferenced[0].size_bytes, Some(12));
    assert_eq!(report.total_bytes, 12);

    // Read-only is part of the public contract, not merely an implementation
    // detail: every referenced, unreferenced, and ignored entry remains intact.
    assert_eq!(
        fs::read(orphan.join("mod.ini")).expect("orphan ini"),
        b"12345"
    );
    assert_eq!(
        fs::read(orphan.join("nested/texture.buf")).expect("orphan buffer"),
        b"1234567",
    );
    assert!(referenced.library_path.is_dir());
    assert!(game_root.join("README.txt").is_file());
    assert!(other_game_orphan.is_dir());

    assert_eq!(
        fs::metadata(outside.join("outside.bin"))
            .expect("outside metadata")
            .len(),
        4096,
    );
}

/// Reinstall creates its reserved stage before committing the witness. A hard
/// process death in that narrow window can leave only that empty directory:
/// extraction has not started, no row claims it, and a future reinstall uses a
/// fresh token. It is harmless internal residue, not a user Mod the audit
/// should offer to recover.
#[tokio::test]
async fn audit_ignores_an_unwitnessed_empty_reinstall_stage() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let game_root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game root");
    fs::create_dir_all(&game_root).expect("game root directory");
    let stage = game_root.join(".gmm-reinstall-01JCRASHBEFOREWITNESS0000");
    fs::create_dir(&stage).expect("empty unwitnessed reinstall stage");

    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit with unwitnessed reinstall stage");

    assert!(
        report.unreferenced.is_empty(),
        "an empty reserved reinstall stage is not user-facing orphan data: {report:?}",
    );
    assert!(
        stage.is_dir(),
        "the read-only audit must not mutate the stage"
    );
}

/// A reserved name is not ownership evidence. If a reinstall stage contains
/// bytes, the audit must surface it like any other unreferenced Library
/// directory rather than hiding the user's only report of stranded data.
#[tokio::test]
async fn audit_surfaces_a_non_empty_unwitnessed_reinstall_stage() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let game_root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game root");
    fs::create_dir_all(&game_root).expect("game root directory");
    let stage = game_root.join(".gmm-reinstall-01JSTRANDEDUSERBYTES00000");
    fs::create_dir(&stage).expect("stranded reinstall stage");
    fs::write(stage.join("merged.ini"), b"user bytes").expect("stranded stage bytes");

    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit with non-empty unwitnessed reinstall stage");

    assert_eq!(
        report.unreferenced.len(),
        1,
        "a non-empty reserved reinstall stage must remain visible: {report:?}",
    );
    assert_eq!(report.unreferenced[0].path, stage);
    assert_eq!(report.unreferenced[0].size_bytes, Some(10));
    assert_eq!(report.total_bytes, 10);
    assert_eq!(
        fs::read(stage.join("merged.ini")).expect("stranded stage bytes after audit"),
        b"user bytes",
        "the read-only audit must not mutate surfaced bytes",
    );
}
