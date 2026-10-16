//! Filesystem facts remain independent of configuration and recovery state.

mod support;

use std::fs;
use std::path::Path;

use gmm_lib::core::game_path::{GamePathState, ModelImporterState};
use gmm_lib::core::{Core, GameCode};
use sqlx::{Row, SqlitePool};
use tempfile::TempDir;

async fn fixture() -> (TempDir, Core, SqlitePool) {
    let tmp = TempDir::new().unwrap();
    let url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(tmp.path().join("library"), &url).await.unwrap();
    let db = SqlitePool::connect(&url).await.unwrap();
    (tmp, core, db)
}

async fn save_fixture_path(db: &SqlitePool, game: GameCode, path: &Path) {
    sqlx::query("UPDATE games SET install_path = ? WHERE code = ?")
        .bind(path.to_str().unwrap())
        .bind(game.as_str())
        .execute(db)
        .await
        .unwrap();
}

fn importer(path: &Path) {
    fs::write(path.join("d3dx.ini"), b"[Loader]\nloader=gmm.exe").unwrap();
    fs::create_dir(path.join("Core")).unwrap();
    fs::create_dir(path.join("ShaderFixes")).unwrap();
}

#[tokio::test]
async fn unset_is_configuration_only_and_importer_is_unverified() {
    let (_tmp, core, _db) = fixture().await;
    for game in GameCode::ported().map(|profile| profile.code) {
        let status = core.game_path_status(game).await.unwrap();
        assert_eq!(status.game, game);
        assert_eq!(status.saved_path, None);
        assert_eq!(status.game_path, GamePathState::NoSavedPath);
        assert_eq!(
            serde_json::to_value(&status).unwrap()["gamePath"]["status"],
            "noSavedPath"
        );
        assert!(matches!(
            status.model_importer,
            ModelImporterState::Unverified { .. }
        ));
    }
}

