mod support;

use gmm_lib::core::preset_apply::{PresetApplyOutcome, PresetApplyProgress};
use gmm_lib::core::{Core, Error, GameCode, Mod, SessionInfo};
use sqlx::SqlitePool;
use std::{fs, path::PathBuf};
use tempfile::TempDir;

struct Fixture {
    tmp: TempDir,
    core: Core,
    pool: SqlitePool,
    url: String,
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
            url,
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

#[tokio::test]
async fn complete_state_restores_variants_disables_later_mods_and_preserves_other_games() {
    let f = Fixture::new().await;
    let captured = f.add("Captured", GameCode::Gimi).await;
    let root = f.add("No selection", GameCode::Gimi).await;
    f.core
        .set_active_variant(&root.id, None, &f.mods)
        .await
        .unwrap();
    f.enable(&captured).await;
    f.enable(&root).await;
    let saved_variant = f.core.active_variant_id(&captured.id).await.unwrap();
    let preset = f
        .core
        .save_preset(GameCode::Gimi, "Complete")
        .await
        .unwrap();
    let red = f
        .core
        .list_variants(&captured.id)
        .await
        .unwrap()
        .into_iter()
        .find(|variant| variant.name == "Red")
        .unwrap();
    f.core
        .set_active_variant(&captured.id, Some(&red.id), &f.mods)
        .await
        .unwrap();
    let root_variant = f.core.list_variants(&root.id).await.unwrap()[0].id.clone();
    f.core
        .set_active_variant(&root.id, Some(&root_variant), &f.mods)
        .await
        .unwrap();
    let later = f.add("Added yesterday", GameCode::Gimi).await;
    f.enable(&later).await;
    let other = f.add("Other Game", GameCode::Srmi).await;
    let before_other = f.core.list_mods(GameCode::Srmi).await.unwrap();
    let mut events = Vec::new();
    let report = f
        .core
        .apply_preset(GameCode::Gimi, &preset.id, |event| events.push(event))
        .await
        .unwrap();
    let wire = serde_json::to_value(&report).unwrap();
    assert_eq!(wire["results"][0]["status"], "applied");
    assert!(wire["results"][0].get("targetEnabled").is_some());
    assert_eq!(
        f.core.active_variant_id(&root.id).await.unwrap(),
        None,
        "captured NULL must clear the later active Variant, not treat this Mod as absent"
    );
    assert_eq!(
        f.core.active_variant_id(&captured.id).await.unwrap(),
        saved_variant
    );
    assert_eq!(
        fs::canonicalize(f.mods.join("No selection")).unwrap(),
        fs::canonicalize(&root.library_path).unwrap()
    );
    let rows = f.core.list_mods(GameCode::Gimi).await.unwrap();
    assert!(
        !rows.iter().find(|row| row.id == later.id).unwrap().enabled,
        "complete apply must disable a Mod added after capture"
    );
    assert!(rows
        .iter()
        .filter(|row| row.id != later.id)
        .all(|row| row.enabled));
    assert!(!f.mods.join("Added yesterday").exists());
    let later_result = report
        .results
        .iter()
        .find(|result| result.item.mod_id == later.id)
        .unwrap();
    assert!(
        matches!(&later_result.outcome, PresetApplyOutcome::Applied { reason } if reason.contains("Added yesterday") && reason.contains("not in this preset"))
    );
    assert_eq!(
        f.core.list_mods(GameCode::Srmi).await.unwrap(),
        before_other
    );
    assert!(!report
        .results
        .iter()
        .any(|result| result.item.mod_id == other.id));
    assert!(report
        .results
        .iter()
        .all(|result| matches!(result.outcome, PresetApplyOutcome::Applied { .. })));
    assert_eq!(
        report.conflicts.as_ref().unwrap().conflicts.len(),
        1,
        "a Conflict warns without preventing the preset"
    );
    assert!(report.conflict_error.is_none());
    assert!(matches!(&events[0], PresetApplyProgress::Started { mods } if mods.len() == 3));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, PresetApplyProgress::Completed { .. }))
            .count(),
        3
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, PresetApplyProgress::CheckingConflicts))
            .count(),
        1
    );
    assert!(events.iter().any(
        |event| matches!(event, PresetApplyProgress::Applying { item } if item.name == "Captured")
    ));
}

