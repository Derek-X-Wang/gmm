//! The release exit for an enable/disable GMM cannot finish (#462).
//!
//! A transition whose recorded Junction parent can never match again used to
//! lock every Library mutation and every launch, in every Game, for good.
//! Release is the user-confirmed way out: GMM gives up its claim and reports
//! the Mod's enabled state as unknown instead of guessing it.

use std::path::{Path, PathBuf};

use gmm_lib::core::{Core, Error, GameCode, Mod};
use sqlx::SqlitePool;
use tempfile::TempDir;

struct TestEnv {
    tmp: TempDir,
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
            tmp,
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

    fn fixture(&self, name: &str) -> PathBuf {
        let source = self.tmp.path().join("fixtures").join(name);
        std::fs::create_dir_all(&source).expect("fixture directory");
        std::fs::write(source.join("merged.ini"), b"[TextureOverride]\nhash=42\n")
            .expect("fixture ini");
        source
    }

    async fn seed_mod(&self, core: &Core, name: &str) -> Mod {
        core.adopt_folder(GameCode::Gimi, &self.fixture(name), name)
            .await
            .expect("adopt fixture Mod")
    }

    fn deployment(&self, name: &str) -> PathBuf {
        self.game_mods.join(name)
    }

    /// Replace the Mods directory with a different directory object at the
    /// same path, so the identity a transition recorded can never match again.
    fn replace_mods_directory(&self) {
        let parked = self.game_mods.with_file_name("Mods-parked");
        std::fs::rename(&self.game_mods, &parked).expect("park the original Mods directory");
        std::fs::create_dir(&self.game_mods).expect("recreate the Mods directory");
    }
}

async fn install_enabled_update_obstruction(pool: &SqlitePool) {
    sqlx::query(
        "CREATE TRIGGER obstruct_enabled_update
         BEFORE UPDATE OF enabled ON mods
         BEGIN
             SELECT RAISE(ABORT, 'enabled update obstruction');
         END",
    )
    .execute(pool)
    .await
    .expect("install enabled update obstruction");
}

async fn remove_enabled_update_obstruction(pool: &SqlitePool) {
    sqlx::query("DROP TRIGGER obstruct_enabled_update")
        .execute(pool)
        .await
        .expect("remove enabled update obstruction");
}

async fn count(pool: &SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar(sql)
        .fetch_one(pool)
        .await
        .expect("count rows")
}

async fn stored_enabled(pool: &SqlitePool, mod_id: &str) -> bool {
    let value: i64 = sqlx::query_scalar("SELECT enabled FROM mods WHERE id = ?")
        .bind(mod_id)
        .fetch_one(pool)
        .await
        .expect("read stored enabled flag");
    value != 0
}

async fn listed(core: &Core, mod_id: &str) -> Mod {
    core.list_mods(GameCode::Gimi)
        .await
        .expect("list Mods")
        .into_iter()
        .find(|candidate| candidate.id == mod_id)
        .expect("find Mod")
}

/// Interrupt an enable, then make its recorded Junction parent unmatchable.
/// Returns the environment, a Core that has already tried and failed to
/// recover, and the affected Mod.
async fn unresolvable_enable() -> (TestEnv, Core, Mod) {
    let env = TestEnv::new();
    let core = env.core().await;
    let affected = env.seed_mod(&core, "Stuck Mod").await;
    let pool = env.pool().await;
    install_enabled_update_obstruction(&pool).await;
    core.set_enabled(&affected.id, true, &env.game_mods)
        .await
        .expect_err("the obstruction must leave the transition unresolved");
    remove_enabled_update_obstruction(&pool).await;
    pool.close().await;
    drop(core);

    env.replace_mods_directory();
    let restarted = env.core().await;
    let recovery = listed(&restarted, &affected.id)
        .await
        .enabled_transition_recovery
        .expect("the unresolved transition stays visible on the Mod");
    assert!(
        recovery
            .reason
            .contains("Junction parent changed filesystem identity"),
        "recovery must fail on the unmatchable identity: {recovery:?}",
    );
    assert!(!recovery.released);
    (env, restarted, affected)
}

async fn library_mutation_is_refused(
    env: &TestEnv,
    core: &Core,
    game: GameCode,
    name: &str,
) -> bool {
    matches!(
        core.adopt_folder(game, &env.fixture(name), name).await,
        Err(Error::EnabledTransitionPending { .. })
    )
}

async fn launch_is_refused(core: &Core, game: GameCode) -> bool {
    matches!(
        core.begin_session_launch(game).await,
        Err(Error::EnabledTransitionPending { .. })
    )
}

async fn launch_succeeds(core: &Core, game: GameCode) -> bool {
    match core.begin_session_launch(game).await {
        Ok(claim) => {
            core.abandon_session_launch(&claim)
                .await
                .expect("abandon the probe launch claim");
            true
        }
        Err(_) => false,
    }
}

