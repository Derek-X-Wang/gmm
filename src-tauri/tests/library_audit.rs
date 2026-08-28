//! Library consistency audit (#70).
//!
//! These tests drive the public Core seam against real SQLite and real
//! filesystem state. The audit is deliberately read-only: an unreferenced
//! directory may hold the user's only copy of an interrupted import.

use std::fs;
use std::path::Path;

#[cfg(windows)]
use std::ffi::OsString;
#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};

use gmm_lib::core::{junction, Core, Error, GameCode, ReviewedDuplicateMod, Source};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;
use ulid::Ulid;

#[cfg(unix)]
fn deny_directory_search(path: &Path) -> fs::Permissions {
    use std::os::unix::fs::PermissionsExt as _;

    let original = fs::metadata(path)
        .expect("read directory permissions before error injection")
        .permissions();
    fs::set_permissions(path, fs::Permissions::from_mode(0o0))
        .expect("inject an unreadable deployment directory");
    original
}

#[cfg(unix)]
fn restore_directory_search(path: &Path, permissions: fs::Permissions) {
    fs::set_permissions(path, permissions).expect("restore deployment directory permissions");
}

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
fn short_path_name(path: &Path) -> std::path::PathBuf {
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;

    let long: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let required = unsafe { GetShortPathNameW(long.as_ptr(), std::ptr::null_mut(), 0) };
    assert_ne!(
        required,
        0,
        "read the Junction's 8.3 alias: {}",
        std::io::Error::last_os_error(),
    );
    let mut short = vec![0_u16; required as usize];
    let written = unsafe { GetShortPathNameW(long.as_ptr(), short.as_mut_ptr(), required) };
    assert!(
        written > 0 && written < required,
        "read the Junction's 8.3 alias into the allocated buffer: {}",
        std::io::Error::last_os_error(),
    );
    std::path::PathBuf::from(OsString::from_wide(&short[..written as usize]))
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

async fn remove_reinstall_cardinality_constraints(pool: &sqlx::SqlitePool) {
    sqlx::query("ALTER TABLE reinstall_swaps RENAME TO constrained_reinstall_swaps")
        .execute(pool)
        .await
        .expect("move constrained witness table aside");
    sqlx::query(
        "CREATE TABLE reinstall_swaps (
            token TEXT NOT NULL,
            mod_id TEXT NOT NULL,
            game_code TEXT NOT NULL,
            library_path TEXT NOT NULL,
            staged_path TEXT NOT NULL,
            quarantine_path TEXT NOT NULL,
            old_identity TEXT NOT NULL,
            staged_identity TEXT NOT NULL,
            created_at TEXT NOT NULL,
            recovery_error TEXT,
            recovery_attempted_at TEXT,
            recovery_attempts INTEGER NOT NULL DEFAULT 0,
            junction_withdrawn INTEGER NOT NULL DEFAULT 0,
            junction_withdrawal_error TEXT
         )",
    )
    .execute(pool)
    .await
    .expect("create same-column witness table without cardinality constraints");
    sqlx::query("INSERT INTO reinstall_swaps SELECT * FROM constrained_reinstall_swaps")
        .execute(pool)
        .await
        .expect("copy the original witness into unconstrained table");
    sqlx::query("DROP TABLE constrained_reinstall_swaps")
        .execute(pool)
        .await
        .expect("drop constrained witness table");
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
    let revealed = core
        .unreferenced_library_dir_for_reveal(GameCode::Gimi, &stage)
        .await;
    assert!(
        matches!(
            revealed,
            Err(Error::NotAnUnreferencedLibraryDir { ref reason, .. })
                if reason.contains("empty interrupted reinstall stage")
        ),
        "the shared orphan guard must reject the same harmless empty stage: {revealed:?}",
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
    assert_eq!(
        core.unreferenced_library_dir_for_reveal(GameCode::Gimi, &stage)
            .await
            .expect("the shared guard accepts the same stranded bytes"),
        stage,
    );
}

/// If GMM cannot inspect a reserved-looking directory, it has no evidence the
/// directory is the harmless empty residue case. Both report and guard must
/// preserve visibility instead of converting uncertainty into absence.
#[cfg(unix)]
#[tokio::test]
async fn audit_and_guard_keep_an_uninspectable_reinstall_stage_visible() {
    use std::os::unix::fs::PermissionsExt as _;

    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let game_root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game root");
    fs::create_dir_all(&game_root).expect("game root directory");
    let stage = game_root.join(".gmm-reinstall-01JUNINSPECTABLE00000000");
    fs::create_dir(&stage).expect("uninspectable reinstall stage");
    let original = fs::metadata(&stage).expect("stage metadata").permissions();
    fs::set_permissions(&stage, fs::Permissions::from_mode(0o000))
        .expect("make the stage uninspectable");
    assert!(
        fs::read_dir(&stage).is_err(),
        "fixture must deny directory inspection"
    );

    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit an uninspectable reserved stage");
    assert!(
        report
            .unreferenced
            .iter()
            .any(|directory| directory.path == stage && directory.size_bytes.is_none()),
        "uncertain reserved bytes must remain visible with an unknown size: {report:?}",
    );
    let reveal = core
        .unreferenced_library_dir_for_reveal(GameCode::Gimi, &stage)
        .await;
    assert!(
        matches!(reveal, Err(Error::Io { .. })),
        "the guard must fail closed until it can establish the directory identity: {reveal:?}",
    );

    fs::set_permissions(&stage, original).expect("restore stage permissions");
}

/// Ownership is global because per-game overrides may legitimately share one
/// root. A GIMI reinstall stage must therefore be hidden from an SRMI audit,
/// just as the action guard already refuses it across Games.
#[tokio::test]
async fn audit_hides_another_games_reinstall_stage_in_a_shared_library_root() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let shared_root = tmp.path().join("shared-reinstall-root");
    core.set_library_path_for_game(GameCode::Gimi, Some(&shared_root))
        .await
        .expect("share GIMI root");
    core.set_library_path_for_game(GameCode::Srmi, Some(&shared_root))
        .await
        .expect("share SRMI root");

    let source = tmp.path().join("shared-reinstall-source");
    fs::create_dir(&source).expect("shared reinstall source");
    fs::write(source.join("merged.ini"), b"installed bytes").expect("installed bytes");
    let installed = core
        .adopt_folder(GameCode::Gimi, &source, "Other Game Reinstall")
        .await
        .expect("adopt GIMI Mod");
    let token = Ulid::new();
    let stage = shared_root.join(format!(".gmm-reinstall-{token}"));
    let quarantine = shared_root.join(format!(".gmm-delete-{token}"));
    fs::create_dir(&stage).expect("other Game reinstall stage");
    fs::write(stage.join("replacement.ini"), b"still extracting").expect("other Game staged bytes");

    let pool = sqlx::SqlitePool::connect(&db_url)
        .await
        .expect("open shared-root DB");
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
    .bind("2026-08-24T00:00:00Z")
    .execute(&pool)
    .await
    .expect("commit other Game reinstall witness");
    pool.close().await;

    let report = core
        .audit_library(GameCode::Srmi)
        .await
        .expect("audit shared root as SRMI");
    assert!(
        report.unreferenced.is_empty(),
        "either Game's active reinstall owns its stage in the shared root: {report:?}",
    );
    let revealed = core
        .unreferenced_library_dir_for_reveal(GameCode::Srmi, &stage)
        .await;
    assert!(
        matches!(
            revealed,
            Err(Error::NotAnUnreferencedLibraryDir { ref reason, .. })
                if reason.contains("interrupted reinstall state")
        ),
        "the shared guard must refuse the same other-Game stage: {revealed:?}",
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
        "Delete must refuse a committed reinstall stage",
    );
    assert_eq!(
        fs::read(stage.join("replacement.ini")).expect("live stage after refused Delete"),
        b"replacement still extracting",
    );
}

