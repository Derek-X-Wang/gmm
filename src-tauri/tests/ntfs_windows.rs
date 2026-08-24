//! Real-NTFS behaviour that only a Windows host can answer.
//!
//! The junction layer (ADR 0003) is the load-bearing piece of GMM's
//! enable/disable model, and every assertion about it on a macOS dev box
//! is really an assertion about unix symlinks. These tests pin the
//! claims that only hold — or only fail — on NTFS:
//!
//! * CJK / emoji mod names survive the round trip. GameBanana mod titles
//!   are frequently Chinese, Japanese, or Korean, so this is the common
//!   case, not an edge case.
//! * Junctions span volumes. ADR 0003 asserts this as a reason to prefer
//!   junctions over copies; it had never been executed.
//! * Deep paths near MAX_PATH still work.
//! * A junction whose Library target disappears is reported as broken
//!   rather than silently treated as healthy.
//!
//! Windows-only; the file compiles away elsewhere.

#![cfg(windows)]

use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt as _;
use std::os::windows::io::AsRawHandle as _;
use std::path::{Path, PathBuf};

use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;
use windows_sys::Win32::Storage::FileSystem::{
    SetFileShortNameW, DELETE, FILE_FLAG_BACKUP_SEMANTICS,
};

async fn core_with_library(library_root: PathBuf, tmp: &Path) -> Core {
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.display());
    Core::new(library_root, &db_url).await.expect("core")
}

fn make_mod_dir(dir: &Path, marker: &str) {
    fs::create_dir_all(dir).expect("mod dir");
    fs::write(dir.join("merged.ini"), format!("; {marker}\nhash = 1234\n")).expect("ini");
}

