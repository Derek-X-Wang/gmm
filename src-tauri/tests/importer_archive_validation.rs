//! Issue #113: an archive is checked for the structural shape of a Model
//! Importer *before* anything in the game directory is touched.
//!
//! The older unwitnessed local-ZIP path validated nothing. Any zip — a mod,
//! an unrelated archive — extracted into the game directory, replaced the
//! existing importer, and was reported as a successful install with a
//! version recorded against it. `rollback_to` could undo it, but only if
//! the user realised what had happened.
//!
//! The required shape is derived from
//! `tests/fixtures/importer_package_layouts.json`, a recording of the
//! real entry listings of all six live packages across their three
//! maintainers. See `the_recorded_packages_are_what_the_shape_rule_was_derived_from`
//! for what those recordings actually agree on — notably ZZMI ships no
//! `Mods/` at all, and EFMI and WWMI ship an empty `ShaderFixes/`, both of
//! which contradict the shape the issue's brief assumed.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use gmm_lib::core::error::Error;
use gmm_lib::core::importer::{
    find_d3dx_ini, install_from_local_zip_unwitnessed_for_test, validate_importer_archive,
    DEFAULT_LOADER_EXE,
};
use serde::Deserialize;
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

/// One live Model Importer package's entry listing, recorded verbatim
/// from the real release asset.
///
/// Recorded rather than written: the brief for #113 said an importer
/// ships `d3dx.ini`, `Core/`, `ShaderFixes/` and an **empty `Mods/`**,
/// and it said to verify that against real packages before fixing the
/// rule. Two of those real packages disagree with the brief — see
/// [`the_recorded_packages_are_what_the_shape_rule_was_derived_from`].
#[derive(Debug, Deserialize)]
struct RecordedLayout {
    game: String,
    repo: String,
    asset: String,
    entries: Vec<String>,
}

fn recorded_layouts() -> Vec<RecordedLayout> {
    serde_json::from_str(include_str!("fixtures/importer_package_layouts.json"))
        .expect("recorded package layouts must be valid JSON")
}

/// Rebuild a zip with a recorded package's exact entry paths. Structural
/// validation reads names only, so the file contents are stand-ins; the
/// *shape* is the recording.
fn build_zip_from_entries(zip_path: &Path, entries: &[String]) {
    let mut zw = ZipWriter::new(File::create(zip_path).expect("create zip"));
    for entry in entries {
        if let Some(dir) = entry.strip_suffix('/') {
            zw.add_directory(dir, opts()).expect("add directory");
        } else {
            zw.start_file(entry, opts()).expect("start file");
            zw.write_all(b"; recorded package entry\n")
                .expect("write entry");
        }
    }
    zw.finish().expect("finish zip");
}

fn opts() -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)
}

/// A zip that is plainly not a Model Importer: a GameBanana-style mod.
fn build_mod_zip(zip_path: &Path) {
    let mut zw = ZipWriter::new(File::create(zip_path).expect("create zip"));
    zw.start_file("merged.ini", opts()).expect("ini");
    zw.write_all(b"[TextureOverride]\nhash = deadbeef\n")
        .expect("write ini");
    zw.start_file("body.dds", opts()).expect("dds");
    zw.write_all(b"DDS ").expect("write dds");
    zw.finish().expect("finish zip");
}

