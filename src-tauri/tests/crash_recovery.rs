//! Issue #59: kill the process between the filesystem step and the DB
//! step of every durable mutation, restart, and assert recovery.
//!
//! # What this is for
//!
//! `tests/session.rs` covers crash recovery for a Game Session, which is
//! easy because a Game Session has an external witness: a PID that is or
//! is not alive. Ordinary mutations have no witness. `set_enabled`
//! creates a Junction and *then* writes the row; nothing was stopping
//! the process in between, so the invariant that matters —
//!
//! > never a DB row saying enabled with a missing or wrong Junction
//!
//! — was maintained by inspection alone.
//!
//! Each test here runs one mutation in a child process
//! (`crates/probe`), kills it at a named point via
//! [`Core::with_crash_hook`], then reopens the database the way a
//! restart would and asserts that a reconcile pass restores the
//! invariant. `std::process::abort` rather than a clean exit: no
//! unwinding, no destructors, no flushed buffers — a process that
//! stopped existing, not one that shut down badly.
//!
//! # Why several of these assert repair rather than prevention
//!
//! Reconcile exists because the Library is the source of truth (ADR
//! 0003) and the Junctions in `<Game>/Mods/` are projections that can
//! drift. Making each mutation individually crash-atomic would mean a
//! filesystem transaction we do not have on NTFS. So for the torn
//! states reconcile can name, "torn on disk, then reconcile fixes it" is
//! the real guarantee, and it is what these tests assert.
//!
//! # Crash outcomes and their recovery surface
//!
//! Two torn states are *not* enabled-state violations and are not
//! repaired automatically. Both are asserted below as the behaviour
//! they actually have, so that a future change to either is visible:
//!
//! * **Orphaned Library directory.** A crash after `adopt_folder` or
//!   `import_zip` copies files into the Library but before the row is
//!   inserted leaves bytes on disk that nothing references. The
//!   read-only Library audit sees that folder and Settings reports it;
//!   neither path deletes or repairs it because it may hold the user's
//!   only copy of the interrupted import. Inspect/delete/recovery actions
//!   are deliberately deferred to issue #72.
//!
//! * **Orphan renamed but not yet adopted.** Recovering a Library-root
//!   directory whose name is not a usable ULID renames it before writing
//!   the row (#72), for the same reason imports copy before inserting: a
//!   row pointing at a directory that is not there is worse than a
//!   directory no row points at. A crash in that window therefore leaves
//!   the same orphan shape under the new name, which the audit reports
//!   and the same feature can recover a second time.
//!
//! Variant detection for adopt and import is intentionally completed before
//! the Library writer fence because it is an unbounded recursive traversal.
//! The later Mod row, Variant rows, and active selection share one bounded
//! transaction. A crash after row insertion therefore rolls back the row and
//! leaves the intact Library directory visible to the orphan audit instead of
//! exposing a referenced Mod with missing Variant state.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use gmm_lib::core::{crash_points, Core, GameCode};
use sqlx::SqlitePool;
use tempfile::TempDir;

// ---------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------

fn probe_bin() -> PathBuf {
    let name = if cfg!(windows) {
        "concurrency-probe.exe"
    } else {
        "concurrency-probe"
    };
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/debug")
        .join(name);
    assert!(
        p.exists(),
        "{name} missing at {p:?} — run `cargo build --workspace` before this test",
    );
    p
}

struct TestEnv {
    tmp: TempDir,
    data_dir: PathBuf,
    db_url: String,
    library: PathBuf,
    game_mods: PathBuf,
}

impl TestEnv {
    fn new() -> Self {
        let tmp = TempDir::new().expect("tmp");
        let data_dir = tmp.path().join("data");
        let library = data_dir.join("library");
        let game_mods = tmp.path().join("Genshin/Mods");
        std::fs::create_dir_all(&game_mods).expect("game mods dir");
        std::fs::create_dir_all(&data_dir).expect("data dir");
        let db_url = format!("sqlite://{}/gmm.db?mode=rwc", data_dir.display());
        Self {
            tmp,
            data_dir,
            db_url,
            library,
            game_mods,
        }
    }

    /// Reopen the database the way a restart does — a brand new `Core`
    /// against the same file, with no memory of what the dead process
    /// was doing.
    async fn restart(&self) -> Core {
        Core::new(self.library.clone(), &self.db_url)
            .await
            .expect("reopen the DB after a crash")
    }

    async fn staging_witness_count(&self) -> i64 {
        let pool = SqlitePool::connect(&self.db_url)
            .await
            .expect("open DB without running startup recovery");
        let count = sqlx::query_scalar("SELECT COUNT(*) FROM staged_library_operations")
            .fetch_one(&pool)
            .await
            .expect("count staged Library witnesses");
        pool.close().await;
        count
    }

    /// Run one mutation in a child process that aborts at `crash_at`.
    /// Asserts the child really died rather than completing, so a
    /// mis-typed point name fails loudly instead of silently testing a
    /// clean run.
    fn crash_during(&self, crash_at: &str, op: &[&str]) {
        assert!(
            crash_points::ALL.contains(&crash_at),
            "{crash_at:?} is not a known crash point; \
             declare it through define_crash_points! so it cannot silently never fire",
        );

        let out = Command::new(probe_bin())
            .arg("--data-dir")
            .arg(&self.data_dir)
            .arg("--db")
            .arg(&self.db_url)
            .arg("--library")
            .arg(&self.library)
            .arg("--crash-at")
            .arg(crash_at)
            .args(op)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("spawn probe");

        assert!(
            !out.status.success(),
            "the probe was supposed to die at {crash_at} but exited cleanly — \
             the crash point never fired, so this test proves nothing. \
             stdout: {}",
            String::from_utf8_lossy(&out.stdout),
        );
        assert!(
            out.stdout.is_empty(),
            "the probe printed an outcome, so it finished the operation \
             instead of dying at {crash_at}: {}",
            String::from_utf8_lossy(&out.stdout),
        );
    }

    fn link(&self, name: &str) -> PathBuf {
        self.game_mods.join(name)
    }
}

/// The invariant, asserted the way `tests/concurrency.rs` asserts it:
/// a reconcile pass that repairs nothing is a pass that found nothing
/// torn.
async fn assert_invariant(core: &Core, game_mods: &Path, context: &str) {
    let r = core
        .reconcile_junctions(GameCode::Gimi, game_mods)
        .await
        .expect("reconcile for invariant check");
    assert!(
        r.recreated.is_empty(),
        "{context}: enabled Mod(s) still without a Junction: {:?}",
        r.recreated,
    );
    assert!(
        r.removed.is_empty(),
        "{context}: disabled Mod(s) still with a stranded Junction: {:?}",
        r.removed,
    );
    assert!(
        r.conflicting.is_empty(),
        "{context}: Junction(s) still resolving somewhere unexpected: {:?}",
        r.conflicting,
    );
}

