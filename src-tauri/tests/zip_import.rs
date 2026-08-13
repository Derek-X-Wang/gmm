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

// ---------------------------------------------------------------------
// Adversarial archive shapes (issue #60).
//
// GameBanana archives are arbitrary user uploads. Everything below is a
// shape a malicious or merely sloppy one can contain, and the rule for
// each is the same: extract safely, or refuse with an error that says
// what was wrong. Never write outside the import target, never leave a
// half-extracted Mod behind.
//
// These are host-runnable even for the NTFS-specific shapes, because the
// defence lives in the planner — the archive is rejected before any path
// reaches the OS, so the assertion holds identically on every platform.
// Gating them to Windows would only mean testing them less often.
// ---------------------------------------------------------------------

/// Import `entries` as a Mod and return the error, asserting that
/// nothing was left behind for the caller to clean up. Every refusal
/// test below shares this shape.
async fn refuse(entries: &[(&str, &[u8])], name: &str) -> String {
    let tmp = TempDir::new().expect("tmp");
    let (core, library_root) = fresh_core(&tmp).await;
    let zip_path = tmp.path().join("adversarial.zip");
    build_zip(&zip_path, entries);

    let err = core
        .import_zip(GameCode::Gimi, &zip_path, name, ImportZipOptions::default())
        .await
        .expect_err("this archive shape must be refused");

    let game_dir = library_root.join(GameCode::Gimi.as_str());
    let leftover = game_dir.read_dir().map(|d| d.count()).unwrap_or(0);
    assert_eq!(
        leftover, 0,
        "a refused archive must leave no partial Library subtree",
    );
    err.to_string()
}

/// `..\..\` is traversal on Windows and an ordinary filename byte
/// everywhere else. Reading entry names with the host's path rules means
/// the same archive escapes on one platform and produces a file with a
/// silly name on another; the planner normalises backslashes so the
/// verdict is the same everywhere.
#[tokio::test]
async fn backslash_traversal_is_refused_like_forward_slash() {
    let msg = refuse(
        &[
            (r"..\..\..\escape.txt", b"never" as &[u8]),
            ("merged.ini", b"[TextureOverride]\nhash=1\n"),
        ],
        "Backslash Escape",
    )
    .await;
    assert!(
        msg.contains("zip-slip"),
        "backslash traversal should be reported as zip-slip, got: {msg}",
    );
}

/// An absolute POSIX path. `enclosed_name` already rejects these, but
/// they are cheap to keep honest.
#[tokio::test]
async fn an_absolute_entry_path_is_refused() {
    let msg = refuse(
        &[("/etc/passwd", b"never" as &[u8]), ("merged.ini", b"x")],
        "Absolute Path",
    )
    .await;
    assert!(msg.contains("zip-slip"), "got: {msg}");
}

/// A drive-qualified path. On Windows `C:\Windows\System32\x.dll` is an
/// absolute write outside the target; elsewhere it is one long filename.
#[tokio::test]
async fn a_drive_qualified_entry_path_is_refused() {
    let msg = refuse(
        &[
            (r"C:\Windows\System32\evil.dll", b"never" as &[u8]),
            ("merged.ini", b"x"),
        ],
        "Drive Path",
    )
    .await;
    assert!(
        msg.contains("zip-slip") || msg.contains("drive-qualified"),
        "got: {msg}",
    );
}

/// A UNC path reaches another machine entirely.
#[tokio::test]
async fn a_unc_entry_path_is_refused() {
    let msg = refuse(
        &[
            (r"\\attacker\share\evil.dll", b"never" as &[u8]),
            ("merged.ini", b"x"),
        ],
        "UNC Path",
    )
    .await;
    assert!(
        msg.contains("zip-slip") || msg.contains("unsafe"),
        "got: {msg}",
    );
}