/// A malformed durable identity is corrupt state, never evidence that the
/// active reinstall owns nothing. This drives the public Delete seam so the
/// regression cannot pass by validating only startup recovery's row shape.
#[tokio::test]
async fn delete_refuses_a_malformed_reinstall_identity_before_classifying_bytes() {
    let tmp = TempDir::new().expect("tmp");
    let (core, stage) = committed_reinstall_stage(&tmp).await;
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let pool = sqlx::SqlitePool::connect(&db_url)
        .await
        .expect("open DB for corrupt reinstall identity");
    sqlx::query("UPDATE reinstall_swaps SET staged_identity = 'not-a-durable-identity'")
        .execute(&pool)
        .await
        .expect("corrupt reinstall identity fixture");
    pool.close().await;

    let deleted = core
        .delete_unreferenced_library_dir(GameCode::Gimi, &stage)
        .await;
    assert!(
        matches!(deleted, Err(Error::ReinstallWitnessCorrupt { .. })),
        "a malformed reinstall identity must refuse Delete before bytes are classified as unreferenced",
    );
    assert_eq!(
        fs::read(stage.join("replacement.ini")).expect("witnessed bytes after refused Delete"),
        b"replacement still extracting",
        "a malformed durable identity must not make active reinstall bytes deletable",
    );
}

/// The migration normally enforces one row per token, but the validated
/// loader must not silently trust that schema invariant. A same-column table
/// without its primary key can contain two valid rows sharing a token; every
/// consumer must reject that cardinality before choosing or deleting rows.
#[tokio::test]
async fn delete_refuses_duplicate_reinstall_tokens_at_the_loader_boundary() {
    let tmp = TempDir::new().expect("tmp");
    let (core, stage) = committed_reinstall_stage(&tmp).await;
    let source = tmp.path().join("second-token-owner-source");
    fs::create_dir(&source).expect("second Mod source");
    fs::write(source.join("merged.ini"), b"second installed bytes").expect("second Mod bytes");
    let second = core
        .adopt_folder(GameCode::Gimi, &source, "Second Token Owner")
        .await
        .expect("adopt second Mod");
    let token = stage
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(".gmm-reinstall-"))
        .expect("token in staged name");
    let root = stage.parent().expect("game Library root");
    let quarantine = root.join(format!(".gmm-delete-{token}"));
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let pool = sqlx::SqlitePool::connect(&db_url)
        .await
        .expect("open DB for duplicate token fixture");
    remove_reinstall_cardinality_constraints(&pool).await;
    sqlx::query(
        "INSERT INTO reinstall_swaps (
            token, mod_id, game_code, library_path, staged_path,
            quarantine_path, old_identity, staged_identity, created_at
         ) VALUES (?, ?, 'gimi', ?, ?, ?, ?, ?, ?)",
    )
    .bind(token)
    .bind(&second.id)
    .bind(second.library_path.to_string_lossy().as_ref())
    .bind(stage.to_string_lossy().as_ref())
    .bind(quarantine.to_string_lossy().as_ref())
    .bind(durable_directory_key(&second.library_path))
    .bind(durable_directory_key(&stage))
    .bind("2026-08-25T00:00:00Z")
    .execute(&pool)
    .await
    .expect("insert second witness with duplicate token");
    pool.close().await;

    let deleted = core
        .delete_unreferenced_library_dir(GameCode::Gimi, &stage)
        .await;
    assert!(
        matches!(
            deleted,
            Err(Error::ReinstallWitnessCorrupt { ref reason, .. })
                if reason.contains("swap token") && reason.contains("appears more than once")
        ),
        "duplicate reinstall tokens must be rejected before any caller chooses a witness: {deleted:?}",
    );
    assert!(
        stage.join("replacement.ini").is_file(),
        "duplicate durable rows must not make witnessed bytes deletable",
    );
}

/// The migration also normally enforces one witness per Mod. The loader owns
/// that assumption too: two valid tokens for one Mod are corrupt durable state,
/// never permission for `.find()` to choose one while rollback deletes both.
#[tokio::test]
async fn delete_refuses_duplicate_reinstall_mods_at_the_loader_boundary() {
    let tmp = TempDir::new().expect("tmp");
    let (core, first_stage) = committed_reinstall_stage(&tmp).await;
    let first_token = first_stage
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(".gmm-reinstall-"))
        .expect("first token in staged name");
    let root = first_stage.parent().expect("game Library root");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let pool = sqlx::SqlitePool::connect(&db_url)
        .await
        .expect("open DB for duplicate Mod fixture");
    let mod_id: String = sqlx::query_scalar("SELECT mod_id FROM reinstall_swaps WHERE token = ?")
        .bind(first_token)
        .fetch_one(&pool)
        .await
        .expect("read witnessed Mod ID");
    let library_path: String =
        sqlx::query_scalar("SELECT library_path FROM reinstall_swaps WHERE token = ?")
            .bind(first_token)
            .fetch_one(&pool)
            .await
            .expect("read witnessed Library path");
    remove_reinstall_cardinality_constraints(&pool).await;
    let second_token = Ulid::new();
    let second_stage = root.join(format!(".gmm-reinstall-{second_token}"));
    let second_quarantine = root.join(format!(".gmm-delete-{second_token}"));
    fs::create_dir(&second_stage).expect("second reinstall stage");
    fs::write(second_stage.join("replacement.ini"), b"second replacement")
        .expect("second replacement bytes");
    sqlx::query(
        "INSERT INTO reinstall_swaps (
            token, mod_id, game_code, library_path, staged_path,
            quarantine_path, old_identity, staged_identity, created_at
         ) VALUES (?, ?, 'gimi', ?, ?, ?, ?, ?, ?)",
    )
    .bind(second_token.to_string())
    .bind(&mod_id)
    .bind(&library_path)
    .bind(second_stage.to_string_lossy().as_ref())
    .bind(second_quarantine.to_string_lossy().as_ref())
    .bind(durable_directory_key(Path::new(&library_path)))
    .bind(durable_directory_key(&second_stage))
    .bind("2026-08-25T00:00:01Z")
    .execute(&pool)
    .await
    .expect("insert second witness for the same Mod");
    pool.close().await;

    let deleted = core
        .delete_unreferenced_library_dir(GameCode::Gimi, &second_stage)
        .await;
    assert!(
        matches!(
            deleted,
            Err(Error::ReinstallWitnessCorrupt { ref reason, .. })
                if reason.contains("has more than one reinstall witness")
        ),
        "duplicate reinstall Mod witnesses must be rejected before any caller chooses a row: {deleted:?}",
    );
    assert!(
        first_stage.join("replacement.ini").is_file()
            && second_stage.join("replacement.ini").is_file(),
        "impossible Mod cardinality must preserve every witnessed byte tree",
    );
}

