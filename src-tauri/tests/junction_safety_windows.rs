//! Destructive-safety guarantees for the junction layer.
//!
//! This is the highest-severity failure class in GMM: a junction is a
//! directory-shaped thing that *contains* the user's mod files as far as
//! most APIs are concerned. Any code path that recurses into it, or
//! deletes it with a recursive delete, destroys the Library copy — the
//! only copy, for a mod the user downloaded months ago and can no longer
//! find on GameBanana.
//!
//! On unix these tests would prove nothing: `remove_dir_all` on a
//! symlink doesn't traverse, and the semantics differ from NTFS reparse
//! points. Only Windows can answer them, which is exactly why they sat
//! unwritten until the Windows runner existed.
//!
//! Each test plants a sentinel file in the Library target and asserts it
//! survives.

#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};

use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;

const SENTINEL: &str = "DO-NOT-DELETE.bin";
const SENTINEL_BYTES: &[u8] = b"the user's only copy of this mod";

async fn fresh_core(tmp: &Path) -> Core {
    let db_url = format!("sqlite://{}/gmm.db?mode=rwc", tmp.display());
    Core::new(tmp.join("library"), &db_url).await.expect("core")
}

fn make_mod(dir: &Path) {
    fs::create_dir_all(dir.join("nested/deeper")).expect("mod dirs");
    fs::write(dir.join("merged.ini"), b"hash = 1234\n").expect("ini");
    fs::write(dir.join(SENTINEL), SENTINEL_BYTES).expect("sentinel");
    fs::write(dir.join("nested/deeper/asset.buf"), b"payload").expect("nested asset");
}

/// Assert the Library copy is fully intact, contents included.
fn assert_library_intact(library_path: &Path) {
    assert!(
        library_path.exists(),
        "Library copy {library_path:?} was deleted",
    );
    let sentinel = library_path.join(SENTINEL);
    assert!(
        sentinel.exists(),
        "sentinel {sentinel:?} was deleted — a junction operation traversed into the target",
    );
    assert_eq!(
        fs::read(&sentinel).expect("read sentinel"),
        SENTINEL_BYTES,
        "sentinel contents were modified",
    );
    assert!(
        library_path.join("nested/deeper/asset.buf").exists(),
        "nested Library content was deleted",
    );
}

async fn adopt_and_enable(
    core: &Core,
    tmp: &Path,
    mods_dir: &Path,
    name: &str,
) -> (String, PathBuf) {
    let fixture = tmp.join("src").join(name);
    make_mod(&fixture);
    let adopted = core
        .adopt_folder(GameCode::Gimi, &fixture, name)
        .await
        .expect("adopt");
    core.set_enabled(&adopted.id, true, mods_dir)
        .await
        .expect("enable");
    (adopted.id, adopted.library_path)
}

/// The everyday path. Disabling must unlink, never recurse.
#[tokio::test]
async fn disabling_removes_only_the_junction_not_the_library_copy() {
    let tmp = TempDir::new().expect("tmp");
    let mods_dir = tmp.path().join("game/Mods");
    fs::create_dir_all(&mods_dir).expect("mods dir");
    let core = fresh_core(tmp.path()).await;

    let (id, library_path) = adopt_and_enable(&core, tmp.path(), &mods_dir, "Sentinel Mod").await;
    let link = mods_dir.join("Sentinel Mod");
    assert!(
        link.join(SENTINEL).exists(),
        "precondition: junction resolves"
    );

    core.set_enabled(&id, false, &mods_dir)
        .await
        .expect("disable");

    assert!(!link.exists(), "junction should be gone");
    assert_library_intact(&library_path);
}

