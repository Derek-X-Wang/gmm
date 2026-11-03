use gmm_lib::core::{Core, Error, GameCode};
use sqlx::SqlitePool;
use std::{fs, path::PathBuf};
use tempfile::TempDir;

async fn fixture() -> (TempDir, Core, SqlitePool, String, PathBuf, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let url = format!("sqlite://{}?mode=rwc", tmp.path().join("gmm.db").display());
    let core = Core::new(tmp.path().join("library"), &url).await.unwrap();
    let pool = SqlitePool::connect(&url).await.unwrap();
    let source = tmp.path().join("source");
    for color in ["Blue", "Red"] {
        fs::create_dir_all(source.join(color)).unwrap();
        fs::write(source.join(color).join("merged.ini"), "hash = shared\n").unwrap();
    }
    let item = core
        .adopt_folder(GameCode::Gimi, &source, "Variant Mod")
        .await
        .unwrap();
    let mods = tmp.path().join("game/Mods");
    fs::create_dir_all(&mods).unwrap();
    (tmp, core, pool, item.id, item.library_path, mods)
}

#[tokio::test]
async fn clear_enabled_variant_persists_null_and_retargets_to_library_root() {
    let (_tmp, core, _pool, id, root, mods) = fixture().await;
    core.set_enabled(&id, true, &mods).await.unwrap();
    let link = mods.join("Variant Mod");
    assert_eq!(
        fs::canonicalize(&link).unwrap(),
        fs::canonicalize(root.join("Blue")).unwrap()
    );
    core.set_active_variant(&id, None, &mods).await.unwrap();
    assert_eq!(
        core.active_variant_id(&id).await.unwrap(),
        None,
        "clearing must persist NULL"
    );
    assert_eq!(
        fs::canonicalize(&link).unwrap(),
        fs::canonicalize(&root).unwrap(),
        "clearing must deploy the Library root, not the old Variant subpath"
    );
}

#[tokio::test]
async fn clear_disabled_variant_persists_null_without_touching_deployment() {
    let (_tmp, core, _pool, id, _root, mods) = fixture().await;
    // A path with unrelated content makes any accidental retarget observable.
    let link = mods.join("Variant Mod");
    fs::create_dir(&link).unwrap();
    fs::write(link.join("keep"), "unchanged").unwrap();
    core.set_active_variant(&id, None, &mods).await.unwrap();
    assert_eq!(core.active_variant_id(&id).await.unwrap(), None);
    assert_eq!(fs::read_to_string(link.join("keep")).unwrap(), "unchanged");
}

#[tokio::test]
async fn nullable_selection_still_rejects_dangling_and_foreign_variant_ids() {
    let (_tmp, core, pool, id, root, mods) = fixture().await;
    core.set_enabled(&id, true, &mods).await.unwrap();
    let before = core.active_variant_id(&id).await.unwrap();
    sqlx::query("INSERT INTO mods (id, game_code, name, source, library_path, junction_dir_name, created_at) VALUES ('other', 'gimi', 'Other', 'manual', 'other', 'Other', 'now')").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO mod_variants (id, mod_id, name, subpath) VALUES ('foreign', 'other', 'Foreign', 'Foreign')").execute(&pool).await.unwrap();
    for selected in ["deleted-variant", "foreign"] {
        assert!(
            core.set_active_variant(&id, Some(selected), &mods)
                .await
                .is_err(),
            "a non-NULL missing or foreign Variant must be rejected, never cleared"
        );
        assert_eq!(core.active_variant_id(&id).await.unwrap(), before);
        assert_eq!(
            fs::canonicalize(mods.join("Variant Mod")).unwrap(),
            fs::canonicalize(root.join("Blue")).unwrap()
        );
    }
}

#[tokio::test]
async fn clear_variant_keeps_reinstall_recovery_guard() {
    let (_tmp, core, pool, id, root, mods) = fixture().await;
    let before = core.active_variant_id(&id).await.unwrap();
    let token = ulid::Ulid::new().to_string();
    let stage = root
        .parent()
        .unwrap()
        .join(format!(".gmm-reinstall-{token}"));
    let quarantine = root.parent().unwrap().join(format!(".gmm-delete-{token}"));
    sqlx::query("INSERT INTO reinstall_swaps (token, mod_id, game_code, library_path, staged_path, quarantine_path, old_identity, staged_identity, created_at, recovery_error, recovery_attempted_at, recovery_attempts) VALUES (?, ?, 'gimi', ?, ?, ?, '0000000000000001:0000000000000002', '0000000000000001:0000000000000003', '2026-10-29T00:00:00Z', 'needs recovery', '2026-10-29T00:00:01Z', 1)")
        .bind(token).bind(&id).bind(root.to_string_lossy().as_ref()).bind(stage.to_string_lossy().as_ref()).bind(quarantine.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
    assert!(matches!(
        core.set_active_variant(&id, None, &mods).await,
        Err(Error::ReinstallRecoveryQuarantined { .. })
    ));
    assert_eq!(core.active_variant_id(&id).await.unwrap(), before);
    assert!(!mods.join("Variant Mod").exists());
}
