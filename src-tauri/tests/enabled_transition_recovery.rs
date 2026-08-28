use std::path::PathBuf;

use gmm_lib::core::{Core, Error, GameCode};
use sqlx::SqlitePool;
use tempfile::TempDir;

struct TestEnv {
    _tmp: TempDir,
    db_url: String,
    library: PathBuf,
    game_mods: PathBuf,
}

impl TestEnv {
    fn new() -> Self {
        let tmp = TempDir::new().expect("temporary directory");
        let data = tmp.path().join("data");
        let library = data.join("library");
        let game_mods = tmp.path().join("Genshin/Mods");
        std::fs::create_dir_all(&data).expect("data directory");
        std::fs::create_dir_all(&game_mods).expect("game Mods directory");
        Self {
            db_url: format!("sqlite://{}/gmm.db?mode=rwc", data.display()),
            _tmp: tmp,
            library,
            game_mods,
        }
    }

    async fn core(&self) -> Core {
        Core::new(self.library.clone(), &self.db_url)
            .await
            .expect("open Core")
    }

    async fn pool(&self) -> SqlitePool {
        SqlitePool::connect(&self.db_url)
            .await
            .expect("open direct database connection")
    }

    async fn seed_mod(&self, core: &Core, name: &str) -> gmm_lib::core::Mod {
        let source = self._tmp.path().join("fixtures").join(name);
        std::fs::create_dir_all(&source).expect("fixture directory");
        std::fs::write(source.join("merged.ini"), b"[TextureOverride]\nhash=42\n")
            .expect("fixture ini");
        core.adopt_folder(GameCode::Gimi, &source, name)
            .await
            .expect("adopt fixture Mod")
    }

    fn deployment(&self, name: &str) -> PathBuf {
        self.game_mods.join(name)
    }
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

async fn assert_flag_and_junction_agree(
    core: &Core,
    env: &TestEnv,
    mod_id: &str,
    name: &str,
    expected_enabled: bool,
    context: &str,
) {
    let row = core
        .list_mods(GameCode::Gimi)
        .await
        .expect("list Mods")
        .into_iter()
        .find(|candidate| candidate.id == mod_id)
        .expect("find recovered Mod");
    let deployed = env.deployment(name).join("merged.ini").is_file();
    assert_eq!(
        (row.enabled, deployed),
        (expected_enabled, expected_enabled),
        "{context}: the enabled flag and Junction must agree",
    );
}

async fn enabled_transition_witness_count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM enabled_transitions")
        .fetch_one(pool)
        .await
        .expect("count enabled-transition witnesses")
}

#[tokio::test]
async fn enabled_update_failure_is_durably_recovered() {
    let env = TestEnv::new();
    let core = env.core().await;
    let imported = env.seed_mod(&core, "Update Failure").await;
    let pool = env.pool().await;
    sqlx::query(
        "CREATE TRIGGER force_enabled_update_failure
         BEFORE UPDATE OF enabled ON mods
         BEGIN
             SELECT RAISE(ABORT, 'forced enabled update failure');
         END",
    )
    .execute(&pool)
    .await
    .expect("install enabled-update failure");

    let failure = core
        .set_enabled(&imported.id, true, &env.game_mods)
        .await
        .expect_err("the injected enabled update must fail");
    assert!(
        failure
            .to_string()
            .contains("forced enabled update failure"),
        "set_enabled must return the injected update error: {failure}",
    );
    assert_eq!(
        enabled_transition_witness_count(&pool).await,
        1,
        "the failed enabled update must leave one durable transition witness",
    );

    sqlx::query("DROP TRIGGER force_enabled_update_failure")
        .execute(&pool)
        .await
        .expect("remove enabled-update failure");
    pool.close().await;
    drop(core);

    let recovered = env.core().await;
    assert_flag_and_junction_agree(
        &recovered,
        &env,
        &imported.id,
        "Update Failure",
        true,
        "startup after enabled-update failure",
    )
    .await;
    let pool = env.pool().await;
    assert_eq!(
        enabled_transition_witness_count(&pool).await,
        0,
        "successful startup recovery must retire the transition witness",
    );
}

