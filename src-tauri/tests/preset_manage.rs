mod support;

use gmm_lib::core::{Core, Error, GameCode, Mod};
use sqlx::{Row, SqlitePool};
use std::{fs, path::PathBuf};
use tempfile::TempDir;

struct Fixture {
    tmp: TempDir,
    core: Core,
    pool: SqlitePool,
    mods: PathBuf,
}
impl Fixture {
    async fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let url = format!("sqlite://{}?mode=rwc", tmp.path().join("gmm.db").display());
        let core = Core::new(tmp.path().join("library"), &url).await.unwrap();
        let pool = SqlitePool::connect(&url).await.unwrap();
        let game = tmp.path().join("game");
        core.set_game_install_path(
            GameCode::Gimi,
            support::seed_game_install(GameCode::Gimi, &game),
        )
        .await
        .unwrap();
        let mods = game.join("Mods");
        fs::create_dir_all(&mods).unwrap();
        Self {
            tmp,
            core,
            pool,
            mods,
        }
    }
    async fn add(&self, name: &str, game: GameCode) -> Mod {
        let source = self.tmp.path().join(format!("source-{name}"));
        for color in ["Blue", "Red"] {
            fs::create_dir_all(source.join(color)).unwrap();
            fs::write(
                source.join(color).join("merged.ini"),
                "[TextureOverrideShared]\nhash = abc12345\n",
            )
            .unwrap();
        }
        self.core.adopt_folder(game, &source, name).await.unwrap()
    }
    async fn enable(&self, item: &Mod) {
        self.core
            .set_enabled(&item.id, true, &self.mods)
            .await
            .unwrap();
    }
}

#[cfg(unix)]
fn durable_directory_key(path: &std::path::Path) -> String {
    use std::os::unix::fs::MetadataExt as _;

    let metadata =
        std::fs::symlink_metadata(path).expect("directory metadata for transition witness");
    format!("{:016x}:{:016x}", metadata.dev(), metadata.ino())
}

#[cfg(windows)]
fn durable_directory_key(path: &std::path::Path) -> String {
    use std::fs::OpenOptions;
    use std::mem::MaybeUninit;
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let directory = OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .expect("open directory for transition witness identity");
    let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    let ok = unsafe { GetFileInformationByHandle(directory.as_raw_handle(), info.as_mut_ptr()) };
    assert_ne!(
        ok,
        0,
        "read directory identity for transition witness: {}",
        std::io::Error::last_os_error(),
    );
    let info = unsafe { info.assume_init() };
    let file = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    format!("{:016x}:{:016x}", info.dwVolumeSerialNumber, file)
}

async fn rows(pool: &SqlitePool, table: &str) -> Vec<Vec<String>> {
    let columns: Vec<String> = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(pool)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get("name"))
        .collect();
    let projection = columns
        .iter()
        .map(|c| format!("quote(\"{c}\")"))
        .collect::<Vec<_>>()
        .join(", ");
    sqlx::query(&format!("SELECT {projection} FROM {table} ORDER BY 1"))
        .fetch_all(pool)
        .await
        .unwrap()
        .iter()
        .map(|r| (0..columns.len()).map(|i| r.get(i)).collect())
        .collect()
}