/// A snapshot of every file under `root`, path → bytes, for byte-for-byte
/// comparison. Directories are recorded as paths with no content so an
/// added or removed empty directory is caught too.
fn snapshot(root: &Path) -> Vec<(String, Option<Vec<u8>>)> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, Option<Vec<u8>>)>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let rel = path
                .strip_prefix(base)
                .expect("under base")
                .to_string_lossy()
                .to_string();
            if path.is_dir() {
                out.push((rel, None));
                walk(&path, base, out);
            } else {
                out.push((rel, Some(fs::read(&path).expect("read file"))));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

#[test]
fn a_zip_that_is_not_an_importer_leaves_the_game_directory_byte_for_byte_unchanged() {
    let tmp = TempDir::new().expect("tmp");
    let game = tmp.path().join("Genshin");
    let backups = tmp.path().join("backups/gimi");

    // A working hand-install the user cares about, plus an enabled mod.
    fs::create_dir_all(game.join("Mods/Hu Tao Skin")).expect("mods");
    fs::write(game.join("Mods/Hu Tao Skin/merged.ini"), b"hash = 1234\n").expect("mod ini");
    fs::write(game.join("d3dx.ini"), b"[Loader]\nloader = gmm.exe\n").expect("d3dx");
    fs::write(game.join("d3d11.dll"), b"MZ\x00\x00real-loader").expect("dll");
    fs::create_dir_all(game.join("ShaderFixes")).expect("shaderfixes");
    fs::write(game.join("ShaderFixes/keep.hlsl"), b"// keep\n").expect("hlsl");
    let before = snapshot(&game);

    let zip = tmp.path().join("some-mod.zip");
    build_mod_zip(&zip);

    let error =
        install_from_local_zip_unwitnessed_for_test(&zip, &game, &backups, DEFAULT_LOADER_EXE)
            .expect_err("a mod archive must not install as a Model Importer");

    assert_eq!(
        snapshot(&game),
        before,
        "a rejected archive must leave the game directory byte-for-byte \
         unchanged; validation has to run before backup_existing_unwitnessed_for_test so there is \
         nothing to roll back. Error was: {error}"
    );
    assert!(
        !backups.exists(),
        "no backup directory may be created for a rejected archive"
    );
}

#[test]
fn every_live_model_importer_package_still_installs() {
    // Six packages, three maintainers. If the shape rule is too strict
    // for any real package, GMM stops being able to install that game's
    // importer at all — a worse failure than the one #113 fixes.
    let layouts = recorded_layouts();
    assert_eq!(layouts.len(), 6, "all six games must be recorded");

    for layout in &layouts {
        let tmp = TempDir::new().expect("tmp");
        let game = tmp.path().join("game");
        let zip = tmp.path().join(&layout.asset);
        build_zip_from_entries(&zip, &layout.entries);

        let report = install_from_local_zip_unwitnessed_for_test(
            &zip,
            &game,
            &tmp.path().join("backups"),
            DEFAULT_LOADER_EXE,
        )
        .unwrap_or_else(|e| {
            panic!(
                "{}'s real package from {} no longer installs: {e}",
                layout.game, layout.repo
            )
        });

        assert!(
            report.rewrote_files.iter().any(|p| p.ends_with("d3dx.ini")),
            "{}: d3dx.ini must be rewritten, not skipped",
            layout.game
        );
        assert!(
            game.join("Core").is_dir() && game.join("ShaderFixes").is_dir(),
            "{}: the package's own directories must land",
            layout.game
        );
    }
}

#[test]
fn the_recorded_packages_are_what_the_shape_rule_was_derived_from() {
    // The brief for #113 described an importer as `d3dx.ini`, `Core/`,
    // `ShaderFixes/` and an empty `Mods/`. It also said to verify that
    // against real packages first. This test records where the real
    // packages disagree, so the rule's looseness reads as a finding
    // rather than an oversight.
    let layouts = recorded_layouts();

    for layout in &layouts {
        assert!(
            layout.entries.iter().any(|e| e == "d3dx.ini"),
            "{} ships no d3dx.ini",
            layout.game
        );
        for dir in ["Core/", "ShaderFixes/"] {
            assert!(
                layout.entries.iter().any(|e| e == dir),
                "{} ships no {dir}",
                layout.game
            );
        }
        assert!(
            !layout
                .entries
                .iter()
                .any(|e| e.ends_with(".dll") || e.ends_with(".exe")),
            "{} ships a compiled binary, which would break the no-binaries rule",
            layout.game
        );
    }

    let without_mods: Vec<&str> = layouts
        .iter()
        .filter(|l| !l.entries.iter().any(|e| e == "Mods/"))
        .map(|l| l.game.as_str())
        .collect();
    assert_eq!(
        without_mods,
        vec!["zzmi"],
        "ZZMI ships no Mods/ entry at all, which is why Mods/ is not required"
    );

    let with_empty_shaderfixes: Vec<&str> = layouts
        .iter()
        .filter(|l| {
            !l.entries
                .iter()
                .any(|e| e.starts_with("ShaderFixes/") && e != "ShaderFixes/")
        })
        .map(|l| l.game.as_str())
        .collect();
    assert_eq!(
        with_empty_shaderfixes,
        vec!["wwmi", "efmi"],
        "WWMI and EFMI ship ShaderFixes/ empty, which is why the required \
         directories are not required to have content"
    );
}

/// A minimal archive with the real shape, so each rejection test can
/// remove exactly one thing and attribute the refusal to it.
fn build_minimal_importer_zip(zip_path: &Path, skip: Option<&str>, extra: &[&str]) {
    let mut zw = ZipWriter::new(File::create(zip_path).expect("create zip"));
    for dir in ["Core/", "ShaderFixes/"] {
        if skip == Some(dir) {
            continue;
        }
        zw.add_directory(dir.trim_end_matches('/'), opts())
            .expect("dir");
    }
    if skip != Some("Core/") {
        zw.start_file("Core/library.ini", opts()).expect("core ini");
        zw.write_all(b"; core\n").expect("write core");
    }
    if skip != Some("d3dx.ini") {
        zw.start_file("d3dx.ini", opts()).expect("d3dx");
        zw.write_all(b"[Loader]\nloader = XXMI Launcher.exe\n")
            .expect("write d3dx");
    }
    for path in extra {
        zw.start_file(*path, opts()).expect("extra");
        zw.write_all(b"MZ\x00\x00").expect("write extra");
    }
    zw.finish().expect("finish zip");
}

#[test]
fn the_rejection_names_what_was_missing() {
    // "Wrong file" and "corrupt download" need different actions from the
    // user, so the message has to say which piece was absent.
    for (skip, needle) in [
        ("d3dx.ini", "d3dx.ini"),
        ("Core/", "Core/"),
        ("ShaderFixes/", "ShaderFixes/"),
    ] {
        let tmp = TempDir::new().expect("tmp");
        let zip = tmp.path().join("candidate.zip");
        build_minimal_importer_zip(&zip, Some(skip), &[]);

        let error = validate_importer_archive(&zip)
            .expect_err(&format!("an archive without {skip} is not an importer"));

        match error {
            Error::NotAModelImporter { missing, expected } => {
                assert!(
                    missing.contains(needle),
                    "the rejection must name {needle:?} so the user can tell a \
                     wrong file from a corrupt download, got {missing:?}"
                );
                assert!(
                    expected.contains("d3dx.ini") && expected.contains("Core/"),
                    "the rejection must also say what an importer looks like, \
                     got {expected:?}"
                );
            }
            other => panic!("expected a shape error naming {needle:?}, got {other:?}"),
        }
    }
}

#[test]
fn a_complete_minimal_package_is_accepted() {
    let tmp = TempDir::new().expect("tmp");
    let zip = tmp.path().join("candidate.zip");
    build_minimal_importer_zip(&zip, None, &[]);
    validate_importer_archive(&zip).expect("the real shape must be accepted");
}

#[test]
fn an_archive_carrying_a_compiled_binary_is_refused() {
    // A Model Importer is configuration and HLSL; the DLLs it drives ship
    // with the Loader (ADR 0001). An executable image here means the
    // archive is something else — and one that would land beside the game
    // executable.
    let tmp = TempDir::new().expect("tmp");
    let zip = tmp.path().join("with-dll.zip");
    build_minimal_importer_zip(&zip, None, &["d3d11.dll"]);

    match validate_importer_archive(&zip).expect_err("a binary must be refused") {
        Error::ImporterArchiveHasBinaries { entries } => {
            assert!(
                entries.contains("d3d11.dll"),
                "the error must name the offending entry, got {entries:?}"
            );
        }
        other => panic!("expected a binaries error, got {other:?}"),
    }
}

#[test]
fn a_package_zipped_inside_a_wrapper_folder_is_still_recognised() {
    // `zip_import::extract` collapses a redundant single root, so
    // validation has to see the same collapsed shape or a user who zipped
    // the folder rather than its contents gets a bogus rejection.
    let tmp = TempDir::new().expect("tmp");
    let zip = tmp.path().join("wrapped.zip");
    let mut zw = ZipWriter::new(File::create(&zip).expect("create zip"));
    zw.add_directory("GIMI-PACKAGE-v8.8.9", opts())
        .expect("root");
    zw.add_directory("GIMI-PACKAGE-v8.8.9/Core", opts())
        .expect("core");
    zw.start_file("GIMI-PACKAGE-v8.8.9/Core/library.ini", opts())
        .expect("core ini");
    zw.write_all(b"; core\n").expect("write");
    zw.add_directory("GIMI-PACKAGE-v8.8.9/ShaderFixes", opts())
        .expect("shaders");
    zw.start_file("GIMI-PACKAGE-v8.8.9/d3dx.ini", opts())
        .expect("d3dx");
    zw.write_all(b"[Loader]\nloader = XXMI Launcher.exe\n")
        .expect("write");
    zw.finish().expect("finish");

    validate_importer_archive(&zip)
        .expect("a wrapper folder is collapsed on extraction and must be on validation too");
}

#[test]
fn a_reinstall_backs_up_core_so_rollback_can_restore_it() {
    // Separate defect, found while deriving the shape rule from the real
    // packages: `Core/` is the largest thing every Model Importer ships,
    // and `backup_existing_unwitnessed_for_test` never captured it — `IMPORTER_ROOT_DIRS` was
    // written from the same wrong picture of a package as the old test
    // fixtures (d3d11.dll plus ShaderFixes). So a reinstall deleted the
    // previous `Core/` outright and "Roll back importer" could not bring
    // it back, quietly making the install path's own safety net partial.
    let tmp = TempDir::new().expect("tmp");
    let game = tmp.path().join("game");
    let backups = tmp.path().join("backups");

    let first = tmp.path().join("first.zip");
    build_minimal_importer_zip(&first, None, &[]);
    install_from_local_zip_unwitnessed_for_test(&first, &game, &backups, DEFAULT_LOADER_EXE)
        .expect("first install");
    let original_core = fs::read(game.join("Core/library.ini")).expect("read installed Core file");

    // A second install replaces Core/ wholesale.
    let second = tmp.path().join("second.zip");
    let mut zw = ZipWriter::new(File::create(&second).expect("create zip"));
    zw.add_directory("Core", opts()).expect("core");
    zw.start_file("Core/library.ini", opts()).expect("core ini");
    zw.write_all(b"; a different version\n").expect("write");
    zw.add_directory("ShaderFixes", opts()).expect("shaders");
    zw.start_file("d3dx.ini", opts()).expect("d3dx");
    zw.write_all(b"[Loader]\nloader = XXMI Launcher.exe\n")
        .expect("write");
    zw.finish().expect("finish");

    let report =
        install_from_local_zip_unwitnessed_for_test(&second, &game, &backups, DEFAULT_LOADER_EXE)
            .expect("reinstall");
    let backup_dir = report
        .backup_dir
        .expect("a reinstall over an existing importer must produce a backup");

    gmm_lib::core::importer::rollback_to(&backup_dir, &game).expect("rollback");

    assert_eq!(
        fs::read(game.join("Core/library.ini")).expect("read restored Core file"),
        original_core,
        "rolling back an importer must restore Core/, or the rollback the \
         install path promises is only partial"
    );
}

#[tokio::test]
async fn a_wrong_asset_from_an_origin_is_refused_the_same_way_a_local_zip_is() {
    // A misconfigured Importer Origin (ADR 0005) that resolves to the
    // wrong asset is exactly as damaging as a wrong local file, so the
    // check has to cover the downloaded path too. Both paths funnel
    // through `install_from_local_zip_unwitnessed_for_test`; this drives the download half
    // with the real `download_to` against a served payload.
    let tmp = TempDir::new().expect("tmp");
    let game = tmp.path().join("game");
    fs::create_dir_all(&game).expect("game dir");
    fs::write(game.join("d3dx.ini"), b"[Loader]\nloader = gmm.exe\n").expect("existing d3dx");
    let before = snapshot(&game);

    // Something an origin could plausibly hand back: a mod archive, or
    // any other zip attached to the release.
    let payload = tmp.path().join("served.zip");
    build_mod_zip(&payload);
    let bytes = fs::read(&payload).expect("read payload");

    let mut server = mockito::Server::new_async().await;
    let route = server
        .mock("GET", "/releases/download/v1.0.0/NOT-AN-IMPORTER.zip")
        .with_status(200)
        .with_header("content-type", "application/zip")
        .with_body(bytes)
        .create_async()
        .await;

    let downloaded = tmp.path().join("downloads/NOT-AN-IMPORTER.zip");
    let url = format!(
        "{}/releases/download/v1.0.0/NOT-AN-IMPORTER.zip",
        server.url()
    );
    let client = reqwest::Client::new();
    gmm_lib::core::importer::download_to(&client, &url, &downloaded)
        .await
        .expect("the download itself succeeds — the archive is the problem");
    route.assert_async().await;

    let error = install_from_local_zip_unwitnessed_for_test(
        &downloaded,
        &game,
        &tmp.path().join("backups"),
        DEFAULT_LOADER_EXE,
    )
    .expect_err("a downloaded archive that is not an importer must be refused");

    assert!(
        matches!(error, Error::NotAModelImporter { .. }),
        "expected the same structural rejection a local zip gets, got {error:?}"
    );
    assert_eq!(
        snapshot(&game),
        before,
        "the game directory must be untouched after a rejected download"
    );
}

#[test]
fn a_game_directory_with_no_d3dx_ini_is_a_contradiction_not_a_silent_skip() {
    // The `loader:` rewrite is the single most importer-specific action in
    // the whole install, and it used to be guarded by
    // `if d3dx.is_file()` — so the one step that proves the input was an
    // importer was also the one that quietly did nothing when it wasn't.
    // Validation now guarantees the file, so its absence here means
    // something went wrong during the swap; that must surface.
    let tmp = TempDir::new().expect("tmp");
    let game = tmp.path().join("game");
    fs::create_dir_all(&game).expect("game dir");

    let error = find_d3dx_ini(&game).expect_err("a missing d3dx.ini must be an error");
    assert!(
        error.to_string().contains("d3dx.ini"),
        "the error must name the file, got {error}"
    );
}

#[test]
fn d3dx_ini_is_found_whatever_case_the_package_wrote_it_in() {
    // Windows compares filenames case-insensitively, so an importer
    // shipping `D3DX.INI` works there and would have skipped the rewrite
    // on a case-sensitive filesystem — silently producing an install that
    // still points at XXMI's loader.
    let tmp = TempDir::new().expect("tmp");
    let game = tmp.path().join("game");
    fs::create_dir_all(&game).expect("game dir");
    fs::write(
        game.join("D3DX.INI"),
        b"[Loader]\nloader = XXMI Launcher.exe\n",
    )
    .expect("write");

    let found = find_d3dx_ini(&game).expect("an uppercase d3dx.ini is still d3dx.ini");
    assert_eq!(
        found.file_name().and_then(|n| n.to_str()),
        Some("D3DX.INI"),
        "the real on-disk name must be returned so the rewrite edits the \
         actual file"
    );
}