fn set_short_name(path: &Path, short_name: &str) {
    let directory = OpenOptions::new()
        .access_mode(DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .expect("open Mod directory with DELETE access for SetFileShortNameW");
    let short_wide: Vec<u16> = OsStr::new(short_name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let ok = unsafe { SetFileShortNameW(directory.as_raw_handle(), short_wide.as_ptr()) };
    assert_ne!(
        ok,
        0,
        "SetFileShortNameW could not create the mandatory 8.3 Mod alias: {}",
        std::io::Error::last_os_error(),
    );
}

/// NTFS can expose an 8.3 name for the same directory. Assign a distinct alias
/// explicitly so a volume that cannot exercise the contract fails this test
/// instead of silently reporting success. Use only the short final component
/// so the old textual parent check cannot refuse it for an unrelated reason.
#[tokio::test]
async fn short_name_alias_of_a_referenced_directory_cannot_be_deleted() {
    let tmp = TempDir::new().expect("tmp");
    let core = core_with_library(tmp.path().join("library"), tmp.path()).await;
    let fixture = tmp.path().join("src/short-name-alias");
    make_mod_dir(&fixture, "8.3 alias");
    let adopted = core
        .adopt_folder(GameCode::Gimi, &fixture, "Short Name Alias")
        .await
        .expect("adopt");

    const SHORT_NAME: &str = "GMMMOD~1";
    set_short_name(&adopted.library_path, SHORT_NAME);
    let alias = adopted
        .library_path
        .parent()
        .expect("Library root")
        .join(SHORT_NAME);
    assert!(alias.is_dir(), "precondition: short name resolves the Mod");

    let deleted = core
        .delete_unreferenced_library_dir(GameCode::Gimi, &alias)
        .await;
    assert!(
        matches!(
            deleted,
            Err(gmm_lib::core::Error::NotAnUnreferencedLibraryDir { .. })
        ),
        "delete must identify the 8.3 alias as the adopted Mod, got {deleted:?}",
    );
    assert!(
        adopted.library_path.join("merged.ini").is_file(),
        "a refused short-name delete must leave the Mod bytes intact",
    );
}

/// Reinstall witness paths retain the spelling present in the Mod row. NTFS
/// can spell the same Library root through an 8.3 alias, so relocation must
/// compare filesystem-resolved paths rather than raw components or it can move
/// an in-flight witness through the copy fallback.
#[tokio::test]
async fn short_name_alias_cannot_bypass_the_reinstall_relocation_guard() {
    let tmp = TempDir::new().expect("tmp");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(tmp.path().join("library"), &db_url)
        .await
        .expect("core");
    let fixture = tmp.path().join("src/reinstall-root-alias");
    make_mod_dir(&fixture, "reinstall root alias");
    let adopted = core
        .adopt_folder(GameCode::Gimi, &fixture, "Reinstall Root Alias")
        .await
        .expect("adopt");
    let game_root = adopted.library_path.parent().expect("game Library root");

    const SHORT_ROOT: &str = "GMMROOT";
    set_short_name(game_root, SHORT_ROOT);
    let alias_root = game_root.parent().expect("Library root").join(SHORT_ROOT);
    let aliased_mod_path = alias_root.join(&adopted.id);
    assert!(
        aliased_mod_path.is_dir(),
        "precondition: 8.3 root alias resolves the installed Mod",
    );

    let token = ulid::Ulid::new().to_string();
    let staged_path = alias_root.join(format!(".gmm-reinstall-{token}"));
    let quarantine_path = alias_root.join(format!(".gmm-delete-{token}"));
    let pool = sqlx::SqlitePool::connect(&db_url)
        .await
        .expect("open DB for alias witness");
    sqlx::query("UPDATE mods SET library_path = ? WHERE id = ?")
        .bind(aliased_mod_path.to_string_lossy().as_ref())
        .bind(&adopted.id)
        .execute(&pool)
        .await
        .expect("record alias-spelled Mod path");
    sqlx::query(
        "INSERT INTO reinstall_swaps (
            token, mod_id, game_code, library_path, staged_path,
            quarantine_path, old_identity, staged_identity, created_at
         ) VALUES (?, ?, 'gimi', ?, ?, ?, 'old-id', 'staged-id', ?)",
    )
    .bind(&token)
    .bind(&adopted.id)
    .bind(aliased_mod_path.to_string_lossy().as_ref())
    .bind(staged_path.to_string_lossy().as_ref())
    .bind(quarantine_path.to_string_lossy().as_ref())
    .bind("2026-08-23T00:00:00Z")
    .execute(&pool)
    .await
    .expect("insert alias-spelled reinstall witness");
    pool.close().await;

    let destination = tmp.path().join("relocated-gimi");
    fs::create_dir_all(&destination).expect("non-empty relocation destination");
    fs::write(destination.join("forces-copy-fallback"), b"keep").expect("copy fallback sentinel");
    let relocation = core
        .set_library_path_for_game(GameCode::Gimi, Some(&destination))
        .await
        .expect_err("an 8.3 alias must not bypass the reinstall relocation guard");

    assert!(
        relocation.to_string().contains("Let the reinstall finish"),
        "an active reinstall should retain the retry-later refusal, got: {relocation}",
    );
    assert!(
        adopted.library_path.join("merged.ini").is_file(),
        "the refused alias-spelled relocation must not touch Library bytes",
    );
}

async fn record_quarantined_reinstall(
    db_url: &str,
    adopted: &gmm_lib::core::Mod,
    token: ulid::Ulid,
) {
    let root = adopted.library_path.parent().expect("game Library root");
    let stage = root.join(format!(".gmm-reinstall-{token}"));
    let quarantine = root.join(format!(".gmm-delete-{token}"));
    let pool = sqlx::SqlitePool::connect(db_url)
        .await
        .expect("open DB for quarantined reinstall fixture");
    sqlx::query(
        "INSERT INTO reinstall_swaps (
            token, mod_id, game_code, library_path, staged_path,
            quarantine_path, old_identity, staged_identity, created_at,
            recovery_error, recovery_attempted_at, recovery_attempts
         ) VALUES (?, ?, 'gimi', ?, ?, ?,
                   '0000000000000001:0000000000000001',
                   '0000000000000002:0000000000000002', ?,
                   'fixture recovery obstruction', ?, 1)",
    )
    .bind(token.to_string())
    .bind(&adopted.id)
    .bind(adopted.library_path.to_string_lossy().as_ref())
    .bind(stage.to_string_lossy().as_ref())
    .bind(quarantine.to_string_lossy().as_ref())
    .bind("2026-08-23T00:00:00Z")
    .bind("2026-08-23T00:01:00Z")
    .execute(&pool)
    .await
    .expect("insert quarantined reinstall fixture");
    pool.close().await;
}

