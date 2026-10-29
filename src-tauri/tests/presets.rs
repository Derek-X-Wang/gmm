use gmm_lib::core::{Core, Error, GameCode};
use sqlx::{Row, SqlitePool};
use tempfile::TempDir;

async fn fixture() -> (TempDir, Core, SqlitePool) {
    let tmp = TempDir::new().unwrap();
    let url = format!("sqlite://{}?mode=rwc", tmp.path().join("gmm.db").display());
    let core = Core::new(tmp.path().join("library"), &url).await.unwrap();
    let pool = SqlitePool::connect(&url).await.unwrap();
    (tmp, core, pool)
}

async fn seed_mod(pool: &SqlitePool, id: &str, game: &str, enabled: bool) {
    // Deliberately absent Library content: capture reads database state only.
    sqlx::query("INSERT INTO mods (id, game_code, name, source, library_path, junction_dir_name, enabled, created_at) VALUES (?, ?, ?, 'manual', 'absent', ?, ?, 'now')")
        .bind(id).bind(game).bind(id).bind(id).bind(enabled).execute(pool).await.unwrap();
}

#[tokio::test]
async fn capture_preserves_enabled_mods_and_active_variants_including_null() {
    let (_tmp, core, pool) = fixture().await;
    seed_mod(&pool, "blue-mod", "gimi", true).await;
    seed_mod(&pool, "root-mod", "gimi", true).await;
    seed_mod(&pool, "disabled-mod", "gimi", false).await;
    seed_mod(&pool, "other-game-mod", "srmi", true).await;
    sqlx::query("INSERT INTO mod_variants (id, mod_id, name, subpath) VALUES ('blue', 'blue-mod', 'Blue', 'Blue')").execute(&pool).await.unwrap();
    sqlx::query("UPDATE mods SET active_variant_id = 'blue' WHERE id = 'blue-mod'")
        .execute(&pool)
        .await
        .unwrap();
    let before = core.list_mods(GameCode::Gimi).await.unwrap();
    let preset = core
        .save_preset(GameCode::Gimi, "Screenshot")
        .await
        .unwrap();
    assert_eq!(preset.mod_count, 2);
    let entries = sqlx::query(
        "SELECT mod_id, active_variant_id FROM preset_entries WHERE preset_id = ? ORDER BY mod_id",
    )
    .bind(&preset.id)
    .fetch_all(&pool)
    .await
    .unwrap();
    let entries: Vec<(String, Option<String>)> = entries
        .iter()
        .map(|row| (row.get("mod_id"), row.get("active_variant_id")))
        .collect();
    assert_eq!(
        entries,
        vec![
            ("blue-mod".into(), Some("blue".into())),
            ("root-mod".into(), None)
        ],
        "capture must preserve each active Variant identity, including null"
    );
    assert_eq!(
        core.list_mods(GameCode::Gimi).await.unwrap(),
        before,
        "capture must not change Mod state"
    );
    assert_eq!(
        core.list_presets(GameCode::Gimi).await.unwrap(),
        vec![preset]
    );
}

#[tokio::test]
async fn listing_is_scoped_to_the_selected_game() {
    let (_tmp, core, _pool) = fixture().await;
    let genshin = core
        .save_preset(GameCode::Gimi, "Genshin only")
        .await
        .unwrap();
    let star_rail = core
        .save_preset(GameCode::Srmi, "Star Rail only")
        .await
        .unwrap();
    assert_eq!(
        core.list_presets(GameCode::Gimi).await.unwrap(),
        vec![genshin],
        "another Game's preset must never appear"
    );
    assert_eq!(
        core.list_presets(GameCode::Srmi).await.unwrap(),
        vec![star_rail]
    );
    assert!(core.list_presets(GameCode::Zzmi).await.unwrap().is_empty());
}