/// The staged adopt/import table crosses the same trust boundary as reinstall
/// recovery. Its identity must be parsed by the shared loader before Delete
/// can decide the staged directory is unowned.
#[tokio::test]
async fn delete_refuses_a_malformed_staging_identity_before_classifying_bytes() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("resolve staging Library root");
    fs::create_dir_all(&root).expect("create staging Library root");
    let id = Ulid::new();
    let stage = root.join(id.to_string());
    fs::create_dir(&stage).expect("corrupt witnessed stage");
    fs::write(stage.join("partial.ini"), b"partial import bytes").expect("partial bytes");
    let pool = sqlx::SqlitePool::connect(&db_url)
        .await
        .expect("open DB for corrupt staging identity");
    sqlx::query(
        "INSERT INTO staged_library_operations (
            id, game_code, operation, staged_path, staged_identity, created_at
         ) VALUES (?, 'gimi', 'adopt', ?, 'not-a-durable-identity', ?)",
    )
    .bind(id.to_string())
    .bind(stage.to_string_lossy().as_ref())
    .bind("2026-08-24T00:00:00Z")
    .execute(&pool)
    .await
    .expect("insert corrupt staging witness");
    pool.close().await;

    let deleted = core
        .delete_unreferenced_library_dir(GameCode::Gimi, &stage)
        .await;
    assert!(
        matches!(deleted, Err(Error::StagingWitnessCorrupt { .. })),
        "a malformed staging identity must refuse Delete before bytes are classified as unreferenced",
    );
    assert_eq!(
        fs::read(stage.join("partial.ini")).expect("partial bytes after refused Delete"),
        b"partial import bytes",
        "a malformed durable identity must not make active staging bytes deletable",
    );
}

/// Adding a durable column must force a validation decision in the same macro
/// declaration that defines the staged raw row. Named-column queries alone do
/// not detect this drift, so exercise the complete row through Delete.
#[tokio::test]
async fn delete_refuses_a_staging_witness_column_without_a_validation_rule() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("resolve staging Library root");
    fs::create_dir_all(&root).expect("create staging Library root");
    let id = Ulid::new();
    let stage = root.join(id.to_string());
    fs::create_dir(&stage).expect("future-column witnessed stage");
    fs::write(stage.join("partial.ini"), b"partial import bytes").expect("partial bytes");
    let pool = sqlx::SqlitePool::connect(&db_url)
        .await
        .expect("open DB for future staged column");
    sqlx::query(
        "INSERT INTO staged_library_operations (
            id, game_code, operation, staged_path, staged_identity, created_at
         ) VALUES (?, 'gimi', 'adopt', ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(stage.to_string_lossy().as_ref())
    .bind(durable_directory_key(&stage))
    .bind("2026-08-24T00:00:00Z")
    .execute(&pool)
    .await
    .expect("insert valid staging witness");
    sqlx::query("ALTER TABLE staged_library_operations ADD COLUMN unruled_future_state TEXT")
        .execute(&pool)
        .await
        .expect("simulate a future staged-witness migration");
    pool.close().await;

    let deleted = core
        .delete_unreferenced_library_dir(GameCode::Gimi, &stage)
        .await;
    assert!(
        matches!(deleted, Err(Error::StagingWitnessCorrupt { .. })),
        "an unruled staged-witness column must refuse Delete at the validated row boundary",
    );
    assert!(
        stage.join("partial.ini").is_file(),
        "schema drift must stop Delete before staged bytes move",
    );
}

struct DuplicateFixture {
    core: Core,
    db_url: String,
    library_path: std::path::PathBuf,
    keeper_id: String,
    duplicate_id: String,
    duplicate_variant_id: String,
    duplicate_junction: std::path::PathBuf,
}

async fn duplicate_fixture(tmp: &TempDir) -> DuplicateFixture {
    let core = fresh_core(tmp).await;
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let source = tmp.path().join("duplicate-source");
    fs::create_dir_all(source.join("Amber")).expect("Amber Variant");
    fs::create_dir_all(source.join("Blue")).expect("Blue Variant");
    fs::write(source.join("Amber/mod.ini"), b"amber").expect("Amber ini");
    fs::write(source.join("Blue/mod.ini"), b"blue").expect("Blue ini");
    let keeper = core
        .adopt_folder(GameCode::Gimi, &source, "Manual Keeper")
        .await
        .expect("adopt keeper");
    fs::write(
        keeper.library_path.join("sentinel.bin"),
        b"shared user bytes",
    )
    .expect("shared sentinel");

    let duplicate_id = Ulid::new().to_string();
    let duplicate_variant_id = Ulid::new().to_string();
    let other_variant_id = Ulid::new().to_string();
    let duplicate_path_spelling = keeper
        .library_path
        .parent()
        .expect("keeper Library parent")
        .join(".")
        .join(
            keeper
                .library_path
                .file_name()
                .expect("keeper Library directory name"),
        );
    assert_ne!(
        duplicate_path_spelling.as_os_str(),
        keeper.library_path.as_os_str(),
        "fixture uses distinct strings for one filesystem identity",
    );
    let pool = sqlx::SqlitePool::connect(&db_url)
        .await
        .expect("open duplicate fixture DB");
    sqlx::query(
        "INSERT INTO mods (
            id, game_code, name, source, library_path, junction_dir_name,
            enabled, created_at, gamebanana_id, source_url, author, version,
            upstream_version, update_check_enabled, screenshot_url
         ) VALUES (?, 'gimi', ?, 'gamebanana', ?, ?, 0, ?, 24680, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&duplicate_id)
    .bind("GameBanana Duplicate")
    .bind(duplicate_path_spelling.to_string_lossy().as_ref())
    .bind("GameBanana Duplicate")
    .bind("2026-08-24T00:00:01Z")
    .bind("https://gamebanana.com/mods/24680")
    .bind("Duplicate Author")
    .bind("9.9.9")
    .bind("10.0.0")
    .bind(0_i64)
    .bind("https://images.example.test/duplicate.png")
    .execute(&pool)
    .await
    .expect("insert duplicate Mod row");
    for (id, name) in [
        (&duplicate_variant_id, "Amber"),
        (&other_variant_id, "Blue"),
    ] {
        sqlx::query("INSERT INTO mod_variants (id, mod_id, name, subpath) VALUES (?, ?, ?, ?)")
            .bind(id)
            .bind(&duplicate_id)
            .bind(name)
            .bind(name)
            .execute(&pool)
            .await
            .expect("insert duplicate Variant");
    }
    sqlx::query("UPDATE mods SET active_variant_id = ? WHERE id = ?")
        .bind(&duplicate_variant_id)
        .bind(&duplicate_id)
        .execute(&pool)
        .await
        .expect("select duplicate active Variant");
    pool.close().await;

    let game_install = tmp.path().join("game");
    let game_mods = game_install.join("Mods");
    fs::create_dir_all(&game_mods).expect("game Mods directory");
    core.set_game_install_path(GameCode::Gimi, &game_install)
        .await
        .expect("record game install");
    core.set_enabled(&duplicate_id, true, &game_mods)
        .await
        .expect("enable duplicate Mod");

    DuplicateFixture {
        core,
        db_url,
        library_path: keeper.library_path,
        keeper_id: keeper.id,
        duplicate_id,
        duplicate_variant_id,
        duplicate_junction: game_mods.join("GameBanana Duplicate"),
    }
}

