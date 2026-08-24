//! Slice 5: Mod Variants.
//!
//! Covers the detection heuristic in isolation and the full
//! import-detect-switch flow against the real Core + filesystem.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use gmm_lib::core::variants::detect_variants;
use gmm_lib::core::{Core, Error, GameCode};
use sqlx::SqlitePool;
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

fn write_ini(dir: &Path, file: &str, contents: &[u8]) {
    fs::create_dir_all(dir).expect("dir");
    fs::write(dir.join(file), contents).expect("ini");
}

#[test]
fn detect_accepts_three_variants_each_with_inis() {
    let tmp = TempDir::new().expect("tmp");
    let root = tmp.path();
    write_ini(&root.join("Blue"), "merged.ini", b"hash=1\n");
    write_ini(&root.join("Red"), "merged.ini", b"hash=2\n");
    write_ini(&root.join("Green"), "merged.ini", b"hash=3\n");

    let detected = detect_variants(root).expect("detect");
    let names: Vec<_> = detected.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, &["Blue", "Green", "Red"], "alphabetical by name");
}

#[test]
fn detect_rejects_when_root_has_ini() {
    let tmp = TempDir::new().expect("tmp");
    let root = tmp.path();
    write_ini(root, "merged.ini", b"hash=1\n"); // root-level ini = single mod
    write_ini(&root.join("PreviewA"), "merged.ini", b"hash=2\n");
    write_ini(&root.join("PreviewB"), "merged.ini", b"hash=3\n");
    assert!(detect_variants(root).expect("detect").is_empty());
}

#[test]
fn detect_rejects_single_directory() {
    let tmp = TempDir::new().expect("tmp");
    let root = tmp.path();
    write_ini(&root.join("OnlyVariant"), "merged.ini", b"hash=1\n");
    assert!(detect_variants(root).expect("detect").is_empty());
}

#[test]
fn detect_rejects_directories_without_inis() {
    let tmp = TempDir::new().expect("tmp");
    let root = tmp.path();
    fs::create_dir_all(root.join("Variant1")).expect("v1");
    fs::create_dir_all(root.join("Variant2")).expect("v2");
    // Neither has an ini → not a real Mod payload.
    assert!(detect_variants(root).expect("detect").is_empty());
}

fn build_three_variant_zip(zip_path: &Path) {
    let f = File::create(zip_path).expect("create");
    let mut zw = ZipWriter::new(f);
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for color in ["Blue", "Green", "Red"] {
        zw.add_directory(format!("{color}/"), opts).expect("dir");
        zw.start_file(format!("{color}/merged.ini"), opts)
            .expect("ini");
        zw.write_all(format!("hash={color}\n").as_bytes())
            .expect("contents");
        zw.start_file(format!("{color}/skin.dds"), opts)
            .expect("dds");
        zw.write_all(b"DDSDATA").expect("dds bytes");
    }
    zw.finish().expect("finish");
}

#[tokio::test]
async fn importing_multi_variant_zip_records_variants_and_sets_first_active() {
    let tmp = TempDir::new().expect("tmp");
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root, &db_url).await.expect("init");

    let zip_path = tmp.path().join("variants.zip");
    build_three_variant_zip(&zip_path);
    let imported = core
        .import_zip(
            GameCode::Gimi,
            &zip_path,
            "Variant Test",
            Default::default(),
        )
        .await
        .expect("import");

    let variants = core.list_variants(&imported.id).await.expect("list");
    assert_eq!(variants.len(), 3, "three variants persisted");
    let names: Vec<_> = variants.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, &["Blue", "Green", "Red"]);

    let active = core
        .active_variant_id(&imported.id)
        .await
        .expect("active")
        .expect("an active variant should be set");
    assert_eq!(active, variants[0].id, "first alphabetical becomes active");
}

#[tokio::test]
async fn switching_active_variant_retargets_the_junction() {
    let tmp = TempDir::new().expect("tmp");
    let library_root = tmp.path().join("library");
    let game_install = tmp.path().join("Genshin");
    let game_mods = game_install.join("Mods");
    fs::create_dir_all(&game_mods).expect("mods dir");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root, &db_url).await.expect("init");
    core.set_game_install_path(GameCode::Gimi, &game_install)
        .await
        .expect("install");

    let zip_path = tmp.path().join("variants.zip");
    build_three_variant_zip(&zip_path);
    let imported = core
        .import_zip(GameCode::Gimi, &zip_path, "Switch Me", Default::default())
        .await
        .expect("import");
    core.set_enabled(&imported.id, true, &game_mods)
        .await
        .expect("enable");

    // Junction starts pointed at the first variant (Blue).
    let link = game_mods.join("Switch Me");
    assert!(link.exists());
    let merged = fs::read(link.join("merged.ini")).expect("read merged");
    assert!(
        merged.starts_with(b"hash=Blue"),
        "first variant payload is Blue, got: {merged:?}",
    );

    // Switch to Red.
    let variants = core.list_variants(&imported.id).await.expect("list");
    let red = variants.iter().find(|v| v.name == "Red").expect("red");
    core.set_active_variant(&imported.id, &red.id, &game_mods)
        .await
        .expect("switch");

    let merged_after = fs::read(link.join("merged.ini")).expect("read merged");
    assert!(
        merged_after.starts_with(b"hash=Red"),
        "after switch, junction must resolve to Red's payload, got: {merged_after:?}",
    );

    // And once more, to Green.
    let green = variants.iter().find(|v| v.name == "Green").expect("green");
    core.set_active_variant(&imported.id, &green.id, &game_mods)
        .await
        .expect("switch");
    let merged_after = fs::read(link.join("merged.ini")).expect("read merged");
    assert!(merged_after.starts_with(b"hash=Green"));
}