#[tokio::test]
async fn enabled_commit_failure_is_durably_recovered() {
    let env = TestEnv::new();
    let core = env.core().await;
    let imported = env.seed_mod(&core, "Commit Failure").await;
    let pool = env.pool().await;
    sqlx::query("CREATE TABLE forced_commit_parent (id INTEGER PRIMARY KEY)")
        .execute(&pool)
        .await
        .expect("create deferred-constraint parent");
    sqlx::query(
        "CREATE TABLE forced_commit_child (
             id INTEGER PRIMARY KEY,
             parent_id INTEGER NOT NULL,
             FOREIGN KEY (parent_id) REFERENCES forced_commit_parent(id)
                 DEFERRABLE INITIALLY DEFERRED
         )",
    )
    .execute(&pool)
    .await
    .expect("create deferred-constraint child");
    sqlx::query(
        "CREATE TRIGGER force_enabled_commit_failure
         AFTER UPDATE OF enabled ON mods
         BEGIN
             INSERT INTO forced_commit_child (id, parent_id) VALUES (1, 404);
         END",
    )
    .execute(&pool)
    .await
    .expect("install commit failure");

    let failure = core
        .set_enabled(&imported.id, true, &env.game_mods)
        .await
        .expect_err("the injected commit must fail");
    assert!(
        failure
            .to_string()
            .contains("FOREIGN KEY constraint failed"),
        "set_enabled must return the injected COMMIT error: {failure}",
    );
    assert_eq!(
        enabled_transition_witness_count(&pool).await,
        1,
        "the failed COMMIT must leave one durable transition witness",
    );

    sqlx::query("DROP TRIGGER force_enabled_commit_failure")
        .execute(&pool)
        .await
        .expect("remove commit failure");
    pool.close().await;
    drop(core);

    let recovered = env.core().await;
    assert_flag_and_junction_agree(
        &recovered,
        &env,
        &imported.id,
        "Commit Failure",
        true,
        "startup after COMMIT failure",
    )
    .await;
    let pool = env.pool().await;
    assert_eq!(
        enabled_transition_witness_count(&pool).await,
        0,
        "successful startup recovery must retire the transition witness",
    );
}

#[tokio::test]
async fn partial_junction_remove_is_completed_before_the_flag_changes() {
    let env = TestEnv::new();
    let core = env.core().await;
    let imported = env.seed_mod(&core, "Partial Remove").await;
    core.set_enabled(&imported.id, true, &env.game_mods)
        .await
        .expect("enable fixture Mod");
    let deployment = env.deployment("Partial Remove");
    let target = std::fs::read_link(&deployment).expect("read enabled Junction target");

    gmm_lib::core::junction::remove(&deployment).expect("clear the fixture Junction");
    std::fs::create_dir(&deployment)
        .expect("inject the empty directory left by a partial Junction removal");
    let pool = env.pool().await;
    sqlx::query(
        "INSERT INTO enabled_transitions (
            mod_id, game_code, intended_enabled, junction_path,
            junction_target, junction_parent_identity, junction_identity, owner_pid,
            owner_started_at, owner_active, created_at,
            junction_target_identity, library_identity
         ) VALUES (?, 'gimi', 0, ?, ?, ?, ?, 0, NULL, 0,
                   '2026-08-28T00:00:00Z', ?, ?)",
    )
    .bind(&imported.id)
    .bind(deployment.to_string_lossy().as_ref())
    .bind(target.to_string_lossy().as_ref())
    .bind(durable_directory_key(&env.game_mods))
    .bind(durable_directory_key(&deployment))
    .bind(durable_directory_key(&target))
    .bind(durable_directory_key(&imported.library_path))
    .execute(&pool)
    .await
    .expect("inject the durable disable witness that preceded the partial removal");
    pool.close().await;
    drop(core);

    let recovered = env.core().await;
    assert_flag_and_junction_agree(
        &recovered,
        &env,
        &imported.id,
        "Partial Remove",
        false,
        "startup after partial Junction removal",
    )
    .await;
    assert!(
        std::fs::symlink_metadata(&deployment).is_err(),
        "startup must remove the empty non-Junction directory before committing disabled",
    );
}

#[tokio::test]
async fn audit_and_mutation_guard_share_enabled_transition_validation() {
    let env = TestEnv::new();
    let core = env.core().await;
    let imported = env.seed_mod(&core, "Shared Witness Rule").await;
    let pool = env.pool().await;
    sqlx::query(
        "INSERT INTO enabled_transitions (
            mod_id, game_code, intended_enabled, junction_path,
            junction_target, junction_parent_identity, junction_identity, owner_pid,
            owner_started_at, owner_active, created_at,
            junction_target_identity, library_identity
         ) VALUES (?, 'gimi', 1, ?, ?, ?, NULL, 0, NULL, 0,
                   'not-a-timestamp', ?, ?)",
    )
    .bind(&imported.id)
    .bind(
        env.deployment("Shared Witness Rule")
            .to_string_lossy()
            .as_ref(),
    )
    .bind(imported.library_path.to_string_lossy().as_ref())
    .bind(durable_directory_key(&env.game_mods))
    .bind(durable_directory_key(&imported.library_path))
    .bind(durable_directory_key(&imported.library_path))
    .execute(&pool)
    .await
    .expect("insert malformed transition witness");

    let audit = core
        .audit_library(GameCode::Gimi)
        .await
        .expect_err("the audit must reject the malformed shared witness");
    let mutation = core
        .set_enabled(&imported.id, true, &env.game_mods)
        .await
        .expect_err("the deployment guard must reject the malformed shared witness");
    for (surface, error) in [("audit", audit), ("guard", mutation)] {
        assert!(
            matches!(
                error,
                Error::EnabledTransitionWitnessCorrupt { ref mod_id, ref reason }
                    if mod_id == &imported.id && reason.contains("created-at")
            ),
            "{surface} must use the shared validated witness rule: {error}",
        );
    }
}