/// A plain directory at the deployment name is not owned by GMM, even when it
/// is empty. The structural link guard must refuse it before the junction
/// crate's fallback can remove the directory and misreport success.
///
/// Mutation oracle: deleting the non-link guard from
/// `withdraw_reinstall_junction` removes the empty directory and fires the
/// named survival assertion.
#[tokio::test]
async fn quarantined_reinstall_withdrawal_refuses_an_empty_non_link_directory() {
    let tmp = TempDir::new().expect("tmp");
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.path().display());
    let core = Core::new(tmp.path().join("library"), &db_url)
        .await
        .expect("core");
    let mods_dir = tmp.path().join("game/Mods");
    fs::create_dir_all(&mods_dir).expect("Mods directory");
    let fixture = tmp.path().join("src/non-link-withdrawal");
    make_mod_dir(&fixture, "non-link withdrawal");
    let adopted = core
        .adopt_folder(GameCode::Gimi, &fixture, "Non Link Withdrawal")
        .await
        .expect("adopt");
    core.set_enabled(&adopted.id, true, &mods_dir)
        .await
        .expect("enable");
    let deployment = mods_dir.join("Non Link Withdrawal");
    gmm_lib::core::junction::remove(&deployment).expect("remove real Junction");
    fs::create_dir(&deployment).expect("empty non-link deployment directory");
    record_quarantined_reinstall(&db_url, &adopted, ulid::Ulid::new()).await;

    let result = core
        .reconcile_junctions(GameCode::Gimi, &mods_dir)
        .await
        .expect("reconcile quarantined non-link");
    let recovery = core
        .list_mods(GameCode::Gimi)
        .await
        .expect("list quarantined Mod")[0]
        .reinstall_recovery
        .clone()
        .expect("reinstall recovery state");

    assert_eq!(result.quarantined, vec![adopted.id]);
    assert!(
        fs::symlink_metadata(&deployment).is_ok(),
        "the non-link guard must preserve an empty deployment directory GMM does not own",
    );
    assert!(
        !recovery.junction_withdrawn,
        "refusing a non-link deployment must not record successful withdrawal",
    );
}

/// GameBanana titles are routinely non-ASCII. If sanitisation mangles
/// them into an empty or colliding directory name, enabling silently
/// puts the mod somewhere the game will never look.
#[tokio::test]
async fn cjk_and_emoji_mod_names_round_trip_through_a_junction() {
    let tmp = TempDir::new().expect("tmp");
    let mods_dir = tmp.path().join("game/Mods");
    fs::create_dir_all(&mods_dir).expect("mods dir");
    let core = core_with_library(tmp.path().join("library"), tmp.path()).await;

    // Simplified Chinese, Japanese, Korean, and an emoji — all names
    // that show up on GameBanana in practice.
    let names = ["胡桃 皮肤", "ホロライブ", "감스트", "Hu Tao ✨"];

    for (i, name) in names.iter().enumerate() {
        let fixture = tmp.path().join(format!("src/{i}"));
        make_mod_dir(&fixture, name);

        let adopted = core
            .adopt_folder(GameCode::Gimi, &fixture, name)
            .await
            .unwrap_or_else(|e| panic!("adopt {name:?}: {e}"));
        assert_eq!(
            &adopted.name, name,
            "display name must be preserved verbatim"
        );

        core.set_enabled(&adopted.id, true, &mods_dir)
            .await
            .unwrap_or_else(|e| panic!("enable {name:?}: {e}"));
    }

    // Every mod must have produced its own resolvable junction.
    let entries: Vec<_> = fs::read_dir(&mods_dir)
        .expect("read mods")
        .filter_map(Result::ok)
        .collect();
    assert_eq!(
        entries.len(),
        names.len(),
        "one junction per mod; got {:?}",
        entries.iter().map(|e| e.file_name()).collect::<Vec<_>>(),
    );
    for entry in entries {
        let ini = entry.path().join("merged.ini");
        assert!(
            ini.exists(),
            "junction {:?} must resolve into the Library",
            entry.file_name(),
        );
    }
}

