//! The Genshin install path the user picks (or auto-detects in a later
//! slice) must survive a restart — it's stored in the games table.

use std::fs;

use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

#[tokio::test]
async fn game_install_path_round_trips_through_the_db() {
    let tmp = TempDir::new().expect("tmp");
    let library_root = tmp.path().join("library");
    let game_path = tmp.path().join("game/Genshin Impact Game");
    fs::create_dir_all(&game_path).expect("game dir");
    fs::write(game_path.join("GenshinImpact.exe"), b"game").expect("game exe");
    fs::create_dir(game_path.join("GenshinImpact_Data")).expect("game data");

    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(library_root, &db_url).await.expect("init");

    // Fresh DB: no path persisted yet.
    let initial = core
        .game_install_path(GameCode::Gimi)
        .await
        .expect("read empty");
    assert!(initial.is_none(), "no install path set on a fresh DB");

    // Set, then read back.
    core.set_game_install_path(GameCode::Gimi, &game_path)
        .await
        .expect("write");

    let after = core.game_install_path(GameCode::Gimi).await.expect("read");
    assert_eq!(after, Some(game_path.clone()));

    // Reopen the same DB to prove persistence (not just in-memory state).
    drop(core);
    let db_url2 = db_url.clone();
    let core2 = Core::new(tmp.path().join("library"), &db_url2)
        .await
        .expect("reopen");

    let reopened = core2
        .game_install_path(GameCode::Gimi)
        .await
        .expect("read after reopen");
    assert_eq!(reopened, Some(game_path));
}

#[tokio::test]
async fn missing_game_path_is_refused_without_replacing_the_saved_path() {
    let tmp = TempDir::new().expect("tmp");
    let core = Core::new(tmp.path().join("library"), "sqlite::memory:")
        .await
        .expect("init");
    let missing = tmp.path().join("missing");
    let error = core
        .set_game_install_path(GameCode::Gimi, &missing)
        .await
        .expect_err("a missing game path must be refused");
    let command_error = gmm_lib::command_error::CommandError::from(error);
    let payload = serde_json::to_value(command_error).expect("structured error");
    assert_eq!(payload["kind"], "other");
    assert!(payload["message"]
        .as_str()
        .unwrap()
        .contains("Genshin Impact"));
    assert_eq!(core.game_install_path(GameCode::Gimi).await.unwrap(), None);
}

