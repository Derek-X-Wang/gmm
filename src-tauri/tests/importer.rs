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

// ---------------------------------------------------------------------
// Regression: `Mods/` is user data and must survive importer installs.
//
// `Mods/` is where GMM materialises Junctions for enabled mods
// (ADR 0003). It used to be listed in IMPORTER_ROOT_DIRS, which meant
// `backup_existing` renamed the whole directory into the backup folder
// on every install — silently stripping every enabled mod out of the
// game while the DB still said `enabled = 1`. Caught by the Windows
// end-to-end test on its first real run; these keep it dead.
// ---------------------------------------------------------------------

/// Build a zip that also ships its own `Mods/` folder, the way some
/// importer packages do (usually example mods).
fn build_importer_zip_with_mods(zip_path: &Path) {
    let file = File::create(zip_path).expect("create zip");
    let mut zw = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    zw.start_file("d3dx.ini", opts).expect("d3dx.ini");
    zw.write_all(b"[Loader]\nloader = XXMI Launcher.exe\n")
        .expect("write d3dx");
    zw.start_file("d3d11.dll", opts).expect("d3d11.dll");
    zw.write_all(b"MZ\x00\x00fake-dll").expect("write dll");
    zw.add_directory("Mods/", opts).expect("Mods dir");
    zw.start_file("Mods/ExampleMod.ini", opts).expect("example");
    zw.write_all(b"; shipped example\n").expect("write example");
    zw.finish().expect("finish zip");
}

#[test]
fn install_never_moves_an_existing_mods_directory() {
    let tmp = TempDir::new().expect("tmp");
    let game = tmp.path().join("game");
    let mods = game.join("Mods");
    fs::create_dir_all(mods.join("Hu Tao Skin")).expect("existing mod dir");
    fs::write(mods.join("Hu Tao Skin/merged.ini"), b"hash = deadbeef\n").expect("mod ini");

    let zip = tmp.path().join("GIMI.zip");
    build_importer_zip(&zip);

    install_from_local_zip(&zip, &game, &tmp.path().join("backups"), DEFAULT_LOADER_EXE)
        .expect("install");

    assert!(
        mods.join("Hu Tao Skin/merged.ini").exists(),
        "an enabled mod's deployment directory must survive an importer install",
    );
    assert_eq!(
        fs::read_to_string(mods.join("Hu Tao Skin/merged.ini")).expect("read"),
        "hash = deadbeef\n",
        "contents must be untouched, not restored from a backup copy",
    );
}

#[test]
fn reinstall_leaves_mods_in_place_while_replacing_importer_files() {
    let tmp = TempDir::new().expect("tmp");
    let game = tmp.path().join("game");
    let backups = tmp.path().join("backups");
    let zip = tmp.path().join("GIMI.zip");
    build_importer_zip(&zip);

    // First install, then the user enables a mod.
    install_from_local_zip(&zip, &game, &backups, DEFAULT_LOADER_EXE).expect("first install");
    let mods = game.join("Mods");
    fs::create_dir_all(mods.join("Nahida")).expect("mod dir");
    fs::write(mods.join("Nahida/merged.ini"), b"hash = 1234\n").expect("mod ini");

    // Reinstall — the exact flow behind "Reinstall importer" and an
    // importer update.
    install_from_local_zip(&zip, &game, &backups, DEFAULT_LOADER_EXE).expect("reinstall");

    assert!(
        mods.join("Nahida/merged.ini").exists(),
        "reinstalling the importer must not orphan enabled mods",
    );
    assert!(
        game.join("d3d11.dll").exists(),
        "importer files should still be replaced normally",
    );
}

#[test]
fn a_package_shipping_its_own_mods_folder_merges_instead_of_replacing() {
    let tmp = TempDir::new().expect("tmp");
    let game = tmp.path().join("game");
    let mods = game.join("Mods");
    fs::create_dir_all(mods.join("Existing")).expect("existing dir");
    fs::write(mods.join("Existing/merged.ini"), b"mine\n").expect("existing ini");

    let zip = tmp.path().join("GIMI-with-mods.zip");
    build_importer_zip_with_mods(&zip);

    install_from_local_zip(&zip, &game, &tmp.path().join("backups"), DEFAULT_LOADER_EXE)
        .expect("install");

    assert!(
        mods.join("Existing/merged.ini").exists(),
        "the user's own mod must survive a package that ships Mods/",
    );
    assert_eq!(
        fs::read_to_string(mods.join("Existing/merged.ini")).expect("read"),
        "mine\n",
    );
    assert!(
        mods.join("ExampleMod.ini").exists(),
        "the package's shipped example should still be merged in",
    );
}