async fn entries(f: &Fixture, id: &str) -> Vec<(String, Option<String>)> {
    sqlx::query_as(
        "SELECT mod_id, active_variant_id FROM preset_entries WHERE preset_id = ? ORDER BY mod_id",
    )
    .bind(id)
    .fetch_all(&f.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn rename_and_save_normalize_identically_to_the_same_stored_value() {
    let f = Fixture::new().await;
    let input = "\u{200b}\u{a0}Screen  \u{2003}shot\u{2060} ";
    let saved = f.core.save_preset(GameCode::Srmi, input).await.unwrap();
    let target = f.core.save_preset(GameCode::Gimi, "Before").await.unwrap();
    f.core
        .rename_preset(GameCode::Gimi, &target.id, input)
        .await
        .unwrap();
    let renamed = f.core.list_presets(GameCode::Gimi).await.unwrap().remove(0);
    assert_eq!(
        renamed.name, saved.name,
        "rename and save must store the same normalized name"
    );
    assert_eq!(renamed.name, "Screen shot");
    for invalid in [
        "\u{200b}",
        " \u{2060} ",
        "Raiden\nteam",
        "Raiden\tteam",
        "Raiden\u{7f}team",
    ] {
        assert!(matches!(
            f.core
                .rename_preset(GameCode::Gimi, &target.id, invalid)
                .await,
            Err(Error::InvalidPresetName)
        ));
    }
}

#[tokio::test]
async fn rename_refuses_another_presets_normalized_name_and_preserves_entries() {
    let f = Fixture::new().await;
    let item = f.add("Captured", GameCode::Gimi).await;
    f.enable(&item).await;
    let first = f
        .core
        .save_preset(GameCode::Gimi, "Screen shot")
        .await
        .unwrap();
    let second = f.core.save_preset(GameCode::Gimi, "Second").await.unwrap();
    let before = entries(&f, &second.id).await;
    let result = f
        .core
        .rename_preset(GameCode::Gimi, &second.id, " Screen  shot ")
        .await;
    assert!(
        matches!(result, Err(Error::PresetNameTaken { ref name }) if name == "Screen shot"),
        "rename must refuse PresetNameTaken: {result:?}"
    );
    f.core
        .rename_preset(GameCode::Gimi, &second.id, "Second")
        .await
        .unwrap();
    assert_eq!(entries(&f, &second.id).await, before);
    assert_eq!(
        f.core.list_presets(GameCode::Gimi).await.unwrap(),
        vec![first, second]
    );
    f.core
        .rename_preset(GameCode::Gimi, &item.id, "Unused")
        .await
        .unwrap_err();
    f.core
        .rename_preset(
            GameCode::Gimi,
            &f.core.save_preset(GameCode::Gimi, "Case").await.unwrap().id,
            "case",
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn management_refuses_missing_ids_and_presets_from_another_game() {
    let f = Fixture::new().await;
    let foreign = f.core.save_preset(GameCode::Srmi, "Other").await.unwrap();
    let before = rows(&f.pool, "presets").await;
    for id in ["missing", foreign.id.as_str()] {
        assert!(matches!(
            f.core.rename_preset(GameCode::Gimi, id, "New").await,
            Err(Error::PresetNotFound)
        ));
        assert!(matches!(
            f.core.delete_preset(GameCode::Gimi, id).await,
            Err(Error::PresetNotFound)
        ));
        assert!(matches!(
            f.core.recapture_preset(GameCode::Gimi, id).await,
            Err(Error::PresetNotFound)
        ));
    }
    assert_eq!(rows(&f.pool, "presets").await, before);
}

#[tokio::test]
async fn recapture_replaces_previous_entries_and_keeps_id_name_and_created_at() {
    let f = Fixture::new().await;
    let old = f.add("Old", GameCode::Gimi).await;
    f.enable(&old).await;
    let preset = f
        .core
        .save_preset(GameCode::Gimi, "Replace me")
        .await
        .unwrap();
    f.core.set_enabled(&old.id, false, &f.mods).await.unwrap();
    let new = f.add("New", GameCode::Gimi).await;
    f.enable(&new).await;
    let root = f.add("Root", GameCode::Gimi).await;
    f.core
        .set_active_variant(&root.id, None, &f.mods)
        .await
        .unwrap();
    f.enable(&root).await;
    let _other = f.add("Other", GameCode::Srmi).await;
    f.core
        .recapture_preset(GameCode::Gimi, &preset.id)
        .await
        .unwrap();
    let mut expected = vec![
        (
            new.id.clone(),
            f.core.active_variant_id(&new.id).await.unwrap(),
        ),
        (root.id.clone(), None),
    ];
    expected.sort();
    assert_eq!(
        entries(&f, &preset.id).await,
        expected,
        "re-capture must replace, never append to, the previous entries"
    );
    let mut expected_preset = preset.clone();
    expected_preset.mod_count = 2;
    assert_eq!(
        f.core.list_presets(GameCode::Gimi).await.unwrap(),
        vec![expected_preset]
    );
    f.core.set_enabled(&new.id, false, &f.mods).await.unwrap();
    f.core.set_enabled(&root.id, false, &f.mods).await.unwrap();
    f.core
        .recapture_preset(GameCode::Gimi, &preset.id)
        .await
        .unwrap();
    assert!(
        entries(&f, &preset.id).await.is_empty(),
        "re-capture can replace a populated preset with the empty setup"
    );
}

#[tokio::test]
async fn delete_cascades_all_saved_entries_and_has_no_mod_bearing_foreign_keys() {
    let f = Fixture::new().await;
    let item = f.add("Saved", GameCode::Gimi).await;
    f.enable(&item).await;
    let deleted = f.core.save_preset(GameCode::Gimi, "Delete").await.unwrap();
    let kept = f.core.save_preset(GameCode::Gimi, "Keep").await.unwrap();
    let kept_entries = entries(&f, &kept.id).await;
    assert_eq!(entries(&f, &deleted.id).await.len(), 1);
    f.core
        .delete_preset(GameCode::Gimi, &deleted.id)
        .await
        .unwrap();
    assert!(
        entries(&f, &deleted.id).await.is_empty(),
        "preset delete must cascade to every saved entry"
    );
    assert_eq!(entries(&f, &kept.id).await, kept_entries);
    assert_eq!(
        f.core.list_presets(GameCode::Gimi).await.unwrap(),
        vec![kept]
    );
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table'")
            .fetch_all(&f.pool)
            .await
            .unwrap();
    for table in tables {
        for fk in sqlx::query(&format!("PRAGMA foreign_key_list(\"{table}\")"))
            .fetch_all(&f.pool)
            .await
            .unwrap()
        {
            if fk.get::<String, _>("table") == "presets" {
                assert_eq!(
                    table, "preset_entries",
                    "only saved descriptions may cascade from presets"
                );
                assert_eq!(fk.get::<String, _>("on_delete"), "CASCADE");
            }
        }
    }
}

async fn plant_transition(f: &Fixture, item: &Mod) {
    sqlx::query("INSERT INTO enabled_transitions (mod_id, game_code, intended_enabled, junction_path, junction_target, junction_parent_identity, junction_identity, owner_pid, owner_started_at, owner_active, created_at, junction_target_identity, library_identity) VALUES (?, ?, 0, 'link', 'target', '0000000000000001:0000000000000002', '0000000000000001:0000000000000003', 42, 1, 1, '2026-10-29T00:00:00Z', '0000000000000001:0000000000000004', '0000000000000001:0000000000000005')")
        .bind(&item.id).bind(item.game.as_str()).execute(&f.pool).await.unwrap();
}

#[tokio::test]
async fn recapture_refuses_pending_transition_without_replacing_saved_entries() {
    let f = Fixture::new().await;
    let preset = f
        .core
        .save_preset(GameCode::Gimi, "Established")
        .await
        .unwrap();
    let item = f.add("Unsettled", GameCode::Srmi).await;
    plant_transition(&f, &item).await;
    let before = rows(&f.pool, "enabled_transitions").await;
    let result = f.core.recapture_preset(GameCode::Gimi, &preset.id).await;
    assert!(
        matches!(result, Err(Error::EnabledTransitionPending { ref mod_id }) if mod_id == &item.id),
        "re-capture must refuse pending enabled transitions globally: {result:?}"
    );
    assert_eq!(
        f.core.list_presets(GameCode::Gimi).await.unwrap(),
        vec![preset]
    );
    assert_eq!(rows(&f.pool, "enabled_transitions").await, before);
}

#[tokio::test]
async fn rename_delete_and_recapture_preserve_every_mod_row_variant_transition_and_junction_identity(
) {
    let f = Fixture::new().await;
    let item = f.add("Deployed", GameCode::Gimi).await;
    f.enable(&item).await;
    let other = f.add("Other Game", GameCode::Srmi).await;
    let _disabled = f.add("Disabled", GameCode::Gimi).await;
    let preset = f.core.save_preset(GameCode::Gimi, "Saved").await.unwrap();
    let identity = durable_directory_key(&f.mods.join("Deployed"));
    let before_mods = rows(&f.pool, "mods").await;
    let before_variants = rows(&f.pool, "mod_variants").await;
    for operation in ["rename", "recapture", "delete"] {
        // Rename and delete must also work while Mod state is unsettled.
        if operation == "delete" {
            plant_transition(&f, &other).await;
        }
        let before_transitions = rows(&f.pool, "enabled_transitions").await;
        match operation {
            "rename" => f
                .core
                .rename_preset(GameCode::Gimi, &preset.id, "Renamed")
                .await
                .unwrap(),
            "recapture" => f
                .core
                .recapture_preset(GameCode::Gimi, &preset.id)
                .await
                .unwrap(),
            _ => {
                f.core
                    .rename_preset(GameCode::Gimi, &preset.id, "Renamed again")
                    .await
                    .unwrap();
                f.core
                    .delete_preset(GameCode::Gimi, &preset.id)
                    .await
                    .unwrap();
            }
        }
        assert_eq!(
            rows(&f.pool, "mods").await,
            before_mods,
            "{operation} changed Mod rows, including active_variant_id"
        );
        assert_eq!(
            rows(&f.pool, "mod_variants").await,
            before_variants,
            "{operation} changed Variants"
        );
        assert_eq!(
            rows(&f.pool, "enabled_transitions").await,
            before_transitions,
            "{operation} changed enabled transitions"
        );
        assert_eq!(
            durable_directory_key(&f.mods.join("Deployed")),
            identity,
            "{operation} replaced the deployed Junction identity"
        );
    }
}
