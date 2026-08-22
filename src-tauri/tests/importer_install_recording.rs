//! Issue #122 — recording a Model Importer install is atomic, and a
//! failure to record it is surfaced.
//!
//! The install command wrote files into the game directory, then called
//! `record_importer_install` and threw the result away with `let _ =`,
//! returning `Ok(report)` either way. That is the project's recurring
//! defect class — an error rendered as a benign result — relocated from
//! the network path (#78, #114) into persistence. The UI said
//! "Installed" while GMM retained the old version, the old Importer
//! Origin, or a half-updated mixture of the two, and every later
//! decision (pin clearing, the update badge, whether a recommendation is
//! proposing a change) then built on state that was never saved.
//!
//! Recording an install is three related writes — the Importer Pin
//! reconciliation, the installed Importer Origin, and the installed
//! version — so it is one transaction. These tests inject a persistence
//! failure between them with a SQLite trigger, which is the only
//! injection that needs no test-only hook in the shipped code.

use gmm_lib::core::importer_origin::{ImporterOrigin, InstalledOrigin};
use gmm_lib::core::{Core, GameCode};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

fn db_url(tmp: &TempDir) -> String {
    format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display())
}

async fn fresh_core(tmp: &TempDir) -> Core {
    // The data dir is the Library root's parent, which is how
    // `build_core` lays it out — so downloads and backups land under
    // `tmp` and not in the developer's real app-data directory.
    Core::new(tmp.path().join("library"), &db_url(tmp))
        .await
        .expect("init")
}

fn gimi_default() -> ImporterOrigin {
    ImporterOrigin::github(
        "SilentNightSound",
        "GIMI-Package",
        r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip",
    )
}

fn a_different_origin() -> ImporterOrigin {
    ImporterOrigin::github("someone", "GIMI-Fork", r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip")
}

/// Make every write to `key` fail, the way a disk-full or I/O error
/// would — after the writes that precede it in the same operation have
/// already been issued.
///
/// A second connection to the same file database, so the shipped `Core`
/// needs no failure-injection seam for this.
async fn fail_writes_to(tmp: &TempDir, key: &str) {
    let pool = sqlx::SqlitePool::connect(&db_url(tmp))
        .await
        .expect("open db for trigger");
    sqlx::query(&format!(
        "CREATE TRIGGER gmm_test_disk_full BEFORE INSERT ON settings
         WHEN NEW.key = '{key}'
         BEGIN SELECT RAISE(ABORT, 'disk I/O error'); END"
    ))
    .execute(&pool)
    .await
    .expect("arm failure trigger");
    pool.close().await;
}

fn opts() -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)
}

/// The smallest archive `validate_importer_archive` accepts: `d3dx.ini`
/// at the root plus `Core/` and `ShaderFixes/`, and no binaries.
fn build_importer_zip(zip_path: &Path) {
    let mut zw = ZipWriter::new(File::create(zip_path).expect("create zip"));
    zw.add_directory("Core", opts()).expect("core dir");
    zw.start_file("Core/library.ini", opts()).expect("core ini");
    zw.write_all(b"; core\n").expect("write core");
    zw.add_directory("ShaderFixes", opts())
        .expect("shaders dir");
    zw.start_file("d3dx.ini", opts()).expect("d3dx");
    zw.write_all(b"[Loader]\nloader = XXMI Launcher.exe\n")
        .expect("write d3dx");
    zw.finish().expect("finish zip");
}

/// A GitHub `releases/latest` payload and the asset it points at, served
/// from a mock host so the real install command path can be driven
/// end-to-end without the network.
struct FakeUpstream {
    server: mockito::ServerGuard,
    _mocks: Vec<mockito::Mock>,
}