/// ADR 0003 chose junctions partly because they span volumes, which
/// lets a user keep the Library on a big data drive and the game on
/// the system drive. GitHub's Windows runners give us C: and D: for
/// free, so the claim is finally executable.
#[tokio::test]
async fn junctions_span_volumes() {
    // A silent skip here would let "cross-volume is tested" quietly
    // decay into "cross-volume is never tested". CI is known to have
    // both volumes (workspace on D:, %TEMP% on C:), so in CI a missing
    // second volume is a failure, not a skip. Locally it stays a skip.
    let other_volume = PathBuf::from(r"D:\");
    if !other_volume.exists() {
        assert!(
            std::env::var("CI").is_err(),
            "no D: volume on a CI runner — the cross-volume junction claim in \
             ADR 0003 would go unverified. Fix the runner or the test, don't skip.",
        );
        eprintln!("skipping cross-volume test: no D: volume on this host");
        return;
    }

    let tmp = TempDir::new().expect("tmp on C:");
    let cross = tempfile::Builder::new()
        .prefix("gmm-xvol-")
        .tempdir_in(r"D:\")
        .expect("tmp on D:");

    // Library on D:, game on C: — opposite volumes by construction.
    let core = core_with_library(cross.path().join("library"), tmp.path()).await;
    let mods_dir = tmp.path().join("game/Mods");
    fs::create_dir_all(&mods_dir).expect("mods dir");

    let fixture = tmp.path().join("src/CrossVolume");
    make_mod_dir(&fixture, "cross-volume");

    let adopted = core
        .adopt_folder(GameCode::Gimi, &fixture, "Cross Volume Mod")
        .await
        .expect("adopt");
    assert!(
        adopted.library_path.starts_with(r"D:\"),
        "library copy should be on D:, got {:?}",
        adopted.library_path,
    );

    core.set_enabled(&adopted.id, true, &mods_dir)
        .await
        .expect("enable across volumes");

    let link = mods_dir.join("Cross Volume Mod");
    assert!(link.exists(), "cross-volume junction must exist");
    assert!(
        link.join("merged.ini").exists(),
        "cross-volume junction must resolve into the Library on the other drive",
    );

    core.set_enabled(&adopted.id, false, &mods_dir)
        .await
        .expect("disable across volumes");
    assert!(!link.exists(), "cross-volume junction must be removable");
    assert!(
        adopted.library_path.join("merged.ini").exists(),
        "the Library copy on D: must survive the disable",
    );
}

/// Long mod names plus a deep Library root push the junction target
/// toward MAX_PATH. Sanitisation caps the directory name at 200 chars;
/// this proves the cap is enough for a realistic nesting depth.
#[tokio::test]
async fn deep_paths_and_long_names_still_produce_working_junctions() {
    let tmp = TempDir::new().expect("tmp");

    // Nest the Library a few levels down, the way a user with an
    // organised drive would.
    let deep = tmp
        .path()
        .join("Games")
        .join("Modding")
        .join("Gacha Mod Manager")
        .join("library-root");
    let core = core_with_library(deep, tmp.path()).await;

    let mods_dir = tmp.path().join("game/Mods");
    fs::create_dir_all(&mods_dir).expect("mods dir");

    let long_name = "A".repeat(180);
    let fixture = tmp.path().join("src/long");
    make_mod_dir(&fixture, "long-name");

    let adopted = core
        .adopt_folder(GameCode::Gimi, &fixture, &long_name)
        .await
        .expect("adopt long name");
    core.set_enabled(&adopted.id, true, &mods_dir)
        .await
        .expect("enable long name");

    let entries: Vec<_> = fs::read_dir(&mods_dir)
        .expect("read mods")
        .filter_map(Result::ok)
        .collect();
    assert_eq!(entries.len(), 1);
    let on_disk = entries[0].file_name().into_string().expect("utf8");
    assert!(
        on_disk.chars().count() <= 200,
        "junction dir name should be capped, got {} chars",
        on_disk.chars().count(),
    );
    assert!(
        entries[0].path().join("merged.ini").exists(),
        "deep + long junction must still resolve",
    );
}

/// A junction whose Library target has been deleted out from under it
/// still *exists* as a reparse point but resolves to nothing. Reconcile
/// must notice rather than reporting the mod as healthy.
#[tokio::test]
async fn reconcile_notices_a_junction_whose_target_vanished() {
    let tmp = TempDir::new().expect("tmp");
    let mods_dir = tmp.path().join("game/Mods");
    fs::create_dir_all(&mods_dir).expect("mods dir");
    let core = core_with_library(tmp.path().join("library"), tmp.path()).await;

    let fixture = tmp.path().join("src/Vanishing");
    make_mod_dir(&fixture, "vanishing");
    let adopted = core
        .adopt_folder(GameCode::Gimi, &fixture, "Vanishing Mod")
        .await
        .expect("adopt");
    core.set_enabled(&adopted.id, true, &mods_dir)
        .await
        .expect("enable");

    // Simulate the user nuking the Library copy by hand.
    fs::remove_dir_all(&adopted.library_path).expect("remove library copy");

    let result = core
        .reconcile_junctions(GameCode::Gimi, &mods_dir)
        .await
        .expect("reconcile must not error on a dangling junction");

    // Exactly how a dangling junction should be classified is not
    // specified anywhere, so this asserts only the part that is
    // unambiguous: a mod whose Library copy no longer exists is not
    // healthy. Calling it healthy would leave the UI showing an
    // enabled mod the game cannot load.
    assert!(
        !result.healthy.contains(&adopted.id),
        "a mod whose Library copy was deleted must not be reported healthy, got {result:?}",
    );
}