/// The reproduction: an unresolvable transition locks every Game, release
/// unlocks every Game.
///
/// Mutation oracle: making `begin_library_mutation` ignore the `released_at`
/// stamp (or removing the stamp) leaves the "after release" assertions red.
#[tokio::test]
async fn release_ends_the_global_lockout_that_recovery_cannot_end() {
    let (env, core, affected) = unresolvable_enable().await;

    assert!(library_mutation_is_refused(&env, &core, GameCode::Gimi, "Locked Gimi").await);
    assert!(
        library_mutation_is_refused(&env, &core, GameCode::Srmi, "Locked Srmi").await,
        "the block is global, not scoped to the affected Game",
    );
    assert!(launch_is_refused(&core, GameCode::Gimi).await);
    assert!(launch_is_refused(&core, GameCode::Srmi).await);

    core.release_enabled_transition(&affected.id, true)
        .await
        .expect("release after the user confirms");

    assert!(
        !library_mutation_is_refused(&env, &core, GameCode::Gimi, "Free Gimi").await,
        "Library mutation must no longer be refused in the affected Game",
    );
    assert!(
        core.adopt_folder(GameCode::Srmi, &env.fixture("Free Srmi"), "Free Srmi")
            .await
            .is_ok(),
        "Library mutation must no longer be refused in another Game",
    );
    assert!(launch_succeeds(&core, GameCode::Gimi).await);
    assert!(launch_succeeds(&core, GameCode::Srmi).await);
}

#[tokio::test]
async fn release_is_refused_without_confirmation_and_changes_nothing() {
    let (env, core, affected) = unresolvable_enable().await;

    let refused = core.release_enabled_transition(&affected.id, false).await;
    assert!(
        matches!(refused, Err(Error::EnabledTransitionReleaseUnconfirmed)),
        "an unconfirmed release must be refused: {refused:?}",
    );

    let pool = env.pool().await;
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM enabled_transitions WHERE released_at IS NULL"
        )
        .await,
        1,
        "the witness must still be active",
    );
    assert!(library_mutation_is_refused(&env, &core, GameCode::Gimi, "Still Locked").await);
    assert!(
        !listed(&core, &affected.id)
            .await
            .enabled_transition_recovery
            .expect("still visible")
            .released
    );
}

#[tokio::test]
async fn a_transition_recovery_can_finish_resolves_normally_and_is_never_released() {
    let env = TestEnv::new();
    let core = env.core().await;
    let affected = env.seed_mod(&core, "Resolvable Mod").await;
    let pool = env.pool().await;
    install_enabled_update_obstruction(&pool).await;
    core.set_enabled(&affected.id, true, &env.game_mods)
        .await
        .expect_err("the obstruction leaves the transition for startup");
    remove_enabled_update_obstruction(&pool).await;
    drop(core);

    let restarted = env.core().await;
    let row = listed(&restarted, &affected.id).await;
    assert!(row.enabled, "forward recovery commits the intended flag");
    assert!(row.enabled_transition_recovery.is_none());
    assert!(env
        .deployment("Resolvable Mod")
        .join("merged.ini")
        .is_file());
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM enabled_transitions").await,
        0,
        "a resolvable transition is deleted, not stamped released",
    );

    // Nothing is left to give up, and asking must not invent a release.
    restarted
        .release_enabled_transition(&affected.id, true)
        .await
        .expect("releasing an already-settled Mod is a no-op");
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM enabled_transitions").await,
        0
    );
}

#[tokio::test]
async fn a_transition_with_no_failed_recovery_cannot_be_released() {
    let env = TestEnv::new();
    let core = env.core().await;
    let imported = env.seed_mod(&core, "Untried Mod").await;
    let pool = env.pool().await;
    sqlx::query(
        "INSERT INTO enabled_transitions (
            mod_id, game_code, intended_enabled, junction_path,
            junction_target, junction_parent_identity, junction_identity, owner_pid,
            owner_started_at, owner_active, created_at,
            junction_target_identity, library_identity
         ) VALUES (?, 'gimi', 1, ?, ?, ?, NULL, 0, NULL, 0,
                   '2026-08-28T00:00:00Z', ?, ?)",
    )
    .bind(&imported.id)
    .bind(env.deployment("Untried Mod").to_string_lossy().as_ref())
    .bind(imported.library_path.to_string_lossy().as_ref())
    .bind(identity_key(&env.game_mods))
    .bind(identity_key(&imported.library_path))
    .bind(identity_key(&imported.library_path))
    .execute(&pool)
    .await
    .expect("insert a dead-owner transition that has not failed recovery");

    let refused = core.release_enabled_transition(&imported.id, true).await;
    assert!(
        matches!(
            refused,
            Err(Error::EnabledTransitionReleaseUnavailable { .. })
        ),
        "GMM can still finish this one, so it must not be given up: {refused:?}",
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM enabled_transitions WHERE released_at IS NULL"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn a_released_mod_reports_unknown_and_is_not_recovered_at_startup() {
    let (env, core, affected) = unresolvable_enable().await;
    core.release_enabled_transition(&affected.id, true)
        .await
        .expect("release");

    let recovery = listed(&core, &affected.id)
        .await
        .enabled_transition_recovery
        .expect("a released Mod must still say its state is unknown");
    assert!(recovery.released);

    // Whatever sits at the recorded path stays exactly as the failed enable
    // left it. Startup must neither adopt it nor recover through it.
    let leftover = env.deployment("Stuck Mod");
    drop(core);
    let restarted = env.core().await;
    let pool = env.pool().await;
    assert!(
        listed(&restarted, &affected.id)
            .await
            .enabled_transition_recovery
            .expect("still unknown after restart")
            .released
    );
    assert!(
        !stored_enabled(&pool, &affected.id).await,
        "startup must not commit the intended flag for a released Mod",
    );
    assert!(
        std::fs::symlink_metadata(&leftover).is_err(),
        "startup must not create anything at the recorded path",
    );

    // Conflict reading must not trust the stored flag for it either.
    let report = restarted.detect_conflicts(GameCode::Gimi).await.unwrap();
    assert!(
        report
            .omitted
            .iter()
            .any(|entry| entry.mod_id == affected.id),
        "a released Mod must be omitted from conflict reading: {report:?}",
    );
}