/// Restart, run the documented recovery pass, then assert the invariant
/// holds — i.e. a second pass finds nothing left to do.
async fn recover_and_assert(env: &TestEnv, context: &str) -> Core {
    let core = env.restart().await;
    core.reconcile_junctions(GameCode::Gimi, &env.game_mods)
        .await
        .expect("recovery reconcile");
    assert_invariant(&core, &env.game_mods, context).await;
    core
}

/// Every enabled Mod has a Junction and every disabled Mod has none —
/// checked against the directory rather than through reconcile, so the
/// two checks cannot agree with each other while both being wrong.
async fn assert_rows_match_disk(core: &Core, env: &TestEnv, context: &str) {
    for m in core.list_mods(GameCode::Gimi).await.expect("list") {
        let present = std::fs::symlink_metadata(env.link(&m.name)).is_ok();
        assert_eq!(
            m.enabled, present,
            "{context}: Mod {:?} has enabled={} but Junction present={present}",
            m.name, m.enabled,
        );
    }
}

async fn seed_mod(env: &TestEnv, core: &Core, name: &str) -> gmm_lib::core::Mod {
    let src = env.tmp.path().join("fixtures").join(name);
    std::fs::create_dir_all(&src).expect("fixture dir");
    std::fs::write(src.join("merged.ini"), b"[TextureOverride]\nhash=42\n").expect("ini");
    core.adopt_folder(GameCode::Gimi, &src, name)
        .await
        .expect("adopt")
}

#[cfg(unix)]
fn durable_directory_key(path: &Path) -> String {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = std::fs::metadata(path).expect("directory metadata for recovery witness");
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
        .expect("open directory for recovery witness identity");
    let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    let ok = unsafe { GetFileInformationByHandle(directory.as_raw_handle(), info.as_mut_ptr()) };
    assert_ne!(
        ok,
        0,
        "read directory identity for recovery witness: {}",
        std::io::Error::last_os_error(),
    );
    let info = unsafe { info.assume_init() };
    let file = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    format!("{:016x}:{:016x}", info.dwVolumeSerialNumber, file)
}

/// At the post-commit seam, the referenced Mod must already have its complete
/// Variant shape and initial active selection. This is intentionally stronger
/// than merely observing the Mod row: it catches a refactor that commits the
/// row first and records Variants in a later transaction.
async fn assert_committed_import_shape(env: &TestEnv, context: &str) {
    let core = recover_and_assert(env, context).await;
    let mods = core.list_mods(GameCode::Gimi).await.expect("list Mods");
    assert_eq!(
        mods.len(),
        1,
        "{context}: the committed import must expose exactly one Mod row: {mods:?}",
    );
    let variants = core
        .list_variants(&mods[0].id)
        .await
        .expect("list committed Variants");
    let names = variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        ["Blue", "Red"],
        "{context}: the commit must already include every detected Variant",
    );
    let active_id = core
        .active_variant_id(&mods[0].id)
        .await
        .expect("read committed active Variant");
    let active_name = active_id.as_deref().and_then(|active_id| {
        variants
            .iter()
            .find(|variant| variant.id == active_id)
            .map(|variant| variant.name.as_str())
    });
    assert_eq!(
        active_name,
        Some("Blue"),
        "{context}: the commit must already include the initial active selection",
    );
}

// ---------------------------------------------------------------------
// set_enabled
// ---------------------------------------------------------------------

/// Crash after the Junction is created, before the row is written.
///
/// On restart the row says disabled and the Junction is on disk. Left
/// alone, the Model Importer loads a Mod the UI says is off.
#[tokio::test]
async fn enable_crashing_after_the_junction_is_created_recovers() {
    let env = TestEnv::new();
    let core = env.restart().await;
    let m = seed_mod(&env, &core, "Enable Crash").await;
    drop(core);

    env.crash_during(
        crash_points::SET_ENABLED_AFTER_JUNCTION_CREATE,
        &[
            "set-enabled",
            "--mod-id",
            &m.id,
            "--enabled",
            "1",
            "--mods-dir",
            &env.game_mods.display().to_string(),
        ],
    );

    // The torn state we expect to find, asserted so the test fails
    // loudly if the crash point ever moves and stops tearing anything.
    assert!(
        env.link("Enable Crash").exists(),
        "precondition: the crash left the Junction on disk",
    );
    let core = env.restart().await;
    assert!(
        !core.list_mods(GameCode::Gimi).await.expect("list")[0].enabled,
        "precondition: the row never got its update",
    );
    drop(core);

    let core = recover_and_assert(&env, "enable crashed after junction create").await;
    assert_rows_match_disk(&core, &env, "enable crashed after junction create").await;
}

/// Crash after the Junction is removed, before the row is written.
///
/// On restart the row says enabled and there is no Junction — the
/// canonical form of the invariant this issue names.
#[tokio::test]
async fn disable_crashing_after_the_junction_is_removed_recovers() {
    let env = TestEnv::new();
    let core = env.restart().await;
    let m = seed_mod(&env, &core, "Disable Crash").await;
    core.set_enabled(&m.id, true, &env.game_mods)
        .await
        .expect("enable");
    drop(core);

    env.crash_during(
        crash_points::SET_ENABLED_AFTER_JUNCTION_REMOVE,
        &[
            "set-enabled",
            "--mod-id",
            &m.id,
            "--enabled",
            "0",
            "--mods-dir",
            &env.game_mods.display().to_string(),
        ],
    );

    assert!(
        std::fs::symlink_metadata(env.link("Disable Crash")).is_err(),
        "precondition: the crash left no Junction",
    );
    let core = env.restart().await;
    assert!(
        core.list_mods(GameCode::Gimi).await.expect("list")[0].enabled,
        "precondition: the row still says enabled",
    );
    drop(core);

    let core = recover_and_assert(&env, "disable crashed after junction remove").await;
    assert_rows_match_disk(&core, &env, "disable crashed after junction remove").await;
}

// ---------------------------------------------------------------------
// adopt_folder / import_zip
// ---------------------------------------------------------------------