/// Rebuild drops and recreates every junction. The drop half is the
/// dangerous one.
#[tokio::test]
async fn rebuilding_junctions_never_touches_library_contents() {
    let tmp = TempDir::new().expect("tmp");
    let mods_dir = tmp.path().join("game/Mods");
    fs::create_dir_all(&mods_dir).expect("mods dir");
    let core = fresh_core(tmp.path()).await;

    let (_id, library_path) = adopt_and_enable(&core, tmp.path(), &mods_dir, "Rebuild Mod").await;

    core.rebuild_junctions(GameCode::Gimi, &mods_dir)
        .await
        .expect("rebuild");

    assert_library_intact(&library_path);
    assert!(
        mods_dir.join("Rebuild Mod").join(SENTINEL).exists(),
        "the rebuilt junction should resolve into the Library again",
    );
}

/// Reconcile walks existing junctions. It must not recurse into them.
#[tokio::test]
async fn reconciling_never_touches_library_contents() {
    let tmp = TempDir::new().expect("tmp");
    let mods_dir = tmp.path().join("game/Mods");
    fs::create_dir_all(&mods_dir).expect("mods dir");
    let core = fresh_core(tmp.path()).await;

    let (_id, library_path) = adopt_and_enable(&core, tmp.path(), &mods_dir, "Reconcile Mod").await;

    core.reconcile_junctions(GameCode::Gimi, &mods_dir)
        .await
        .expect("reconcile");

    assert_library_intact(&library_path);
}

/// Switching variants re-targets the junction, which means removing the
/// old one while the Library still holds every variant's files.
#[tokio::test]
async fn switching_variants_never_touches_library_contents() {
    let tmp = TempDir::new().expect("tmp");
    let mods_dir = tmp.path().join("game/Mods");
    fs::create_dir_all(&mods_dir).expect("mods dir");
    let core = fresh_core(tmp.path()).await;

    // A two-variant mod, each variant carrying its own sentinel.
    let fixture = tmp.path().join("src/VariantMod");
    make_mod(&fixture.join("Red"));
    make_mod(&fixture.join("Blue"));

    let adopted = core
        .adopt_folder(GameCode::Gimi, &fixture, "Variant Mod")
        .await
        .expect("adopt");
    let variants = core.list_variants(&adopted.id).await.expect("variants");
    assert!(variants.len() >= 2, "fixture should yield 2 variants");

    core.set_enabled(&adopted.id, true, &mods_dir)
        .await
        .expect("enable");

    for v in &variants {
        core.set_active_variant(&adopted.id, &v.id, &mods_dir)
            .await
            .unwrap_or_else(|e| panic!("switch to {:?}: {e}", v.id));

        // Both variants' files must still be on disk after every switch.
        for sub in ["Red", "Blue"] {
            let sentinel = adopted.library_path.join(sub).join(SENTINEL);
            assert!(
                sentinel.exists(),
                "variant switch deleted {sentinel:?} from the Library",
            );
        }
    }
}

/// Moving the Library relocates files and rebuilds junctions. A bug
/// here could delete the source before the copy completes.
#[tokio::test]
async fn relocating_the_library_preserves_every_mod_file() {
    let tmp = TempDir::new().expect("tmp");
    let mods_dir = tmp.path().join("game/Mods");
    fs::create_dir_all(&mods_dir).expect("mods dir");
    let core = fresh_core(tmp.path()).await;
    core.set_game_install_path(GameCode::Gimi, &tmp.path().join("game"))
        .await
        .expect("install path");

    let (_id, _old_path) = adopt_and_enable(&core, tmp.path(), &mods_dir, "Relocating Mod").await;

    let new_root = tmp.path().join("new-library-root");
    fs::create_dir_all(&new_root).expect("new root");
    core.set_library_root(Some(&new_root))
        .await
        .expect("relocate library");

    // The mod's files must exist under the new root, sentinel included.
    let listed = core.list_mods(GameCode::Gimi).await.expect("list");
    assert_eq!(listed.len(), 1);
    let moved = &listed[0];
    assert!(
        moved.library_path.starts_with(&new_root),
        "library_path should now be under {new_root:?}, got {:?}",
        moved.library_path,
    );
    assert_library_intact(&moved.library_path);
}
