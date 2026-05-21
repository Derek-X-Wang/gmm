//! Slice 3: GIMI Model Importer install + rollback.
//!
//! The tests here go through the local-zip orchestrator
//! ([`install_from_local_zip`]) so no network is required. The full
//! production path is identical apart from the zip-fetch step at the
//! front.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use gmm_lib::core::importer::{
    install_from_local_zip, rewrite_d3dx_loader, rollback_to, DEFAULT_LOADER_EXE,
};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

fn build_importer_zip(zip_path: &Path) {
    let file = File::create(zip_path).expect("create zip");
    let mut zw = ZipWriter::new(file);
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    let d3dx_contents = b"; 3dmigoto importer\n[Loader]\nloader = XXMI Launcher.exe\n";
    zw.start_file("d3dx.ini", opts).expect("d3dx.ini");
    zw.write_all(d3dx_contents).expect("write d3dx");
    zw.start_file("d3d11.dll", opts).expect("d3d11.dll");
    zw.write_all(b"MZ\x00\x00fake-dll").expect("write dll");
    zw.add_directory("ShaderFixes/", opts).expect("dir");
    zw.start_file("ShaderFixes/sample.hlsl", opts)
        .expect("hlsl");
    zw.write_all(b"// sample shader\n").expect("write hlsl");
    zw.finish().expect("finish zip");
}

#[test]
fn install_from_local_zip_places_files_and_rewrites_loader() {
    let tmp = TempDir::new().expect("tmp");
    let game_dir = tmp.path().join("Genshin");
    let backups = tmp.path().join("backups/gimi");
    let zip_path = tmp.path().join("gimi.zip");
    build_importer_zip(&zip_path);

    let report = install_from_local_zip(&zip_path, &game_dir, &backups, DEFAULT_LOADER_EXE)
        .expect("install");

    assert!(report.backup_dir.is_none(), "no backup for a clean install");
    assert!(!report.sha256.is_empty());
    assert!(report.rewrote_files.iter().any(|p| p.ends_with("d3dx.ini")));

    assert!(game_dir.join("d3d11.dll").is_file());
    assert!(game_dir.join("ShaderFixes/sample.hlsl").is_file());

    let d3dx = fs::read_to_string(game_dir.join("d3dx.ini")).expect("read d3dx");
    assert!(
        d3dx.contains("loader = gmm.exe"),
        "loader rewritten: {d3dx}"
    );
    assert!(
        !d3dx.contains("XXMI Launcher"),
        "old loader line replaced: {d3dx}",
    );
}

#[test]
fn rollback_restores_byte_for_byte_after_simulated_failure() {
    let tmp = TempDir::new().expect("tmp");
    let game_dir = tmp.path().join("Genshin");
    let backups = tmp.path().join("backups/gimi");
    fs::create_dir_all(&game_dir).expect("game dir");

    // Pre-existing importer files we'll be backing up.
    let original_d3dx = b"; previous install\n[Loader]\nloader = old-loader.exe\n";
    let original_dll = b"OLDDLL";
    fs::write(game_dir.join("d3dx.ini"), original_d3dx).expect("write old d3dx");
    fs::write(game_dir.join("d3d11.dll"), original_dll).expect("write old dll");
    fs::create_dir_all(game_dir.join("ShaderFixes")).expect("old shader dir");
    fs::write(game_dir.join("ShaderFixes/old.hlsl"), b"// old shader").expect("old hlsl");

    // Drive the install/backup/swap manually so we can inject a failure
    // *after* the swap has happened but before d3dx rewrite would
    // complete. Using the same primitives the orchestrator uses.
    let zip_path = tmp.path().join("gimi.zip");
    build_importer_zip(&zip_path);

    let report = install_from_local_zip(&zip_path, &game_dir, &backups, DEFAULT_LOADER_EXE)
        .expect("install");
    assert!(report.backup_dir.is_some(), "must have backed up");
    let backup_dir = report.backup_dir.unwrap();

    // Now simulate a catastrophic mid-install failure that was
    // detected *after* swap — call rollback_to and assert state.
    rollback_to(&backup_dir, &game_dir).expect("rollback");

    let d3dx = fs::read(game_dir.join("d3dx.ini")).expect("read d3dx");
    assert_eq!(d3dx, original_d3dx, "d3dx.ini restored byte-for-byte",);
    let dll = fs::read(game_dir.join("d3d11.dll")).expect("read dll");
    assert_eq!(dll, original_dll, "d3d11.dll restored byte-for-byte");
    let old_hlsl = fs::read_to_string(game_dir.join("ShaderFixes/old.hlsl")).expect("hlsl");
    assert_eq!(old_hlsl, "// old shader");
}

#[test]
fn rewrite_d3dx_loader_idempotent() {
    let tmp = TempDir::new().expect("tmp");
    let d3dx = tmp.path().join("d3dx.ini");
    fs::write(
        &d3dx,
        b"; comment\n[Loader]\nloader = XXMI Launcher.exe\nother = 1\n" as &[u8],
    )
    .expect("write");
    rewrite_d3dx_loader(&d3dx, "gmm.exe").expect("first");
    let after_first = fs::read_to_string(&d3dx).expect("read");
    rewrite_d3dx_loader(&d3dx, "gmm.exe").expect("second");
    let after_second = fs::read_to_string(&d3dx).expect("read");
    assert_eq!(after_first, after_second, "rewrite must be idempotent",);
    assert!(after_first.contains("loader = gmm.exe"));
    assert!(!after_first.contains("XXMI Launcher"));
    assert!(after_first.contains("other = 1"), "other keys preserved");
}