impl FakeUpstream {
    async fn start(origin: &ImporterOrigin, tag: &str, asset: &str, zip_bytes: Vec<u8>) -> Self {
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

/// Set `game` up so the real install command path has somewhere to
/// install to and something to install from.
async fn ready_to_install(core: &Core, tmp: &TempDir, origin: &ImporterOrigin) {
    core.set_game_install_path(GameCode::Gimi, &tmp.path().join("Genshin"))
        .await
        .expect("set install path");
    core.set_importer_origin_override(GameCode::Gimi, Some(origin))
        .await
        .expect("set origin override");
}

#[tokio::test]
async fn recording_an_install_is_all_or_nothing() {
    // The three writes belong to one install. A partial landing is
    // worse than no landing: an origin recorded without its version,
    // or a pin cleared for a move that never completed, is state no
    // later decision can read correctly.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    core.record_importer_install(GameCode::Gimi, "v8.8.9", &gimi_default())
        .await
        .expect("seed install");
    core.set_importer_pinned(GameCode::Gimi, Some("v8.8.9"))
        .await
        .expect("pin");

    fail_writes_to(&tmp, "importer.gimi.installed_version").await;

    let error = core
        .record_importer_install(GameCode::Gimi, "v1.4.4", &a_different_origin())
        .await
        .expect_err("a failed write must not be reported as a recorded install");

    assert_eq!(
        core.importer_pinned(GameCode::Gimi)
            .await
            .expect("read pin"),
        Some("v8.8.9".to_string()),
        "the pin is cleared *because* the origin moved; if the move did not \
         record, clearing it discards the user's ban-wave escape hatch for \
         nothing. Error was: {error}",
    );
    assert_eq!(
        core.installed_importer_origin(GameCode::Gimi)
            .await
            .expect("read origin"),
        InstalledOrigin::Known(gimi_default()),
        "the recorded Importer Origin must still describe the files on disk",
    );
    assert_eq!(
        core.installed_importer_version(GameCode::Gimi)
            .await
            .expect("read version"),
        Some("v8.8.9".to_string()),
    );
}

#[tokio::test]
async fn the_install_command_records_the_version_and_the_origin_it_came_from() {
    // Drives the real install path — resolve origin, fetch release,
    // download, unpack, record — and asserts the state afterwards. The
    // test this replaces asserted that `record_importer_install` merely
    // *appeared* in `commands.rs`, which no persistence failure could
    // ever have failed.
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let zip = tmp.path().join("GIMI-PACKAGE-v8.8.9.zip");
    build_importer_zip(&zip);
    let bytes = std::fs::read(&zip).expect("read zip");

    // A working install from one origin, pinned — then the user accepts
    // a different origin. Changing origin clears the pin (ADR 0005).
    core.record_importer_install(GameCode::Gimi, "v8.8.8", &gimi_default())
        .await
        .expect("seed install");
    core.set_importer_pinned(GameCode::Gimi, Some("v8.8.8"))
        .await
        .expect("pin");

    let mine = a_different_origin();
    ready_to_install(&core, &tmp, &mine).await;
    let upstream =
        FakeUpstream::start(&mine, "v8.8.9", "GIMI-PACKAGE-v8.8.9.zip", bytes.clone()).await;

    core.install_importer_with_endpoints(GameCode::Gimi, &upstream.endpoints())
        .await
        .expect("install");

    assert_eq!(
        core.installed_importer_version(GameCode::Gimi)
            .await
            .expect("read version"),
        Some("v8.8.9".to_string()),
    );
    assert_eq!(
        core.installed_importer_origin(GameCode::Gimi)
            .await
            .expect("read origin"),
        InstalledOrigin::Known(mine),
        "an install is the only way an unknown Importer Origin becomes known (#99)",
    );
    assert_eq!(
        core.importer_pinned(GameCode::Gimi)
            .await
            .expect("read pin"),
        None,
        "a version string taken against one origin is meaningless against another",
    );
}

#[tokio::test]
async fn the_install_command_refuses_to_report_success_over_an_unrecorded_install() {
    // Files on disk, nothing recorded — the exact state the `let _ =`
    // rendered as "Installed".
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;

    let zip = tmp.path().join("GIMI-PACKAGE-v8.8.9.zip");
    build_importer_zip(&zip);
    let bytes = std::fs::read(&zip).expect("read zip");

    let mine = a_different_origin();
    ready_to_install(&core, &tmp, &mine).await;
    let upstream =
        FakeUpstream::start(&mine, "v8.8.9", "GIMI-PACKAGE-v8.8.9.zip", bytes.clone()).await;

    fail_writes_to(&tmp, "importer.gimi.installed_version").await;

    let error = core
        .install_importer_with_endpoints(GameCode::Gimi, &upstream.endpoints())
        .await
        .expect_err("an install whose state was not recorded is not a success");

    let message = error.to_string();
    assert!(
        message.contains("v8.8.9"),
        "the failure must name the version whose record was lost, so the user \
         knows what is on disk: {message}",
    );
    assert!(
        message.contains("installed"),
        "the failure must say the files did install — the game directory has \
         been rewritten and the user needs to know it: {message}",
    );
    assert_eq!(
        core.installed_importer_version(GameCode::Gimi)
            .await
            .expect("read version"),
        None,
        "nothing was recorded, and GMM must not claim otherwise",
    );
    assert_eq!(
        core.installed_importer_origin(GameCode::Gimi)
            .await
            .expect("read origin"),
        InstalledOrigin::Unknown,
    );
}