async fn reviewed_duplicate_mods(fixture: &DuplicateFixture) -> Vec<ReviewedDuplicateMod> {
    fixture
        .core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit duplicate records before review")
        .duplicates
        .into_iter()
        .find(|group| {
            group
                .mods
                .iter()
                .any(|record| record.id == fixture.keeper_id)
        })
        .expect("fixture duplicate group")
        .mods
        .into_iter()
        .map(|record| ReviewedDuplicateMod {
            id: record.id,
            fingerprint: record.fingerprint,
        })
        .collect()
}

#[tokio::test]
async fn audit_surfaces_every_informed_duplicate_choice_without_mutating_it() {
    let tmp = TempDir::new().expect("tmp");
    let fixture = duplicate_fixture(&tmp).await;

    let report = fixture
        .core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit duplicates");

    assert_eq!(
        report.duplicates.len(),
        1,
        "one filesystem identity is duplicated"
    );
    let duplicate = &report.duplicates[0];
    assert_eq!(duplicate.path, fixture.library_path);
    assert_eq!(duplicate.mods.len(), 2);
    let keeper = duplicate
        .mods
        .iter()
        .find(|record| record.id == fixture.keeper_id)
        .expect("keeper record is surfaced");
    assert_eq!(keeper.name, "Manual Keeper");
    assert_eq!(keeper.source, Source::Manual);
    assert!(!keeper.enabled);

    let rejected = duplicate
        .mods
        .iter()
        .find(|record| record.id == fixture.duplicate_id)
        .expect("duplicate record is surfaced");
    assert_eq!(rejected.name, "GameBanana Duplicate");
    assert_eq!(rejected.source, Source::Gamebanana);
    assert!(rejected.enabled, "the enabled state is part of the choice");
    assert_eq!(rejected.gamebanana_id, Some(24680));
    assert_eq!(rejected.author.as_deref(), Some("Duplicate Author"));
    assert_eq!(rejected.version.as_deref(), Some("9.9.9"));
    assert_eq!(rejected.upstream_version.as_deref(), Some("10.0.0"));
    assert!(!rejected.update_check_enabled);
    assert_eq!(rejected.junction_dir_name, "GameBanana Duplicate");
    assert_eq!(rejected.variants.len(), 2, "the full Variant set is shown");
    assert!(
        rejected
            .variants
            .iter()
            .any(|variant| variant.id == fixture.duplicate_variant_id && variant.active),
        "the active Variant selection is shown",
    );
    assert!(
        fixture.duplicate_junction.exists(),
        "the existing Junction remains"
    );
    assert_eq!(
        fs::read(fixture.library_path.join("sentinel.bin")).expect("shared bytes"),
        b"shared user bytes",
        "the audit is read-only",
    );
}

#[tokio::test]
async fn duplicate_fingerprint_covers_every_rendered_review_field() {
    let tmp = TempDir::new().expect("tmp");
    let fixture = duplicate_fixture(&tmp).await;
    let report = fixture
        .core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit duplicates");
    let record = report.duplicates[0]
        .mods
        .iter()
        .find(|record| record.id == fixture.duplicate_id)
        .expect("rendered duplicate record");
    let mut rendered = serde_json::to_value(record).expect("serialise rendered record");
    let fingerprint = rendered
        .as_object_mut()
        .expect("rendered record is an object")
        .remove("fingerprint")
        .expect("rendered record includes its fingerprint");
    let expected = hex::encode(Sha256::digest(
        serde_json::to_vec(&rendered).expect("encode rendered review state"),
    ));

    assert_eq!(
        fingerprint.as_str(),
        Some(expected.as_str()),
        "the fingerprint must cover the complete rendered review state",
    );
}

#[tokio::test]
async fn explicit_duplicate_resolution_keeps_shared_bytes_and_withdraws_the_rejected_junction() {
    let tmp = TempDir::new().expect("tmp");
    let fixture = duplicate_fixture(&tmp).await;
    let reviewed = reviewed_duplicate_mods(&fixture).await;

    let resolved = fixture
        .core
        .resolve_duplicate_mods(&fixture.keeper_id, &reviewed)
        .await
        .expect("resolve reviewed duplicates");

    assert_eq!(resolved.keeper_id, fixture.keeper_id);
    assert_eq!(resolved.removed_mod_ids.len(), 1);
    assert_eq!(resolved.removed_mod_ids[0], fixture.duplicate_id);
    assert_eq!(
        fs::read(fixture.library_path.join("sentinel.bin")).expect("shared bytes after resolution"),
        b"shared user bytes",
        "duplicate resolution must not touch a single shared Library byte",
    );
    assert!(
        fixture.library_path.is_dir(),
        "the keeper still owns the directory"
    );
    assert!(
        fs::symlink_metadata(&fixture.duplicate_junction).is_err(),
        "the rejected record's Junction is withdrawn",
    );

    let pool = sqlx::SqlitePool::connect(&fixture.db_url)
        .await
        .expect("reopen resolved DB");
    let keeper_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mods WHERE id = ?")
        .bind(&fixture.keeper_id)
        .fetch_one(&pool)
        .await
        .expect("count keeper");
    let rejected_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mods WHERE id = ?")
        .bind(&fixture.duplicate_id)
        .fetch_one(&pool)
        .await
        .expect("count rejected row");
    let rejected_variants: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM mod_variants WHERE mod_id = ?")
            .bind(&fixture.duplicate_id)
            .fetch_one(&pool)
            .await
            .expect("count rejected Variants");
    assert_eq!(keeper_rows, 1);
    assert_eq!(rejected_rows, 0);
    assert_eq!(
        rejected_variants, 0,
        "the explicitly rejected Variant set cascades"
    );
    pool.close().await;

    let report = fixture
        .core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit resolved state");
    assert!(
        report.duplicates.is_empty(),
        "the user reached one row per directory"
    );
}