/// `merged.ini:hidden` writes an NTFS alternate data stream on
/// `merged.ini` rather than a file — content that never shows up in a
/// directory listing, inside a Mod the user thinks they can read.
#[tokio::test]
async fn an_alternate_data_stream_name_is_refused() {
    let msg = refuse(
        &[
            ("merged.ini:payload", b"hidden" as &[u8]),
            ("merged.ini", b"x"),
        ],
        "ADS Mod",
    )
    .await;
    assert!(
        msg.contains("unsafe") || msg.contains("stream"),
        "the error should explain the stream name, got: {msg}",
    );
}

/// NTFS silently strips trailing dots and spaces, so `merged.ini.` and
/// `merged.ini ` both land on `merged.ini` — two archive entries, one
/// file, last one wins.
#[tokio::test]
async fn trailing_dots_and_spaces_are_refused() {
    for name in ["merged.ini.", "merged.ini ", "textures./body.dds"] {
        let msg = refuse(&[(name, b"x" as &[u8])], "Trailing Junk").await;
        assert!(
            msg.contains("unsafe"),
            "{name} should be refused as unsafe, got: {msg}",
        );
    }
}

/// Reserved DOS device names are unusable as *any* path component, not
/// just the last one: `CON/body.dds` cannot be created on Windows at
/// all, and the OS error it produces explains nothing.
#[tokio::test]
async fn a_reserved_dos_name_in_an_intermediate_component_is_refused() {
    let msg = refuse(
        &[("CON/body.dds", b"x" as &[u8]), ("merged.ini", b"y")],
        "Reserved Component",
    )
    .await;
    assert!(
        msg.contains("reserved"),
        "the error should name the reserved component, got: {msg}",
    );
}

/// Distinct entries in the zip, one file on NTFS: the second silently
/// overwrites the first, so what the user gets depends on archive order
/// rather than on anything they can see.
#[tokio::test]
async fn case_insensitively_colliding_entries_are_refused() {
    let msg = refuse(
        &[("merged.ini", b"first" as &[u8]), ("MERGED.INI", b"second")],
        "Case Collision",
    )
    .await;
    assert!(
        msg.contains("collide") || msg.contains("collision"),
        "the error should explain the collision, got: {msg}",
    );
}

/// The same collision one directory up: `Textures/` and `textures/` are
/// two prefixes in the archive and one directory on disk.
#[tokio::test]
async fn case_insensitively_colliding_directories_are_refused() {
    let msg = refuse(
        &[
            ("Textures/body.dds", b"first" as &[u8]),
            ("textures/body.dds", b"second"),
        ],
        "Case Collision Dir",
    )
    .await;
    assert!(
        msg.contains("collide") || msg.contains("collision"),
        "the error should explain the collision, got: {msg}",
    );
}

/// A zip can carry a Unix symlink: mode bits say "link", and the entry's
/// content is the target path. Extracting it as an ordinary file writes
/// a Mod that lies about what it contains; honouring it would let an
/// archive point into the game directory. Refuse either way.
#[tokio::test]
async fn a_symlink_entry_is_refused() {
    let tmp = TempDir::new().expect("tmp");
    let (core, library_root) = fresh_core(&tmp).await;
    let zip_path = tmp.path().join("symlink.zip");

    {
        use std::io::Write as _;
        let file = File::create(&zip_path).expect("create zip");
        let mut zw = ZipWriter::new(file);
        // The real thing: `add_symlink` sets S_IFLNK in the entry's
        // mode bits, which is what `zip -y` produces and what the
        // planner looks for.
        zw.add_symlink(
            "link.ini",
            "../../../../etc/passwd",
            SimpleFileOptions::default(),
        )
        .expect("add symlink entry");
        zw.start_file("merged.ini", SimpleFileOptions::default())
            .expect("start file");
        zw.write_all(b"[TextureOverride]\nhash=1\n").expect("write");
        zw.finish().expect("finalise");
    }

    let err = core
        .import_zip(
            GameCode::Gimi,
            &zip_path,
            "Symlink Mod",
            ImportZipOptions::default(),
        )
        .await
        .expect_err("a symlink entry must be refused");
    assert!(
        err.to_string().contains("symlink"),
        "the error should name the symlink, got: {err}",
    );

    let game_dir = library_root.join(GameCode::Gimi.as_str());
    assert_eq!(
        game_dir.read_dir().map(|d| d.count()).unwrap_or(0),
        0,
        "a refused archive must leave no partial Library subtree",
    );
}