/// Crash after the Library copy, before the row insert.
///
/// The enabled-state invariant is untouched — there is no row, so there
/// is nothing to be inconsistent with. What is left is an orphaned
/// Library directory that nothing references. Asserted as the known
/// read-only audit now reports it; see the module docs.
#[tokio::test]
async fn adopt_crashing_after_the_library_copy_reports_the_intact_orphan() {
    let env = TestEnv::new();
    let src = env.tmp.path().join("to-adopt");
    std::fs::create_dir_all(&src).expect("src");
    std::fs::write(src.join("merged.ini"), b"hash=7\n").expect("ini");

    env.crash_during(
        crash_points::ADOPT_AFTER_LIBRARY_COPY,
        &[
            "adopt",
            "--from",
            &src.display().to_string(),
            "--name",
            "Orphaned Mod",
        ],
    );

    assert_eq!(
        env.staging_witness_count().await,
        1,
        "a process death after copy must leave the durable staging owner",
    );

    let core = recover_and_assert(&env, "adopt crashed after the library copy").await;
    assert_eq!(
        env.staging_witness_count().await,
        0,
        "startup must release the crashed producer's staging witness",
    );
    assert!(
        core.list_mods(GameCode::Gimi)
            .await
            .expect("list")
            .is_empty(),
        "no Mod row should exist — the insert never ran",
    );

    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit after adopt crash");
    assert_eq!(
        report.unreferenced.len(),
        1,
        "the audit must report exactly the crashed adopt: {report:?}",
    );
    let orphan = &report.unreferenced[0];
    assert!(
        orphan.path.join("merged.ini").exists(),
        "auditing must leave the user's only copy of the import intact",
    );
    assert!(
        orphan.size_bytes.is_some(),
        "the reported orphan includes its size"
    );
}

/// Crash after the row insert but before the already-detected Variants are
/// recorded. The row and Variant shape are one transaction, so restart sees
/// no referenced Mod and the intact copied directory remains recoverable.
#[tokio::test]
async fn adopt_crashing_before_variant_recording_rolls_back_the_mod_row() {
    let env = TestEnv::new();
    let src = env.tmp.path().join("multi-variant");
    for variant in ["Red", "Blue"] {
        let d = src.join(variant);
        std::fs::create_dir_all(&d).expect("variant dir");
        std::fs::write(d.join("merged.ini"), b"hash=9\n").expect("ini");
    }

    env.crash_during(
        crash_points::ADOPT_AFTER_ROW_INSERT,
        &[
            "adopt",
            "--from",
            &src.display().to_string(),
            "--name",
            "Half Adopted",
        ],
    );

    let core = recover_and_assert(&env, "adopt crashed before Variant recording").await;
    let listed = core.list_mods(GameCode::Gimi).await.expect("list");
    assert!(
        listed.is_empty(),
        "the in-transaction adopt row must roll back when the process dies: {listed:?}",
    );
    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit after adopt transaction crash");
    assert_eq!(
        report.unreferenced.len(),
        1,
        "the copied Mod stays actionable"
    );
    assert!(report.unreferenced[0].path.join("Red/merged.ini").exists());
}

/// Crash immediately after the adopt fence commits. The committed row must
/// already include every detected Variant and the initial active selection.
#[tokio::test]
async fn adopt_crashing_after_commit_preserves_complete_variant_state() {
    let env = TestEnv::new();
    let src = env.tmp.path().join("committed-adopt-variants");
    for variant in ["Red", "Blue"] {
        let directory = src.join(variant);
        std::fs::create_dir_all(&directory).expect("variant directory");
        std::fs::write(directory.join("merged.ini"), b"hash=9\n").expect("variant ini");
    }

    env.crash_during(
        crash_points::ADOPT_AFTER_FENCE_COMMIT,
        &[
            "adopt",
            "--from",
            &src.display().to_string(),
            "--name",
            "Committed Adopt",
        ],
    );

    assert_committed_import_shape(&env, "adopt crashed immediately after commit").await;
}

/// Build a Mod archive with two Variant subfolders.
fn build_mod_zip(zip_path: &Path) {
    use std::io::Write as _;
    use zip::write::SimpleFileOptions;

    let file = std::fs::File::create(zip_path).expect("create zip");
    let mut zw = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for variant in ["Red", "Blue"] {
        zw.add_directory(format!("{variant}/"), opts).expect("dir");
        zw.start_file(format!("{variant}/merged.ini"), opts)
            .expect("ini");
        zw.write_all(format!("; {variant}\n").as_bytes())
            .expect("write ini");
    }
    zw.finish().expect("finish zip");
}

async fn crash_gamebanana_import(env: &TestEnv, crash_at: &str, id: u64) {
    let zip = env.tmp.path().join("gamebanana-mod.zip");
    build_mod_zip(&zip);
    let archive_bytes = std::fs::read(&zip).expect("GameBanana ZIP bytes");
    let api_path = format!("/apiv11/Mod/{id}");
    let file_path = format!("/file/{id}/mod.zip");
    let mut server = mockito::Server::new_async().await;
    let _api = server
        .mock("GET", mockito::Matcher::Regex(format!("{api_path}.*")))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
                "_idRow": {id}, "_sName": "Atomic GameBanana Mod",
                "_sProfileUrl": "https://gamebanana.com/mods/{id}", "_sVersion": "1.0.0",
                "_aSubmitter": {{ "_sName": "Author" }},
                "_aPreviewMedia": {{ "_aImages": [] }},
                "_aFiles": [{{ "_sFile": "mod.zip", "_sDownloadUrl": "{base}{file_path}" }}]
            }}"#,
            base = server.url(),
        ))
        .create_async()
        .await;
    let _file = server
        .mock("GET", file_path.as_str())
        .with_status(200)
        .with_header("content-type", "application/zip")
        .with_body(archive_bytes)
        .create_async()
        .await;

    env.crash_during(
        crash_at,
        &[
            "import-gamebanana",
            "--id",
            &id.to_string(),
            "--api-base",
            &server.url(),
        ],
    );
}

async fn import_gamebanana_fixture(env: &TestEnv, core: &Core, id: u64) -> gmm_lib::core::Mod {
    let archive = env.tmp.path().join("coverage-gamebanana-v1.zip");
    build_mod_zip(&archive);
    let archive_bytes = std::fs::read(&archive).expect("coverage GameBanana ZIP bytes");
    let mut server = mockito::Server::new_async().await;
    let api_path = format!("/apiv11/Mod/{id}");
    let file_path = format!("/file/{id}/mod.zip");
    let _api = server
        .mock("GET", mockito::Matcher::Regex(format!("{api_path}.*")))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
                "_idRow": {id}, "_sName": "Coverage GameBanana Mod",
                "_sProfileUrl": "https://gamebanana.com/mods/{id}", "_sVersion": "1.0.0",
                "_aSubmitter": {{ "_sName": "Author" }},
                "_aPreviewMedia": {{ "_aImages": [] }},
                "_aFiles": [{{ "_sFile": "mod.zip", "_sDownloadUrl": "{base}{file_path}" }}]
            }}"#,
            base = server.url(),
        ))
        .create_async()
        .await;
    let _file = server
        .mock("GET", file_path.as_str())
        .with_status(200)
        .with_body(archive_bytes)
        .create_async()
        .await;
    let endpoints = gmm_lib::core::gamebanana::Endpoints {
        api_base: server.url(),
    };

    core.import_gamebanana_with_endpoints(GameCode::Gimi, &id.to_string(), &endpoints)
        .await
        .expect("coverage GameBanana import")
}