#[tokio::test]
async fn duplicate_resolution_refuses_an_active_reinstall_witness_without_changing_any_state() {
    let tmp = TempDir::new().expect("tmp");
    let fixture = duplicate_fixture(&tmp).await;
    let reviewed = reviewed_duplicate_mods(&fixture).await;
    let token = Ulid::new();
    let root = fixture.library_path.parent().expect("game Library root");
    let stage = root.join(format!(".gmm-reinstall-{token}"));
    let quarantine = root.join(format!(".gmm-delete-{token}"));
    fs::create_dir(&stage).expect("reinstall stage");
    fs::write(stage.join("replacement.ini"), b"unfinished update").expect("staged bytes");
    let pool = sqlx::SqlitePool::connect(&fixture.db_url)
        .await
        .expect("open witnessed duplicate DB");
    sqlx::query(
        "INSERT INTO reinstall_swaps (
            token, mod_id, game_code, library_path, staged_path,
            quarantine_path, old_identity, staged_identity, created_at
         ) VALUES (?, ?, 'gimi', ?, ?, ?, ?, ?, ?)",
    )
    .bind(token.to_string())
    .bind(&fixture.keeper_id)
    .bind(fixture.library_path.to_string_lossy().as_ref())
    .bind(stage.to_string_lossy().as_ref())
    .bind(quarantine.to_string_lossy().as_ref())
    .bind(durable_directory_key(&fixture.library_path))
    .bind(durable_directory_key(&stage))
    .bind("2026-08-24T00:00:02Z")
    .execute(&pool)
    .await
    .expect("insert active reinstall witness");

    let result = fixture
        .core
        .resolve_duplicate_mods(&fixture.keeper_id, &reviewed)
        .await;
    assert!(
        matches!(
            result,
            Err(Error::DuplicateModResolutionBlockedByReinstall { ref mod_id })
                if mod_id == &fixture.keeper_id
        ),
        "the witness must refuse row deletion, got {result:?}",
    );

    let mod_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mods WHERE id IN (?, ?)")
        .bind(&fixture.keeper_id)
        .bind(&fixture.duplicate_id)
        .fetch_one(&pool)
        .await
        .expect("count preserved duplicate rows");
    let witnesses: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reinstall_swaps WHERE token = ?")
        .bind(token.to_string())
        .fetch_one(&pool)
        .await
        .expect("count preserved witness");
    let variants: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mod_variants WHERE mod_id = ?")
        .bind(&fixture.duplicate_id)
        .fetch_one(&pool)
        .await
        .expect("count preserved Variants");
    assert_eq!(mod_rows, 2, "both Mod records survive refusal");
    assert_eq!(witnesses, 1, "the reinstall witness survives refusal");
    assert_eq!(variants, 2, "the active Variant set survives refusal");
    assert!(
        fixture.duplicate_junction.exists(),
        "the existing Junction survives refusal"
    );
    assert_eq!(
        fs::read(fixture.library_path.join("sentinel.bin")).expect("shared bytes after refusal"),
        b"shared user bytes",
    );
    assert_eq!(
        fs::read(stage.join("replacement.ini")).expect("staged bytes after refusal"),
        b"unfinished update",
    );
    pool.close().await;
}

#[tokio::test]
async fn duplicate_resolution_refuses_when_the_reviewed_group_is_stale() {
    let tmp = TempDir::new().expect("tmp");
    let fixture = duplicate_fixture(&tmp).await;
    let reviewed = reviewed_duplicate_mods(&fixture).await;
    let unseen_id = Ulid::new().to_string();
    let pool = sqlx::SqlitePool::connect(&fixture.db_url)
        .await
        .expect("open duplicate DB");
    sqlx::query(
        "INSERT INTO mods (
            id, game_code, name, source, library_path, junction_dir_name, enabled, created_at
         ) VALUES (?, 'gimi', 'Unseen Duplicate', 'manual', ?, 'Unseen Duplicate', 0, ?)",
    )
    .bind(&unseen_id)
    .bind(fixture.library_path.to_string_lossy().as_ref())
    .bind("2026-08-24T00:00:03Z")
    .execute(&pool)
    .await
    .expect("insert duplicate after the user's report");

    let result = fixture
        .core
        .resolve_duplicate_mods(&fixture.keeper_id, &reviewed)
        .await;
    assert!(
        matches!(result, Err(Error::DuplicateModResolutionChanged { .. })),
        "resolution must refuse instead of widening a stale choice, got {result:?}",
    );
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mods WHERE id IN (?, ?, ?)")
        .bind(&fixture.keeper_id)
        .bind(&fixture.duplicate_id)
        .bind(&unseen_id)
        .fetch_one(&pool)
        .await
        .expect("count duplicate rows after refusal");
    assert_eq!(rows, 3, "all reviewed and unseen records survive refusal");
    assert!(
        fixture.duplicate_junction.exists(),
        "the Junction survives refusal"
    );
    assert_eq!(
        fs::read(fixture.library_path.join("sentinel.bin")).expect("shared bytes after refusal"),
        b"shared user bytes",
    );
    pool.close().await;
}

#[tokio::test]
async fn duplicate_resolution_refuses_when_a_displayed_field_changed_after_review() {
    let tmp = TempDir::new().expect("tmp");
    let fixture = duplicate_fixture(&tmp).await;
    let reviewed = reviewed_duplicate_mods(&fixture).await;
    let pool = sqlx::SqlitePool::connect(&fixture.db_url)
        .await
        .expect("open duplicate DB");
    sqlx::query("UPDATE mods SET name = 'Changed after review' WHERE id = ?")
        .bind(&fixture.duplicate_id)
        .execute(&pool)
        .await
        .expect("change displayed Mod name");

    let result = fixture
        .core
        .resolve_duplicate_mods(&fixture.keeper_id, &reviewed)
        .await;
    assert!(
        matches!(result, Err(Error::DuplicateModResolutionChanged { ref reason }) if reason.contains("changed after the audit")),
        "displayed-field drift must require a fresh review, got {result:?}",
    );
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mods WHERE id IN (?, ?)")
        .bind(&fixture.keeper_id)
        .bind(&fixture.duplicate_id)
        .fetch_one(&pool)
        .await
        .expect("count records after drift refusal");
    assert_eq!(
        rows, 2,
        "ID equality must not authorize deleting changed contents"
    );
    assert!(
        fixture.duplicate_junction.exists(),
        "the Junction survives drift refusal"
    );
}

#[tokio::test]
async fn duplicate_resolution_refuses_a_junction_path_claimed_by_the_keeper() {
    let tmp = TempDir::new().expect("tmp");
    let fixture = duplicate_fixture(&tmp).await;
    let install = fixture
        .duplicate_junction
        .parent()
        .and_then(std::path::Path::parent)
        .expect("game install")
        .to_path_buf();
    fixture
        .core
        .set_game_install_path(GameCode::Srmi, &install)
        .await
        .expect("share one install path across games");
    let pool = sqlx::SqlitePool::connect(&fixture.db_url)
        .await
        .expect("open duplicate DB");
    sqlx::query(
        "UPDATE mods SET game_code = 'srmi', junction_dir_name = 'GameBanana Duplicate' WHERE id = ?",
    )
    .bind(&fixture.keeper_id)
    .execute(&pool)
    .await
    .expect("make keeper claim the rejected row's physical Junction path");
    let reviewed = reviewed_duplicate_mods(&fixture).await;

    let result = fixture
        .core
        .resolve_duplicate_mods(&fixture.keeper_id, &reviewed)
        .await;
    assert!(
        matches!(
            result,
            Err(Error::DuplicateModJunctionClaimedBySurvivor {
                ref mod_id,
                ref surviving_mod_id,
                ..
            }) if mod_id == &fixture.duplicate_id && surviving_mod_id == &fixture.keeper_id
        ),
        "a surviving keeper's deployment path must never be removed, got {result:?}",
    );
    assert!(
        fixture.duplicate_junction.exists(),
        "the shared physical Junction remains for the keeper",
    );
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mods WHERE id IN (?, ?)")
        .bind(&fixture.keeper_id)
        .bind(&fixture.duplicate_id)
        .fetch_one(&pool)
        .await
        .expect("count records after shared-Junction refusal");
    assert_eq!(rows, 2);
}