#[tokio::test]
async fn wrong_game_folder_is_refused_and_preserves_the_saved_path() {
    let tmp = TempDir::new().expect("tmp");
    let core = Core::new(tmp.path().join("library"), "sqlite::memory:")
        .await
        .unwrap();
    let valid = tmp.path().join("valid");
    fs::create_dir_all(valid.join("GenshinImpact_Data")).unwrap();
    fs::write(valid.join("GenshinImpact.exe"), b"game").unwrap();
    core.set_game_install_path(GameCode::Gimi, &valid)
        .await
        .unwrap();
    let wrong = tmp.path().join("wrong");
    fs::create_dir_all(wrong.join("StarRail_Data")).unwrap();
    fs::write(wrong.join("StarRail.exe"), b"game").unwrap();
    let error = core
        .set_game_install_path(GameCode::Gimi, &wrong)
        .await
        .expect_err("a different game's folder must be refused");
    assert!(error.to_string().contains("Genshin Impact"));
    assert_eq!(
        core.game_install_path(GameCode::Gimi).await.unwrap(),
        Some(valid)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn unreadable_game_folder_is_an_io_error_not_a_wrong_game() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new().unwrap();
    let core = Core::new(tmp.path().join("library"), "sqlite::memory:")
        .await
        .unwrap();
    let candidate = tmp.path().join("unreadable");
    fs::create_dir(&candidate).unwrap();
    fs::set_permissions(&candidate, fs::Permissions::from_mode(0o000)).unwrap();
    let probe = fs::read_dir(&candidate).expect_err("fixture must really be unreadable");
    assert_eq!(probe.kind(), std::io::ErrorKind::PermissionDenied);
    let result = core.set_game_install_path(GameCode::Gimi, &candidate).await;
    fs::set_permissions(&candidate, fs::Permissions::from_mode(0o700)).unwrap();
    let error = result.expect_err("an unreadable game folder must be refused");
    assert!(
        matches!(error, gmm_lib::core::Error::Io { ref source, .. }
        if source.kind() == std::io::ErrorKind::PermissionDenied),
        "uncertainty must stay an I/O error: {error}"
    );
    assert_eq!(core.game_install_path(GameCode::Gimi).await.unwrap(), None);
}

#[cfg(unix)]
#[tokio::test]
async fn uncertain_executable_is_an_io_error_not_a_wrong_game() {
    use std::os::unix::fs::symlink;
    let tmp = TempDir::new().unwrap();
    let core = Core::new(tmp.path().join("library"), "sqlite::memory:")
        .await
        .unwrap();
    let candidate = tmp.path().join("uncertain");
    fs::create_dir_all(candidate.join("GenshinImpact_Data")).unwrap();
    let exe = candidate.join("GenshinImpact.exe");
    symlink("GenshinImpact.exe", &exe).unwrap();
    assert_ne!(
        fs::metadata(&exe).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    let error = core
        .set_game_install_path(GameCode::Gimi, &candidate)
        .await
        .expect_err("an uncertain executable must be refused");
    assert!(
        matches!(error, gmm_lib::core::Error::Io { ref source, .. }
        if source.kind() != std::io::ErrorKind::NotFound),
        "uncertainty must stay an I/O error: {error}"
    );
    assert_eq!(core.game_install_path(GameCode::Gimi).await.unwrap(), None);
}

#[tokio::test]
async fn uncertain_path_lookup_preserves_the_io_error() {
    let tmp = TempDir::new().unwrap();
    let core = Core::new(tmp.path().join("library"), "sqlite::memory:")
        .await
        .unwrap();
    let candidate = tmp.path().join("invalid\0game");
    let probe = fs::metadata(&candidate).expect_err("fixture must fail lookup");
    assert_eq!(probe.kind(), std::io::ErrorKind::InvalidInput);
    let error = core
        .set_game_install_path(GameCode::Gimi, &candidate)
        .await
        .expect_err("an uncertain path lookup must be refused");
    assert!(
        matches!(error, gmm_lib::core::Error::Io { ref source, .. }
        if source.kind() == std::io::ErrorKind::InvalidInput),
        "uncertainty must stay an I/O error: {error}"
    );
    assert_eq!(core.game_install_path(GameCode::Gimi).await.unwrap(), None);
}

#[tokio::test]
async fn all_profiles_accept_their_game_layouts_including_alternate_executables() {
    let tmp = TempDir::new().unwrap();
    let core = Core::new(tmp.path().join("library"), "sqlite::memory:")
        .await
        .unwrap();
    let cases = [
        (
            GameCode::Gimi,
            "GenshinImpact.exe",
            "GenshinImpact_Data",
            false,
        ),
        (GameCode::Gimi, "YuanShen.exe", "GenshinImpact_Data", false),
        (GameCode::Srmi, "StarRail.exe", "StarRail_Data", false),
        (
            GameCode::Zzmi,
            "ZenlessZoneZero.exe",
            "ZenlessZoneZero_Data",
            false,
        ),
        (GameCode::Himi, "BH3.exe", "BH3_Data", false),
        (GameCode::Himi, "Bh3.exe", "BH3_Data", false),
        (GameCode::Wwmi, "Client-Win64-Shipping.exe", "Content", true),
        (
            GameCode::Efmi,
            "Endfield-Win64-Shipping.exe",
            "Content",
            true,
        ),
        (GameCode::Efmi, "Endfield.exe", "Content", true),
    ];
    for (index, (game, exe, data, unreal)) in cases.into_iter().enumerate() {
        let root = tmp.path().join(format!("install-{index}"));
        let candidate = if unreal {
            root.join("Binaries/Win64")
        } else {
            root.clone()
        };
        fs::create_dir_all(&candidate).unwrap();
        fs::create_dir_all(root.join(data)).unwrap();
        fs::write(candidate.join(exe), b"game").unwrap();
        core.set_game_install_path(game, &candidate)
            .await
            .expect("valid game layout");
        assert_eq!(
            core.game_install_path(game).await.unwrap(),
            Some(candidate.clone())
        );
        // Every other profile must refuse this layout, including the two
        // Unreal games that share the same Content-directory shape.
        for other in [
            GameCode::Gimi,
            GameCode::Srmi,
            GameCode::Zzmi,
            GameCode::Himi,
            GameCode::Wwmi,
            GameCode::Efmi,
        ] {
            if other != game {
                let before = core.game_install_path(other).await.unwrap();
                let error = core
                    .set_game_install_path(other, &candidate)
                    .await
                    .expect_err("another game's layout must be refused");
                assert!(matches!(
                    error,
                    gmm_lib::core::Error::InvalidGameInstallPath { .. }
                ));
                assert_eq!(core.game_install_path(other).await.unwrap(), before);
            }
        }
    }
}

#[tokio::test]
async fn files_and_incomplete_game_layouts_are_refused() {
    let tmp = TempDir::new().unwrap();
    let core = Core::new(tmp.path().join("library"), "sqlite::memory:")
        .await
        .unwrap();
    let file = tmp.path().join("file");
    fs::write(&file, b"file").unwrap();
    let exe_only = tmp.path().join("exe-only");
    fs::create_dir(&exe_only).unwrap();
    fs::write(exe_only.join("GenshinImpact.exe"), b"game").unwrap();
    let data_only = tmp.path().join("data-only");
    fs::create_dir_all(data_only.join("GenshinImpact_Data")).unwrap();
    let wrong_types = tmp.path().join("wrong-types");
    fs::create_dir_all(wrong_types.join("GenshinImpact.exe")).unwrap();
    fs::write(wrong_types.join("GenshinImpact_Data"), b"not a directory").unwrap();
    for candidate in [file, exe_only, data_only, wrong_types] {
        let error = core
            .set_game_install_path(GameCode::Gimi, &candidate)
            .await
            .expect_err("a directory with both game executable and data is required");
        assert!(matches!(
            error,
            gmm_lib::core::Error::InvalidGameInstallPath { .. }
        ));
        assert!(error.to_string().contains("GenshinImpact.exe"));
        assert_eq!(core.game_install_path(GameCode::Gimi).await.unwrap(), None);
    }
}

#[tokio::test]
async fn saved_path_command_returns_the_existing_structured_error() {
    use tauri::Manager;
    let tmp = TempDir::new().unwrap();
    let core = Core::new(tmp.path().join("library"), "sqlite::memory:")
        .await
        .unwrap();
    let app = tauri::test::mock_app();
    app.manage(core);
    let wrong = tmp.path().join("wrong");
    fs::create_dir(&wrong).unwrap();
    for candidate in [tmp.path().join("missing"), wrong] {
        let error =
            gmm_lib::commands::set_game_install_path(app.state(), GameCode::Gimi, candidate)
                .await
                .expect_err("the shared typed/picked command must refuse the path");
        let payload = serde_json::to_value(error).unwrap();
        assert_eq!(payload["kind"], "other");
        assert!(payload["message"]
            .as_str()
            .unwrap()
            .contains("Genshin Impact"));
        assert_eq!(
            app.state::<Core>()
                .game_install_path(GameCode::Gimi)
                .await
                .unwrap(),
            None
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn unreadable_executable_or_data_directory_preserves_the_io_error() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new().unwrap();
    let core = Core::new(tmp.path().join("library"), "sqlite::memory:")
        .await
        .unwrap();
    let candidate = tmp.path().join("game");
    let data = candidate.join("GenshinImpact_Data");
    let exe = candidate.join("GenshinImpact.exe");
    fs::create_dir_all(&data).unwrap();
    fs::write(&exe, b"game").unwrap();
    for (path, mode) in [(&exe, 0o600), (&data, 0o700)] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
        let result = core.set_game_install_path(GameCode::Gimi, &candidate).await;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
        let error = result.expect_err("unreadable game evidence must be refused");
        assert!(
            matches!(error, gmm_lib::core::Error::Io { ref source, ref path, .. }
            if source.kind() == std::io::ErrorKind::PermissionDenied && (path == &exe || path == &data)),
            "uncertainty in executable or data must stay an I/O error: {error}"
        );
        assert_eq!(core.game_install_path(GameCode::Gimi).await.unwrap(), None);
    }
    core.set_game_install_path(GameCode::Gimi, &candidate)
        .await
        .expect("readable install");
}

#[cfg(windows)]
#[tokio::test]
async fn unreadable_executable_is_an_io_error_not_a_wrong_game() {
    use std::os::windows::fs::OpenOptionsExt;
    let tmp = TempDir::new().unwrap();
    let core = Core::new(tmp.path().join("library"), "sqlite::memory:")
        .await
        .unwrap();
    let candidate = tmp.path().join("locked");
    fs::create_dir_all(candidate.join("GenshinImpact_Data")).unwrap();
    let exe = candidate.join("GenshinImpact.exe");
    fs::write(&exe, b"game").unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&exe)
        .unwrap();
    let probe = fs::File::open(&exe).expect_err("fixture must deny reading the executable");
    assert_ne!(probe.kind(), std::io::ErrorKind::NotFound);
    let error = core
        .set_game_install_path(GameCode::Gimi, &candidate)
        .await
        .expect_err("a locked executable must be refused");
    assert!(
        matches!(error, gmm_lib::core::Error::Io { ref source, .. }
        if source.kind() != std::io::ErrorKind::NotFound),
        "uncertainty must stay an I/O error: {error}"
    );
    assert_eq!(core.game_install_path(GameCode::Gimi).await.unwrap(), None);
    drop(lock);
    core.set_game_install_path(GameCode::Gimi, &candidate)
        .await
        .expect("readable after unlock");
}