#[tokio::test]
async fn empty_preset_disables_every_mod() {
    let f = Fixture::new().await;
    let preset = f.core.save_preset(GameCode::Gimi, "Clean").await.unwrap();
    let item = f.add("Later", GameCode::Gimi).await;
    f.enable(&item).await;
    let report = f
        .core
        .apply_preset(GameCode::Gimi, &preset.id, |_| {})
        .await
        .unwrap();
    assert!(!f.core.list_mods(GameCode::Gimi).await.unwrap()[0].enabled);
    assert!(matches!(
        report.results[0].outcome,
        PresetApplyOutcome::Applied { .. }
    ));
    assert!(report.conflicts.unwrap().conflicts.is_empty());
}

#[tokio::test]
async fn live_session_refuses_before_progress_or_any_mod_mutation() {
    let f = Fixture::new().await;
    let preset = f.core.save_preset(GameCode::Gimi, "Clean").await.unwrap();
    let item = f.add("Unchanged", GameCode::Gimi).await;
    f.enable(&item).await;
    let before = f.core.list_mods(GameCode::Gimi).await.unwrap();
    let target = fs::canonicalize(f.mods.join("Unchanged")).unwrap();
    f.core
        .start_session(&SessionInfo {
            game: GameCode::Srmi,
            pid: std::process::id(),
            started_at: chrono::Utc::now(),
        })
        .await
        .unwrap();
    let mut events = Vec::new();
    let result = f
        .core
        .apply_preset(GameCode::Gimi, &preset.id, |event| events.push(event))
        .await;
    assert!(
        matches!(result, Err(Error::SessionActive { .. })),
        "a live Game Session must refuse the whole apply up front"
    );
    assert!(
        events.is_empty(),
        "pre-flight must refuse before any Mod is attempted"
    );
    assert_eq!(
        f.core.list_mods(GameCode::Gimi).await.unwrap(),
        before,
        "pre-flight must leave every Mod unchanged"
    );
    assert_eq!(fs::canonicalize(f.mods.join("Unchanged")).unwrap(), target);
}

async fn plant_pending_transition(f: &Fixture, item: &Mod) {
    sqlx::query("INSERT INTO enabled_transitions (mod_id, game_code, intended_enabled, junction_path, junction_target, junction_parent_identity, junction_identity, owner_pid, owner_started_at, owner_active, created_at, junction_target_identity, library_identity) VALUES (?, ?, 0, 'link', 'target', '0000000000000001:0000000000000002', '0000000000000001:0000000000000003', 42, 1, 1, '2026-10-29T00:00:00Z', '0000000000000001:0000000000000004', '0000000000000001:0000000000000005')")
        .bind(&item.id).bind(item.game.as_str()).execute(&f.pool).await.unwrap();
}

#[tokio::test]
async fn unrelated_game_pending_transition_refuses_the_entire_apply_up_front() {
    let f = Fixture::new().await;
    let preset = f.core.save_preset(GameCode::Gimi, "Clean").await.unwrap();
    let item = f.add("Unchanged", GameCode::Gimi).await;
    f.enable(&item).await;
    let other = f.add("Other", GameCode::Srmi).await;
    plant_pending_transition(&f, &other).await;
    let before = f.core.list_mods(GameCode::Gimi).await.unwrap();
    let mut events = Vec::new();
    let result = f
        .core
        .apply_preset(GameCode::Gimi, &preset.id, |event| events.push(event))
        .await;
    assert!(
        matches!(result, Err(Error::EnabledTransitionPending { mod_id }) if mod_id == other.id)
    );
    assert!(events.is_empty());
    assert_eq!(f.core.list_mods(GameCode::Gimi).await.unwrap(), before);
}