#[test]
fn rollback_never_replaces_the_live_mods_directory() {
    let tmp = TempDir::new().expect("tmp");
    let game = tmp.path().join("game");
    let mods = game.join("Mods");
    fs::create_dir_all(&mods).expect("mods");
    fs::write(mods.join("live.ini"), b"live\n").expect("live");

    // Simulate a backup taken by an older GMM that still captured Mods/.
    let backup = tmp.path().join("backups/20260101T000000");
    fs::create_dir_all(backup.join("Mods")).expect("backup mods");
    fs::write(backup.join("Mods/stale.ini"), b"stale\n").expect("stale");
    fs::write(backup.join("d3d11.dll"), b"old-dll").expect("old dll");

    rollback_to(&backup, &game).expect("rollback");

    assert!(
        mods.join("live.ini").exists(),
        "rollback must never wholesale-replace the live Mods directory",
    );
    // NOTE: this assertion was inverted after code review. It
    // originally required `stale.ini` to stay in the backup, encoding a
    // blanket skip of user-owned directories. That is wrong: backups
    // containing Mods/ exist *because* the old build moved the user's
    // whole mods directory into one, and rollback is the recovery path
    // for exactly that. Skipping stranded the mods in the backup
    // forever. Merge-preferring-live restores them without clobbering
    // anything current.
    assert!(
        mods.join("stale.ini").exists(),
        "a backed-up entry with no live counterpart must be brought back, \
         not stranded in the backup",
    );
    assert!(
        game.join("d3d11.dll").exists(),
        "non-user-owned files should still roll back normally",
    );
}

// ---------------------------------------------------------------------
// Code-review follow-ups. `Mods/` is user-owned, but "don't touch it"
// is not the same as "ignore it" — rollback is the recovery path for
// backups taken by the old build that *did* move it.
// ---------------------------------------------------------------------

/// The recovery case. A backup written by a pre-fix build contains the
/// user's whole `Mods/` directory. Rolling back must bring it home,
/// not strand it in the backup forever.
#[test]
fn rollback_restores_mods_when_the_game_has_none() {
    let tmp = TempDir::new().expect("tmp");
    let game = tmp.path().join("game");
    fs::create_dir_all(&game).expect("game dir");

    // Wreckage of the old bug: Mods/ sitting in a backup, nothing live.
    let backup = tmp.path().join("backups/20260101T000000");
    fs::create_dir_all(backup.join("Mods/Hu Tao Skin")).expect("backup mods");
    fs::write(backup.join("Mods/Hu Tao Skin/merged.ini"), b"rescued\n").expect("ini");
    fs::write(backup.join("d3d11.dll"), b"old-dll").expect("old dll");

    rollback_to(&backup, &game).expect("rollback");

    assert!(
        game.join("Mods/Hu Tao Skin/merged.ini").exists(),
        "rollback must restore a Mods/ directory the game no longer has — \
         this is the recovery path for backups taken by the old build",
    );
    assert_eq!(
        fs::read_to_string(game.join("Mods/Hu Tao Skin/merged.ini")).expect("read"),
        "rescued\n",
    );
}

/// When both sides have a `Mods/`, merge and prefer live. Neither
/// stranding the backup nor clobbering the user's current state is
/// acceptable.
#[test]
fn rollback_merges_backup_mods_into_a_live_one_preferring_live() {
    let tmp = TempDir::new().expect("tmp");
    let game = tmp.path().join("game");
    let mods = game.join("Mods");
    fs::create_dir_all(mods.join("Current")).expect("live dir");
    fs::write(mods.join("Current/merged.ini"), b"live\n").expect("live ini");
    fs::write(mods.join("shared.ini"), b"live-wins\n").expect("shared live");

    let backup = tmp.path().join("backups/20260101T000000");
    fs::create_dir_all(backup.join("Mods/Stranded")).expect("backup dir");
    fs::write(backup.join("Mods/Stranded/merged.ini"), b"rescued\n").expect("stranded");
    fs::write(backup.join("Mods/shared.ini"), b"backup-loses\n").expect("shared backup");

    rollback_to(&backup, &game).expect("rollback");

    assert!(
        mods.join("Current/merged.ini").exists(),
        "the live mod must survive",
    );
    assert!(
        mods.join("Stranded/merged.ini").exists(),
        "the stranded backup entry must be brought back",
    );
    assert_eq!(
        fs::read_to_string(mods.join("shared.ini")).expect("read shared"),
        "live-wins\n",
        "on a collision the live copy wins — never silently overwrite current state",
    );
}

/// `merge_into` must recurse. Skipping a whole subtree because its top
/// directory already exists means a package's nested files never land.
#[test]
fn merging_a_shipped_mods_dir_recurses_into_existing_subdirectories() {
    let tmp = TempDir::new().expect("tmp");
    let game = tmp.path().join("game");
    let mods = game.join("Mods");
    // The user already has a directory with the same name the package
    // ships, containing their own file.
    fs::create_dir_all(mods.join("Examples")).expect("existing dir");
    fs::write(mods.join("Examples/mine.ini"), b"mine\n").expect("mine");

    let zip = tmp.path().join("GIMI-nested.zip");
    {
        let file = File::create(&zip).expect("create zip");
        let mut zw = ZipWriter::new(file);
        let opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zw.start_file("d3d11.dll", opts).expect("dll");
        zw.write_all(b"MZ\x00\x00").expect("write dll");
        zw.add_directory("Mods/", opts).expect("mods dir");
        zw.add_directory("Mods/Examples/", opts)
            .expect("examples dir");
        zw.start_file("Mods/Examples/shipped.ini", opts)
            .expect("shipped");
        zw.write_all(b"; shipped\n").expect("write shipped");
        zw.finish().expect("finish");
    }

    install_from_local_zip(&zip, &game, &tmp.path().join("backups"), DEFAULT_LOADER_EXE)
        .expect("install");

    assert!(
        mods.join("Examples/mine.ini").exists(),
        "the user's file must survive",
    );
    assert!(
        mods.join("Examples/shipped.ini").exists(),
        "a shipped file nested under an existing directory must still be merged in — \
         a non-recursive merge would skip the whole subtree",
    );
}
