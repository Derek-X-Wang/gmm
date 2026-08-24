//! Atomic Mod/Variant persistence for every Library ingest path (#186).
//!
//! Variant detection is an unbounded filesystem traversal, so adopt and ZIP
//! import complete it before acquiring the Library writer fence. These tests
//! pin both sides of that staged design: ordinary detection failures never
//! expose a referenced Mod, and successful adopt, ZIP, GameBanana, and
//! recovery paths persist the same Variant shape and initial selection.

use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gmm_lib::core::{crash_points, Core, Error, GameCode, Mod};
use tempfile::TempDir;
use ulid::Ulid;
use zip::write::SimpleFileOptions;

async fn fresh_core(tmp: &TempDir) -> Core {
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    Core::new(library_root, &db_url).await.expect("init core")
}

fn write_tree(root: &Path, files: &[(&str, &[u8])]) {
    for (relative, contents) in files {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("fixture directory");
        fs::write(path, contents).expect("fixture file");
    }
}

fn build_zip(path: &Path, files: &[(&str, &[u8])]) {
    let file = File::create(path).expect("create ZIP");
    let mut archive = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for (relative, contents) in files {
        archive.start_file(relative, options).expect("ZIP entry");
        archive.write_all(contents).expect("ZIP contents");
    }
    archive.finish().expect("finish ZIP");
}

async fn import_gamebanana_archive(
    core: &Core,
    archive_path: &Path,
    id: u64,
) -> gmm_lib::core::Result<Mod> {
    let archive_bytes = fs::read(archive_path).expect("GameBanana ZIP bytes");
    let api_path = format!("/apiv11/Mod/{id}");
    let file_path = format!("/file/{id}/mod.zip");
    let mut server = mockito::Server::new_async().await;
    let _api = server
        .mock("GET", mockito::Matcher::Regex(format!("{api_path}.*")))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
                "_idRow": {id}, "_sName": "Equivalent GameBanana",
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

    core.import_gamebanana_with_endpoints(
        GameCode::Gimi,
        &id.to_string(),
        &gmm_lib::core::gamebanana::Endpoints {
            api_base: server.url(),
        },
    )
    .await
}

#[derive(Debug, PartialEq, Eq)]
struct RecordedShape {
    variants: Vec<(String, PathBuf)>,
    active_name: Option<String>,
}

async fn recorded_shape(core: &Core, imported: &Mod) -> RecordedShape {
    let variants = core
        .list_variants(&imported.id)
        .await
        .expect("list recorded Variants");
    let active_id = core
        .active_variant_id(&imported.id)
        .await
        .expect("read active Variant");
    let active_name = active_id.map(|active_id| {
        variants
            .iter()
            .find(|variant| variant.id == active_id)
            .expect("active Variant belongs to the Mod")
            .name
            .clone()
    });
    RecordedShape {
        variants: variants
            .into_iter()
            .map(|variant| (variant.name, variant.subpath))
            .collect(),
        active_name,
    }
}

async fn import_all_four_shapes(
    tmp: &TempDir,
    core: &Core,
    files: &[(&str, &[u8])],
) -> Vec<RecordedShape> {
    let source = tmp.path().join(format!("adopt-source-{}", Ulid::new()));
    write_tree(&source, files);
    let adopted = core
        .adopt_folder(GameCode::Gimi, &source, "Equivalent Adopt")
        .await
        .expect("adopt fixture");

    let archive = tmp.path().join(format!("equivalent-{}.zip", Ulid::new()));
    build_zip(&archive, files);
    let imported = core
        .import_zip(
            GameCode::Gimi,
            &archive,
            "Equivalent ZIP",
            Default::default(),
        )
        .await
        .expect("import ZIP fixture");
    let gamebanana = import_gamebanana_archive(core, &archive, 186_100)
        .await
        .expect("import GameBanana fixture");

    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("resolved game Library root");
    let orphan = root.join(Ulid::new().to_string());
    write_tree(&orphan, files);
    let recovered = core
        .recover_unreferenced_library_dir(GameCode::Gimi, &orphan, "Equivalent Recovery")
        .await
        .expect("recover fixture");

    let mut shapes = Vec::new();
    for imported in [&adopted, &imported, &gamebanana, &recovered] {
        shapes.push(recorded_shape(core, imported).await);
    }
    shapes
}

#[tokio::test]
async fn all_four_ingest_paths_record_the_same_multi_variant_shape() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let files: &[(&str, &[u8])] = &[
        ("Red/merged.ini", b"hash=Red\n"),
        ("Blue/merged.ini", b"hash=Blue\n"),
        ("Green/merged.ini", b"hash=Green\n"),
    ];

    let shapes = import_all_four_shapes(&tmp, &core, files).await;
    let expected = RecordedShape {
        variants: vec![
            ("Blue".to_string(), PathBuf::from("Blue")),
            ("Green".to_string(), PathBuf::from("Green")),
            ("Red".to_string(), PathBuf::from("Red")),
        ],
        active_name: Some("Blue".to_string()),
    };
    for (path, shape) in ["adopt", "ZIP", "GameBanana", "recovery"]
        .into_iter()
        .zip(shapes)
    {
        assert_eq!(shape, expected, "{path} recorded a different Variant shape");
    }
}

