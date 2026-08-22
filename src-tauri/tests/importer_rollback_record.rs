//! Issue #126 — after a rollback, GMM's record describes what is
//! actually on disk.
//!
//! Rollback restored the previous package into the game directory and
//! left the recorded version and Importer Origin pointing at the install
//! it had just undone. That is the disk-versus-database disagreement the
//! origin-change path (#110) exists to prevent, still open on this path,
//! and the Importer Origin work made it materially worse: the recorded
//! origin drives pin clearing and the change-proposal logic, so rolling
//! back an origin switch left GMM believing the game was on the new
//! origin while the disk held the old package — and the proposal logic
//! then reported nothing to propose, because as far as the database was
//! concerned the switch had already happened.
//!
//! The wrinkle the brief flags is real: a backup is a pile of files and
//! carries no provenance of its own. GMM now writes what it knew about
//! the install being replaced *beside* the backup at install time, and
//! rollback reads it. When there is nothing to read — a backup taken by
//! an older GMM, or files that predate GMM entirely — the record becomes
//! **unknown**, which is a first-class state (#99) and strictly better
//! than a confident wrong answer.

use gmm_lib::core::importer::Endpoints;
use gmm_lib::core::importer_origin::{ImporterOrigin, InstalledOrigin};
use gmm_lib::core::{Core, GameCode};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

fn db_url(tmp: &TempDir) -> String {
    format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display())
}

async fn fresh_core(tmp: &TempDir) -> Core {
    Core::new(tmp.path().join("library"), &db_url(tmp))
        .await
        .expect("init")
}

fn game_dir(tmp: &TempDir) -> PathBuf {
    tmp.path().join("Genshin")
}

fn backups_root(tmp: &TempDir) -> PathBuf {
    tmp.path().join("backups").join("gimi")
}

fn origin_a() -> ImporterOrigin {
    ImporterOrigin::github(
        "SilentNightSound",
        "GIMI-Package",
        r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip",
    )
}

fn origin_b() -> ImporterOrigin {
    ImporterOrigin::github("someone", "GIMI-Fork", r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip")
}

fn opts() -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)
}

/// A minimal archive with the shape `validate_importer_archive` accepts.
/// `marker` goes into `Core/library.ini` so a test can tell which
/// package is currently deployed.
fn build_importer_zip(zip_path: &Path, marker: &str) {
    let mut zw = ZipWriter::new(File::create(zip_path).expect("create zip"));
    zw.add_directory("Core", opts()).expect("core dir");
    zw.start_file("Core/library.ini", opts()).expect("core ini");
    zw.write_all(marker.as_bytes()).expect("write core");
    zw.add_directory("ShaderFixes", opts()).expect("shaders");
    zw.start_file("d3dx.ini", opts()).expect("d3dx");
    zw.write_all(b"[Loader]\nloader = XXMI Launcher.exe\n")
        .expect("write d3dx");
    zw.finish().expect("finish zip");
}

struct FakeUpstream {
    server: mockito::ServerGuard,
    _mocks: Vec<mockito::Mock>,
}

impl FakeUpstream {
    async fn start(origin: &ImporterOrigin, tag: &str, asset: &str, bytes: Vec<u8>) -> Self {
        let mut server = mockito::Server::new_async().await;
        let asset_url = format!("{}/download/{asset}", server.url());
        let body = serde_json::json!({
            "tag_name": tag,
            "assets": [{ "name": asset, "browser_download_url": asset_url }],
        })
        .to_string();
        let releases = server
            .mock(
                "GET",
                format!("/repos/{}/releases/latest", origin.repo_slug()).as_str(),
            )
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        let download = server
            .mock("GET", format!("/download/{asset}").as_str())
            .with_status(200)
            .with_body(bytes)
            .create_async()
            .await;
        Self {
            server,
            _mocks: vec![releases, download],
        }
    }

    fn endpoints(&self) -> Endpoints {
        Endpoints {
            api_base: self.server.url(),
        }
    }
}