#[tokio::test]
async fn duplicate_resolution_refuses_a_junction_path_claimed_outside_the_reviewed_group() {
    let tmp = TempDir::new().expect("tmp");
    let fixture = duplicate_fixture(&tmp).await;
    let outside_source = tmp.path().join("outside-survivor-source");
    fs::create_dir(&outside_source).expect("outside survivor source");
    fs::write(outside_source.join("outside.ini"), b"outside").expect("outside bytes");
    let outside = fixture
        .core
        .adopt_folder(GameCode::Srmi, &outside_source, "Outside Survivor")
        .await
        .expect("adopt survivor outside the reviewed duplicate group");
    let install = fixture
        .duplicate_junction
        .parent()
        .and_then(std::path::Path::parent)
        .expect("game install");
    fixture
        .core
        .set_game_install_path(GameCode::Srmi, install)
        .await
        .expect("share one install path across games");
    let pool = sqlx::SqlitePool::connect(&fixture.db_url)
        .await
        .expect("open duplicate DB");
    sqlx::query("UPDATE mods SET junction_dir_name = ? WHERE id = ?")
        .bind(
            fixture
                .duplicate_junction
                .file_name()
                .expect("rejected Junction leaf")
                .to_string_lossy()
                .as_ref(),
        )
        .bind(&outside.id)
        .execute(&pool)
        .await
        .expect("claim the reviewed group's Junction from an outside Mod");
    let reviewed = reviewed_duplicate_mods(&fixture).await;

    let result = fixture
        .core
        .resolve_duplicate_mods(&fixture.keeper_id, &reviewed)
        .await;
    assert!(
        matches!(
            result,
            Err(Error::DuplicateModJunctionClaimedBySurvivor {
                ref mod_id,
                ref surviving_mod_id,
                ..
            }) if mod_id == &fixture.duplicate_id && surviving_mod_id == &outside.id
        ),
        "a survivor outside the reviewed group must protect its Junction, got {result:?}",
    );
    assert!(
        fixture.duplicate_junction.exists(),
        "the outside survivor's physical Junction remains",
    );
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mods WHERE id IN (?, ?, ?)")
        .bind(&fixture.keeper_id)
        .bind(&fixture.duplicate_id)
        .bind(&outside.id)
        .fetch_one(&pool)
        .await
        .expect("count every record after outside-survivor refusal");
    assert_eq!(rows, 3, "every record survives refusal");
}