#[tokio::test]
async fn missing_mod_and_dangling_variant_are_distinct_and_do_not_stop_the_rest() {
    let f = Fixture::new().await;
    let gone = f.add("Removed Mod", GameCode::Gimi).await;
    let dangling = f.add("Lost Variant", GameCode::Gimi).await;
    let okay = f.add("Still here", GameCode::Gimi).await;
    for item in [&gone, &dangling, &okay] {
        f.enable(item).await;
    }
    let preset = f.core.save_preset(GameCode::Gimi, "Mixed").await.unwrap();
    f.core.set_enabled(&gone.id, false, &f.mods).await.unwrap();
    f.core
        .set_enabled(&dangling.id, false, &f.mods)
        .await
        .unwrap();
    let lost = f
        .core
        .active_variant_id(&dangling.id)
        .await
        .unwrap()
        .unwrap();
    f.core
        .set_active_variant(&dangling.id, None, &f.mods)
        .await
        .unwrap();
    sqlx::query("DELETE FROM mod_variants WHERE id = ?")
        .bind(&lost)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM mods WHERE id = ?")
        .bind(&gone.id)
        .execute(&f.pool)
        .await
        .unwrap();
    let report = f
        .core
        .apply_preset(GameCode::Gimi, &preset.id, |_| {})
        .await
        .unwrap();
    assert!(
        matches!(&report.results.iter().find(|r| r.item.mod_id == gone.id).unwrap().outcome, PresetApplyOutcome::Skipped { reason } if reason == &format!("Mod with recorded identifier {} was skipped because it is no longer in this Game's Library.", gone.id))
    );
    assert!(
        matches!(&report.results.iter().find(|r| r.item.mod_id == dangling.id).unwrap().outcome, PresetApplyOutcome::Failed { reason } if reason.contains("Lost Variant") && reason.contains("recorded Variant is gone"))
    );
    assert!(matches!(
        &report
            .results
            .iter()
            .find(|r| r.item.mod_id == okay.id)
            .unwrap()
            .outcome,
        PresetApplyOutcome::Applied { .. }
    ));
    assert_eq!(
        f.core.active_variant_id(&dangling.id).await.unwrap(),
        None,
        "a missing recorded Variant must not silently become an applicable NULL selection"
    );
    assert!(
        !f.core
            .list_mods(GameCode::Gimi)
            .await
            .unwrap()
            .iter()
            .find(|row| row.id == dangling.id)
            .unwrap()
            .enabled
    );
}