async fn reinstall_gamebanana_fixture(env: &TestEnv, core: &Core, id: u64, mod_id: &str) {
    let archive = env.tmp.path().join("coverage-gamebanana-v2.zip");
    build_mod_zip(&archive);
    let archive_bytes = std::fs::read(&archive).expect("coverage reinstall ZIP bytes");
    let mut server = mockito::Server::new_async().await;
    let api_path = format!("/apiv11/Mod/{id}");
    let file_path = format!("/file/{id}/mod.zip");
    let _api = server
        .mock("GET", mockito::Matcher::Regex(format!("{api_path}.*")))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
                "_idRow": {id}, "_sName": "Coverage GameBanana Mod v2",
                "_sProfileUrl": "https://gamebanana.com/mods/{id}", "_sVersion": "2.0.0",
                "_aSubmitter": {{ "_sName": "Author" }},
                "_aPreviewMedia": {{ "_aImages": [] }},
                "_aFiles": [{{ "_sFile": "mod.zip", "_sDownloadUrl": "{base}{file_path}" }}]
            }}"#,
            base = server.url(),
        ))
        .create_async()
        .await;
    let _file = server
        .mock("GET", file_path.as_str())
        .with_status(200)
        .with_body(archive_bytes)
        .create_async()
        .await;
    let endpoints = gmm_lib::core::gamebanana::Endpoints {
        api_base: server.url(),
    };

    core.reinstall_gamebanana_mod_with_endpoints(mod_id, &endpoints)
        .await
        .expect("coverage GameBanana reinstall");
}

/// A staging-release failure must not turn the durable witness into a
/// permanent concealment record. Preserve the witness as failure evidence,
/// but surface the bytes through the ordinary orphan workflow so the user can
/// inspect, recover, or delete them without hand-editing SQLite.
#[tokio::test]
async fn failed_startup_staging_resolution_keeps_directory_visible_and_actionable() {
    let env = TestEnv::new();
    let src = env.tmp.path().join("failed-startup-staging-resolution");
    std::fs::create_dir_all(&src).expect("src");
    std::fs::write(src.join("merged.ini"), b"hash=failed-startup-release\n").expect("ini");

    env.crash_during(
        crash_points::ADOPT_AFTER_LIBRARY_COPY,
        &[
            "adopt",
            "--from",
            &src.display().to_string(),
            "--name",
            "Interrupted Unknown Mod",
        ],
    );
    assert_eq!(
        env.staging_witness_count().await,
        1,
        "the crashed producer must leave one durable staging witness",
    );

    let pool = SqlitePool::connect(&env.db_url)
        .await
        .expect("open DB to force startup release failure");
    sqlx::query(
        "CREATE TRIGGER reject_staging_witness_release
         BEFORE DELETE ON staged_library_operations
         BEGIN
             SELECT RAISE(ABORT, 'forced staging witness release failure');
         END",
    )
    .execute(&pool)
    .await
    .expect("install forced staging witness release failure");
    pool.close().await;

    let core = env.restart().await;
    let pool = SqlitePool::connect(&env.db_url)
        .await
        .expect("open DB to inspect durable startup failure");
    let failure: (Option<String>, i64) =
        sqlx::query_as("SELECT recovery_error, recovery_attempts FROM staged_library_operations")
            .fetch_one(&pool)
            .await
            .expect("read failed staging witness");
    pool.close().await;
    assert_eq!(
        failure.1, 1,
        "startup must durably count the failed release"
    );
    assert!(
        failure
            .0
            .as_deref()
            .is_some_and(|reason| reason.contains("forced staging witness release failure")),
        "the durable witness must record the real release obstruction: {failure:?}",
    );
    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit after failed staging witness release");
    assert_eq!(
        report.unreferenced.len(),
        1,
        "a failed startup release must leave the staging directory visible: {report:?}",
    );
    let orphan = &report.unreferenced[0];
    assert!(
        orphan.path.join("merged.ini").is_file(),
        "surfacing the failed staging directory must preserve its bytes",
    );

    let recovered = core
        .recover_unreferenced_library_dir(
            GameCode::Gimi,
            &orphan.path,
            "User Identified Recovered Mod",
        )
        .await
        .expect("the surfaced staging directory must remain actionable");
    assert_eq!(recovered.library_path, orphan.path);
    assert!(
        recovered.library_path.join("merged.ini").is_file(),
        "recovery must adopt the preserved bytes in place",
    );
}

/// Crash after the archive is extracted into the Library, before the row
/// insert. Same orphan shape as `adopt`, reached by the other import
/// path — worth its own case because `import_zip` has a cleanup branch
/// that `adopt_folder` does not, and that branch runs only on a returned
/// error, never on a crash.
#[tokio::test]
async fn import_zip_crashing_after_extract_reports_the_intact_orphan() {
    let env = TestEnv::new();
    let zip = env.tmp.path().join("mod.zip");
    build_mod_zip(&zip);

    env.crash_during(
        crash_points::IMPORT_ZIP_AFTER_EXTRACT,
        &[
            "import-zip",
            "--zip",
            &zip.display().to_string(),
            "--name",
            "Zip Orphan",
        ],
    );

    assert_eq!(
        env.staging_witness_count().await,
        1,
        "a process death after extraction must leave the durable staging owner",
    );

    let core = recover_and_assert(&env, "import_zip crashed after extract").await;
    assert_eq!(
        env.staging_witness_count().await,
        0,
        "startup must release the crashed ZIP producer's staging witness",
    );
    assert!(
        core.list_mods(GameCode::Gimi)
            .await
            .expect("list")
            .is_empty(),
        "no Mod row should exist — the insert never ran",
    );

    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit after zip crash");
    assert_eq!(
        report.unreferenced.len(),
        1,
        "the audit must report exactly the crashed ZIP import: {report:?}",
    );
    assert!(
        report.unreferenced[0].path.join("Red/merged.ini").exists(),
        "auditing must leave extracted Variant files intact",
    );
}