/// Install `origin` at `tag` through the real install path.
async fn install(core: &Core, tmp: &TempDir, origin: &ImporterOrigin, tag: &str, marker: &str) {
    let zip = tmp.path().join(format!("{tag}.zip"));
    build_importer_zip(&zip, marker);
    let bytes = fs::read(&zip).expect("read zip");

    core.set_importer_origin_override(GameCode::Gimi, Some(origin))
        .await
        .expect("set origin");
    let upstream =
        FakeUpstream::start(origin, tag, &format!("GIMI-PACKAGE-{tag}.zip"), bytes).await;
    core.install_importer_with_endpoints(GameCode::Gimi, &upstream.endpoints())
        .await
        .expect("install");
}

#[tokio::test]
async fn rolling_back_an_origin_switch_stops_the_record_describing_the_undone_install() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.set_game_install_path(GameCode::Gimi, &game_dir(&tmp))
        .await
        .expect("set install path");

    install(&core, &tmp, &origin_a(), "v8.8.9", "; package A\n").await;
    install(&core, &tmp, &origin_b(), "v1.4.4", "; package B\n").await;

    assert_eq!(
        core.installed_importer_origin(GameCode::Gimi)
            .await
            .expect("read"),
        InstalledOrigin::Known(origin_b()),
        "precondition: the second install is what GMM has recorded",
    );

    let restored = core
        .rollback_importer(GameCode::Gimi)
        .await
        .expect("rollback")
        .expect("a backup exists to roll back to");
    assert!(restored.is_dir());

    // The disk now holds package A again…
    assert_eq!(
        fs::read_to_string(game_dir(&tmp).join("Core/library.ini")).expect("read deployed Core"),
        "; package A\n",
    );
    // …so the record must not still say B.
    let recorded = core
        .installed_importer_origin(GameCode::Gimi)
        .await
        .expect("read");
    assert_ne!(
        recorded,
        InstalledOrigin::Known(origin_b()),
        "the record must not describe the install that was just undone",
    );

    // And the honest answer here is **unknown**, not A. Switching the
    // override off A already invalidated the record of A's install
    // (#110) — deliberately, because the game directory then held a
    // package GMM no longer had a record for. So by the time B was
    // installed there was nothing left to write into the backup's
    // provenance, and claiming A now would mean GMM remembering
    // something it had decided to discard. Unknown is a first-class
    // state (#99) and is what the brief calls the acceptable outcome
    // when provenance cannot be determined.
    assert_eq!(
        recorded,
        InstalledOrigin::Unknown,
        "GMM discarded A's record when the origin moved, so it cannot claim A back",
    );
    assert_eq!(
        core.installed_importer_version(GameCode::Gimi)
            .await
            .expect("read"),
        None,
    );
}

#[tokio::test]
async fn rolling_back_a_version_update_restores_the_exact_previous_install() {
    // The common rollback: "this importer update broke my mods". Same
    // Importer Origin throughout, so nothing invalidated the record and
    // GMM knows precisely what it replaced — the case where provenance
    // pays for itself.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.set_game_install_path(GameCode::Gimi, &game_dir(&tmp))
        .await
        .expect("set install path");

    install(&core, &tmp, &origin_a(), "v8.8.8", "; package A, older\n").await;
    core.set_importer_pinned(GameCode::Gimi, Some("v8.8.8"))
        .await
        .expect("pin");
    install(&core, &tmp, &origin_a(), "v8.8.9", "; package A, newer\n").await;

    assert_eq!(
        core.installed_importer_version(GameCode::Gimi)
            .await
            .expect("read"),
        Some("v8.8.9".to_string()),
        "precondition: the newer version is what GMM has recorded",
    );

    core.rollback_importer(GameCode::Gimi)
        .await
        .expect("rollback")
        .expect("a backup exists");

    assert_eq!(
        fs::read_to_string(game_dir(&tmp).join("Core/library.ini")).expect("read"),
        "; package A, older\n",
    );
    assert_eq!(
        core.installed_importer_version(GameCode::Gimi)
            .await
            .expect("read"),
        Some("v8.8.8".to_string()),
        "the record has to describe the package now on disk",
    );
    assert_eq!(
        core.installed_importer_origin(GameCode::Gimi)
            .await
            .expect("read"),
        InstalledOrigin::Known(origin_a()),
    );
    assert_eq!(
        core.importer_pinned(GameCode::Gimi)
            .await
            .expect("read pin"),
        Some("v8.8.8".to_string()),
        "the origin never moved, so the pin is still meaningful and is left alone",
    );
}