#[tokio::test]
async fn duplicate_name_is_refused_within_game_but_allowed_for_another_game() {
    let (_tmp, core, pool) = fixture().await;
    let first = core.save_preset(GameCode::Gimi, "Patch day").await.unwrap();
    seed_mod(&pool, "later-mod", "gimi", true).await;
    let error = core
        .save_preset(GameCode::Gimi, "Patch day")
        .await
        .expect_err("duplicate name must be refused, never overwritten");
    assert!(matches!(error, Error::PresetNameTaken { .. }));
    assert!(error.to_string().contains("Patch day"));
    assert!(error.to_string().contains("gimi"));
    assert_eq!(
        core.list_presets(GameCode::Gimi).await.unwrap(),
        vec![first]
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM preset_entries")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0,
        "duplicate capture must roll back"
    );
    core.save_preset(GameCode::Srmi, "Patch day").await.unwrap();
    assert_eq!(core.list_presets(GameCode::Srmi).await.unwrap().len(), 1);
}

#[tokio::test]
async fn empty_enabled_set_is_saved_and_listed_as_zero_mods() {
    let (_tmp, core, pool) = fixture().await;
    seed_mod(&pool, "disabled-mod", "gimi", false).await;
    let preset = core
        .save_preset(GameCode::Gimi, "Clean for patch day")
        .await
        .unwrap();
    assert_eq!(preset.mod_count, 0);
    assert_eq!(
        core.list_presets(GameCode::Gimi).await.unwrap(),
        vec![preset]
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM preset_entries")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0,
        "disabled Mods must not be captured"
    );
}

#[tokio::test]
async fn names_are_trimmed_and_blank_names_are_refused() {
    let (_tmp, core, _pool) = fixture().await;
    assert!(matches!(
        core.save_preset(GameCode::Gimi, "  ").await,
        Err(Error::InvalidPresetName)
    ));
    let preset = core
        .save_preset(GameCode::Gimi, "  Screenshot  ")
        .await
        .unwrap();
    assert_eq!(preset.name, "Screenshot");
    assert!(matches!(
        core.save_preset(GameCode::Gimi, "Screenshot").await,
        Err(Error::PresetNameTaken { .. })
    ));
}

#[tokio::test]
async fn entries_keep_identities_after_library_records_are_removed_and_store_no_paths() {
    let (_tmp, core, pool) = fixture().await;
    seed_mod(&pool, "mod", "gimi", true).await;
    sqlx::query("INSERT INTO mod_variants (id, mod_id, name, subpath) VALUES ('variant', 'mod', 'Blue', 'Blue')").execute(&pool).await.unwrap();
    sqlx::query("UPDATE mods SET active_variant_id = 'variant' WHERE id = 'mod'")
        .execute(&pool)
        .await
        .unwrap();
    let preset = core
        .save_preset(GameCode::Gimi, "Keep identities")
        .await
        .unwrap();
    sqlx::query("DELETE FROM mods WHERE id = 'mod'")
        .execute(&pool)
        .await
        .unwrap();
    let row =
        sqlx::query("SELECT mod_id, active_variant_id FROM preset_entries WHERE preset_id = ?")
            .bind(&preset.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.get::<String, _>("mod_id"), "mod");
    assert_eq!(
        row.get::<Option<String>, _>("active_variant_id"),
        Some("variant".into())
    );
    for (table, expected) in [
        ("presets", vec!["id", "game_code", "name", "created_at"]),
        (
            "preset_entries",
            vec!["preset_id", "mod_id", "active_variant_id"],
        ),
    ] {
        let columns: Vec<String> = sqlx::query(&format!("PRAGMA table_info({table})"))
            .fetch_all(&pool)
            .await
            .unwrap()
            .iter()
            .map(|row| row.get("name"))
            .collect();
        assert_eq!(
            columns, expected,
            "preset rows contain identities and metadata only"
        );
    }
    assert_eq!(
        core.list_presets(GameCode::Gimi).await.unwrap(),
        vec![preset]
    );
}
