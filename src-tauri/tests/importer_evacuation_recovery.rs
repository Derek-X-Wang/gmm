//! Issue #227 — a partial Model Importer evacuation stays durably recoverable.

use gmm_lib::core::importer;
use gmm_lib::core::importer_origin::{ImporterOrigin, StoredOverride};
use gmm_lib::core::{Core, GameCode};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

fn db_url(tmp: &TempDir) -> String {
    format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display())
}

fn origin() -> ImporterOrigin {
    ImporterOrigin::github(
        "SilentNightSound",
        "GIMI-Package",
        r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip",
    )
}

fn build_importer_zip(zip_path: &Path) {
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut zip = ZipWriter::new(File::create(zip_path).expect("create importer zip"));
    zip.add_directory("Core", opts).expect("Core directory");
    zip.start_file("Core/new.ini", opts).expect("new Core file");
    zip.write_all(b"new core bytes").expect("write new Core");
    zip.add_directory("ShaderFixes", opts)
        .expect("ShaderFixes directory");
    zip.start_file("d3dx.ini", opts).expect("new d3dx.ini");
    zip.write_all(b"[Loader]\nloader = old.exe\n")
        .expect("write d3dx.ini");
    zip.finish().expect("finish importer zip");
}

struct FakeUpstream {
    server: mockito::ServerGuard,
    _mocks: Vec<mockito::Mock>,
}

