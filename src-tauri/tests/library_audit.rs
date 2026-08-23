//! Library consistency audit (#70).
//!
//! These tests drive the public Core seam against real SQLite and real
//! filesystem state. The audit is deliberately read-only: an unreferenced
//! directory may hold the user's only copy of an interrupted import.

use std::fs;
use std::path::Path;

use gmm_lib::core::{junction, Core, GameCode};
use tempfile::TempDir;
use ulid::Ulid;

async fn fresh_core(tmp: &TempDir) -> Core {
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    Core::new(library_root, &db_url).await.expect("init core")
}

#[cfg(unix)]
fn durable_directory_key(path: &Path) -> String {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = fs::metadata(path).expect("directory metadata for reinstall witness");
    format!("{:016x}:{:016x}", metadata.dev(), metadata.ino())
}

#[cfg(windows)]
fn durable_directory_key(path: &Path) -> String {
    use std::fs::OpenOptions;
    use std::mem::MaybeUninit;
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let directory = OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .expect("open directory for reinstall witness identity");
    let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    let ok = unsafe { GetFileInformationByHandle(directory.as_raw_handle(), info.as_mut_ptr()) };
    assert_ne!(
        ok,
        0,
        "read directory identity for reinstall witness: {}",
        std::io::Error::last_os_error(),
    );
    let info = unsafe { info.assume_init() };
    let file = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    format!("{:016x}:{:016x}", info.dwVolumeSerialNumber, file)
}

async fn committed_reinstall_stage(tmp: &TempDir) -> (Core, std::path::PathBuf) {
    let core = fresh_core(tmp).await;
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let source = tmp.path().join("witnessed-source");
    fs::create_dir_all(&source).expect("witnessed source");
    fs::write(source.join("merged.ini"), b"installed bytes").expect("installed bytes");
    let installed = core
        .adopt_folder(GameCode::Gimi, &source, "Witnessed Reinstall")
        .await
        .expect("adopt witnessed Mod");
    let root = installed.library_path.parent().expect("game Library root");
    let token = Ulid::new();
    let stage = root.join(format!(".gmm-reinstall-{token}"));
    let quarantine = root.join(format!(".gmm-delete-{token}"));
    fs::create_dir(&stage).expect("witnessed reinstall stage");
    fs::write(
        stage.join("replacement.ini"),
        b"replacement still extracting",
    )
    .expect("live staged bytes");

    let pool = sqlx::SqlitePool::connect(&db_url)
        .await
        .expect("open DB for reinstall witness");
    sqlx::query(
        "INSERT INTO reinstall_swaps (
            token, mod_id, game_code, library_path, staged_path,
            quarantine_path, old_identity, staged_identity, created_at
         ) VALUES (?, ?, 'gimi', ?, ?, ?, ?, ?, ?)",
    )
    .bind(token.to_string())
    .bind(&installed.id)
    .bind(installed.library_path.to_string_lossy().as_ref())
    .bind(stage.to_string_lossy().as_ref())
    .bind(quarantine.to_string_lossy().as_ref())
    .bind(durable_directory_key(&installed.library_path))
    .bind(durable_directory_key(&stage))
    .bind("2026-08-23T00:00:00Z")
    .execute(&pool)
    .await
    .expect("commit reinstall witness");
    pool.close().await;

    (core, stage)
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

#[tokio::test]
async fn audit_treats_another_games_mod_as_referenced_when_library_roots_are_shared() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let shared_root = tmp.path().join("shared-library-root");
    core.set_library_path_for_game(GameCode::Gimi, Some(&shared_root))
        .await
        .expect("share GIMI Library root");
    core.set_library_path_for_game(GameCode::Srmi, Some(&shared_root))
        .await
        .expect("share SRMI Library root");

    let source = tmp.path().join("shared-root-source");
    fs::create_dir_all(&source).expect("source directory");
    fs::write(source.join("merged.ini"), b"owned by GIMI").expect("source bytes");
    let installed = core
        .adopt_folder(GameCode::Gimi, &source, "Shared Root Mod")
        .await
        .expect("adopt GIMI Mod into shared root");

    let report = core
        .audit_library(GameCode::Srmi)
        .await
        .expect("audit SRMI shared root");

    assert!(
        report.unreferenced.is_empty(),
        "a Mod row from either Game owns its directory in a shared Library root: {report:?}",
    );
    assert_eq!(
        fs::read(installed.library_path.join("merged.ini")).expect("installed bytes"),
        b"owned by GIMI",
        "the read-only audit must leave the other Game's Mod intact",
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

/// Extraction runs after the reinstall witness commits and outside the writer
/// fence. The witness's filesystem identity proves this non-empty stage is
/// live GMM work, so the audit must not offer it as recoverable user data.
#[tokio::test]
async fn audit_suppresses_a_non_empty_witnessed_reinstall_stage() {
    let tmp = TempDir::new().expect("tmp");
    let (core, stage) = committed_reinstall_stage(&tmp).await;

    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit with live witnessed reinstall stage");

    assert!(
        report.unreferenced.is_empty(),
        "a committed witness must keep its live stage out of the orphan report: {report:?}",
    );
    assert_eq!(
        fs::read(stage.join("replacement.ini")).expect("live stage after audit"),
        b"replacement still extracting",
        "the read-only audit must leave the active extraction bytes untouched",
    );
}

#[tokio::test]
async fn recover_refuses_a_witnessed_reinstall_stage() {
    let tmp = TempDir::new().expect("tmp");
    let (core, stage) = committed_reinstall_stage(&tmp).await;

    let recovered = core
        .recover_unreferenced_library_dir(GameCode::Gimi, &stage, "Must Not Recover")
        .await;

    assert!(
        matches!(
            recovered,
            Err(gmm_lib::core::Error::NotAnUnreferencedLibraryDir { .. })
        ),
        "Recover must refuse a committed reinstall stage, got {recovered:?}",
    );
    assert_eq!(
        fs::read(stage.join("replacement.ini")).expect("live stage after refused Recover"),
        b"replacement still extracting",
    );
}

#[tokio::test]
async fn delete_refuses_a_witnessed_reinstall_stage() {
    let tmp = TempDir::new().expect("tmp");
    let (core, stage) = committed_reinstall_stage(&tmp).await;

    let deleted = core
        .delete_unreferenced_library_dir(GameCode::Gimi, &stage)
        .await;

    assert!(
        matches!(
            deleted,
            Err(gmm_lib::core::Error::NotAnUnreferencedLibraryDir { .. })
        ),
        "Delete must refuse a committed reinstall stage, got {deleted:?}",
    );
    assert_eq!(
        fs::read(stage.join("replacement.ini")).expect("live stage after refused Delete"),
        b"replacement still extracting",
    );
}