/// Crash after `import_zip` inserts its row but before it records the
/// already-detected Variants. The single transaction rolls the row back and
/// leaves the extracted directory visible to recovery.
#[tokio::test]
async fn import_zip_crashing_before_variant_recording_rolls_back_the_mod_row() {
    let env = TestEnv::new();
    let zip = env.tmp.path().join("mod2.zip");
    build_mod_zip(&zip);

    env.crash_during(
        crash_points::IMPORT_ZIP_AFTER_ROW_INSERT,
        &[
            "import-zip",
            "--zip",
            &zip.display().to_string(),
            "--name",
            "Half Imported",
        ],
    );

    let core = recover_and_assert(&env, "import_zip crashed before Variant recording").await;
    let listed = core.list_mods(GameCode::Gimi).await.expect("list");
    assert!(
        listed.is_empty(),
        "the in-transaction ZIP-import row must roll back when the process dies: {listed:?}",
    );
    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit after ZIP-import transaction crash");
    assert_eq!(
        report.unreferenced.len(),
        1,
        "the extracted Mod stays actionable"
    );
    assert!(report.unreferenced[0].path.join("Blue/merged.ini").exists());
}

/// Crash immediately after the ZIP import fence commits. The committed row
/// must already include every detected Variant and active selection.
#[tokio::test]
async fn import_zip_crashing_after_commit_preserves_complete_variant_state() {
    let env = TestEnv::new();
    let zip = env.tmp.path().join("committed-mod.zip");
    build_mod_zip(&zip);

    env.crash_during(
        crash_points::IMPORT_ZIP_AFTER_FENCE_COMMIT,
        &[
            "import-zip",
            "--zip",
            &zip.display().to_string(),
            "--name",
            "Committed ZIP",
        ],
    );

    assert_committed_import_shape(&env, "ZIP import crashed immediately after commit").await;
}

/// GameBanana ingest delegates its downloaded archive to `import_zip`. Drive
/// that public path in a real child process so the inherited transaction seam
/// is proven rather than inferred from the local-ZIP test.
#[tokio::test]
async fn gamebanana_import_crashing_before_variant_recording_rolls_back_the_mod_row() {
    let env = TestEnv::new();
    crash_gamebanana_import(&env, crash_points::IMPORT_ZIP_AFTER_ROW_INSERT, 186_001).await;

    let core = recover_and_assert(&env, "GameBanana import crashed before Variant recording").await;
    let listed = core.list_mods(GameCode::Gimi).await.expect("list");
    assert!(
        listed.is_empty(),
        "the delegated in-transaction GameBanana row must roll back: {listed:?}",
    );
    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit after GameBanana transaction crash");
    assert_eq!(
        report.unreferenced.len(),
        1,
        "the downloaded Mod stays actionable"
    );
    assert!(report.unreferenced[0].path.join("Red/merged.ini").exists());
}

/// GameBanana delegates its downloaded archive to `import_zip`; crashing at
/// the delegated post-commit seam must expose the same complete durable shape.
#[tokio::test]
async fn gamebanana_import_crashing_after_commit_preserves_complete_variant_state() {
    let env = TestEnv::new();
    crash_gamebanana_import(&env, crash_points::IMPORT_ZIP_AFTER_FENCE_COMMIT, 186_002).await;

    assert_committed_import_shape(
        &env,
        "GameBanana import crashed immediately after delegated commit",
    )
    .await;
}

// ---------------------------------------------------------------------
// set_active_variant
// ---------------------------------------------------------------------

/// Crash after the row names the new Variant but before the Junction is
/// re-pointed.
///
/// The Variant row update and Junction retarget now share the Library writer
/// fence. A crash after the in-transaction row update therefore rolls it back:
/// the old persisted selection and old Junction remain in agreement.
#[tokio::test]
async fn variant_switch_crashing_before_the_junction_moves_recovers() {
    let env = TestEnv::new();
    let core = env.restart().await;

    let src = env.tmp.path().join("variant-src");
    for variant in ["Red", "Blue"] {
        let d = src.join(variant);
        std::fs::create_dir_all(&d).expect("variant dir");
        std::fs::write(d.join("merged.ini"), format!("; {variant}\n")).expect("ini");
    }
    let m = core
        .adopt_folder(GameCode::Gimi, &src, "Variant Mod")
        .await
        .expect("adopt");
    let variants = core.list_variants(&m.id).await.expect("variants");
    assert_eq!(variants.len(), 2, "fixture has two Variants");
    core.set_enabled(&m.id, true, &env.game_mods)
        .await
        .expect("enable");

    // Switch to whichever Variant is not currently active.
    let active = core
        .active_variant_id(&m.id)
        .await
        .expect("active")
        .expect("an active variant");
    let target_variant = variants
        .iter()
        .find(|v| v.id != active)
        .expect("the other variant");
    let target_name = target_variant.name.clone();
    let target_id = target_variant.id.clone();
    drop(core);

    env.crash_during(
        crash_points::SET_ACTIVE_VARIANT_AFTER_DB_UPDATE,
        &[
            "set-active-variant",
            "--mod-id",
            &m.id,
            "--variant-id",
            &target_id,
            "--mods-dir",
            &env.game_mods.display().to_string(),
        ],
    );

    // Precondition: the Junction still resolves into the old Variant.
    let resolved = std::fs::canonicalize(env.link("Variant Mod")).expect("resolve junction");
    assert!(
        !resolved.ends_with(&target_name),
        "precondition: the Junction has not moved to {target_name} yet, got {resolved:?}",
    );

    let core = recover_and_assert(&env, "variant switch crashed before the junction moved").await;

    let resolved = std::fs::canonicalize(env.link("Variant Mod")).expect("resolve junction");
    assert!(
        !resolved.ends_with(&target_name),
        "the uncommitted Variant choice must roll back with its writer fence; \
         the Junction unexpectedly moved to {target_name:?}: {resolved:?}",
    );
    assert_eq!(
        core.active_variant_id(&m.id)
            .await
            .expect("read recovered active Variant")
            .as_deref(),
        Some(active.as_str()),
        "a crash before the fenced transition commits must preserve the old Variant selection",
    );
    assert_rows_match_disk(&core, &env, "variant switch recovered").await;
}

