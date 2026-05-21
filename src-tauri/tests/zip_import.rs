//! Slice 1b: local ZIP import.
//!
//! Tracer-bullet test plus one test per acceptance criterion:
//! - happy path: round-trip a clean zip → Mod with Source=Local
//! - single-root normalisation: archive with one top-level dir collapses
//! - multi-root archive: contents become the Mod root verbatim
//! - junk-file drop: __MACOSX/, .DS_Store, Thumbs.db never land on disk
//! - zip-slip refusal: malicious `../` entries abort with cleanup
//! - size cap: oversize archives refused with cleanup
//! - entry cap: too-many-entry archives refused with cleanup
//!
//! No Tauri runtime; these exercise the pure-Rust `Core` API directly.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use gmm_lib::core::{Core, GameCode, ImportZipOptions, Source};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

/// Build a zip on disk from a slice of (path, bytes) entries. Paths use
/// forward slashes (per the zip spec) and directories end in `/`.
fn build_zip(zip_path: &Path, entries: &[(&str, &[u8])]) {
    let file = File::create(zip_path).expect("create zip file");
    let mut zw = ZipWriter::new(file);
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for (name, bytes) in entries {
        if name.ends_with('/') {
            zw.add_directory(*name, opts).expect("add dir");
        } else {
            zw.start_file(*name, opts).expect("start file");
            zw.write_all(bytes).expect("write entry bytes");
        }
    }
    zw.finish().expect("finalise zip");
}

async fn fresh_core(tmp: &TempDir) -> (Core, std::path::PathBuf) {
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root.clone(), &db_url)
        .await
        .expect("init core");
    (core, library_root)
}

#[tokio::test]
async fn import_zip_happy_path_round_trip() {
    let tmp = TempDir::new().expect("tmp");
    let (core, library_root) = fresh_core(&tmp).await;

    let zip_path = tmp.path().join("hutao.zip");
    build_zip(
        &zip_path,
        &[("merged.ini", b"[TextureOverride]\nhash=12345\n" as &[u8])],
    );

    let imported = core
        .import_zip(
            GameCode::Gimi,
            &zip_path,
            "Hu Tao Outfit",
            ImportZipOptions::default(),
        )
        .await
        .expect("import");

    assert_eq!(imported.name, "Hu Tao Outfit");
    assert_eq!(imported.game, GameCode::Gimi);
    assert_eq!(imported.source, Source::Local);
    assert!(!imported.enabled);
    assert!(
        imported.library_path.starts_with(&library_root),
        "library_path should live under library_root, got {:?}",
        imported.library_path,
    );
    assert!(
        imported.library_path.join("merged.ini").exists(),
        "merged.ini should be extracted into the Mod's library path",
    );

    let listed = core.list_mods(GameCode::Gimi).await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].source, Source::Local);
}

#[tokio::test]
async fn single_top_level_directory_is_stripped() {
    let tmp = TempDir::new().expect("tmp");
    let (core, _) = fresh_core(&tmp).await;

    let zip_path = tmp.path().join("nested.zip");
    build_zip(
        &zip_path,
        &[
            ("HuTaoSkin/", b"" as &[u8]),
            ("HuTaoSkin/merged.ini", b"[TextureOverride]\nhash=abc\n"),
            ("HuTaoSkin/preview.png", b"PNGDATA"),
        ],
    );

    let imported = core
        .import_zip(
            GameCode::Gimi,
            &zip_path,
            "Hu Tao Skin",
            ImportZipOptions::default(),
        )
        .await
        .expect("import");

    assert!(
        imported.library_path.join("merged.ini").exists(),
        "single-root normalisation should collapse HuTaoSkin/ — merged.ini must sit at the Mod root",
    );
    assert!(
        imported.library_path.join("preview.png").exists(),
        "preview.png should also be at the Mod root after normalisation",
    );
    assert!(
        !imported.library_path.join("HuTaoSkin").exists(),
        "the redundant outer directory must not be present after normalisation",
    );
}

#[tokio::test]
async fn multi_root_archive_keeps_contents_verbatim() {
    let tmp = TempDir::new().expect("tmp");
    let (core, _) = fresh_core(&tmp).await;

    let zip_path = tmp.path().join("multi.zip");
    build_zip(
        &zip_path,
        &[
            ("merged.ini", b"[TextureOverride]\nhash=1\n" as &[u8]),
            ("readme.txt", b"please install"),
            ("textures/skin.dds", b"DDSDATA"),
        ],
    );

    let imported = core
        .import_zip(
            GameCode::Gimi,
            &zip_path,
            "Multi Root",
            ImportZipOptions::default(),
        )
        .await
        .expect("import");

    assert!(imported.library_path.join("merged.ini").exists());
    assert!(imported.library_path.join("readme.txt").exists());
    assert!(imported.library_path.join("textures/skin.dds").exists());
}