#[tokio::test]
async fn unresolved_transition_records_the_error_on_the_affected_mod() {
    let env = TestEnv::new();
    let core = env.core().await;
    let imported = env.seed_mod(&core, "Recorded Recovery Failure").await;
    let pool = env.pool().await;
    sqlx::query(
        "CREATE TRIGGER keep_enabled_update_failing
         BEFORE UPDATE OF enabled ON mods
         BEGIN
             SELECT RAISE(ABORT, 'persistent enabled update obstruction');
         END",
    )
    .execute(&pool)
    .await
    .expect("install persistent update failure");

    core.set_enabled(&imported.id, true, &env.game_mods)
        .await
        .expect_err("the injected update must remain unresolved");
    let listed = core.list_mods(GameCode::Gimi).await.expect("list Mods");
    let recovery = listed[0]
        .enabled_transition_recovery
        .as_ref()
        .expect("the affected Mod must state its unresolved transition");
    assert!(recovery.intended_enabled);
    assert_eq!(recovery.attempts, 1);
    assert!(
        recovery
            .reason
            .contains("persistent enabled update obstruction"),
        "the recorded recovery state must preserve the real failure: {recovery:?}",
    );
    assert_eq!(
        recovery.junction_path,
        env.deployment("Recorded Recovery Failure")
    );
}

/// A pre-identity witness whose numeric owner PID has been reused remains
/// conservatively live at startup. The user-confirmed retirement path is the
/// only safe way to release that uncertainty and complete the transition.
///
/// Mutation oracle: removing the retirement update leaves the named durable
/// witness-count assertion red.
#[tokio::test]
async fn unknown_reused_owner_can_be_confirmed_retired_and_recovered() {
    let env = TestEnv::new();
    let core = env.core().await;
    let imported = env.seed_mod(&core, "Unknown Reused Owner").await;
    let deployment = env.deployment("Unknown Reused Owner");
    let pool = env.pool().await;
    sqlx::query(
        "INSERT INTO enabled_transitions (
            mod_id, game_code, intended_enabled, junction_path,
            junction_target, junction_parent_identity, junction_identity, owner_pid,
            owner_started_at, owner_active, created_at,
            junction_target_identity, library_identity
         ) VALUES (?, 'gimi', 1, ?, ?, ?, NULL, ?, NULL, 1,
                   '2026-08-28T00:00:00Z', ?, ?)",
    )
    .bind(&imported.id)
    .bind(deployment.to_string_lossy().as_ref())
    .bind(imported.library_path.to_string_lossy().as_ref())
    .bind(durable_directory_key(&env.game_mods))
    .bind(std::process::id() as i64)
    .bind(durable_directory_key(&imported.library_path))
    .bind(durable_directory_key(&imported.library_path))
    .execute(&pool)
    .await
    .expect("insert transition whose old owner PID has been reused");
    pool.close().await;
    drop(core);

    let restarted = env.core().await;
    let listed = restarted
        .list_mods(GameCode::Gimi)
        .await
        .expect("list Mods");
    let recovery = listed[0]
        .enabled_transition_recovery
        .as_ref()
        .expect("unknown owner identity must be surfaced for confirmation");
    assert!(
        recovery.owner_uncertain,
        "the UI contract must distinguish producer uncertainty from a filesystem recovery error",
    );
    restarted
        .retire_interrupted_enabled_transition(&imported.id)
        .await
        .expect("retire after the user confirms no original transition is running");
    assert_flag_and_junction_agree(
        &restarted,
        &env,
        &imported.id,
        "Unknown Reused Owner",
        true,
        "user-confirmed retirement of an unknown reused owner",
    )
    .await;
    let pool = env.pool().await;
    assert_eq!(
        enabled_transition_witness_count(&pool).await,
        0,
        "confirmed retirement must release and resolve the durable transition witness",
    );
}