#[tokio::test]
async fn indeterminate_enable_is_unknown_and_existing_startup_recovery_can_finish_it() {
    let f = Fixture::new().await;
    let item = f.add("Interrupted", GameCode::Gimi).await;
    f.enable(&item).await;
    let preset = f
        .core
        .save_preset(GameCode::Gimi, "Recoverable")
        .await
        .unwrap();
    f.core.set_enabled(&item.id, false, &f.mods).await.unwrap();
    let next = f.add("Still processed", GameCode::Gimi).await;
    sqlx::query("CREATE TRIGGER reject_enable BEFORE UPDATE OF enabled ON mods WHEN NEW.enabled = 1 BEGIN SELECT RAISE(ABORT, 'injected failure'); END").execute(&f.pool).await.unwrap();
    let report = f
        .core
        .apply_preset(GameCode::Gimi, &preset.id, |_| {})
        .await
        .unwrap();
    assert!(
        matches!(&report.results[0].outcome, PresetApplyOutcome::Unknown { reason } if reason == "GMM could not confirm the final state of Interrupted. Check its state in the Library, then retry the preset." && !reason.contains("injected failure")),
        "indeterminate deployment must be unknown, never reported as applied or failed"
    );
    assert_eq!(
        report.results.len(),
        2,
        "an indeterminate Mod must not abandon later results"
    );
    assert!(
        matches!(&report.results.iter().find(|r| r.item.mod_id == next.id).unwrap().outcome, PresetApplyOutcome::Skipped { reason } if reason == "Still processed could not continue because Library recovery is pending. Check the Library for the Mod that needs recovery; it may be a different Mod. Complete that recovery, then retry.")
    );
    assert!(
        f.mods.join("Interrupted").exists(),
        "the operation reached deployment before its database failure"
    );
    assert!(!f.core.list_mods(GameCode::Gimi).await.unwrap()[0].enabled);
    assert_eq!(
        report.conflicts.unwrap().omitted,
        vec![item.id.clone()],
        "an unresolved enable must be omitted even while its persisted flag is disabled"
    );
    assert!(report.conflict_error.unwrap().contains("unverified"));
    sqlx::query("DROP TRIGGER reject_enable")
        .execute(&f.pool)
        .await
        .unwrap();
    let restarted = Core::new(f.tmp.path().join("library"), &f.url)
        .await
        .unwrap();
    let rows = restarted.list_mods(GameCode::Gimi).await.unwrap();
    assert!(rows[0].enabled);
    assert!(rows[0].enabled_transition_recovery.is_none());
    assert_eq!(
        fs::canonicalize(f.mods.join("Interrupted")).unwrap(),
        fs::canonicalize(item.library_path.join("Blue")).unwrap()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM enabled_transitions")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn quarantined_reinstall_is_skipped_and_conflict_reading_is_unverified() {
    let f = Fixture::new().await;
    let item = f.add("Quarantined", GameCode::Gimi).await;
    f.enable(&item).await;
    let preset = f
        .core
        .save_preset(GameCode::Gimi, "Recovery")
        .await
        .unwrap();
    let other = f.add("Applicable", GameCode::Gimi).await;
    f.enable(&other).await;
    let token = ulid::Ulid::new().to_string();
    let stage = item
        .library_path
        .parent()
        .unwrap()
        .join(format!(".gmm-reinstall-{token}"));
    let quarantine = item
        .library_path
        .parent()
        .unwrap()
        .join(format!(".gmm-delete-{token}"));
    sqlx::query("INSERT INTO reinstall_swaps (token, mod_id, game_code, library_path, staged_path, quarantine_path, old_identity, staged_identity, created_at, recovery_error, recovery_attempted_at, recovery_attempts) VALUES (?, ?, 'gimi', ?, ?, ?, '0000000000000001:0000000000000002', '0000000000000001:0000000000000003', '2026-10-29T00:00:00Z', 'needs recovery', '2026-10-29T00:00:01Z', 1)")
        .bind(token).bind(&item.id).bind(item.library_path.to_string_lossy().as_ref()).bind(stage.to_string_lossy().as_ref()).bind(quarantine.to_string_lossy().as_ref()).execute(&f.pool).await.unwrap();
    let report = f
        .core
        .apply_preset(GameCode::Gimi, &preset.id, |_| {})
        .await
        .unwrap();
    assert!(
        matches!(&report.results.iter().find(|r| r.item.mod_id == item.id).unwrap().outcome, PresetApplyOutcome::Skipped { reason } if reason.contains("Quarantined") && reason.contains("recovery"))
    );
    assert!(matches!(
        &report
            .results
            .iter()
            .find(|r| r.item.mod_id == other.id)
            .unwrap()
            .outcome,
        PresetApplyOutcome::Applied { .. }
    ));
    assert_eq!(report.conflicts.unwrap().omitted, vec![item.id]);
    assert!(report.conflict_error.unwrap().contains("unverified"));
}

#[tokio::test]
async fn preset_identity_is_scoped_to_game_before_progress() {
    let f = Fixture::new().await;
    let preset = f
        .core
        .save_preset(GameCode::Srmi, "Other Game")
        .await
        .unwrap();
    let mut events = Vec::new();
    assert!(matches!(
        f.core
            .apply_preset(GameCode::Gimi, &preset.id, |event| events.push(event))
            .await,
        Err(Error::PresetNotFound)
    ));
    assert!(events.is_empty());
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

#[tokio::test]
async fn already_matching_preset_performs_no_library_mutation() {
    for clear_selection in [false, true] {
        let f = Fixture::new().await;
        let item = f.add("Already matched", GameCode::Gimi).await;
        if clear_selection {
            f.core
                .set_active_variant(&item.id, None, &f.mods)
                .await
                .unwrap();
        }
        f.enable(&item).await;
        let preset = f
            .core
            .save_preset(GameCode::Gimi, "Same state")
            .await
            .unwrap();
        let before = f.core.list_mods(GameCode::Gimi).await.unwrap();
        let identity = durable_directory_key(&f.mods.join("Already matched"));
        // Retain evidence even if a transient witness is deleted before apply returns.
        sqlx::query("CREATE TABLE observed_transitions (mod_id TEXT NOT NULL)")
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("CREATE TRIGGER observe_transition AFTER INSERT ON enabled_transitions BEGIN INSERT INTO observed_transitions VALUES (NEW.mod_id); END")
            .execute(&f.pool).await.unwrap();
        for _ in 0..2 {
            let report = f
                .core
                .apply_preset(GameCode::Gimi, &preset.id, |_| {})
                .await
                .unwrap();
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM observed_transitions")
                    .fetch_one(&f.pool)
                    .await
                    .unwrap(),
                0,
                "an already-matching apply must never create an enabled-transition witness"
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM enabled_transitions")
                    .fetch_one(&f.pool)
                    .await
                    .unwrap(),
                0
            );
            assert_eq!(
                durable_directory_key(&f.mods.join("Already matched")),
                identity,
                "an already-matching apply must preserve the deployed Junction identity"
            );
            assert_eq!(f.core.list_mods(GameCode::Gimi).await.unwrap(), before);
            assert!(
                matches!(&report.results[0].outcome, PresetApplyOutcome::Applied { reason }
                if reason == "Already matched already matched the preset; no change was needed.")
            );
        }
    }
}

#[tokio::test]
async fn missing_install_path_refuses_before_progress_or_mod_mutation() {
    let f = Fixture::new().await;
    let preset = f.core.save_preset(GameCode::Gimi, "Clean").await.unwrap();
    let item = f.add("Unchanged", GameCode::Gimi).await;
    f.enable(&item).await;
    let before = f.core.list_mods(GameCode::Gimi).await.unwrap();
    let identity = durable_directory_key(&f.mods.join("Unchanged"));
    sqlx::query("UPDATE games SET install_path = NULL WHERE code = 'gimi'")
        .execute(&f.pool)
        .await
        .unwrap();
    let mut events = Vec::new();
    assert!(matches!(
        f.core
            .apply_preset(GameCode::Gimi, &preset.id, |event| events.push(event))
            .await,
        Err(Error::PresetGamePathMissing)
    ));
    assert!(events.is_empty());
    assert_eq!(f.core.list_mods(GameCode::Gimi).await.unwrap(), before);
    assert_eq!(durable_directory_key(&f.mods.join("Unchanged")), identity);
}

#[tokio::test]
async fn missing_mods_directory_refuses_once_without_creating_it_or_mutating_mods() {
    let f = Fixture::new().await;
    let item = f.add("Unchanged", GameCode::Gimi).await;
    f.enable(&item).await;
    let preset = f
        .core
        .save_preset(GameCode::Gimi, "Same state")
        .await
        .unwrap();
    let later = f.add("Added later", GameCode::Gimi).await;
    f.enable(&later).await;
    let before = f.core.list_mods(GameCode::Gimi).await.unwrap();
    let moved = f.mods.with_file_name("Mods-backup");
    fs::rename(&f.mods, &moved).unwrap();
    let identity = durable_directory_key(&moved.join("Unchanged"));
    let mut events = Vec::new();
    let error = f
        .core
        .apply_preset(GameCode::Gimi, &preset.id, |event| events.push(event))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::PresetGamePathMissing));
    assert!(error.to_string().contains("Mods directory exists"));
    assert!(events.is_empty());
    assert!(!f.mods.exists());
    assert_eq!(f.core.list_mods(GameCode::Gimi).await.unwrap(), before);
    assert_eq!(durable_directory_key(&moved.join("Unchanged")), identity);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM enabled_transitions")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        0,
        "a missing Mods directory must refuse before any enabled transition"
    );
}