/// Crash after the old Junction is removed and before the new one is
/// created. The row says enabled with no Junction at all — the plain
/// case reconcile already handled.
#[tokio::test]
async fn variant_switch_crashing_between_junctions_recovers() {
    let env = TestEnv::new();
    let core = env.restart().await;

    let src = env.tmp.path().join("variant-src-2");
    for variant in ["Red", "Blue"] {
        let d = src.join(variant);
        std::fs::create_dir_all(&d).expect("variant dir");
        std::fs::write(d.join("merged.ini"), format!("; {variant}\n")).expect("ini");
    }
    let m = core
        .adopt_folder(GameCode::Gimi, &src, "Between Mod")
        .await
        .expect("adopt");
    let variants = core.list_variants(&m.id).await.expect("variants");
    core.set_enabled(&m.id, true, &env.game_mods)
        .await
        .expect("enable");
    let active = core
        .active_variant_id(&m.id)
        .await
        .expect("active")
        .expect("active variant");
    let target_id = variants
        .iter()
        .find(|v| v.id != active)
        .expect("other variant")
        .id
        .clone();
    drop(core);

    env.crash_during(
        crash_points::SET_ACTIVE_VARIANT_AFTER_JUNCTION_REMOVE,
        &[
            "set-active-variant",
            "--mod-id",
            &m.id,
            "--variant-id",
            &target_id,
            "--mods-dir",
            &env.game_mods.display().to_string(),
        ],
    );

    assert!(
        std::fs::symlink_metadata(env.link("Between Mod")).is_err(),
        "precondition: the crash left no Junction",
    );

    let core = recover_and_assert(&env, "variant switch crashed between junctions").await;
    assert_rows_match_disk(&core, &env, "variant switch crashed between junctions").await;
}

// ---------------------------------------------------------------------
// recover_unreferenced_library_dir
// ---------------------------------------------------------------------

/// Crash after the rename, before the row insert.
///
/// Nothing is lost and nothing is duplicated: the bytes sit under the
/// fresh ULID, no row claims them, and the audit reports them again — so
/// the user's next click recovers exactly the same folder. That is the
/// whole reason the rename goes first.
#[tokio::test]
async fn recovery_crashing_after_the_rename_leaves_the_folder_recoverable_again() {
    let env = TestEnv::new();
    let game_root = env.library.join("gimi");
    let dropped = game_root.join("Dropped In By Hand");
    std::fs::create_dir_all(&dropped).expect("dropped dir");
    std::fs::write(dropped.join("merged.ini"), b"hash=11\n").expect("ini");

    env.crash_during(
        crash_points::RECOVER_AFTER_LIBRARY_MOVE,
        &[
            "recover",
            "--path",
            &dropped.display().to_string(),
            "--name",
            "Recovered By Hand",
        ],
    );

    let core = recover_and_assert(&env, "recovery crashed after the rename").await;
    assert!(
        core.list_mods(GameCode::Gimi)
            .await
            .expect("list")
            .is_empty(),
        "no Mod row should exist — the insert never ran",
    );
    assert!(
        !dropped.exists(),
        "the rename committed before the crash, so the old name is gone",
    );

    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit after the crashed recovery");
    assert_eq!(
        report.unreferenced.len(),
        1,
        "the renamed folder must still be reported: {report:?}",
    );
    let orphan = &report.unreferenced[0];
    assert_eq!(
        std::fs::read(orphan.path.join("merged.ini")).expect("the user's bytes"),
        b"hash=11\n",
        "an interrupted recovery must not have cost the user their files",
    );

    // And a second attempt now succeeds against the reported path.
    let recovered = core
        .recover_unreferenced_library_dir(GameCode::Gimi, &orphan.path, "Recovered By Hand")
        .await
        .expect("recover the folder the crashed attempt renamed");
    assert_eq!(recovered.library_path, orphan.path);
    assert!(core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit after the second attempt")
        .unreferenced
        .is_empty(),);
}

// ---------------------------------------------------------------------
// delete_unreferenced_library_dir
// ---------------------------------------------------------------------

/// A crash after the durable intent but before the same-volume rename leaves
/// the user's directory untouched. Startup removes only the stranded marker,
/// so the ordinary Library audit can still offer recovery or deletion.
#[tokio::test]
async fn delete_crashing_after_intent_write_keeps_the_orphan_and_cleans_the_intent() {
    let env = TestEnv::new();
    let core = env.restart().await;
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game Library root");
    let orphan = root.join(ulid::Ulid::new().to_string());
    std::fs::create_dir_all(&orphan).expect("orphan");
    std::fs::write(orphan.join("precious.buf"), b"keep me").expect("orphan bytes");
    let orphan_s = orphan.display().to_string();

    env.crash_during(
        crash_points::DELETE_AFTER_INTENT_WRITE,
        &["delete-library-dir", "--path", &orphan_s],
    );
    assert_eq!(
        std::fs::read(orphan.join("precious.buf")).expect("orphan survives pre-rename crash"),
        b"keep me",
    );
    assert!(
        std::fs::read_dir(&root)
            .expect("root after crash")
            .any(|entry| entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .ends_with(".intent")),
        "the crash seam must leave the durable intent before restart",
    );

    let restarted = env.restart().await;
    assert!(
        std::fs::read_dir(&root)
            .expect("root after restart")
            .all(|entry| !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .ends_with(".intent")),
        "startup must remove an intent whose quarantine rename never happened",
    );
    assert_eq!(
        std::fs::read(orphan.join("precious.buf")).expect("startup preserves orphan"),
        b"keep me",
    );
    assert!(
        restarted
            .audit_library(GameCode::Gimi)
            .await
            .expect("audit after restart")
            .unreferenced
            .iter()
            .any(|entry| entry.path == orphan),
        "the intact orphan remains available for explicit user action",
    );
}

