use gmm_lib::core::{Core, GameCode};
use sqlx::SqlitePool;
use tempfile::TempDir;

async fn fixture() -> (TempDir, Core, SqlitePool) {
    let tmp = TempDir::new().unwrap();
    let url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(tmp.path().join("library"), &url).await.unwrap();
    let pool = SqlitePool::connect(&url).await.unwrap();
    (tmp, core, pool)
}

async fn adopt(tmp: &TempDir, core: &Core) -> gmm_lib::core::Mod {
    let source = tmp.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("mod.ini"), "[TextureOverride]\nhash=123\n").unwrap();
    core.adopt_folder(GameCode::Gimi, &source, "Example")
        .await
        .unwrap()
}

#[tokio::test]
async fn clean_library_is_safe() {
    let (_tmp, core, _pool) = fixture().await;
    let status = core.attention_status().await.unwrap();
    assert!(status.safe_to_proceed);
    assert_eq!(status.library_audits.len(), 6);
}

#[tokio::test]
async fn unreferenced_library_directory_needs_attention() {
    let (tmp, core, _pool) = fixture().await;
    std::fs::create_dir_all(tmp.path().join("library/gimi/orphan")).unwrap();
    let status = core.attention_status().await.unwrap();
    assert!(
        !status.safe_to_proceed,
        "an unreferenced Library directory must make status unsafe"
    );
    assert_eq!(status.library_audits[0].unreferenced.len(), 1);
}

#[tokio::test]
async fn backup_overlap_is_reported_instead_of_hiding_status() {
    let (tmp, core, pool) = fixture().await;
    sqlx::query("INSERT INTO settings (key, value) VALUES (?, ?)")
        .bind(gmm_lib::core::settings::keys::library_root())
        .bind(tmp.path().join("backups").to_string_lossy().as_ref())
        .execute(&pool)
        .await
        .unwrap();
    let status = core.attention_status().await.unwrap();
    assert!(
        !status.safe_to_proceed,
        "a Library root overlapping backups must make status unsafe"
    );
    assert!(!status.library_root_overlaps.is_empty());
}

#[tokio::test]
async fn unreleased_session_claim_needs_attention() {
    let (_tmp, core, pool) = fixture().await;
    sqlx::query("INSERT INTO session_launch_claims (token, game_code, owner_pid, owner_started_at, child_pid, child_started_at, started_at) VALUES ('claim', 'gimi', 0, NULL, NULL, NULL, '2026-08-24T00:00:00Z')")
        .execute(&pool).await.unwrap();
    let status = core.attention_status().await.unwrap();
    assert!(
        !status.safe_to_proceed,
        "an unreleased Game Session claim must make status unsafe"
    );
    assert_eq!(status.session_launches[0].id, "claim");
}

#[tokio::test]
async fn pending_enable_transition_needs_attention_without_recovery_error() {
    let (tmp, core, pool) = fixture().await;
    let item = adopt(&tmp, &core).await;
    std::fs::create_dir_all(tmp.path().join("Game/Mods")).unwrap();
    let target_key = durable_directory_key(&item.library_path);
    let parent_key = durable_directory_key(&tmp.path().join("Game/Mods"));
    sqlx::query("INSERT INTO enabled_transitions (mod_id, game_code, intended_enabled, junction_path, junction_target, junction_parent_identity, junction_identity, owner_pid, owner_started_at, owner_active, created_at, junction_target_identity, library_identity) VALUES (?, 'gimi', 1, ?, ?, ?, NULL, 0, NULL, 0, '2026-08-28T00:00:00Z', ?, ?)")
        .bind(&item.id).bind(tmp.path().join("Game/Mods/Example").to_string_lossy().as_ref())
        .bind(item.library_path.to_string_lossy().as_ref()).bind(&parent_key).bind(&target_key).bind(&target_key)
        .execute(&pool).await.unwrap();
    let status = core.attention_status().await.unwrap();

    assert!(
        !status.safe_to_proceed,
        "a pending enable transition must make status unsafe"
    );
    assert_eq!(status.enabled_transitions[0].mod_id, item.id);
    assert!(status.enabled_transitions[0].recovery.is_none());
}