#[tokio::test]
async fn a_rollback_with_no_provenance_records_unknown_rather_than_guessing() {
    // A backup taken by a GMM that predates provenance, or a game
    // directory GMM never installed into. There is no honest answer
    // available, and unknown is a real state — a confident wrong one is
    // what the pin logic and the proposal logic would then trust.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.set_game_install_path(GameCode::Gimi, &game_dir(&tmp))
        .await
        .expect("set install path");

    // A hand-made backup with nothing beside it.
    let backup = backups_root(&tmp).join("20250101T000000");
    fs::create_dir_all(backup.join("Core")).expect("backup dir");
    fs::write(backup.join("Core/library.ini"), b"; hand-installed\n").expect("core");
    fs::write(backup.join("d3dx.ini"), b"[Loader]\nloader = gmm.exe\n").expect("d3dx");

    core.record_importer_install(GameCode::Gimi, "v1.4.4", &origin_b())
        .await
        .expect("record");
    core.set_importer_pinned(GameCode::Gimi, Some("v1.4.4"))
        .await
        .expect("pin");

    core.rollback_importer(GameCode::Gimi)
        .await
        .expect("rollback")
        .expect("the backup is found");

    assert_eq!(
        core.installed_importer_origin(GameCode::Gimi)
            .await
            .expect("read"),
        InstalledOrigin::Unknown,
        "GMM cannot say where these files came from, and must not guess",
    );
    assert_eq!(
        core.installed_importer_version(GameCode::Gimi)
            .await
            .expect("read"),
        None,
    );
    assert_eq!(
        core.importer_pinned(GameCode::Gimi)
            .await
            .expect("read pin"),
        None,
        "a pin taken against the origin that was just rolled off is meaningless",
    );
}

#[tokio::test]
async fn rollback_still_restores_the_files_and_leaves_nothing_of_its_own_behind() {
    // The filesystem behaviour is unchanged, and the provenance record
    // lives *beside* the backup rather than inside it — `rollback_to`
    // moves every entry of the backup directory into the game
    // directory, so a sidecar stored inside would be deposited next to
    // the user's `d3dx.ini`.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.set_game_install_path(GameCode::Gimi, &game_dir(&tmp))
        .await
        .expect("set install path");

    install(&core, &tmp, &origin_a(), "v8.8.9", "; package A\n").await;
    install(&core, &tmp, &origin_b(), "v1.4.4", "; package B\n").await;
    core.rollback_importer(GameCode::Gimi)
        .await
        .expect("rollback");

    assert_eq!(
        fs::read_to_string(game_dir(&tmp).join("Core/library.ini")).expect("read"),
        "; package A\n",
    );
    assert!(game_dir(&tmp).join("d3dx.ini").is_file());

    let stray: Vec<String> = fs::read_dir(game_dir(&tmp))
        .expect("read game dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.contains("provenance") || n.contains("gmm-"))
        .collect();
    assert!(
        stray.is_empty(),
        "rollback must not deposit GMM bookkeeping into the game directory: {stray:?}",
    );
}

#[tokio::test]
async fn a_rollback_whose_record_cannot_be_written_is_not_reported_as_a_plain_success() {
    // Same rule as #122 on the install path: the files moved and the
    // bookkeeping did not, so the caller has to be told.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    core.set_game_install_path(GameCode::Gimi, &game_dir(&tmp))
        .await
        .expect("set install path");

    install(&core, &tmp, &origin_a(), "v8.8.9", "; package A\n").await;
    install(&core, &tmp, &origin_b(), "v1.4.4", "; package B\n").await;

    let pool = sqlx::SqlitePool::connect(&db_url(&tmp))
        .await
        .expect("open db");
    sqlx::query(
        "CREATE TRIGGER gmm_test_disk_full BEFORE INSERT ON settings
         WHEN NEW.key = 'importer.gimi.installed_version'
         BEGIN SELECT RAISE(ABORT, 'disk I/O error'); END",
    )
    .execute(&pool)
    .await
    .expect("arm trigger");
    pool.close().await;

    let error = core
        .rollback_importer(GameCode::Gimi)
        .await
        .expect_err("a rollback whose record was not updated is not a success");
    let message = error.to_string();
    assert!(
        message.contains("rolled back") || message.contains("rollback"),
        "the failure must say what actually happened on disk: {message}",
    );
}
