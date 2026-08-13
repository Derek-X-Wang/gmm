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

use std::fs;
use std::path::{Path, PathBuf};

use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

async fn core_with_library(library_root: PathBuf, tmp: &Path) -> Core {
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.display());
    Core::new(library_root, &db_url).await.expect("core")
}

fn make_mod_dir(dir: &Path, marker: &str) {
    fs::create_dir_all(dir).expect("mod dir");
    fs::write(dir.join("merged.ini"), format!("; {marker}\nhash = 1234\n")).expect("ini");
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