/// Delete first atomically removes the proven orphan from the user-visible
/// Library namespace. A crash before recursive purge leaves a reserved
/// quarantine, and constructing the Core on restart must finish that purge.
/// This proves both the durable state and the real startup wiring: removing
/// cleanup from `Core::new` makes the quarantine assertion fail.
#[tokio::test]
async fn delete_crashing_after_quarantine_is_finished_on_restart() {
    let env = TestEnv::new();
    let core = env.restart().await;
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game Library root");
    let orphan = root.join(ulid::Ulid::new().to_string());
    std::fs::create_dir_all(orphan.join("nested")).expect("orphan tree");
    std::fs::write(orphan.join("nested/precious.buf"), b"delete me").expect("orphan bytes");
    let orphan_s = orphan.display().to_string();

    env.crash_during(
        crash_points::DELETE_AFTER_QUARANTINE_MOVE,
        &["delete-library-dir", "--path", &orphan_s],
    );

    assert!(
        !orphan.exists(),
        "the atomic quarantine rename committed before the crash",
    );
    let quarantines: Vec<_> = std::fs::read_dir(&root)
        .expect("Library root after crash")
        .filter_map(std::result::Result::ok)
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_dir())
                && entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".gmm-delete-")
        })
        .collect();
    assert_eq!(
        quarantines.len(),
        1,
        "the crash must leave exactly one resumable delete quarantine",
    );
    assert_eq!(
        std::fs::read(quarantines[0].path().join("nested/precious.buf"))
            .expect("quarantined bytes"),
        b"delete me",
        "the crash point fires before recursive purge begins",
    );

    let restarted = env.restart().await;
    assert!(
        std::fs::read_dir(&root)
            .expect("Library root after restart")
            .all(|entry| !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".gmm-delete-")),
        "Core startup must finish every interrupted delete quarantine",
    );
    assert!(
        restarted
            .audit_library(GameCode::Gimi)
            .await
            .expect("audit after cleanup")
            .unreferenced
            .is_empty(),
        "the completed delete leaves neither a Mod row nor an orphan",
    );
}

/// A quarantined delete remains owned after its SQLite claim commits and
/// before recursive removal finishes. Recovery must not rename those bytes
/// back into the Mod namespace while the delete worker may still be walking
/// them.
#[tokio::test]
async fn recover_refuses_a_live_interrupted_delete_quarantine() {
    let env = TestEnv::new();
    let core = env.restart().await;
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game Library root");
    let orphan = root.join(ulid::Ulid::new().to_string());
    std::fs::create_dir_all(orphan.join("nested")).expect("orphan tree");
    std::fs::write(orphan.join("nested/precious.buf"), b"still being deleted")
        .expect("orphan bytes");
    let orphan_s = orphan.display().to_string();

    env.crash_during(
        crash_points::DELETE_AFTER_QUARANTINE_MOVE,
        &["delete-library-dir", "--path", &orphan_s],
    );
    let quarantine = std::fs::read_dir(&root)
        .expect("Library root after crash")
        .filter_map(std::result::Result::ok)
        .find(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_dir())
                && entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".gmm-delete-")
        })
        .expect("the crash left a delete quarantine")
        .path();

    let recovered = core
        .recover_unreferenced_library_dir(GameCode::Gimi, &quarantine, "Must Stay Quarantined")
        .await;
    assert!(
        matches!(
            recovered,
            Err(gmm_lib::core::Error::NotAnUnreferencedLibraryDir { .. })
        ),
        "recover must not reclaim bytes still owned by a committed delete, got {recovered:?}",
    );
    assert_eq!(
        std::fs::read(quarantine.join("nested/precious.buf"))
            .expect("refused recovery leaves quarantine bytes in place"),
        b"still being deleted",
    );
    assert!(
        core.list_mods(GameCode::Gimi)
            .await
            .expect("list after refusal")
            .is_empty(),
        "a refused recovery must not create a Mod row",
    );
}

// ---------------------------------------------------------------------
// Coverage
// ---------------------------------------------------------------------