#[tokio::test]
async fn missing_saved_directory_and_present_directory_with_missing_entries_are_distinct() {
    let (tmp, core, db) = fixture().await;
    let missing = tmp.path().join("missing");
    assert_eq!(
        fs::metadata(&missing).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    save_fixture_path(&db, GameCode::Gimi, &missing).await;
    let status = core.game_path_status(GameCode::Gimi).await.unwrap();
    assert_eq!(status.saved_path, Some(missing.clone()));
    assert_eq!(
        serde_json::to_value(&status).unwrap()["gamePath"]["status"],
        "savedPathNotFound"
    );
    assert_eq!(
        status.game_path,
        GamePathState::SavedPathNotFound {
            paths: vec![missing.clone()]
        }
    );
    assert!(matches!(
        status.model_importer,
        ModelImporterState::Unverified { .. }
    ));
    assert_eq!(
        core.game_install_path(GameCode::Gimi).await.unwrap(),
        Some(missing.clone())
    );
    // The same saved path now names an existing, readable directory. Only its
    // required Game entries are absent; importer inspection remains unverified.
    fs::create_dir(&missing).unwrap();
    fs::read_dir(&missing).unwrap();
    let status = core.game_path_status(GameCode::Gimi).await.unwrap();
    assert_eq!(status.saved_path, Some(missing.clone()));
    assert_eq!(
        serde_json::to_value(&status).unwrap()["gamePath"]["status"],
        "requiredEntriesNotFound"
    );
    assert_eq!(
        status.game_path,
        GamePathState::RequiredEntriesNotFound {
            paths: vec![
                missing.join("GenshinImpact.exe"),
                missing.join("YuanShen.exe"),
                missing.join("GenshinImpact_Data")
            ],
        }
    );
    assert!(matches!(
        status.model_importer,
        ModelImporterState::Unverified { .. }
    ));
    assert_eq!(
        core.game_install_path(GameCode::Gimi).await.unwrap(),
        Some(missing)
    );
}

#[tokio::test]
async fn missing_game_files_are_named_without_claiming_an_absent_importer() {
    let (tmp, core, db) = fixture().await;
    let game = tmp.path().join("game");
    support::seed_game_install(GameCode::Gimi, &game);
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    fs::remove_file(game.join("GenshinImpact.exe")).unwrap();
    let status = core.game_path_status(GameCode::Gimi).await.unwrap();
    assert_eq!(status.saved_path, Some(game.clone()));
    assert_eq!(
        serde_json::to_value(&status).unwrap()["gamePath"]["status"],
        "requiredEntriesNotFound"
    );
    assert_eq!(
        status.game_path,
        GamePathState::RequiredEntriesNotFound {
            paths: vec![game.join("GenshinImpact.exe"), game.join("YuanShen.exe")],
        }
    );
    assert!(matches!(
        status.model_importer,
        ModelImporterState::Unverified { .. }
    ));
    fs::write(game.join("YuanShen.exe"), b"CN executable").unwrap();
    fs::remove_dir(game.join("GenshinImpact_Data")).unwrap();
    let status = core.game_path_status(GameCode::Gimi).await.unwrap();
    assert_eq!(status.saved_path, Some(game.clone()));
    assert_eq!(
        serde_json::to_value(&status).unwrap()["gamePath"]["status"],
        "requiredEntriesNotFound"
    );
    assert_eq!(
        status.game_path,
        GamePathState::RequiredEntriesNotFound {
            paths: vec![game.join("GenshinImpact_Data")]
        }
    );
}

#[tokio::test]
async fn wrong_types_are_invalid_not_confirmed_missing_files() {
    let (tmp, core, db) = fixture().await;
    let game = tmp.path().join("game");
    fs::write(&game, b"not a directory").unwrap();
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    assert!(matches!(
        core.game_path_status(GameCode::Gimi)
            .await
            .unwrap()
            .game_path,
        GamePathState::Invalid { .. }
    ));
    fs::remove_file(&game).unwrap();
    fs::create_dir_all(game.join("GenshinImpact.exe")).unwrap();
    fs::create_dir(game.join("GenshinImpact_Data")).unwrap();
    assert!(matches!(
        core.game_path_status(GameCode::Gimi)
            .await
            .unwrap()
            .game_path,
        GamePathState::Invalid { .. }
    ));
}

#[tokio::test]
async fn every_game_uses_its_existing_validator_and_importer_layout() {
    let (tmp, core, db) = fixture().await;
    for profile in GameCode::ported() {
        let game = tmp
            .path()
            .join(profile.code.as_str())
            .join("Binaries/Win64");
        support::seed_game_install(profile.code, &game);
        save_fixture_path(&db, profile.code, &game).await;
        let status = core.game_path_status(profile.code).await.unwrap();
        assert_eq!(status.game_path, GamePathState::Readable);
        assert_eq!(
            status.model_importer,
            ModelImporterState::NoImporterEntriesFound {
                paths: vec![
                    game.join("d3dx.ini"),
                    game.join("Core"),
                    game.join("ShaderFixes")
                ],
            }
        );
        importer(&game);
        let status = core.game_path_status(profile.code).await.unwrap();
        assert_eq!(status.game_path, GamePathState::Readable);
        assert_eq!(
            status.model_importer,
            ModelImporterState::UnrecordedImporterPresent { paths: vec![] }
        );
        // No Mods directory, ShaderCache, DLL, recorded version, or Origin
        // is necessary to establish the existing package layout.
    }
}

#[tokio::test]
async fn recorded_install_with_missing_entries_is_damaged() {
    let (tmp, core, db) = fixture().await;
    let game = tmp.path().join("game");
    support::seed_game_install(GameCode::Gimi, &game);
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    core.set_importer_pinned(GameCode::Gimi, Some("v9.9.9"))
        .await
        .unwrap();
    gmm_lib::core::updates::set_importer_installed(&db, GameCode::Gimi, "v9.9.9")
        .await
        .unwrap();
    sqlx::query("INSERT INTO settings (key, value) VALUES ('importer.gimi.installed_origin', ?)")
        .bind(
            serde_json::to_string(&gmm_lib::core::importer_origin::ImporterOrigin::github(
                "SilentNightSound",
                "GIMI-Package",
                r"GIMI-PACKAGE-v\d+\.\d+\.\d+\.zip",
            ))
            .unwrap(),
        )
        .execute(&db)
        .await
        .unwrap();
    let status = core.game_path_status(GameCode::Gimi).await.unwrap();
    assert_eq!(status.game_path, GamePathState::Readable);
    assert!(matches!(
        status.model_importer,
        ModelImporterState::Damaged { .. }
    ));
    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(json["modelImporter"]["status"], "damaged");
    assert_eq!(
        json["modelImporter"]["record"]["installedVersion"],
        "v9.9.9"
    );
    assert_eq!(json["modelImporter"]["record"]["origin"]["state"], "known");
}

#[tokio::test]
async fn importer_names_follow_existing_case_insensitive_conventions() {
    let (tmp, core, db) = fixture().await;
    let game = tmp.path().join("game");
    support::seed_game_install(GameCode::Gimi, &game);
    fs::write(game.join("D3DX.INI"), b"configuration").unwrap();
    fs::create_dir(game.join("cOrE")).unwrap();
    fs::create_dir(game.join("SHADERFIXES")).unwrap();
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    assert_eq!(
        core.game_path_status(GameCode::Gimi)
            .await
            .unwrap()
            .model_importer,
        ModelImporterState::UnrecordedImporterPresent { paths: vec![] }
    );
}

#[tokio::test]
async fn invalid_input_is_uncertainty_and_keeps_the_saved_path() {
    let (tmp, core, db) = fixture().await;
    let path = tmp.path().join("invalid\0path");
    assert_eq!(
        fs::metadata(&path).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
    save_fixture_path(&db, GameCode::Gimi, &path).await;
    let status = core.game_path_status(GameCode::Gimi).await.unwrap();
    assert_eq!(status.saved_path, Some(path.clone()));
    assert!(
        matches!(status.game_path, GamePathState::Unreadable { path: ref failed, ref error }
        if failed == &path && error.contains("io error"))
    );
    assert!(matches!(
        status.model_importer,
        ModelImporterState::Unverified { .. }
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn directory_read_failure_cannot_be_collapsed_to_a_healthy_game() {
    use std::os::unix::fs::PermissionsExt;
    let (tmp, core, db) = fixture().await;
    let game = tmp.path().join("game");
    support::seed_game_install(GameCode::Gimi, &game);
    importer(&game);
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    // Searchable but not enumerable: best-effort metadata checks still pass.
    fs::set_permissions(&game, fs::Permissions::from_mode(0o111)).unwrap();
    assert_eq!(
        fs::read_dir(&game).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert!(
        gmm_lib::core::detect::genshin::validate(&game),
        "fixture distinguishes the two validators"
    );
    let result = core.game_path_status(GameCode::Gimi).await;
    fs::set_permissions(&game, fs::Permissions::from_mode(0o700)).unwrap();
    let status = result.unwrap();
    assert!(
        matches!(status.game_path, GamePathState::Unreadable { path: ref failed, .. } if failed == &game),
        "a metadata-only collapsing probe must not report this Game readable: {status:?}"
    );
    assert!(matches!(
        status.model_importer,
        ModelImporterState::Unverified { .. }
    ));
    assert_eq!(status.saved_path, Some(game));
}

#[cfg(unix)]
#[tokio::test]
async fn importer_read_failure_keeps_the_readable_game_fact() {
    use std::os::unix::fs::PermissionsExt;
    let (tmp, core, db) = fixture().await;
    let game = tmp.path().join("game");
    support::seed_game_install(GameCode::Gimi, &game);
    importer(&game);
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    for (entry, mode) in [(game.join("d3dx.ini"), 0o600), (game.join("Core"), 0o700)] {
        fs::set_permissions(&entry, fs::Permissions::from_mode(0o000)).unwrap();
        let result = core.game_path_status(GameCode::Gimi).await;
        fs::set_permissions(&entry, fs::Permissions::from_mode(mode)).unwrap();
        let status = result.unwrap();
        assert_eq!(status.game_path, GamePathState::Readable);
        assert!(
            matches!(status.model_importer, ModelImporterState::Unverified { path: Some(ref failed), ref error, .. }
            if failed == &entry && error.contains(&entry.display().to_string())),
            "{status:?}"
        );
        assert_eq!(status.saved_path, Some(game.clone()));
    }
}

#[cfg(windows)]
#[tokio::test]
async fn sharing_violations_preserve_independent_game_and_importer_facts() {
    use std::os::windows::fs::OpenOptionsExt;
    let (tmp, core, db) = fixture().await;
    let game = tmp.path().join("game");
    support::seed_game_install(GameCode::Gimi, &game);
    importer(&game);
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    for (entry, game_readable) in [
        (game.join("GenshinImpact.exe"), false),
        (game.join("d3dx.ini"), true),
    ] {
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&entry)
            .unwrap();
        assert_ne!(
            fs::File::open(&entry).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        let status = core.game_path_status(GameCode::Gimi).await.unwrap();
        if game_readable {
            assert_eq!(status.game_path, GamePathState::Readable);
        } else {
            assert!(matches!(status.game_path, GamePathState::Unreadable { .. }));
        }
        assert!(matches!(
            status.model_importer,
            ModelImporterState::Unverified { .. }
        ));
        assert_eq!(status.saved_path, Some(game.clone()));
        drop(lock);
    }
}

#[tokio::test]
async fn wrong_importer_types_are_unverified_not_absent() {
    let (tmp, core, db) = fixture().await;
    let game = tmp.path().join("game");
    support::seed_game_install(GameCode::Gimi, &game);
    fs::create_dir(game.join("d3dx.ini")).unwrap();
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    let status = core.game_path_status(GameCode::Gimi).await.unwrap();
    assert_eq!(status.game_path, GamePathState::Readable);
    assert!(matches!(
        status.model_importer,
        ModelImporterState::Unverified { path: Some(ref failed), .. } if failed == &game.join("d3dx.ini")
    ));
}

#[tokio::test]
async fn missing_executable_alternative_and_unreadable_other_remain_uncertain() {
    let (tmp, core, db) = fixture().await;
    let game = tmp.path().join("game");
    support::seed_game_install(GameCode::Gimi, &game);
    let alternative = game.join("YuanShen.exe");
    fs::rename(game.join("GenshinImpact.exe"), &alternative).unwrap();
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&alternative, fs::Permissions::from_mode(0o000)).unwrap();
    }
    #[cfg(windows)]
    let lock = {
        use std::os::windows::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&alternative)
            .unwrap()
    };
    assert_eq!(
        fs::metadata(game.join("GenshinImpact.exe"))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
    );
    assert_ne!(
        fs::File::open(&alternative).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    let result = core.game_path_status(GameCode::Gimi).await;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&alternative, fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[cfg(windows)]
    drop(lock);
    let status = result.unwrap();
    assert!(
        matches!(status.game_path, GamePathState::Unreadable { path: ref failed, .. }
        if failed == &alternative),
        "not every executable alternative is confirmed missing: {status:?}"
    );
    assert_eq!(status.saved_path, Some(game));
    assert!(matches!(
        status.model_importer,
        ModelImporterState::Unverified { path: None, .. }
    ));
}

#[tokio::test]
async fn dangling_game_directory_link_is_unreadable_and_the_entry_is_retained() {
    let (tmp, core, db) = fixture().await;
    let target = tmp.path().join("target");
    support::seed_game_install(GameCode::Gimi, &target);
    let link = tmp.path().join("game-link");
    gmm_lib::core::junction::create(&link, &target).unwrap();
    save_fixture_path(&db, GameCode::Gimi, &link).await;
    fs::remove_dir_all(&target).unwrap();
    fs::symlink_metadata(&link).expect("the link entry still exists");
    fs::metadata(&link).expect_err("the target is unresolved");
    let status = core.game_path_status(GameCode::Gimi).await.unwrap();
    assert!(
        matches!(status.game_path, GamePathState::Unreadable { path: ref failed, .. } if failed == &link)
    );
    assert_eq!(status.saved_path, Some(link.clone()));
    assert!(matches!(
        status.model_importer,
        ModelImporterState::Unverified { path: None, .. }
    ));
    fs::symlink_metadata(&link).expect("inspection must retain the existing entry");
    gmm_lib::core::junction::remove(&link).unwrap();
}

#[tokio::test]
async fn dangling_importer_directory_link_is_unverified_with_its_path() {
    let (tmp, core, db) = fixture().await;
    let game = tmp.path().join("game");
    support::seed_game_install(GameCode::Gimi, &game);
    importer(&game);
    let core_path = game.join("Core");
    fs::remove_dir(&core_path).unwrap();
    let target = tmp.path().join("importer-target");
    fs::create_dir(&target).unwrap();
    gmm_lib::core::junction::create(&core_path, &target).unwrap();
    fs::remove_dir(&target).unwrap();
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    fs::symlink_metadata(&core_path).expect("the link entry still exists");
    fs::metadata(&core_path).expect_err("the target is unresolved");
    let status = core.game_path_status(GameCode::Gimi).await.unwrap();
    assert_eq!(status.game_path, GamePathState::Readable);
    assert!(
        matches!(status.model_importer, ModelImporterState::Unverified { path: Some(ref failed), .. }
        if failed == &core_path)
    );
    let json = serde_json::to_value(status).unwrap();
    assert_eq!(json["modelImporter"]["status"], "unverified");
    assert_eq!(json["modelImporter"]["path"], core_path.to_str().unwrap());
    fs::symlink_metadata(&core_path).expect("inspection must retain the existing entry");
    gmm_lib::core::junction::remove(&core_path).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn dangling_importer_file_link_is_unverified_not_confirmed_uninstalled() {
    use std::os::unix::fs::symlink;
    let (tmp, core, db) = fixture().await;
    let game = tmp.path().join("game");
    support::seed_game_install(GameCode::Gimi, &game);
    importer(&game);
    let ini = game.join("d3dx.ini");
    fs::remove_file(&ini).unwrap();
    symlink("missing-config.ini", &ini).unwrap();
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    assert!(fs::symlink_metadata(&ini).unwrap().file_type().is_symlink());
    assert_eq!(
        fs::metadata(&ini).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    let status = core.game_path_status(GameCode::Gimi).await.unwrap();
    assert_eq!(status.game_path, GamePathState::Readable);
    assert!(
        matches!(status.model_importer, ModelImporterState::Unverified { path: Some(ref failed), .. }
        if failed == &ini)
    );
    assert!(fs::symlink_metadata(&ini).unwrap().file_type().is_symlink());
}

#[tokio::test]
async fn each_missing_importer_requirement_is_identified() {
    let (tmp, core, db) = fixture().await;
    let game = tmp.path().join("game");
    support::seed_game_install(GameCode::Gimi, &game);
    importer(&game);
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    for name in ["d3dx.ini", "Core", "ShaderFixes"] {
        let path = game.join(name);
        if name == "d3dx.ini" {
            fs::remove_file(&path).unwrap();
        } else {
            fs::remove_dir(&path).unwrap();
        }
        let status = core.game_path_status(GameCode::Gimi).await.unwrap();
        assert_eq!(status.game_path, GamePathState::Readable);
        assert_eq!(
            status.model_importer,
            ModelImporterState::UnrecordedImporterPresent {
                paths: vec![path.clone()]
            }
        );
        if name == "d3dx.ini" {
            fs::write(path, b"configuration").unwrap();
        } else {
            fs::create_dir(path).unwrap();
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn importer_uncertainty_overrides_an_earlier_missing_requirement() {
    use std::os::unix::fs::symlink;
    let (tmp, core, db) = fixture().await;
    let game = tmp.path().join("game");
    support::seed_game_install(GameCode::Gimi, &game);
    symlink("Core", game.join("Core")).unwrap();
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    assert_ne!(
        fs::metadata(game.join("Core")).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    let status = core.game_path_status(GameCode::Gimi).await.unwrap();
    assert_eq!(status.game_path, GamePathState::Readable);
    assert!(
        matches!(status.model_importer, ModelImporterState::Unverified { ref error, .. }
        if error.contains(&game.join("Core").display().to_string()))
    );
}

#[tokio::test]
async fn database_read_failure_is_a_command_error_not_unset_configuration() {
    use tauri::Manager;
    let (_tmp, core, db) = fixture().await;
    sqlx::query("DROP TABLE games").execute(&db).await.unwrap();
    let app = tauri::test::mock_app();
    app.manage(core);
    let error = gmm_lib::commands::get_game_path_status(app.state(), GameCode::Gimi)
        .await
        .expect_err("database uncertainty must not become no saved path");
    assert_eq!(error.kind, gmm_lib::core::error::SurfaceFailureKind::Other);
    assert!(error.message.contains("database error"));
}

#[tokio::test]
async fn tauri_read_command_serializes_the_separate_facts() {
    use tauri::Manager;
    let (tmp, core, db) = fixture().await;
    let game = tmp.path().join("game");
    support::seed_game_install(GameCode::Gimi, &game);
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    let app = tauri::test::mock_app();
    app.manage(core);
    let status = gmm_lib::commands::get_game_path_status(app.state(), GameCode::Gimi)
        .await
        .unwrap();
    let json = serde_json::to_value(status).unwrap();
    assert_eq!(json["game"], "gimi");
    assert_eq!(json["savedPath"], game.to_str().unwrap());
    assert_eq!(json["gamePath"]["status"], "readable");
    assert_eq!(json["modelImporter"]["status"], "noImporterEntriesFound");
    assert_eq!(
        json["modelImporter"]["paths"][0],
        game.join("d3dx.ini").to_str().unwrap()
    );
}

async fn database_snapshot(db: &SqlitePool) -> Vec<(String, Vec<String>)> {
    let tables: Vec<String> = sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .fetch_all(db).await.unwrap();
    let mut snapshot = Vec::new();
    for table in tables {
        let columns = sqlx::query(&format!("PRAGMA table_info(\"{table}\")"))
            .fetch_all(db)
            .await
            .unwrap();
        let values = columns
            .iter()
            .map(|row| format!("quote(\"{}\")", row.get::<String, _>("name")))
            .collect::<Vec<_>>()
            .join(" || '|' || ");
        let rows = sqlx::query_scalar(&format!("SELECT {values} FROM \"{table}\" ORDER BY 1"))
            .fetch_all(db)
            .await
            .unwrap();
        snapshot.push((table, rows));
    }
    snapshot
}

#[tokio::test]
async fn check_never_persists_settings_or_recovery_records() {
    let (tmp, core, db) = fixture().await;
    let game = tmp.path().join("game");
    support::seed_game_install(GameCode::Gimi, &game);
    importer(&game);
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    gmm_lib::core::updates::set_importer_installed(&db, GameCode::Gimi, "v1.0.0")
        .await
        .unwrap();
    // A pending witness must neither block this read nor be reconciled by it.
    sqlx::query("INSERT INTO importer_evacuations (token, game_code, game_path, game_identity, backup_path, backup_identity, backup_root_identity, entries_json, owner_pid, owner_active, created_at) VALUES ('read-only-test', 'gimi', ?, '0000000000000001:0000000000000002', ?, '0000000000000001:0000000000000003', '0000000000000001:0000000000000004', '[\"d3dx.ini\"]', 0, 0, '2026-08-28T00:00:00Z')")
        .bind(game.to_str().unwrap())
        .bind(tmp.path().join("backups/read-only-test").to_str().unwrap())
        .execute(&db).await.unwrap();
    let before = database_snapshot(&db).await;
    // Make even a write-then-rollback or same-value write observable.
    for (table, _) in &before {
        for operation in ["INSERT", "UPDATE", "DELETE"] {
            sqlx::query(&format!("CREATE TRIGGER \"forbid_{table}_{operation}\" BEFORE {operation} ON \"{table}\" BEGIN SELECT RAISE(ABORT, 'path-state check attempted a write'); END"))
                .execute(&db).await.unwrap();
        }
    }
    let status = core.game_path_status(GameCode::Gimi).await.unwrap();
    assert_eq!(status.game_path, GamePathState::Readable);
    assert!(matches!(
        status.model_importer,
        ModelImporterState::Installed { .. }
    ));
    fs::remove_file(game.join("d3dx.ini")).unwrap();
    assert!(matches!(
        core.game_path_status(GameCode::Gimi)
            .await
            .unwrap()
            .model_importer,
        ModelImporterState::Damaged { .. }
    ));
    fs::remove_dir_all(&game).unwrap();
    assert!(matches!(
        core.game_path_status(GameCode::Gimi)
            .await
            .unwrap()
            .game_path,
        GamePathState::SavedPathNotFound { .. }
    ));
    assert_eq!(
        database_snapshot(&db).await,
        before,
        "all settings and recovery rows must remain byte-for-byte equivalent"
    );
}

#[tokio::test]
async fn recorded_healthy_install_exposes_local_version_and_origin() {
    let (tmp, core, db) = fixture().await;
    let game = tmp.path().join("game");
    support::seed_game_install(GameCode::Gimi, &game);
    importer(&game);
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    gmm_lib::core::updates::set_importer_installed(&db, GameCode::Gimi, "v3.4.1")
        .await
        .unwrap();
    let origin =
        gmm_lib::core::importer_origin::ImporterOrigin::github("Example", "Package", "Package.zip");
    sqlx::query("INSERT INTO settings (key, value) VALUES ('importer.gimi.installed_origin', ?)")
        .bind(serde_json::to_string(&origin).unwrap())
        .execute(&db)
        .await
        .unwrap();
    let state = core
        .game_path_status(GameCode::Gimi)
        .await
        .unwrap()
        .model_importer;
    let ModelImporterState::Installed { record } = &state else {
        panic!("{state:?}")
    };
    assert_eq!(record.installed_version.as_deref(), Some("v3.4.1"));
    assert_eq!(
        record.origin,
        gmm_lib::core::importer_origin::InstalledOrigin::Known(origin)
    );
    assert_eq!(serde_json::to_value(&state).unwrap()["status"], "installed");
}

#[tokio::test]
async fn hand_installed_files_are_unrecorded_not_absent_and_pin_is_not_a_receipt() {
    let (tmp, core, db) = fixture().await;
    let game = tmp.path().join("game");
    support::seed_game_install(GameCode::Gimi, &game);
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    core.set_importer_pinned(GameCode::Gimi, Some("v9"))
        .await
        .unwrap();
    let before = database_snapshot(&db).await;
    let state = core
        .game_path_status(GameCode::Gimi)
        .await
        .unwrap()
        .model_importer;
    assert!(
        matches!(state, ModelImporterState::NoImporterEntriesFound { .. }),
        "{state:?}"
    );
    assert_eq!(
        serde_json::to_value(&state).unwrap()["status"],
        "noImporterEntriesFound"
    );
    importer(&game);
    let state = core
        .game_path_status(GameCode::Gimi)
        .await
        .unwrap()
        .model_importer;
    assert_eq!(
        state,
        ModelImporterState::UnrecordedImporterPresent { paths: vec![] }
    );
    assert_eq!(
        serde_json::to_value(&state).unwrap()["status"],
        "unrecordedImporterPresent"
    );
    fs::remove_file(game.join("d3dx.ini")).unwrap();
    assert_eq!(
        core.game_path_status(GameCode::Gimi)
            .await
            .unwrap()
            .model_importer,
        ModelImporterState::UnrecordedImporterPresent {
            paths: vec![game.join("d3dx.ini")]
        }
    );
    assert_eq!(
        database_snapshot(&db).await,
        before,
        "hand installs never gain receipts"
    );
}

#[tokio::test]
async fn version_only_receipt_keeps_unknown_origin_without_backfilling() {
    let (tmp, core, db) = fixture().await;
    let game = tmp.path().join("game");
    support::seed_game_install(GameCode::Gimi, &game);
    importer(&game);
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    gmm_lib::core::updates::set_importer_installed(&db, GameCode::Gimi, "v1")
        .await
        .unwrap();
    let before = database_snapshot(&db).await;
    let state = core
        .game_path_status(GameCode::Gimi)
        .await
        .unwrap()
        .model_importer;
    let ModelImporterState::Installed { record } = state else {
        panic!("{state:?}")
    };
    assert_eq!(
        record.origin,
        gmm_lib::core::importer_origin::InstalledOrigin::Unknown
    );
    assert_eq!(database_snapshot(&db).await, before);
}

#[tokio::test]
async fn origin_only_and_unreadable_receipts_still_establish_recorded_damage() {
    let (tmp, core, db) = fixture().await;
    let game = tmp.path().join("game");
    support::seed_game_install(GameCode::Gimi, &game);
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    for raw in [
        r#"{"kind":"gitHubRelease","owner":"Example","repo":"Package","asset_pattern":"Package.zip"}"#,
        "invalid origin",
    ] {
        sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES ('importer.gimi.installed_origin', ?)")
            .bind(raw).execute(&db).await.unwrap();
        let state = core
            .game_path_status(GameCode::Gimi)
            .await
            .unwrap()
            .model_importer;
        let ModelImporterState::Damaged { record, .. } = state else {
            panic!("{state:?}")
        };
        assert_eq!(record.installed_version, None);
        assert_ne!(
            record.origin,
            gmm_lib::core::importer_origin::InstalledOrigin::Unknown
        );
    }
}

#[tokio::test]
async fn origin_change_clears_receipt_and_reports_uncertainty_for_both_layouts() {
    let (tmp, core, db) = fixture().await;
    let game = tmp.path().join("game");
    support::seed_game_install(GameCode::Gimi, &game);
    importer(&game);
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    gmm_lib::core::updates::set_importer_installed(&db, GameCode::Gimi, "v1")
        .await
        .unwrap();
    let old =
        gmm_lib::core::importer_origin::ImporterOrigin::github("Example", "Old", "Package.zip");
    sqlx::query("INSERT INTO settings (key, value) VALUES ('importer.gimi.installed_origin', ?)")
        .bind(serde_json::to_string(&old).unwrap())
        .execute(&db)
        .await
        .unwrap();
    let origin =
        gmm_lib::core::importer_origin::ImporterOrigin::github("Example", "New", "Package.zip");
    core.set_importer_origin_override(GameCode::Gimi, Some(&origin))
        .await
        .unwrap();
    assert_eq!(
        core.game_path_status(GameCode::Gimi)
            .await
            .unwrap()
            .model_importer,
        ModelImporterState::UnrecordedImporterPresent { paths: vec![] }
    );
    fs::remove_file(game.join("d3dx.ini")).unwrap();
    fs::remove_dir(game.join("Core")).unwrap();
    fs::remove_dir(game.join("ShaderFixes")).unwrap();
    assert!(matches!(
        core.game_path_status(GameCode::Gimi)
            .await
            .unwrap()
            .model_importer,
        ModelImporterState::NoImporterEntriesFound { .. }
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn recorded_unreadable_entry_is_damaged_and_wrong_type_stays_unverified() {
    use std::os::unix::fs::PermissionsExt;
    let (tmp, core, db) = fixture().await;
    let game = tmp.path().join("game");
    support::seed_game_install(GameCode::Gimi, &game);
    importer(&game);
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    gmm_lib::core::updates::set_importer_installed(&db, GameCode::Gimi, "v1")
        .await
        .unwrap();
    let entry = game.join("d3dx.ini");
    fs::set_permissions(&entry, fs::Permissions::from_mode(0o000)).unwrap();
    let result = core.game_path_status(GameCode::Gimi).await;
    fs::set_permissions(&entry, fs::Permissions::from_mode(0o600)).unwrap();
    let state = result.unwrap().model_importer;
    assert!(
        matches!(state, ModelImporterState::Damaged { ref paths, error: Some(ref error), .. }
        if paths == &vec![entry.clone()] && error.contains(&entry.display().to_string())),
        "{state:?}"
    );
    fs::remove_file(&entry).unwrap();
    fs::create_dir(&entry).unwrap();
    assert!(
        matches!(core.game_path_status(GameCode::Gimi).await.unwrap().model_importer,
        ModelImporterState::Unverified { ref error, record: Some(_), .. } if error.contains("must be a file"))
    );
}

#[tokio::test]
async fn local_pin_version_is_independent_of_install_receipts() {
    use tauri::Manager;
    let (tmp, core, db) = fixture().await;
    let status = core.game_path_status(GameCode::Gimi).await.unwrap();
    assert_eq!(status.pinned_version, None);
    assert!(serde_json::to_value(status).unwrap()["pinnedVersion"].is_null());
    core.set_importer_pinned(GameCode::Gimi, Some("v0.9"))
        .await
        .unwrap();
    let status = core.game_path_status(GameCode::Gimi).await.unwrap();
    assert_eq!(status.pinned_version.as_deref(), Some("v0.9"));
    assert_eq!(status.game_path, GamePathState::NoSavedPath);
    assert!(matches!(
        status.model_importer,
        ModelImporterState::Unverified { record: None, .. }
    ));
    assert_eq!(
        core.game_path_status(GameCode::Srmi)
            .await
            .unwrap()
            .pinned_version,
        None
    );

    let game = tmp.path().join("game");
    support::seed_game_install(GameCode::Gimi, &game);
    importer(&game);
    save_fixture_path(&db, GameCode::Gimi, &game).await;
    let status = core.game_path_status(GameCode::Gimi).await.unwrap();
    assert_eq!(status.pinned_version.as_deref(), Some("v0.9"));
    assert!(matches!(
        status.model_importer,
        ModelImporterState::UnrecordedImporterPresent { .. }
    ));
    gmm_lib::core::updates::set_importer_installed(&db, GameCode::Gimi, "v1.0")
        .await
        .unwrap();
    let app = tauri::test::mock_app();
    app.manage(core);
    let status = gmm_lib::commands::get_game_path_status(app.state(), GameCode::Gimi)
        .await
        .unwrap();
    assert!(matches!(
        status.model_importer,
        ModelImporterState::Installed { .. }
    ));
    let json = serde_json::to_value(status).unwrap();
    assert_eq!(json["pinnedVersion"], "v0.9");
    assert_eq!(json["modelImporter"]["record"]["installedVersion"], "v1.0");

    fs::remove_dir(game.join("Core")).unwrap();
    let core = app.state::<Core>();
    let status = core.game_path_status(GameCode::Gimi).await.unwrap();
    assert!(matches!(
        status.model_importer,
        ModelImporterState::Damaged { .. }
    ));
    assert_eq!(status.pinned_version.as_deref(), Some("v0.9"));
    core.set_importer_pinned(GameCode::Gimi, None)
        .await
        .unwrap();
    assert_eq!(
        core.game_path_status(GameCode::Gimi)
            .await
            .unwrap()
            .pinned_version,
        None
    );
}