#[tokio::test]
async fn explicitly_enabling_a_released_mod_re_establishes_its_state() {
    let (env, core, affected) = unresolvable_enable().await;
    core.release_enabled_transition(&affected.id, true)
        .await
        .expect("release");

    core.set_enabled(&affected.id, true, &env.game_mods)
        .await
        .expect("an explicit enable settles the unknown state");

    let pool = env.pool().await;
    let row = listed(&core, &affected.id).await;
    assert!(row.enabled);
    assert!(row.enabled_transition_recovery.is_none());
    assert!(env.deployment("Stuck Mod").join("merged.ini").is_file());
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM enabled_transitions").await,
        0,
        "the released record is removed only once the work succeeded",
    );
}

#[tokio::test]
async fn explicitly_disabling_a_released_mod_with_nothing_deployed_settles_it() {
    let (env, core, affected) = unresolvable_enable().await;
    core.release_enabled_transition(&affected.id, true)
        .await
        .expect("release");

    core.set_enabled(&affected.id, false, &env.game_mods)
        .await
        .expect("an explicit disable settles the unknown state");

    let pool = env.pool().await;
    let row = listed(&core, &affected.id).await;
    assert!(!row.enabled);
    assert!(row.enabled_transition_recovery.is_none());
    assert!(!env.deployment("Stuck Mod").exists());
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM enabled_transitions").await,
        0
    );
}

/// A re-enable that fails after doing some of the work must leave the Mod
/// unknown, not authoritative.
///
/// Mutation oracle: deleting the released record before the Junction work
/// (instead of with the flag update) leaves the final assertions red.
#[tokio::test]
async fn a_failed_re_enable_leaves_the_mod_unknown() {
    let (env, core, affected) = unresolvable_enable().await;
    core.release_enabled_transition(&affected.id, true)
        .await
        .expect("release");

    let pool = env.pool().await;
    install_enabled_update_obstruction(&pool).await;
    core.set_enabled(&affected.id, true, &env.game_mods)
        .await
        .expect_err("the obstruction fails the flag update after the Junction exists");

    let recovery = listed(&core, &affected.id)
        .await
        .enabled_transition_recovery
        .expect("the Mod must still be unknown");
    assert!(recovery.released);
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM enabled_transitions WHERE released_at IS NOT NULL"
        )
        .await,
        1
    );
    assert!(!stored_enabled(&pool, &affected.id).await);

    // The obstruction clears; the explicit retry now settles it.
    remove_enabled_update_obstruction(&pool).await;
    core.set_enabled(&affected.id, true, &env.game_mods)
        .await
        .expect("retry the explicit enable");
    assert!(listed(&core, &affected.id)
        .await
        .enabled_transition_recovery
        .is_none());
}

#[tokio::test]
async fn a_released_mod_blocks_preset_capture_instead_of_recording_its_flag() {
    let (_env, core, affected) = unresolvable_enable().await;
    core.release_enabled_transition(&affected.id, true)
        .await
        .expect("release");

    let refused = core.save_preset(GameCode::Gimi, "Snapshot").await;
    assert!(
        matches!(refused, Err(Error::EnabledStateUnknown { .. })),
        "a preset must not record a flag GMM has said it does not know: {refused:?}",
    );
}

#[cfg(unix)]
fn identity_key(path: &Path) -> String {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = std::fs::metadata(path).expect("directory metadata");
    format!("{:016x}:{:016x}", metadata.dev(), metadata.ino())
}

#[cfg(windows)]
fn identity_key(path: &Path) -> String {
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
        "read directory identity: {}",
        std::io::Error::last_os_error()
    );
    let info = unsafe { info.assume_init() };
    let file = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    format!("{:016x}:{:016x}", info.dwVolumeSerialNumber, file)
}