#[tokio::test]
async fn junk_files_are_dropped_on_import() {
    let tmp = TempDir::new().expect("tmp");
    let (core, _) = fresh_core(&tmp).await;

    let zip_path = tmp.path().join("junk.zip");
    build_zip(
        &zip_path,
        &[
            ("merged.ini", b"[TextureOverride]\nhash=1\n" as &[u8]),
            ("__MACOSX/", b""),
            ("__MACOSX/._merged.ini", b"resource fork"),
            (".DS_Store", b"finder gunk"),
            ("Thumbs.db", b"explorer gunk"),
            ("textures/.DS_Store", b"nested finder gunk"),
        ],
    );

    let imported = core
        .import_zip(
            GameCode::Gimi,
            &zip_path,
            "Junk Drop",
            ImportZipOptions::default(),
        )
        .await
        .expect("import");

    assert!(imported.library_path.join("merged.ini").exists());
    assert!(!imported.library_path.join("__MACOSX").exists());
    assert!(!imported.library_path.join(".DS_Store").exists());
    assert!(!imported.library_path.join("Thumbs.db").exists());
    assert!(!imported.library_path.join("textures/.DS_Store").exists());
}

#[tokio::test]
async fn zip_slip_entry_aborts_with_cleanup() {
    let tmp = TempDir::new().expect("tmp");
    let (core, library_root) = fresh_core(&tmp).await;

    // Hand-craft a zip whose central directory names a zip-slip entry.
    // `ZipWriter::start_file` happily writes `../` names — that's how
    // real attackers ship these.
    let zip_path = tmp.path().join("evil.zip");
    build_zip(
        &zip_path,
        &[
            (
                "../../../escape.txt",
                b"this should never land on disk" as &[u8],
            ),
            ("merged.ini", b"[TextureOverride]\nhash=1\n"),
        ],
    );

    let err = core
        .import_zip(
            GameCode::Gimi,
            &zip_path,
            "Evil Mod",
            ImportZipOptions::default(),
        )
        .await
        .expect_err("zip-slip import must fail");

    // Error must mention zip-slip so the UI can surface the right copy.
    let msg = err.to_string();
    assert!(
        msg.contains("zip-slip"),
        "error should mention zip-slip, got: {msg}",
    );

    // Nothing under the library root must exist for this Mod.
    let game_dir = library_root.join(GameCode::Gimi.as_str());
    let leftover = game_dir.read_dir().map(|d| d.count()).unwrap_or(0);
    assert_eq!(
        leftover, 0,
        "no partial Library subtree may remain after a zip-slip refusal",
    );

    // The escape target must not exist anywhere on disk.
    let escape_candidate = tmp.path().join("escape.txt");
    assert!(
        !escape_candidate.exists(),
        "escape.txt must never be written outside the target",
    );

    // No Mod row was inserted.
    let listed = core.list_mods(GameCode::Gimi).await.expect("list");
    assert_eq!(listed.len(), 0);
}

#[tokio::test]
async fn entry_cap_refuses_too_many_entries_with_cleanup() {
    let tmp = TempDir::new().expect("tmp");
    let (core, library_root) = fresh_core(&tmp).await;

    let zip_path = tmp.path().join("many.zip");
    // Five tiny files, cap at 3.
    let entries: Vec<(String, Vec<u8>)> = (0..5)
        .map(|i| (format!("file_{i}.ini"), b"hash=1\n".to_vec()))
        .collect();
    let refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    build_zip(&zip_path, &refs);

    let opts = ImportZipOptions {
        max_entries: 3,
        ..ImportZipOptions::default()
    };

    let err = core
        .import_zip(GameCode::Gimi, &zip_path, "Too Many", opts)
        .await
        .expect_err("entry-cap must refuse the archive");

    let msg = err.to_string();
    assert!(
        msg.contains('3') && msg.contains('5'),
        "entry-cap error should cite both cap and actual, got: {msg}",
    );

    let game_dir = library_root.join(GameCode::Gimi.as_str());
    let leftover = game_dir.read_dir().map(|d| d.count()).unwrap_or(0);
    assert_eq!(leftover, 0, "no partial subtree after entry-cap refusal");
}

#[tokio::test]
async fn size_cap_refuses_oversize_archive_with_cleanup() {
    let tmp = TempDir::new().expect("tmp");
    let (core, library_root) = fresh_core(&tmp).await;

    let zip_path = tmp.path().join("big.zip");
    // 512 bytes of payload, but we'll cap the import at 100 bytes.
    let payload = vec![b'A'; 512];
    build_zip(&zip_path, &[("blob.bin", &payload)]);

    let opts = ImportZipOptions {
        max_uncompressed_bytes: 100,
        ..ImportZipOptions::default()
    };

    let err = core
        .import_zip(GameCode::Gimi, &zip_path, "Big Mod", opts)
        .await
        .expect_err("size-cap must refuse the archive");

    let msg = err.to_string();
    assert!(
        msg.contains("100") && msg.contains("512"),
        "size-cap error should cite both cap and actual, got: {msg}",
    );

    let game_dir = library_root.join(GameCode::Gimi.as_str());
    let leftover = game_dir.read_dir().map(|d| d.count()).unwrap_or(0);
    assert_eq!(leftover, 0, "no partial subtree after size-cap refusal");

    let listed = core.list_mods(GameCode::Gimi).await.expect("list");
    assert_eq!(listed.len(), 0);
}