#[tokio::test]
async fn pending_reinstall_needs_attention_without_recovery_error() {
    let (tmp, core, pool) = fixture().await;
    let item = adopt(&tmp, &core).await;
    let token = ulid::Ulid::new().to_string();
    let root = item.library_path.parent().unwrap();
    let key = "0000000000000001:0000000000000002";
    sqlx::query("INSERT INTO reinstall_swaps (token, mod_id, game_code, library_path, staged_path, quarantine_path, old_identity, staged_identity, created_at) VALUES (?, ?, 'gimi', ?, ?, ?, ?, ?, '2026-08-27T00:00:00Z')")
        .bind(&token).bind(&item.id).bind(item.library_path.to_string_lossy().as_ref())
        .bind(root.join(format!(".gmm-reinstall-{token}")).to_string_lossy().as_ref())
        .bind(root.join(format!(".gmm-delete-{token}")).to_string_lossy().as_ref())
        .bind(key).bind(key).execute(&pool).await.unwrap();
    let status = core.attention_status().await.unwrap();
    assert!(
        !status.safe_to_proceed,
        "a pending reinstall must make status unsafe"
    );
    assert_eq!(status.reinstalls[0].mod_id, item.id);
}

#[tokio::test]
async fn pending_importer_evacuation_needs_attention_without_recovery_error() {
    let (tmp, core, pool) = fixture().await;
    let token = ulid::Ulid::new().to_string();
    let key = "0000000000000001:0000000000000002";
    sqlx::query("INSERT INTO importer_evacuations (token, game_code, game_path, game_identity, backup_path, backup_identity, backup_root_identity, entries_json, owner_pid, owner_started_at, owner_active, created_at) VALUES (?, 'gimi', ?, ?, ?, ?, ?, '[\"d3dx.ini\"]', 0, NULL, 0, '2026-08-28T00:00:00Z')")
        .bind(&token).bind(tmp.path().join("Game").to_string_lossy().as_ref()).bind(key)
        .bind(tmp.path().join(format!("backups/gimi/fixture-{token}")).to_string_lossy().as_ref())
        .bind(key).bind(key).execute(&pool).await.unwrap();
    let status = core.attention_status().await.unwrap();
    assert!(
        !status.safe_to_proceed,
        "a pending Model Importer evacuation must make status unsafe"
    );
    assert_eq!(status.importer_evacuations[0].game, GameCode::Gimi);
}

#[tokio::test]
async fn duplicate_mod_records_need_attention() {
    let (tmp, core, pool) = fixture().await;
    let item = adopt(&tmp, &core).await;
    sqlx::query("INSERT INTO mods (id, game_code, name, source, library_path, enabled, created_at, junction_dir_name) VALUES (?, 'gimi', 'Duplicate', 'manual', ?, 0, '2026-08-27T00:00:00Z', 'Duplicate')")
        .bind(ulid::Ulid::new().to_string()).bind(item.library_path.to_string_lossy().as_ref())
        .execute(&pool).await.unwrap();
    let status = core.attention_status().await.unwrap();
    assert!(
        !status.safe_to_proceed,
        "duplicate Mod records must make status unsafe"
    );
    assert_eq!(status.library_audits[0].duplicates.len(), 1);
}

#[tokio::test]
async fn recorded_mod_path_in_backups_needs_attention() {
    let (tmp, core, pool) = fixture().await;
    let item = adopt(&tmp, &core).await;
    std::fs::remove_dir_all(&item.library_path).unwrap();
    sqlx::query("UPDATE mods SET library_path = ? WHERE id = ?")
        .bind(
            tmp.path()
                .join("backups/gimi/old/Mods/Example")
                .to_string_lossy()
                .as_ref(),
        )
        .bind(&item.id)
        .execute(&pool)
        .await
        .unwrap();
    let status = core.attention_status().await.unwrap();
    assert!(
        !status.safe_to_proceed,
        "a recorded Mod path in backups must make status unsafe"
    );
    assert_eq!(status.mod_path_overlaps[0].mod_id, item.id);
}

#[tokio::test]
async fn active_session_is_not_safe_for_mutation() {
    let (_tmp, core, _pool) = fixture().await;
    core.start_session(&gmm_lib::core::SessionInfo {
        game: GameCode::Gimi,
        pid: std::process::id(),
        started_at: chrono::Utc::now(),
    })
    .await
    .unwrap();
    let status = core.attention_status().await.unwrap();
    assert!(
        !status.safe_to_proceed,
        "an active Game Session must make status unsafe"
    );
    assert!(status.active_session.is_some());
}

#[cfg(unix)]
fn durable_directory_key(path: &std::path::Path) -> String {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = std::fs::metadata(path).expect("directory metadata for transition witness");
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