/// A zip bomb is a compression *ratio*, and the guard is a size cap —
/// so a legitimately compressible Mod (8 MiB of texture padding down to
/// a few KiB) must still import, while the same bytes against a smaller
/// cap must not. Testing both directions keeps the guard from drifting
/// into "refuse anything that compresses well", which would reject real
/// mods.
#[tokio::test]
async fn a_high_compression_ratio_is_judged_by_size_not_by_ratio() {
    // 8 MiB of zeroes: a ~1000:1 ratio, the shape of a zip bomb.
    let payload = vec![0u8; 8 * 1024 * 1024];

    {
        let tmp = TempDir::new().expect("tmp");
        let (core, _library_root) = fresh_core(&tmp).await;
        let zip_path = tmp.path().join("compressible.zip");
        build_zip(&zip_path, &[("blob.bin", &payload)]);
        let compressed = std::fs::metadata(&zip_path).expect("stat zip").len();
        assert!(
            compressed < payload.len() as u64 / 100,
            "fixture must actually be highly compressible, got {compressed} bytes",
        );

        let imported = core
            .import_zip(
                GameCode::Gimi,
                &zip_path,
                "Compressible",
                ImportZipOptions::default(),
            )
            .await
            .expect("a compressible Mod under the cap must import");
        assert_eq!(
            std::fs::metadata(imported.library_path.join("blob.bin"))
                .expect("stat extracted blob")
                .len(),
            payload.len() as u64,
            "the whole entry must be extracted, not a truncated prefix",
        );
    }

    let tmp = TempDir::new().expect("tmp");
    let (core, library_root) = fresh_core(&tmp).await;
    let zip_path = tmp.path().join("bomb.zip");
    build_zip(&zip_path, &[("blob.bin", &payload)]);

    let err = core
        .import_zip(
            GameCode::Gimi,
            &zip_path,
            "Bomb",
            ImportZipOptions {
                max_uncompressed_bytes: 64 * 1024,
                max_entries: 10_000,
            },
        )
        .await
        .expect_err("the same archive must be refused once it exceeds the cap");
    assert!(
        err.to_string().contains("import limit"),
        "the error should name the limit, got: {err}",
    );

    let game_dir = library_root.join(GameCode::Gimi.as_str());
    assert_eq!(
        game_dir.read_dir().map(|d| d.count()).unwrap_or(0),
        0,
        "a refused archive must leave no partial Library subtree",
    );
}

/// Failure partway through extraction: an entry names a directory whose
/// path an earlier entry already occupies as a file. Some of the Mod is
/// on disk when it fails, which is exactly when a leftover would slip
/// through.
#[tokio::test]
async fn a_failure_partway_through_extraction_leaves_nothing_behind() {
    let tmp = TempDir::new().expect("tmp");
    let (core, library_root) = fresh_core(&tmp).await;
    let zip_path = tmp.path().join("torn.zip");

    build_zip(
        &zip_path,
        &[
            ("merged.ini", b"[TextureOverride]\nhash=1\n" as &[u8]),
            ("body", b"a file, not a directory"),
            // `body` is already a file, so creating `body/` fails after
            // two entries have been written.
            ("body/texture.dds", b"never"),
        ],
    );

    let err = core
        .import_zip(
            GameCode::Gimi,
            &zip_path,
            "Torn Mod",
            ImportZipOptions::default(),
        )
        .await
        .expect_err("extraction must fail once an entry collides with a file");
    let _ = err;

    let game_dir = library_root.join(GameCode::Gimi.as_str());
    assert_eq!(
        game_dir.read_dir().map(|d| d.count()).unwrap_or(0),
        0,
        "a Mod that failed halfway through extraction must not survive",
    );
    let listed = core.list_mods(GameCode::Gimi).await.expect("list mods");
    assert!(
        listed.is_empty(),
        "a Mod that failed to extract must not be recorded, got: {listed:?}",
    );
}