/// Exercise the ordinary success and cleanup paths with a hook that records
/// the points the implementation actually reaches. A new entry in `ALL`, or
/// a point accidentally removed from its mutation, stays red until a real
/// operation drives that seam; comments, ignored tests, and dead branches do
/// not affect the observation.
#[tokio::test]
async fn every_crash_point_is_exercised_by_an_operation() {
    let reached = Arc::new(Mutex::new(HashSet::<String>::new()));
    let observe = |core: Core| {
        let reached = Arc::clone(&reached);
        core.with_crash_hook(Arc::new(move |point| {
            reached
                .lock()
                .expect("crash-point observation lock")
                .insert(point.to_owned());
        }))
    };

    // The launch reservation has two durable boundaries before the ordinary
    // active_session row exists: claim commit, then spawned-child recording.
    let env = TestEnv::new();
    let core = observe(env.restart().await);
    let claim = core
        .begin_session_launch(GameCode::Gimi)
        .await
        .expect("coverage launch claim");
    core.record_session_launch_child(&claim, std::process::id())
        .await
        .expect("coverage launch child");
    core.abandon_session_launch(&claim)
        .await
        .expect("coverage abandon launch");

    // Startup recovery runs before `with_crash_hook` can decorate an
    // initialized Core, so install the observer while constructing one.
    let env = TestEnv::new();
    let startup_reached = Arc::clone(&reached);
    Core::new_with_crash_hook(
        env.library.clone(),
        &env.db_url,
        Arc::new(move |point| {
            startup_reached
                .lock()
                .expect("startup crash-point observation lock")
                .insert(point.to_owned());
        }),
    )
    .await
    .expect("coverage startup recovery");

    // Adopt, enable, switch Variant, and disable cover both filesystem/row
    // orderings on the common Mod mutation paths.
    let env = TestEnv::new();
    let core = observe(env.restart().await);
    let source = env.tmp.path().join("coverage-variants");
    for variant in ["Red", "Blue"] {
        let directory = source.join(variant);
        std::fs::create_dir_all(&directory).expect("coverage Variant directory");
        std::fs::write(directory.join("merged.ini"), format!("; {variant}\n"))
            .expect("coverage Variant ini");
    }
    let adopted = core
        .adopt_folder(GameCode::Gimi, &source, "Coverage Mod")
        .await
        .expect("coverage adopt");
    core.set_enabled(&adopted.id, true, &env.game_mods)
        .await
        .expect("coverage enable");
    let variants = core
        .list_variants(&adopted.id)
        .await
        .expect("coverage variants");
    let active = core
        .active_variant_id(&adopted.id)
        .await
        .expect("coverage active Variant")
        .expect("coverage Mod has an active Variant");
    let other = variants
        .iter()
        .find(|variant| variant.id != active)
        .expect("coverage Mod has a second Variant");
    core.set_active_variant(&adopted.id, &other.id, &env.game_mods)
        .await
        .expect("coverage Variant switch");
    core.set_enabled(&adopted.id, false, &env.game_mods)
        .await
        .expect("coverage disable");

    // ZIP import has distinct durable seams from folder adoption.
    let env = TestEnv::new();
    let core = observe(env.restart().await);
    let archive = env.tmp.path().join("coverage.zip");
    build_mod_zip(&archive);
    core.import_zip(
        GameCode::Gimi,
        &archive,
        "Coverage ZIP",
        gmm_lib::core::ImportZipOptions::default(),
    )
    .await
    .expect("coverage ZIP import");

    // A successful GameBanana reinstall drives all four durable swap seams:
    // witness commit, old-tree quarantine, replacement move, and metadata
    // commit. PR #176 added these after this inventory test was introduced.
    let env = TestEnv::new();
    let core = observe(env.restart().await);
    let gamebanana_id = 157_178;
    let imported = import_gamebanana_fixture(&env, &core, gamebanana_id).await;
    reinstall_gamebanana_fixture(&env, &core, gamebanana_id, &imported.id).await;
    core.retry_reinstall_recovery(&imported.id)
        .await
        .expect("coverage retry after completed reinstall");

    // A quarantined reinstall drives the serialized withdrawal seam through
    // the same validated witness loader used by startup and retry recovery.
    let env = TestEnv::new();
    let core = observe(env.restart().await);
    let quarantined = seed_mod(&env, &core, "Coverage Quarantined Withdrawal").await;
    let root = quarantined
        .library_path
        .parent()
        .expect("coverage Mod Library root");
    let token = ulid::Ulid::new();
    let stage = root.join(format!(".gmm-reinstall-{token}"));
    let quarantine = root.join(format!(".gmm-delete-{token}"));
    std::fs::create_dir(&stage).expect("coverage reinstall stage");
    let pool = SqlitePool::connect(&env.db_url)
        .await
        .expect("coverage quarantined withdrawal DB");
    sqlx::query(
        "INSERT INTO reinstall_swaps (
            token, mod_id, game_code, library_path, staged_path,
            quarantine_path, old_identity, staged_identity, created_at,
            recovery_error, recovery_attempted_at, recovery_attempts
         ) VALUES (?, ?, 'gimi', ?, ?, ?, ?, ?, ?, ?, ?, 1)",
    )
    .bind(token.to_string())
    .bind(&quarantined.id)
    .bind(quarantined.library_path.to_string_lossy().as_ref())
    .bind(stage.to_string_lossy().as_ref())
    .bind(quarantine.to_string_lossy().as_ref())
    .bind(durable_directory_key(&quarantined.library_path))
    .bind(durable_directory_key(&stage))
    .bind("2026-08-27T00:00:00Z")
    .bind("coverage recovery obstruction")
    .bind("2026-08-27T00:01:00Z")
    .execute(&pool)
    .await
    .expect("insert coverage quarantined witness");
    pool.close().await;
    core.reconcile_junctions(GameCode::Gimi, &env.game_mods)
        .await
        .expect("coverage quarantined withdrawal");

    // A missing source creates a staged destination and then forces the
    // identity-checked quarantine cleanup path.
    let env = TestEnv::new();
    let core = observe(env.restart().await);
    let missing_source = env.tmp.path().join("missing-coverage-source");
    assert!(
        core.adopt_folder(GameCode::Gimi, &missing_source, "Coverage Cleanup")
            .await
            .is_err(),
        "coverage staged adopt must fail and clean up",
    );

    // Relocating one enabled Mod reaches the snapshot, Junction-restore, and
    // fence-commit seams.
    let env = TestEnv::new();
    let core = observe(env.restart().await);
    let relocated = seed_mod(&env, &core, "Coverage Relocation").await;
    core.set_game_install_path(
        GameCode::Gimi,
        env.game_mods.parent().expect("coverage game install root"),
    )
    .await
    .expect("coverage game install path");
    core.set_enabled(&relocated.id, true, &env.game_mods)
        .await
        .expect("coverage relocation enable");
    let new_root = env.tmp.path().join("coverage-relocated-library");
    core.set_library_path_for_game(GameCode::Gimi, Some(&new_root))
        .await
        .expect("coverage relocation");

    // Resolving a duplicate drives the seam between Junction withdrawal and
    // the verification that the deployment entry is truly gone.
    let env = TestEnv::new();
    let core = observe(env.restart().await);
    let keeper = seed_mod(&env, &core, "Coverage Duplicate Keeper").await;
    core.set_game_install_path(
        GameCode::Gimi,
        env.game_mods.parent().expect("coverage game install root"),
    )
    .await
    .expect("coverage duplicate game install path");
    let duplicate_id = ulid::Ulid::new().to_string();
    let pool = sqlx::SqlitePool::connect(&env.db_url)
        .await
        .expect("coverage duplicate DB");
    sqlx::query(
        "INSERT INTO mods (
            id, game_code, name, source, library_path, junction_dir_name,
            enabled, created_at
         ) VALUES (?, 'gimi', 'Coverage Duplicate Rejected', 'manual', ?,
                   'Coverage Duplicate Rejected', 0, ?)",
    )
    .bind(&duplicate_id)
    .bind(keeper.library_path.to_string_lossy().as_ref())
    .bind("2026-08-24T00:00:00Z")
    .execute(&pool)
    .await
    .expect("insert coverage duplicate row");
    core.set_enabled(&duplicate_id, true, &env.game_mods)
        .await
        .expect("enable coverage duplicate");
    let group = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit coverage duplicate")
        .duplicates
        .into_iter()
        .find(|group| group.mods.iter().any(|record| record.id == keeper.id))
        .expect("coverage duplicate group");
    let reviewed: Vec<_> = group
        .mods
        .into_iter()
        .map(|record| gmm_lib::core::ReviewedDuplicateMod {
            id: record.id,
            fingerprint: record.fingerprint,
        })
        .collect();
    core.resolve_duplicate_mods(&keeper.id, &reviewed)
        .await
        .expect("coverage duplicate resolution");

    // Recovery of a non-ULID directory and explicit deletion reach the two
    // remaining Library-recovery paths.
    let env = TestEnv::new();
    let core = observe(env.restart().await);
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("coverage Library root");
    let dropped = root.join("Coverage Dropped Folder");
    std::fs::create_dir_all(&dropped).expect("coverage dropped directory");
    std::fs::write(dropped.join("merged.ini"), b"hash=coverage\n").expect("coverage dropped ini");
    core.recover_unreferenced_library_dir(GameCode::Gimi, &dropped, "Coverage Recovery")
        .await
        .expect("coverage recovery");

    let orphan = root.join(ulid::Ulid::new().to_string());
    std::fs::create_dir_all(&orphan).expect("coverage delete directory");
    std::fs::write(orphan.join("delete-me"), b"coverage").expect("coverage delete marker");
    core.delete_unreferenced_library_dir(GameCode::Gimi, &orphan)
        .await
        .expect("coverage delete");

    let reached = reached.lock().expect("crash-point observation lock");
    let missing: Vec<_> = crash_points::ALL
        .iter()
        .copied()
        .filter(|point| !reached.contains(*point))
        .collect();
    assert!(
        missing.is_empty(),
        "registered crash points no operation actually exercised: {missing:?}",
    );
}