#[tokio::test]
async fn a_foreign_persisted_active_variant_is_not_used_as_a_junction_target() {
    let tmp = TempDir::new().expect("tmp");
    let library_root = tmp.path().join("library");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root, &db_url).await.expect("init");

    let zip_path = tmp.path().join("variants.zip");
    build_three_variant_zip(&zip_path);
    let first = core
        .import_zip(GameCode::Gimi, &zip_path, "First Mod", Default::default())
        .await
        .expect("import first Mod");
    let second = core
        .import_zip(GameCode::Gimi, &zip_path, "Second Mod", Default::default())
        .await
        .expect("import second Mod");
    let foreign_variant_id = core
        .list_variants(&second.id)
        .await
        .expect("list second Mod's Variants")[0]
        .id
        .clone();

    let pool = SqlitePool::connect(&db_url).await.expect("open fixture DB");
    sqlx::query("UPDATE mods SET enabled = 1, active_variant_id = ? WHERE id = ?")
        .bind(&foreign_variant_id)
        .bind(&first.id)
        .execute(&pool)
        .await
        .expect("plant foreign active Variant ID");

    let error = core
        .detect_conflicts(GameCode::Gimi)
        .await
        .expect_err("a foreign active Variant must be rejected");
    assert!(
        matches!(
            &error,
            Error::InvalidActiveVariant {
                mod_id,
                mod_name,
                variant_id,
            } if mod_id == &first.id
                && mod_name == "First Mod"
                && variant_id == &foreign_variant_id
        ),
        "the corruption error must name both mismatched rows, got: {error}",
    );
    assert_eq!(
        error.to_string(),
        "Mod \"First Mod\" has an invalid active Variant selection. Select a valid Variant for this Mod, or reinstall it.",
        "the corruption error must name the Mod and give the user both repair routes",
    );
}

#[tokio::test]
async fn a_dangling_persisted_active_variant_is_not_replaced_with_the_mod_root() {
    let tmp = TempDir::new().expect("tmp");
    let library_root = tmp.path().join("library");
    let game_mods = tmp.path().join("Genshin/Mods");
    fs::create_dir_all(&game_mods).expect("game Mods dir");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root, &db_url).await.expect("init");

    let zip_path = tmp.path().join("variants.zip");
    build_three_variant_zip(&zip_path);
    let imported = core
        .import_zip(
            GameCode::Gimi,
            &zip_path,
            "Dangling Variant",
            Default::default(),
        )
        .await
        .expect("import Mod");
    let dangling_variant_id = "missing-active-variant";
    let pool = SqlitePool::connect(&db_url).await.expect("open fixture DB");
    let mut connection = pool.acquire().await.expect("acquire fixture DB connection");
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *connection)
        .await
        .expect("allow planting a legacy dangling reference");
    sqlx::query("UPDATE mods SET active_variant_id = ? WHERE id = ?")
        .bind(dangling_variant_id)
        .bind(&imported.id)
        .execute(&mut *connection)
        .await
        .expect("plant dangling active Variant ID");
    drop(connection);

    let error = core
        .set_enabled(&imported.id, true, &game_mods)
        .await
        .expect_err("a dangling active Variant must be rejected");
    assert!(
        matches!(
            &error,
            Error::InvalidActiveVariant {
                mod_id,
                mod_name,
                variant_id,
            } if mod_id == &imported.id
                && mod_name == "Dangling Variant"
                && variant_id == dangling_variant_id
        ),
        "the corruption error must name the dangling reference, got: {error}",
    );
    assert!(
        !game_mods.join("Dangling Variant").exists(),
        "corrupt Variant state must not silently deploy the Mod root",
    );
}

#[tokio::test]
async fn a_dangling_persisted_active_variant_does_not_block_disable() {
    let tmp = TempDir::new().expect("tmp");
    let library_root = tmp.path().join("library");
    let game_mods = tmp.path().join("Genshin/Mods");
    fs::create_dir_all(&game_mods).expect("game Mods dir");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root, &db_url).await.expect("init");

    let zip_path = tmp.path().join("variants.zip");
    build_three_variant_zip(&zip_path);
    let imported = core
        .import_zip(
            GameCode::Gimi,
            &zip_path,
            "Disable Corrupt Variant",
            Default::default(),
        )
        .await
        .expect("import Mod");
    core.set_enabled(&imported.id, true, &game_mods)
        .await
        .expect("enable before planting corruption");
    let link = game_mods.join("Disable Corrupt Variant");
    assert!(link.join("merged.ini").is_file(), "precondition: Mod loads");

    let pool = SqlitePool::connect(&db_url).await.expect("open fixture DB");
    let mut connection = pool.acquire().await.expect("acquire fixture DB connection");
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *connection)
        .await
        .expect("allow planting a legacy dangling reference");
    sqlx::query("UPDATE mods SET active_variant_id = ? WHERE id = ?")
        .bind("missing-active-variant")
        .bind(&imported.id)
        .execute(&mut *connection)
        .await
        .expect("plant dangling active Variant ID");
    drop(connection);

    core.set_enabled(&imported.id, false, &game_mods)
        .await
        .expect("a dangling active Variant must not block disable");

    let row = core
        .list_mods(GameCode::Gimi)
        .await
        .expect("list Mods")
        .into_iter()
        .find(|candidate| candidate.id == imported.id)
        .expect("disabled Mod row");
    assert!(!row.enabled, "disable must persist enabled = false");
    assert!(
        fs::symlink_metadata(&link).is_err(),
        "disable must remove the Junction despite corrupt Variant state",
    );
}