#[cfg(windows)]
#[tokio::test]
async fn duplicate_resolution_refuses_a_survivor_claiming_the_junction_by_its_short_name() {
    let tmp = TempDir::new().expect("tmp");
    let fixture = duplicate_fixture(&tmp).await;
    let short_path = short_path_name(&fixture.duplicate_junction);
    let short_name = short_path.file_name().expect("short Junction leaf name");
    assert_ne!(
        short_name,
        fixture
            .duplicate_junction
            .file_name()
            .expect("long Junction leaf name"),
        "the Windows CI volume must expose a distinct 8.3 alias for this Junction",
    );

    let outside_source = tmp.path().join("outside-reviewed-group");
    fs::create_dir(&outside_source).expect("outside source");
    fs::write(outside_source.join("outside.ini"), b"outside").expect("outside bytes");
    let outside = fixture
        .core
        .adopt_folder(GameCode::Srmi, &outside_source, "Outside Survivor")
        .await
        .expect("adopt survivor outside the reviewed duplicate group");
    let install = fixture
        .duplicate_junction
        .parent()
        .and_then(std::path::Path::parent)
        .expect("game install");
    fixture
        .core
        .set_game_install_path(GameCode::Srmi, install)
        .await
        .expect("share one install path across games");
    let pool = sqlx::SqlitePool::connect(&fixture.db_url)
        .await
        .expect("open duplicate DB");
    sqlx::query("UPDATE mods SET junction_dir_name = ? WHERE id = ?")
        .bind(short_name.to_string_lossy().as_ref())
        .bind(&outside.id)
        .execute(&pool)
        .await
        .expect("claim the existing Junction through its 8.3 alias");
    let reviewed = reviewed_duplicate_mods(&fixture).await;

    let result = fixture
        .core
        .resolve_duplicate_mods(&fixture.keeper_id, &reviewed)
        .await;
    assert!(
        matches!(
            result,
            Err(Error::DuplicateModJunctionClaimedBySurvivor {
                ref mod_id,
                ref surviving_mod_id,
                ..
            }) if mod_id == &fixture.duplicate_id && surviving_mod_id == &outside.id
        ),
        "a survivor's 8.3 spelling must identify the same Junction entry, got {result:?}",
    );
    assert!(
        fixture.duplicate_junction.exists(),
        "the survivor's physical Junction remains",
    );
    let duplicate_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mods WHERE id IN (?, ?)")
        .bind(&fixture.keeper_id)
        .bind(&fixture.duplicate_id)
        .fetch_one(&pool)
        .await
        .expect("count reviewed duplicate rows after refusal");
    assert_eq!(duplicate_rows, 2, "both reviewed records survive refusal");
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn duplicate_resolution_refuses_a_survivor_claiming_the_same_case_insensitive_entry() {
    use std::os::unix::fs::MetadataExt as _;

    let tmp = TempDir::new().expect("tmp");
    let fixture = duplicate_fixture(&tmp).await;
    let alias_name = fixture
        .duplicate_junction
        .file_name()
        .expect("Junction leaf name")
        .to_string_lossy()
        .to_ascii_lowercase();
    let alias_path = fixture
        .duplicate_junction
        .parent()
        .expect("game Mods directory")
        .join(&alias_name);
    assert_ne!(
        alias_path, fixture.duplicate_junction,
        "the test must use a distinct path spelling",
    );
    let original_metadata = fs::symlink_metadata(&fixture.duplicate_junction)
        .expect("read the original deployment entry");
    let alias_metadata = fs::symlink_metadata(&alias_path)
        .expect("default macOS storage must resolve the case-insensitive alias");
    assert_eq!(
        (original_metadata.dev(), original_metadata.ino()),
        (alias_metadata.dev(), alias_metadata.ino()),
        "both spellings must identify one deployment entry",
    );

    let outside_source = tmp.path().join("outside-case-alias");
    fs::create_dir(&outside_source).expect("outside source");
    fs::write(outside_source.join("outside.ini"), b"outside").expect("outside bytes");
    let outside = fixture
        .core
        .adopt_folder(GameCode::Srmi, &outside_source, "Outside Case Survivor")
        .await
        .expect("adopt survivor outside the reviewed duplicate group");
    let install = fixture
        .duplicate_junction
        .parent()
        .and_then(std::path::Path::parent)
        .expect("game install");
    fixture
        .core
        .set_game_install_path(GameCode::Srmi, install)
        .await
        .expect("share one install path across games");
    let pool = sqlx::SqlitePool::connect(&fixture.db_url)
        .await
        .expect("open duplicate DB");
    sqlx::query("UPDATE mods SET junction_dir_name = ? WHERE id = ?")
        .bind(&alias_name)
        .bind(&outside.id)
        .execute(&pool)
        .await
        .expect("claim the existing Junction through a case-insensitive alias");
    let reviewed = reviewed_duplicate_mods(&fixture).await;

    let result = fixture
        .core
        .resolve_duplicate_mods(&fixture.keeper_id, &reviewed)
        .await;
    assert!(
        matches!(
            result,
            Err(Error::DuplicateModJunctionClaimedBySurvivor {
                ref mod_id,
                ref surviving_mod_id,
                ..
            }) if mod_id == &fixture.duplicate_id && surviving_mod_id == &outside.id
        ),
        "a survivor's case-insensitive spelling must identify the same Junction entry, got {result:?}",
    );
    assert!(
        fixture.duplicate_junction.exists(),
        "the survivor's physical Junction remains",
    );
    let duplicate_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mods WHERE id IN (?, ?)")
        .bind(&fixture.keeper_id)
        .bind(&fixture.duplicate_id)
        .fetch_one(&pool)
        .await
        .expect("count reviewed duplicate rows after refusal");
    assert_eq!(duplicate_rows, 2, "both reviewed records survive refusal");
}

#[tokio::test]
async fn duplicate_resolution_distinguishes_two_junction_names_with_one_target() {
    let tmp = TempDir::new().expect("tmp");
    let fixture = duplicate_fixture(&tmp).await;
    let game_mods = fixture.duplicate_junction.parent().expect("game Mods");
    fixture
        .core
        .set_enabled(&fixture.keeper_id, true, game_mods)
        .await
        .expect("enable keeper under its distinct Junction name");
    let keeper_junction = game_mods.join("Manual Keeper");
    let reviewed = reviewed_duplicate_mods(&fixture).await;

    fixture
        .core
        .resolve_duplicate_mods(&fixture.keeper_id, &reviewed)
        .await
        .expect("distinct physical Junction path is not a survivor conflict");
    assert!(
        keeper_junction.exists(),
        "the keeper's distinct Junction remains"
    );
    assert!(
        fs::symlink_metadata(&fixture.duplicate_junction).is_err(),
        "only the rejected row's distinct Junction is withdrawn",
    );
}

#[tokio::test]
async fn duplicate_resolution_refuses_enabled_record_without_an_install_path() {
    let tmp = TempDir::new().expect("tmp");
    let fixture = duplicate_fixture(&tmp).await;
    let pool = sqlx::SqlitePool::connect(&fixture.db_url).await.unwrap();
    sqlx::query("UPDATE games SET install_path = NULL WHERE code = 'gimi'")
        .execute(&pool)
        .await
        .unwrap();
    let reviewed = reviewed_duplicate_mods(&fixture).await;
    let result = fixture
        .core
        .resolve_duplicate_mods(&fixture.keeper_id, &reviewed)
        .await;
    assert!(matches!(
        result,
        Err(Error::DuplicateModInstallPathMissing { .. })
    ));
    assert!(
        fs::symlink_metadata(&fixture.duplicate_junction).is_ok(),
        "the Junction entry remains after install-path refusal",
    );
}

#[tokio::test]
async fn duplicate_resolution_refuses_a_regular_directory_at_the_junction_path() {
    let tmp = TempDir::new().expect("tmp");
    let fixture = duplicate_fixture(&tmp).await;
    let reviewed = reviewed_duplicate_mods(&fixture).await;
    junction::remove(&fixture.duplicate_junction).expect("remove fixture Junction");
    fs::create_dir(&fixture.duplicate_junction).expect("replace it with user directory");
    fs::write(fixture.duplicate_junction.join("user.ini"), b"keep").unwrap();

    let result = fixture
        .core
        .resolve_duplicate_mods(&fixture.keeper_id, &reviewed)
        .await;
    assert!(matches!(
        result,
        Err(Error::DuplicateModJunctionConflict { .. })
    ));
    assert_eq!(
        fs::read(fixture.duplicate_junction.join("user.ini")).unwrap(),
        b"keep"
    );
}

#[tokio::test]
async fn duplicate_resolution_refuses_a_junction_repointed_outside_the_library() {
    let tmp = TempDir::new().expect("tmp");
    let fixture = duplicate_fixture(&tmp).await;
    let reviewed = reviewed_duplicate_mods(&fixture).await;
    let outside = tmp.path().join("outside-library");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("outside.ini"), b"keep").unwrap();
    junction::remove(&fixture.duplicate_junction).expect("remove fixture Junction");
    junction::create(&fixture.duplicate_junction, &outside).expect("repoint outside Library");

    let result = fixture
        .core
        .resolve_duplicate_mods(&fixture.keeper_id, &reviewed)
        .await;
    assert!(matches!(
        result,
        Err(Error::DuplicateModJunctionConflict { .. })
    ));
    assert_eq!(fs::read(outside.join("outside.ini")).unwrap(), b"keep");
}