#[tokio::test]
async fn all_four_ingest_paths_record_the_same_zero_variant_shape() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    // Root-level INI wins over otherwise Variant-looking subdirectories: this
    // is a single-folder Mod and every path must persist zero Variant rows.
    let shapes = import_all_four_shapes(
        &tmp,
        &core,
        &[
            ("merged.ini", b"hash=root\n"),
            ("PreviewA/merged.ini", b"hash=preview-a\n"),
            ("PreviewB/merged.ini", b"hash=preview-b\n"),
        ],
    )
    .await;
    let expected = RecordedShape {
        variants: Vec::new(),
        active_name: None,
    };
    for (path, shape) in ["adopt", "ZIP", "GameBanana", "recovery"]
        .into_iter()
        .zip(shapes)
    {
        assert_eq!(
            shape, expected,
            "{path} recorded a different zero-Variant shape"
        );
    }
}

#[cfg(unix)]
fn inject_detection_error(
    core: Core,
    game_root: PathBuf,
    after_filesystem_step: &'static str,
) -> (Core, Arc<Mutex<Option<PathBuf>>>) {
    use std::os::unix::fs::PermissionsExt as _;

    let inaccessible = Arc::new(Mutex::new(None));
    let observed = Arc::clone(&inaccessible);
    let hooked = core.with_crash_hook(Arc::new(move |point| {
        if point != after_filesystem_step {
            return;
        }
        let staged = fs::read_dir(&game_root)
            .expect("read staged game root")
            .map(|entry| entry.expect("staged entry").path())
            .find(|path| path.is_dir())
            .expect("one staged Mod directory");
        let path = staged.join("Red");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o0))
            .expect("make one copied Variant unreadable before detection");
        *observed.lock().expect("inaccessible path lock") = Some(path);
    }));
    (hooked, inaccessible)
}

#[cfg(unix)]
fn restore_detection_fixture(inaccessible: &Arc<Mutex<Option<PathBuf>>>) {
    use std::os::unix::fs::PermissionsExt as _;

    if let Some(path) = inaccessible
        .lock()
        .expect("inaccessible path lock")
        .as_ref()
        .filter(|path| path.exists())
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .expect("restore unreadable Variant fixture");
    }
}

#[cfg(unix)]
async fn assert_detection_error_left_no_mod(core: &Core, result: gmm_lib::core::Result<Mod>) {
    assert!(
        matches!(result, Err(Error::Io { ref path, .. }) if path.ends_with("Red")),
        "Variant detection must surface the injected unreadable subtree, got {result:?}",
    );
    let mods = core.list_mods(GameCode::Gimi).await.expect("list Mods");
    assert!(
        mods.is_empty(),
        "a Variant-detection error must not expose a referenced Mod: {mods:?}",
    );
}

#[cfg(unix)]
#[tokio::test]
async fn adopt_variant_detection_error_leaves_no_referenced_mod() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let source = tmp.path().join("adopt-detection-error");
    write_tree(
        &source,
        &[
            ("Blue/merged.ini", b"hash=Blue\n"),
            ("Red/merged.ini", b"hash=Red\n"),
        ],
    );
    let game_root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game root");
    let (hooked, inaccessible) = inject_detection_error(
        core.clone(),
        game_root,
        crash_points::ADOPT_AFTER_LIBRARY_COPY,
    );

    let result = hooked
        .adopt_folder(GameCode::Gimi, &source, "Detection Error Adopt")
        .await;
    restore_detection_fixture(&inaccessible);
    assert_detection_error_left_no_mod(&core, result).await;
}

#[cfg(unix)]
#[tokio::test]
async fn zip_variant_detection_error_leaves_no_referenced_mod() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let archive = tmp.path().join("zip-detection-error.zip");
    build_zip(
        &archive,
        &[
            ("Blue/merged.ini", b"hash=Blue\n"),
            ("Red/merged.ini", b"hash=Red\n"),
        ],
    );
    let game_root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game root");
    let (hooked, inaccessible) = inject_detection_error(
        core.clone(),
        game_root,
        crash_points::IMPORT_ZIP_AFTER_EXTRACT,
    );

    let result = hooked
        .import_zip(
            GameCode::Gimi,
            &archive,
            "Detection Error ZIP",
            Default::default(),
        )
        .await;
    restore_detection_fixture(&inaccessible);
    assert_detection_error_left_no_mod(&core, result).await;
}

#[cfg(unix)]
#[tokio::test]
async fn gamebanana_variant_detection_error_leaves_no_referenced_mod() {
    let tmp = TempDir::new().expect("tmp");
    let core = fresh_core(&tmp).await;
    let archive = tmp.path().join("gamebanana-detection-error.zip");
    build_zip(
        &archive,
        &[
            ("Blue/merged.ini", b"hash=Blue\n"),
            ("Red/merged.ini", b"hash=Red\n"),
        ],
    );
    let game_root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game root");
    let (hooked, inaccessible) = inject_detection_error(
        core.clone(),
        game_root,
        crash_points::IMPORT_ZIP_AFTER_EXTRACT,
    );

    let result = import_gamebanana_archive(&hooked, &archive, 186_200).await;
    restore_detection_fixture(&inaccessible);
    assert_detection_error_left_no_mod(&core, result).await;
}