impl FakeUpstream {
    async fn start(zip_bytes: Vec<u8>) -> Self {
        let mut server = mockito::Server::new_async().await;
        let asset = "GIMI-PACKAGE-v9.0.0.zip";
        let asset_url = format!("{}/download/{asset}", server.url());
        let body = serde_json::json!({
            "tag_name": "v9.0.0",
            "assets": [{ "name": asset, "browser_download_url": asset_url }],
        })
        .to_string();
        let releases = server
            .mock(
                "GET",
                "/repos/SilentNightSound/GIMI-Package/releases/latest",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await;
        let download = server
            .mock("GET", format!("/download/{asset}").as_str())
            .with_status(200)
            .with_body(zip_bytes)
            .create_async()
            .await;
        Self {
            server,
            _mocks: vec![releases, download],
        }
    }

    fn endpoints(&self) -> gmm_lib::core::importer::Endpoints {
        gmm_lib::core::importer::Endpoints {
            api_base: self.server.url(),
        }
    }
}

#[tokio::test]
async fn partial_importer_evacuation_keeps_its_witness_and_explains_the_recovery() {
    let tmp = TempDir::new().expect("temporary app data");
    let library = tmp.path().join("library");
    let game = tmp.path().join("Genshin");
    std::fs::create_dir_all(game.join("Core")).expect("create old Core");
    std::fs::write(game.join("d3dx.ini"), b"old d3dx bytes").expect("write old d3dx");
    std::fs::write(game.join("Core/original.ini"), b"old Core bytes").expect("write old Core");

    let zip_path = tmp.path().join("fixture.zip");
    build_importer_zip(&zip_path);
    let upstream = FakeUpstream::start(std::fs::read(&zip_path).expect("read fixture zip")).await;

    let fired = Arc::new(AtomicUsize::new(0));
    let fired_for_hook = Arc::clone(&fired);
    let hook = Arc::new(move |point: &str| {
        if point == importer::BACKUP_AFTER_ENTRY_TEST_SEAM
            && fired_for_hook.fetch_add(1, Ordering::SeqCst) == 0
        {
            panic!("injected failure after the first evacuated importer entry");
        }
    });
    let core = Core::new(library.clone(), &db_url(&tmp))
        .await
        .expect("initialize Core")
        .with_crash_hook(hook);
    core.set_game_install_path(GameCode::Gimi, &game)
        .await
        .expect("set game path");
    core.set_importer_origin_override(GameCode::Gimi, Some(&origin()))
        .await
        .expect("set importer origin");

    let error = core
        .install_importer_with_endpoints(GameCode::Gimi, &upstream.endpoints())
        .await
        .expect_err("the injected second-half evacuation failure must fail the install");
    assert_eq!(
        fired.load(Ordering::SeqCst),
        1,
        "the failure seam must be reached once"
    );
    assert!(
        !game.join("d3dx.ini").exists() && game.join("Core/original.ini").is_file(),
        "the fixture must prove a genuinely partial evacuation before recovery: {error}",
    );

    let pool = sqlx::SqlitePool::connect(&db_url(&tmp))
        .await
        .expect("inspect durable state");
    let witnesses: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM importer_evacuations")
        .fetch_one(&pool)
        .await
        .expect("count evacuation witnesses");
    assert_eq!(
        witnesses, 1,
        "the partial evacuation must leave one durable importer recovery witness",
    );
    let recovery = core
        .importer_evacuation_recovery(GameCode::Gimi)
        .await
        .expect("load user-visible recovery state")
        .expect("the failed evacuation must be user-visible");
    assert!(
        recovery.reason.contains("install task join error")
            && recovery.game_path == game
            && recovery.backup_path.starts_with(tmp.path().join("backups/gimi")),
        "the user-visible state must explain the interrupted evacuation and identify both locations: {recovery:?}",
    );
    pool.close().await;
    drop(core);

    let recovered = Core::new(library.clone(), &db_url(&tmp))
        .await
        .expect("startup must recover the interrupted evacuation");
    assert_eq!(
        std::fs::read(game.join("d3dx.ini")).expect("restored d3dx.ini"),
        b"old d3dx bytes",
        "startup must restore the entry evacuated before the failure",
    );
    assert_eq!(
        std::fs::read(game.join("Core/original.ini")).expect("preserved old Core"),
        b"old Core bytes",
        "startup must preserve the entry that had not been evacuated",
    );
    assert!(
        recovered
            .importer_evacuation_recovery(GameCode::Gimi)
            .await
            .expect("read restored state")
            .is_none(),
        "successful startup rollback must retire the durable witness",
    );
}

#[tokio::test]
async fn rollback_ignores_a_partial_backup_retained_by_recovery() {
    let tmp = TempDir::new().expect("temporary app data");
    let library = tmp.path().join("library");
    let game = tmp.path().join("Genshin");
    std::fs::create_dir_all(game.join("Core")).expect("create old Core");
    std::fs::write(game.join("d3dx.ini"), b"old d3dx bytes").expect("write old d3dx");
    std::fs::write(game.join("Core/original.ini"), b"old Core bytes").expect("write old Core");

    let zip_path = tmp.path().join("fixture.zip");
    build_importer_zip(&zip_path);
    let upstream = FakeUpstream::start(std::fs::read(&zip_path).expect("read fixture zip")).await;
    let fired = Arc::new(AtomicUsize::new(0));
    let fired_for_hook = Arc::clone(&fired);
    let hook = Arc::new(move |point: &str| {
        if point == importer::BACKUP_AFTER_ENTRY_TEST_SEAM
            && fired_for_hook.fetch_add(1, Ordering::SeqCst) == 0
        {
            panic!("injected failure after the first evacuated importer entry");
        }
    });
    let core = Core::new(library.clone(), &db_url(&tmp))
        .await
        .expect("initialize Core")
        .with_crash_hook(hook);
    core.set_game_install_path(GameCode::Gimi, &game)
        .await
        .expect("set game path");
    core.set_importer_origin_override(GameCode::Gimi, Some(&origin()))
        .await
        .expect("set importer origin");
    core.install_importer_with_endpoints(GameCode::Gimi, &upstream.endpoints())
        .await
        .expect_err("the injected evacuation failure must fail the install");
    let backup = core
        .importer_evacuation_recovery(GameCode::Gimi)
        .await
        .expect("load recovery")
        .expect("the interrupted evacuation must stay visible")
        .backup_path;
    std::fs::write(game.join("d3dx.ini"), b"old d3dx bytes")
        .expect("restore an equal live entry before recovery");
    drop(core);

    let recovered = Core::new(library, &db_url(&tmp))
        .await
        .expect("startup must settle the equal live and backup entries");
    assert!(
        recovered
            .importer_evacuation_recovery(GameCode::Gimi)
            .await
            .expect("read settled recovery state")
            .is_none(),
        "equal entries must let recovery retire the durable witness",
    );
    assert!(
        backup.join("d3dx.ini").is_file() && !backup.join("Core").exists(),
        "the fixture must retain a genuinely partial backup after recovery",
    );

    std::fs::write(game.join("d3dx.ini"), b"current importer bytes")
        .expect("make a rollback from the remnant observable");
    let rolled_back = recovered
        .rollback_importer(GameCode::Gimi)
        .await
        .expect("attempt rollback after recovery retained a partial backup");
    assert!(
        rolled_back.is_none(),
        "rollback must not select a partial backup retained by recovery: {rolled_back:?}",
    );
    assert_eq!(
        std::fs::read(game.join("d3dx.ini")).expect("read current importer after rollback"),
        b"current importer bytes",
        "rollback must leave the live importer untouched when only a retained remnant exists",
    );
    assert_eq!(
        std::fs::read(backup.join("d3dx.ini")).expect("read retained partial backup"),
        b"old d3dx bytes",
        "rejecting the retained remnant must preserve its bytes",
    );
}

#[tokio::test]
async fn recovery_preserves_a_user_repaired_importer_entry_and_its_backup() {
    let tmp = TempDir::new().expect("temporary app data");
    let library = tmp.path().join("library");
    let game = tmp.path().join("Genshin");
    std::fs::create_dir_all(game.join("Core")).expect("create old Core");
    std::fs::write(game.join("d3dx.ini"), b"old d3dx bytes").expect("write old d3dx");
    std::fs::write(game.join("Core/original.ini"), b"old Core bytes").expect("write old Core");

    let zip_path = tmp.path().join("fixture.zip");
    build_importer_zip(&zip_path);
    let upstream = FakeUpstream::start(std::fs::read(&zip_path).expect("read fixture zip")).await;

    let fired = Arc::new(AtomicUsize::new(0));
    let fired_for_hook = Arc::clone(&fired);
    let hook = Arc::new(move |point: &str| {
        if point == importer::BACKUP_AFTER_ENTRY_TEST_SEAM
            && fired_for_hook.fetch_add(1, Ordering::SeqCst) == 0
        {
            panic!("injected failure after the first evacuated importer entry");
        }
    });
    let core = Core::new(library.clone(), &db_url(&tmp))
        .await
        .expect("initialize Core")
        .with_crash_hook(hook);
    core.set_game_install_path(GameCode::Gimi, &game)
        .await
        .expect("set game path");
    core.set_importer_origin_override(GameCode::Gimi, Some(&origin()))
        .await
        .expect("set importer origin");
    core.install_importer_with_endpoints(GameCode::Gimi, &upstream.endpoints())
        .await
        .expect_err("the injected evacuation failure must fail the install");
    let backup = core
        .importer_evacuation_recovery(GameCode::Gimi)
        .await
        .expect("load recovery")
        .expect("the interrupted evacuation must stay visible")
        .backup_path;
    drop(core);

    std::fs::write(game.join("d3dx.ini"), b"user repaired bytes")
        .expect("simulate a user repair before the next startup");
    let recovered = Core::new(library, &db_url(&tmp))
        .await
        .expect("startup must preserve an unresolved recovery");

    assert_eq!(
        std::fs::read(game.join("d3dx.ini")).expect("read repaired live entry"),
        b"user repaired bytes",
        "recovery must not overwrite a live importer entry whose contents differ from the recorded backup",
    );
    assert_eq!(
        std::fs::read(backup.join("d3dx.ini")).expect("read retained backup entry"),
        b"old d3dx bytes",
        "recovery must retain the recorded backup when it cannot choose between two different entries",
    );
    let recovery = recovered
        .importer_evacuation_recovery(GameCode::Gimi)
        .await
        .expect("load unresolved recovery")
        .expect("the differing live entry must keep recovery visible");
    assert!(
        recovery.reason.contains("differs from its recorded backup"),
        "the recovery warning must explain why GMM preserved both entries: {recovery:?}",
    );

    std::fs::remove_file(game.join("d3dx.ini"))
        .expect("simulate the user resolving the conflict in favour of the backup");
    recovered
        .retry_importer_evacuation_recovery(GameCode::Gimi)
        .await
        .expect("the in-session retry must rerun the same validated recovery");
    assert!(
        game.join("d3dx.ini").is_file(),
        "the in-session retry must restore the retained backup without restarting GMM",
    );
    assert_eq!(
        std::fs::read(game.join("d3dx.ini")).expect("read retry-restored entry"),
        b"old d3dx bytes",
        "the in-session retry must restore the retained backup once the conflict is resolved",
    );
    assert!(
        recovered
            .importer_evacuation_recovery(GameCode::Gimi)
            .await
            .expect("read recovery after retry")
            .is_none(),
        "a successful in-session retry must retire the durable witness",
    );
}

#[tokio::test]
async fn backup_identity_change_becomes_retryable_when_the_original_directory_returns() {
    let tmp = TempDir::new().expect("temporary app data");
    let library = tmp.path().join("library");
    let game = tmp.path().join("Genshin");
    std::fs::create_dir_all(game.join("Core")).expect("create old Core");
    std::fs::write(game.join("d3dx.ini"), b"old d3dx bytes").expect("write old d3dx");
    std::fs::write(game.join("Core/original.ini"), b"old Core bytes").expect("write old Core");

    let zip_path = tmp.path().join("fixture.zip");
    build_importer_zip(&zip_path);
    let upstream = FakeUpstream::start(std::fs::read(&zip_path).expect("read fixture zip")).await;
    let fired = Arc::new(AtomicUsize::new(0));
    let fired_for_hook = Arc::clone(&fired);
    let hook = Arc::new(move |point: &str| {
        if point == importer::BACKUP_AFTER_ENTRY_TEST_SEAM
            && fired_for_hook.fetch_add(1, Ordering::SeqCst) == 0
        {
            panic!("injected failure after the first evacuated importer entry");
        }
    });
    let core = Core::new(library.clone(), &db_url(&tmp))
        .await
        .expect("initialize Core")
        .with_crash_hook(hook);
    core.set_game_install_path(GameCode::Gimi, &game)
        .await
        .expect("set game path");
    core.set_importer_origin_override(GameCode::Gimi, Some(&origin()))
        .await
        .expect("set importer origin");
    core.install_importer_with_endpoints(GameCode::Gimi, &upstream.endpoints())
        .await
        .expect_err("the injected evacuation failure must fail the install");
    let backup = core
        .importer_evacuation_recovery(GameCode::Gimi)
        .await
        .expect("load recovery")
        .expect("the interrupted evacuation must stay visible")
        .backup_path;
    drop(core);

    let displaced = backup.with_extension("original");
    std::fs::rename(&backup, &displaced).expect("move the recorded backup object aside");
    std::fs::create_dir(&backup).expect("recreate the recorded backup directory");
    std::fs::copy(displaced.join("d3dx.ini"), backup.join("d3dx.ini"))
        .expect("preserve the recorded backup bytes in the recreated directory");

    let recovered = Core::new(library.clone(), &db_url(&tmp))
        .await
        .expect("startup must expose the terminal recovery state");
    let recovery = recovered
        .importer_evacuation_recovery(GameCode::Gimi)
        .await
        .expect("load terminal recovery")
        .expect("the identity mismatch must remain visible");
    assert!(
        recovery.reason.contains("changed filesystem identity")
            && recovery.reason.contains("restore the original directory object")
            && recovery.reason.contains(backup.to_string_lossy().as_ref())
            && recovery.reason.contains("will not search"),
        "the recovery state must explain what exact recorded object and path would make retry viable: {recovery:?}",
    );
    assert_eq!(
        recovery.action,
        importer::ImporterEvacuationRecoveryAction::AcknowledgeAndRelease,
        "a presently mismatched directory must still offer the safe release path",
    );
    let attempts = recovery.attempts;
    drop(recovered);

    let recovered = Core::new(library, &db_url(&tmp))
        .await
        .expect("a later startup must preserve the terminal recovery state");
    assert_eq!(
        recovered
            .importer_evacuation_recovery(GameCode::Gimi)
            .await
            .expect("reload terminal recovery")
            .expect("terminal recovery remains visible")
            .attempts,
        attempts,
        "startup must not keep retrying a recovery whose recorded identity cannot return",
    );
    recovered
        .retry_importer_evacuation_recovery(GameCode::Gimi)
        .await
        .expect_err("retry must remain unavailable until the original directory object returns");

    std::fs::remove_dir_all(&backup).expect("remove the replacement backup directory");
    std::fs::rename(&displaced, &backup)
        .expect("restore the original backup directory object to its recorded path");
    let recovery = recovered
        .importer_evacuation_recovery(GameCode::Gimi)
        .await
        .expect("reclassify the restored recorded state")
        .expect("the witness remains until recovery completes");
    assert_eq!(
        recovery.action,
        importer::ImporterEvacuationRecoveryAction::Retry,
        "restoring the original directory object must make retry available again",
    );

    recovered
        .retry_importer_evacuation_recovery(GameCode::Gimi)
        .await
        .expect("restoring the original directory object must let recovery complete");
    assert!(
        recovered
            .importer_evacuation_recovery(GameCode::Gimi)
            .await
            .expect("read released recovery state")
            .is_none(),
        "successful retry must retire the durable blocker",
    );
    assert_eq!(
        std::fs::read(game.join("d3dx.ini")).expect("read restored importer entry"),
        b"old d3dx bytes",
        "retry must restore the original backup bytes",
    );
    let replacement =
        ImporterOrigin::github("example", "replacement-importer", r"replacement-v\d+\.zip");
    recovered
        .set_importer_origin_override(GameCode::Gimi, Some(&replacement))
        .await
        .expect("releasing terminal recovery must unblock importer operations for the Game");
}

#[tokio::test]
async fn acknowledge_and_release_preserves_bytes_at_both_recorded_locations() {
    let tmp = TempDir::new().expect("temporary app data");
    let library = tmp.path().join("library");
    let game = tmp.path().join("Genshin");
    std::fs::create_dir_all(game.join("Core")).expect("create old Core");
    std::fs::write(game.join("d3dx.ini"), b"old d3dx bytes").expect("write old d3dx");
    std::fs::write(game.join("Core/original.ini"), b"old Core bytes").expect("write old Core");

    let zip_path = tmp.path().join("fixture.zip");
    build_importer_zip(&zip_path);
    let upstream = FakeUpstream::start(std::fs::read(&zip_path).expect("read fixture zip")).await;
    let fired = Arc::new(AtomicUsize::new(0));
    let fired_for_hook = Arc::clone(&fired);
    let hook = Arc::new(move |point: &str| {
        if point == importer::BACKUP_AFTER_ENTRY_TEST_SEAM
            && fired_for_hook.fetch_add(1, Ordering::SeqCst) == 0
        {
            panic!("injected failure after the first evacuated importer entry");
        }
    });
    let core = Core::new(library.clone(), &db_url(&tmp))
        .await
        .expect("initialize Core")
        .with_crash_hook(hook);
    core.set_game_install_path(GameCode::Gimi, &game)
        .await
        .expect("set game path");
    core.set_importer_origin_override(GameCode::Gimi, Some(&origin()))
        .await
        .expect("set importer origin");
    core.install_importer_with_endpoints(GameCode::Gimi, &upstream.endpoints())
        .await
        .expect_err("the injected evacuation failure must fail the install");
    let backup = core
        .importer_evacuation_recovery(GameCode::Gimi)
        .await
        .expect("load recovery")
        .expect("the interrupted evacuation must stay visible")
        .backup_path;
    drop(core);

    let displaced = backup.with_extension("original");
    std::fs::rename(&backup, &displaced).expect("move the recorded backup object aside");
    std::fs::create_dir(&backup).expect("recreate the recorded backup directory");
    std::fs::copy(displaced.join("d3dx.ini"), backup.join("d3dx.ini"))
        .expect("preserve backup bytes at the recorded path");

    let recovered = Core::new(library, &db_url(&tmp))
        .await
        .expect("startup must expose the identity mismatch");
    let recovery = recovered
        .importer_evacuation_recovery(GameCode::Gimi)
        .await
        .expect("load terminal recovery")
        .expect("the identity mismatch must remain visible");
    assert_eq!(
        recovery.action,
        importer::ImporterEvacuationRecoveryAction::AcknowledgeAndRelease,
        "a changed backup identity must expose acknowledge-and-release",
    );

    recovered
        .retire_interrupted_importer_evacuation(GameCode::Gimi)
        .await
        .expect("acknowledge and release the terminal recovery blocker");
    assert!(
        recovered
            .importer_evacuation_recovery(GameCode::Gimi)
            .await
            .expect("read released recovery state")
            .is_none(),
        "acknowledge-and-release must retire only the durable witness",
    );
    assert_eq!(
        std::fs::read(game.join("Core/original.ini")).expect("read recorded game location"),
        b"old Core bytes",
        "acknowledge-and-release must preserve bytes at the recorded game location",
    );
    assert_eq!(
        std::fs::read(backup.join("d3dx.ini")).expect("read recorded backup location"),
        b"old d3dx bytes",
        "acknowledge-and-release must preserve bytes at the recorded backup location",
    );
    assert_eq!(
        std::fs::read(displaced.join("d3dx.ini")).expect("read displaced original object"),
        b"old d3dx bytes",
        "acknowledge-and-release must not search for or delete the displaced original object",
    );
}

#[tokio::test]
async fn pending_importer_evacuation_blocks_origin_override_change() {
    let tmp = TempDir::new().expect("temporary app data");
    let library = tmp.path().join("library");
    let game = tmp.path().join("Genshin");
    std::fs::create_dir_all(game.join("Core")).expect("create old Core");
    std::fs::write(game.join("d3dx.ini"), b"old d3dx bytes").expect("write old d3dx");
    std::fs::write(game.join("Core/original.ini"), b"old Core bytes").expect("write old Core");

    let zip_path = tmp.path().join("fixture.zip");
    build_importer_zip(&zip_path);
    let upstream = FakeUpstream::start(std::fs::read(&zip_path).expect("read fixture zip")).await;
    let fired = Arc::new(AtomicUsize::new(0));
    let fired_for_hook = Arc::clone(&fired);
    let hook = Arc::new(move |point: &str| {
        if point == importer::BACKUP_AFTER_ENTRY_TEST_SEAM
            && fired_for_hook.fetch_add(1, Ordering::SeqCst) == 0
        {
            panic!("injected failure after the first evacuated importer entry");
        }
    });
    let core = Core::new(library, &db_url(&tmp))
        .await
        .expect("initialize Core")
        .with_crash_hook(hook);
    core.set_game_install_path(GameCode::Gimi, &game)
        .await
        .expect("set game path");
    core.set_importer_origin_override(GameCode::Gimi, Some(&origin()))
        .await
        .expect("set initial importer origin");
    core.install_importer_with_endpoints(GameCode::Gimi, &upstream.endpoints())
        .await
        .expect_err("the injected evacuation failure must fail the install");

    let replacement =
        ImporterOrigin::github("example", "replacement-importer", r"replacement-v\d+\.zip");
    let error = core
        .set_importer_origin_override(GameCode::Gimi, Some(&replacement))
        .await
        .expect_err("a pending evacuation must block an Importer Origin change");
    assert!(
        error
            .to_string()
            .contains("interrupted Model Importer evacuation"),
        "the refusal must name the pending Model Importer recovery: {error}",
    );
    let stored = core
        .importer_origin_override(GameCode::Gimi)
        .await
        .expect("read preserved override");
    assert!(
        matches!(stored, StoredOverride::Set(ref value) if value == &origin()),
        "a refused origin change must preserve the previously configured Importer Origin: {stored:?}",
    );
}