#[tokio::test]
async fn duplicate_resolution_refuses_a_missing_selected_variant_target() {
    let tmp = TempDir::new().expect("tmp");
    let fixture = duplicate_fixture(&tmp).await;
    let reviewed = reviewed_duplicate_mods(&fixture).await;
    fs::remove_dir_all(fixture.library_path.join("Amber")).expect("remove selected Variant target");

    let result = fixture
        .core
        .resolve_duplicate_mods(&fixture.keeper_id, &reviewed)
        .await;
    assert!(
        matches!(result, Err(Error::DuplicateModResolutionChanged { ref reason }) if reason.contains("selected Library target")),
        "missing selected Variant target must refuse before Junction removal, got {result:?}",
    );
    assert!(
        fs::symlink_metadata(&fixture.duplicate_junction).is_ok(),
        "the broken Junction entry remains after target refusal",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn duplicate_resolution_propagates_selected_target_metadata_uncertainty() {
    use std::os::unix::fs::symlink;

    let tmp = TempDir::new().expect("tmp");
    let fixture = duplicate_fixture(&tmp).await;
    let reviewed = reviewed_duplicate_mods(&fixture).await;
    let target = fixture.library_path.join("Amber");
    fs::remove_dir_all(&target).expect("remove readable selected Variant target");
    symlink(&target, &target).expect("self-referential selected Variant target");

    let result = fixture
        .core
        .resolve_duplicate_mods(&fixture.keeper_id, &reviewed)
        .await;

    assert!(
        matches!(result, Err(Error::Io { ref path, .. }) if path == &target),
        "an unreadable selected Variant target must remain an I/O error, got {result:?}",
    );
    assert!(
        fs::symlink_metadata(&fixture.duplicate_junction).is_ok(),
        "target uncertainty must stop before Junction withdrawal",
    );
}

#[tokio::test]
async fn duplicate_resolution_preflights_every_junction_before_withdrawing_any() {
    let tmp = TempDir::new().expect("tmp");
    let fixture = duplicate_fixture(&tmp).await;
    let later_id = Ulid::new().to_string();
    let pool = sqlx::SqlitePool::connect(&fixture.db_url).await.unwrap();
    sqlx::query(
        "INSERT INTO mods (
            id, game_code, name, source, library_path, junction_dir_name, enabled, created_at
         ) VALUES (?, 'gimi', 'Later Conflict', 'manual', ?, 'Later Conflict', 0, ?)",
    )
    .bind(&later_id)
    .bind(fixture.library_path.to_string_lossy().as_ref())
    .bind("2026-08-24T00:00:04Z")
    .execute(&pool)
    .await
    .unwrap();
    let game_mods = fixture.duplicate_junction.parent().expect("game Mods");
    fixture
        .core
        .set_enabled(&later_id, true, game_mods)
        .await
        .unwrap();
    let later_junction = game_mods.join("Later Conflict");
    let reviewed = reviewed_duplicate_mods(&fixture).await;
    junction::remove(&later_junction).unwrap();
    fs::create_dir(&later_junction).unwrap();

    let result = fixture
        .core
        .resolve_duplicate_mods(&fixture.keeper_id, &reviewed)
        .await;
    assert!(
        matches!(result, Err(Error::DuplicateModJunctionConflict { ref mod_id, .. }) if mod_id == &later_id)
    );
    assert!(
        fixture.duplicate_junction.exists(),
        "an earlier valid Junction must remain when a later Junction fails preflight",
    );
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mods WHERE id IN (?, ?, ?)")
        .bind(&fixture.keeper_id)
        .bind(&fixture.duplicate_id)
        .bind(&later_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 3, "multi-Junction refusal preserves every record");
}

#[tokio::test]
async fn duplicate_resolution_refuses_when_a_withdrawn_junction_is_still_present() {
    let tmp = TempDir::new().expect("tmp");
    let fixture = duplicate_fixture(&tmp).await;
    let reviewed = reviewed_duplicate_mods(&fixture).await;
    let link = fixture.duplicate_junction.clone();
    let target = fixture.library_path.join("Amber");
    let hooked = fixture
        .core
        .clone()
        .with_crash_hook(std::sync::Arc::new(move |point| {
            if point == gmm_lib::core::crash_points::RESOLVE_DUPLICATES_AFTER_JUNCTION_WITHDRAWAL {
                junction::create(&link, &target)
                    .expect("recreate the Junction at the post-withdrawal test seam");
            }
        }));

    let result = hooked
        .resolve_duplicate_mods(&fixture.keeper_id, &reviewed)
        .await;
    assert!(
        matches!(
            result,
            Err(Error::DuplicateModJunctionStillPresent { ref mod_id, .. })
                if mod_id == &fixture.duplicate_id
        ),
        "a deployment entry still present after withdrawal must refuse row deletion, got {result:?}",
    );
    assert!(
        fixture.duplicate_junction.exists(),
        "the still-present Junction remains visible for recovery",
    );
    let pool = sqlx::SqlitePool::connect(&fixture.db_url)
        .await
        .expect("open duplicate DB after post-withdrawal refusal");
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mods WHERE id IN (?, ?)")
        .bind(&fixture.keeper_id)
        .bind(&fixture.duplicate_id)
        .fetch_one(&pool)
        .await
        .expect("count records after post-withdrawal refusal");
    assert_eq!(rows, 2, "both reviewed records survive refusal");
}

#[cfg(unix)]
#[tokio::test]
async fn duplicate_resolution_refuses_an_unreadable_present_junction_during_preflight() {
    let tmp = TempDir::new().expect("tmp");
    let fixture = duplicate_fixture(&tmp).await;
    let reviewed = reviewed_duplicate_mods(&fixture).await;
    let game_mods = fixture
        .duplicate_junction
        .parent()
        .expect("game Mods directory");
    let original_permissions = deny_directory_search(game_mods);

    let result = fixture
        .core
        .resolve_duplicate_mods(&fixture.keeper_id, &reviewed)
        .await;
    restore_directory_search(game_mods, original_permissions);

    assert!(
        matches!(
            result,
            Err(Error::Io { ref path, ref source })
                if path == &fixture.duplicate_junction
                    && source.kind() == std::io::ErrorKind::PermissionDenied
        ),
        "an unreadable deployment entry must refuse before withdrawal, got {result:?}",
    );
    assert!(
        fixture.duplicate_junction.exists(),
        "the unreadable but present Junction survives preflight refusal",
    );
    let pool = sqlx::SqlitePool::connect(&fixture.db_url)
        .await
        .expect("open duplicate DB after preflight refusal");
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mods WHERE id IN (?, ?)")
        .bind(&fixture.keeper_id)
        .bind(&fixture.duplicate_id)
        .fetch_one(&pool)
        .await
        .expect("count records after unreadable preflight refusal");
    assert_eq!(rows, 2, "both reviewed records survive refusal");
}

#[cfg(unix)]
#[tokio::test]
async fn duplicate_resolution_refuses_an_unreadable_recreated_junction_after_withdrawal() {
    let tmp = TempDir::new().expect("tmp");
    let fixture = duplicate_fixture(&tmp).await;
    let reviewed = reviewed_duplicate_mods(&fixture).await;
    let link = fixture.duplicate_junction.clone();
    let target = fixture.library_path.join("Amber");
    let game_mods = link.parent().expect("game Mods directory").to_path_buf();
    let original_permissions = fs::metadata(&game_mods)
        .expect("read game Mods permissions")
        .permissions();
    let hook_game_mods = game_mods.clone();
    let hooked = fixture
        .core
        .clone()
        .with_crash_hook(std::sync::Arc::new(move |point| {
            if point == gmm_lib::core::crash_points::RESOLVE_DUPLICATES_AFTER_JUNCTION_WITHDRAWAL {
                junction::create(&link, &target)
                    .expect("recreate the Junction at the post-withdrawal test seam");
                let _ = deny_directory_search(&hook_game_mods);
            }
        }));

    let result = hooked
        .resolve_duplicate_mods(&fixture.keeper_id, &reviewed)
        .await;
    restore_directory_search(&game_mods, original_permissions);

    assert!(
        matches!(
            result,
            Err(Error::Io { ref path, ref source })
                if path == &fixture.duplicate_junction
                    && source.kind() == std::io::ErrorKind::PermissionDenied
        ),
        "an unreadable post-withdrawal deployment entry must refuse row deletion, got {result:?}",
    );
    assert!(
        fixture.duplicate_junction.exists(),
        "the unreadable but still-present Junction remains visible for recovery",
    );
    let pool = sqlx::SqlitePool::connect(&fixture.db_url)
        .await
        .expect("open duplicate DB after unreadable post-withdrawal refusal");
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mods WHERE id IN (?, ?)")
        .bind(&fixture.keeper_id)
        .bind(&fixture.duplicate_id)
        .fetch_one(&pool)
        .await
        .expect("count records after unreadable post-withdrawal refusal");
    assert_eq!(rows, 2, "both reviewed records survive refusal");
}