#[tokio::test]
async fn mods_location_file_refuses_distinctly_before_progress_or_any_mod_mutation() {
    let f = Fixture::new().await;
    let item = f.add("Unchanged", GameCode::Gimi).await;
    f.enable(&item).await;
    let preset = f
        .core
        .save_preset(GameCode::Gimi, "Same state")
        .await
        .unwrap();
    let later = f.add("Added later", GameCode::Gimi).await;
    f.enable(&later).await;
    let before = f.core.list_mods(GameCode::Gimi).await.unwrap();
    let moved = f.mods.with_file_name("Mods-backup");
    fs::rename(&f.mods, &moved).unwrap();
    let identity = durable_directory_key(&moved.join("Unchanged"));
    let later_identity = durable_directory_key(&moved.join("Added later"));
    fs::write(&f.mods, b"existing file at Mods location").unwrap();
    let mut events = Vec::new();
    let error = f
        .core
        .apply_preset(GameCode::Gimi, &preset.id, |event| events.push(event))
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::PresetModsPathNotDirectory),
        "an existing file at Mods must have a distinct refusal, never PresetGamePathMissing"
    );
    assert_eq!(
        error.to_string(),
        "This Game's Mods location exists but is not a directory. Choose a Game install path with a Mods directory before applying a preset."
    );
    assert!(
        events.is_empty(),
        "a Mods file must refuse the whole apply up front"
    );
    assert_eq!(
        fs::read(&f.mods).unwrap(),
        b"existing file at Mods location"
    );
    assert_eq!(f.core.list_mods(GameCode::Gimi).await.unwrap(), before);
    assert_eq!(durable_directory_key(&moved.join("Unchanged")), identity);
    assert_eq!(
        durable_directory_key(&moved.join("Added later")),
        later_identity
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM enabled_transitions")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        0,
        "a Mods file must refuse before any enabled transition"
    );
}

#[tokio::test]
async fn missing_and_foreign_presets_take_precedence_over_session_and_recovery_blockers() {
    for session in [true, false] {
        let f = Fixture::new().await;
        let foreign = f.core.save_preset(GameCode::Srmi, "Foreign").await.unwrap();
        let item = f.add("Unchanged", GameCode::Gimi).await;
        f.enable(&item).await;
        if session {
            f.core
                .start_session(&SessionInfo {
                    game: GameCode::Srmi,
                    pid: std::process::id(),
                    started_at: chrono::Utc::now(),
                })
                .await
                .unwrap();
        } else {
            plant_pending_transition(&f, &item).await;
        }
        let before = f.core.list_mods(GameCode::Gimi).await.unwrap();
        let identity = durable_directory_key(&f.mods.join("Unchanged"));
        for id in ["missing", foreign.id.as_str()] {
            let mut events = Vec::new();
            assert!(matches!(
                f.core
                    .apply_preset(GameCode::Gimi, id, |event| events.push(event))
                    .await,
                Err(Error::PresetNotFound)
            ));
            assert!(events.is_empty());
        }
        assert_eq!(f.core.list_mods(GameCode::Gimi).await.unwrap(), before);
        assert_eq!(durable_directory_key(&f.mods.join("Unchanged")), identity);
    }
}

#[tokio::test]
async fn mod_moved_to_another_game_keeps_its_resolvable_name_in_skip_reason() {
    let f = Fixture::new().await;
    let item = f.add("Moved Mod", GameCode::Gimi).await;
    f.enable(&item).await;
    let preset = f
        .core
        .save_preset(GameCode::Gimi, "Before move")
        .await
        .unwrap();
    f.core.set_enabled(&item.id, false, &f.mods).await.unwrap();
    sqlx::query("UPDATE mods SET game_code = 'srmi' WHERE id = ?")
        .bind(&item.id)
        .execute(&f.pool)
        .await
        .unwrap();
    let report = f
        .core
        .apply_preset(GameCode::Gimi, &preset.id, |_| {})
        .await
        .unwrap();
    assert_eq!(report.results[0].item.name, "Moved Mod");
    assert!(
        matches!(&report.results[0].outcome, PresetApplyOutcome::Skipped { reason }
        if reason == "Moved Mod was skipped because it is no longer in this Game's Library.")
    );
}

#[tokio::test]
async fn untouched_io_refusal_is_unknown_without_claiming_interruption_or_recovery() {
    let f = Fixture::new().await;
    let preset = f.core.save_preset(GameCode::Gimi, "Clean").await.unwrap();
    let item = f.add("Blocked", GameCode::Gimi).await;
    f.enable(&item).await;
    let link = f.mods.join("Blocked");
    #[cfg(unix)]
    fs::remove_file(&link).unwrap();
    #[cfg(windows)]
    fs::remove_dir(&link).unwrap();
    fs::create_dir(&link).unwrap();
    fs::write(link.join("keep"), "unchanged").unwrap();
    let before = f.core.list_mods(GameCode::Gimi).await.unwrap();
    let identity = durable_directory_key(&link);
    let report = f
        .core
        .apply_preset(GameCode::Gimi, &preset.id, |_| {})
        .await
        .unwrap();
    assert!(
        matches!(&report.results[0].outcome, PresetApplyOutcome::Unknown { reason }
        if reason == "GMM could not confirm the final state of Blocked. Check its state in the Library, then retry the preset.")
    );
    assert_eq!(f.core.list_mods(GameCode::Gimi).await.unwrap(), before);
    assert_eq!(durable_directory_key(&link), identity);
    assert_eq!(fs::read_to_string(link.join("keep")).unwrap(), "unchanged");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM enabled_transitions")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        0
    );
}
