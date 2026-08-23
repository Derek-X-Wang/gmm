//! Issue #59: kill the process between the filesystem step and the DB
//! step of every durable mutation, restart, and assert recovery.
//!
//! # What this is for
//!
//! `tests/session.rs` covers crash recovery for a Game Session, which is
//! easy because a Game Session has an external witness: a PID that is or
//! is not alive. Ordinary mutations have no witness. `set_enabled`
//! creates a Junction and *then* writes the row; nothing was stopping
//! the process in between, so the invariant that matters —
//!
//! > never a DB row saying enabled with a missing or wrong Junction
//!
//! — was maintained by inspection alone.
//!
//! Each test here runs one mutation in a child process
//! (`crates/probe`), kills it at a named point via
//! [`Core::with_crash_hook`], then reopens the database the way a
//! restart would and asserts that a reconcile pass restores the
//! invariant. `std::process::abort` rather than a clean exit: no
//! unwinding, no destructors, no flushed buffers — a process that
//! stopped existing, not one that shut down badly.
//!
//! # Why several of these assert repair rather than prevention
//!
//! Reconcile exists because the Library is the source of truth (ADR
//! 0003) and the Junctions in `<Game>/Mods/` are projections that can
//! drift. Making each mutation individually crash-atomic would mean a
//! filesystem transaction we do not have on NTFS. So for the torn
//! states reconcile can name, "torn on disk, then reconcile fixes it" is
//! the real guarantee, and it is what these tests assert.
//!
//! # Crash outcomes and their recovery surface
//!
//! Two torn states are *not* enabled-state violations and are not
//! repaired automatically. Both are asserted below as the behaviour
//! they actually have, so that a future change to either is visible:
//!
//! * **Orphaned Library directory.** A crash after `adopt_folder` or
//!   `import_zip` copies files into the Library but before the row is
//!   inserted leaves bytes on disk that nothing references. The
//!   read-only Library audit sees that folder and Settings reports it;
//!   neither path deletes or repairs it because it may hold the user's
//!   only copy of the interrupted import. Inspect/delete/recovery actions
//!   are deliberately deferred to issue #72.
//!
//! * **Orphan renamed but not yet adopted.** Recovering a Library-root
//!   directory whose name is not a usable ULID renames it before writing
//!   the row (#72), for the same reason imports copy before inserting: a
//!   row pointing at a directory that is not there is worse than a
//!   directory no row points at. A crash in that window therefore leaves
//!   the same orphan shape under the new name, which the audit reports
//!   and the same feature can recover a second time.
//!
//! * **Missing Variant rows.** A crash between the row insert and
//!   `detect_and_record_variants` leaves a Mod whose Library subtree has
//!   Variant subfolders but whose `mod_variants` table is empty, so
//!   enabling it junctions to the Mod root instead of a Variant. The
//!   enabled-state invariant still holds — the Junction matches what the
//!   row says — so this is a content bug, not a consistency bug.
//!   Re-running detection on load would fix it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gmm_lib::core::{crash_points, Core, GameCode};
use tempfile::TempDir;

// ---------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------

fn probe_bin() -> PathBuf {
    let name = if cfg!(windows) {
        "concurrency-probe.exe"
    } else {
        "concurrency-probe"
    };
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/debug")
        .join(name);
    assert!(
        p.exists(),
        "{name} missing at {p:?} — run `cargo build --workspace` before this test",
    );
    p
}

struct TestEnv {
    tmp: TempDir,
    data_dir: PathBuf,
    db_url: String,
    library: PathBuf,
    game_mods: PathBuf,
}

impl TestEnv {
    fn new() -> Self {
        let tmp = TempDir::new().expect("tmp");
        let data_dir = tmp.path().join("data");
        let library = data_dir.join("library");
        let game_mods = tmp.path().join("Genshin/Mods");
        std::fs::create_dir_all(&game_mods).expect("game mods dir");
        std::fs::create_dir_all(&data_dir).expect("data dir");
        let db_url = format!("sqlite://{}/gmm.db?mode=rwc", data_dir.display());
        Self {
            tmp,
            data_dir,
            db_url,
            library,
            game_mods,
        }
    }

    /// Reopen the database the way a restart does — a brand new `Core`
    /// against the same file, with no memory of what the dead process
    /// was doing.
    async fn restart(&self) -> Core {
        Core::new(self.library.clone(), &self.db_url)
            .await
            .expect("reopen the DB after a crash")
    }

    /// Run one mutation in a child process that aborts at `crash_at`.
    /// Asserts the child really died rather than completing, so a
    /// mis-typed point name fails loudly instead of silently testing a
    /// clean run.
    fn crash_during(&self, crash_at: &str, op: &[&str]) {
        assert!(
            crash_points::ALL.contains(&crash_at),
            "{crash_at:?} is not a known crash point; \
             add it to crash_points::ALL so it cannot silently never fire",
        );

        let out = Command::new(probe_bin())
            .arg("--data-dir")
            .arg(&self.data_dir)
            .arg("--db")
            .arg(&self.db_url)
            .arg("--library")
            .arg(&self.library)
            .arg("--crash-at")
            .arg(crash_at)
            .args(op)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("spawn probe");

        assert!(
            !out.status.success(),
            "the probe was supposed to die at {crash_at} but exited cleanly — \
             the crash point never fired, so this test proves nothing. \
             stdout: {}",
            String::from_utf8_lossy(&out.stdout),
        );
        assert!(
            out.stdout.is_empty(),
            "the probe printed an outcome, so it finished the operation \
             instead of dying at {crash_at}: {}",
            String::from_utf8_lossy(&out.stdout),
        );
    }

    fn link(&self, name: &str) -> PathBuf {
        self.game_mods.join(name)
    }
}

/// The invariant, asserted the way `tests/concurrency.rs` asserts it:
/// a reconcile pass that repairs nothing is a pass that found nothing
/// torn.
async fn assert_invariant(core: &Core, game_mods: &Path, context: &str) {
    let r = core
        .reconcile_junctions(GameCode::Gimi, game_mods)
        .await
        .expect("reconcile for invariant check");
    assert!(
        r.recreated.is_empty(),
        "{context}: enabled Mod(s) still without a Junction: {:?}",
        r.recreated,
    );
    assert!(
        r.removed.is_empty(),
        "{context}: disabled Mod(s) still with a stranded Junction: {:?}",
        r.removed,
    );
    assert!(
        r.conflicting.is_empty(),
        "{context}: Junction(s) still resolving somewhere unexpected: {:?}",
        r.conflicting,
    );
}

/// Restart, run the documented recovery pass, then assert the invariant
/// holds — i.e. a second pass finds nothing left to do.
async fn recover_and_assert(env: &TestEnv, context: &str) -> Core {
    let core = env.restart().await;
    core.reconcile_junctions(GameCode::Gimi, &env.game_mods)
        .await
        .expect("recovery reconcile");
    assert_invariant(&core, &env.game_mods, context).await;
    core
}

/// Every enabled Mod has a Junction and every disabled Mod has none —
/// checked against the directory rather than through reconcile, so the
/// two checks cannot agree with each other while both being wrong.
async fn assert_rows_match_disk(core: &Core, env: &TestEnv, context: &str) {
    for m in core.list_mods(GameCode::Gimi).await.expect("list") {
        let present = std::fs::symlink_metadata(env.link(&m.name)).is_ok();
        assert_eq!(
            m.enabled, present,
            "{context}: Mod {:?} has enabled={} but Junction present={present}",
            m.name, m.enabled,
        );
    }
}

async fn seed_mod(env: &TestEnv, core: &Core, name: &str) -> gmm_lib::core::Mod {
    let src = env.tmp.path().join("fixtures").join(name);
    std::fs::create_dir_all(&src).expect("fixture dir");
    std::fs::write(src.join("merged.ini"), b"[TextureOverride]\nhash=42\n").expect("ini");
    core.adopt_folder(GameCode::Gimi, &src, name)
        .await
        .expect("adopt")
}

// ---------------------------------------------------------------------
// set_enabled
// ---------------------------------------------------------------------

/// Crash after the Junction is created, before the row is written.
///
/// On restart the row says disabled and the Junction is on disk. Left
/// alone, the Model Importer loads a Mod the UI says is off.
#[tokio::test]
async fn enable_crashing_after_the_junction_is_created_recovers() {
    let env = TestEnv::new();
    let core = env.restart().await;
    let m = seed_mod(&env, &core, "Enable Crash").await;
    drop(core);

    env.crash_during(
        crash_points::SET_ENABLED_AFTER_JUNCTION_CREATE,
        &[
            "set-enabled",
            "--mod-id",
            &m.id,
            "--enabled",
            "1",
            "--mods-dir",
            &env.game_mods.display().to_string(),
        ],
    );

    // The torn state we expect to find, asserted so the test fails
    // loudly if the crash point ever moves and stops tearing anything.
    assert!(
        env.link("Enable Crash").exists(),
        "precondition: the crash left the Junction on disk",
    );
    let core = env.restart().await;
    assert!(
        !core.list_mods(GameCode::Gimi).await.expect("list")[0].enabled,
        "precondition: the row never got its update",
    );
    drop(core);

    let core = recover_and_assert(&env, "enable crashed after junction create").await;
    assert_rows_match_disk(&core, &env, "enable crashed after junction create").await;
}

/// Crash after the Junction is removed, before the row is written.
///
/// On restart the row says enabled and there is no Junction — the
/// canonical form of the invariant this issue names.
#[tokio::test]
async fn disable_crashing_after_the_junction_is_removed_recovers() {
    let env = TestEnv::new();
    let core = env.restart().await;
    let m = seed_mod(&env, &core, "Disable Crash").await;
    core.set_enabled(&m.id, true, &env.game_mods)
        .await
        .expect("enable");
    drop(core);

    env.crash_during(
        crash_points::SET_ENABLED_AFTER_JUNCTION_REMOVE,
        &[
            "set-enabled",
            "--mod-id",
            &m.id,
            "--enabled",
            "0",
            "--mods-dir",
            &env.game_mods.display().to_string(),
        ],
    );

    assert!(
        std::fs::symlink_metadata(env.link("Disable Crash")).is_err(),
        "precondition: the crash left no Junction",
    );
    let core = env.restart().await;
    assert!(
        core.list_mods(GameCode::Gimi).await.expect("list")[0].enabled,
        "precondition: the row still says enabled",
    );
    drop(core);

    let core = recover_and_assert(&env, "disable crashed after junction remove").await;
    assert_rows_match_disk(&core, &env, "disable crashed after junction remove").await;
}

// ---------------------------------------------------------------------
// adopt_folder / import_zip
// ---------------------------------------------------------------------

/// Crash after the Library copy, before the row insert.
///
/// The enabled-state invariant is untouched — there is no row, so there
/// is nothing to be inconsistent with. What is left is an orphaned
/// Library directory that nothing references. Asserted as the known
/// read-only audit now reports it; see the module docs.
#[tokio::test]
async fn adopt_crashing_after_the_library_copy_reports_the_intact_orphan() {
    let env = TestEnv::new();
    let src = env.tmp.path().join("to-adopt");
    std::fs::create_dir_all(&src).expect("src");
    std::fs::write(src.join("merged.ini"), b"hash=7\n").expect("ini");

    env.crash_during(
        crash_points::ADOPT_AFTER_LIBRARY_COPY,
        &[
            "adopt",
            "--from",
            &src.display().to_string(),
            "--name",
            "Orphaned Mod",
        ],
    );

    let core = recover_and_assert(&env, "adopt crashed after the library copy").await;
    assert!(
        core.list_mods(GameCode::Gimi)
            .await
            .expect("list")
            .is_empty(),
        "no Mod row should exist — the insert never ran",
    );

    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit after adopt crash");
    assert_eq!(
        report.unreferenced.len(),
        1,
        "the audit must report exactly the crashed adopt: {report:?}",
    );
    let orphan = &report.unreferenced[0];
    assert!(
        orphan.path.join("merged.ini").exists(),
        "auditing must leave the user's only copy of the import intact",
    );
    assert!(
        orphan.size_bytes.is_some(),
        "the reported orphan includes its size"
    );
}

/// Crash between the row insert and Variant detection.
///
/// The Mod exists and is disabled, so the enabled-state invariant holds.
/// The Variant rows are missing, which is a content bug rather than a
/// consistency one — see the module docs.
#[tokio::test]
async fn adopt_crashing_before_variant_detection_still_holds_the_invariant() {
    let env = TestEnv::new();
    let src = env.tmp.path().join("multi-variant");
    for variant in ["Red", "Blue"] {
        let d = src.join(variant);
        std::fs::create_dir_all(&d).expect("variant dir");
        std::fs::write(d.join("merged.ini"), b"hash=9\n").expect("ini");
    }

    env.crash_during(
        crash_points::ADOPT_AFTER_ROW_INSERT,
        &[
            "adopt",
            "--from",
            &src.display().to_string(),
            "--name",
            "Half Adopted",
        ],
    );

    let core = recover_and_assert(&env, "adopt crashed before variant detection").await;
    let listed = core.list_mods(GameCode::Gimi).await.expect("list");
    assert_eq!(listed.len(), 1, "the row insert committed before the crash");
    assert!(!listed[0].enabled, "a freshly adopted Mod is disabled");
    assert_rows_match_disk(&core, &env, "adopt crashed before variant detection").await;

    // Known limitation, pinned: detection never ran, so the Mod looks
    // single-folder even though its Library subtree has two Variants.
    assert!(
        core.list_variants(&listed[0].id)
            .await
            .expect("variants")
            .is_empty(),
        "Variant rows are missing after a crash before detection",
    );
}

/// Build a Mod archive with two Variant subfolders.
fn build_mod_zip(zip_path: &Path) {
    use std::io::Write as _;
    use zip::write::SimpleFileOptions;

    let file = std::fs::File::create(zip_path).expect("create zip");
    let mut zw = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for variant in ["Red", "Blue"] {
        zw.add_directory(format!("{variant}/"), opts).expect("dir");
        zw.start_file(format!("{variant}/merged.ini"), opts)
            .expect("ini");
        zw.write_all(format!("; {variant}\n").as_bytes())
            .expect("write ini");
    }
    zw.finish().expect("finish zip");
}

/// Crash after the archive is extracted into the Library, before the row
/// insert. Same orphan shape as `adopt`, reached by the other import
/// path — worth its own case because `import_zip` has a cleanup branch
/// that `adopt_folder` does not, and that branch runs only on a returned
/// error, never on a crash.
#[tokio::test]
async fn import_zip_crashing_after_extract_reports_the_intact_orphan() {
    let env = TestEnv::new();
    let zip = env.tmp.path().join("mod.zip");
    build_mod_zip(&zip);

    env.crash_during(
        crash_points::IMPORT_ZIP_AFTER_EXTRACT,
        &[
            "import-zip",
            "--zip",
            &zip.display().to_string(),
            "--name",
            "Zip Orphan",
        ],
    );

    let core = recover_and_assert(&env, "import_zip crashed after extract").await;
    assert!(
        core.list_mods(GameCode::Gimi)
            .await
            .expect("list")
            .is_empty(),
        "no Mod row should exist — the insert never ran",
    );

    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit after zip crash");
    assert_eq!(
        report.unreferenced.len(),
        1,
        "the audit must report exactly the crashed ZIP import: {report:?}",
    );
    assert!(
        report.unreferenced[0].path.join("Red/merged.ini").exists(),
        "auditing must leave extracted Variant files intact",
    );
}

/// Crash between `import_zip`'s row insert and Variant detection. The
/// enabled-state invariant holds; the Variant rows are missing.
#[tokio::test]
async fn import_zip_crashing_before_variant_detection_still_holds_the_invariant() {
    let env = TestEnv::new();
    let zip = env.tmp.path().join("mod2.zip");
    build_mod_zip(&zip);

    env.crash_during(
        crash_points::IMPORT_ZIP_AFTER_ROW_INSERT,
        &[
            "import-zip",
            "--zip",
            &zip.display().to_string(),
            "--name",
            "Half Imported",
        ],
    );

    let core = recover_and_assert(&env, "import_zip crashed before variant detection").await;
    let listed = core.list_mods(GameCode::Gimi).await.expect("list");
    assert_eq!(listed.len(), 1, "the row insert committed before the crash");
    assert!(!listed[0].enabled, "a freshly imported Mod is disabled");
    assert_rows_match_disk(&core, &env, "import_zip crashed before variant detection").await;
    assert!(
        core.list_variants(&listed[0].id)
            .await
            .expect("variants")
            .is_empty(),
        "Variant rows are missing after a crash before detection",
    );
}

// ---------------------------------------------------------------------
// set_active_variant
// ---------------------------------------------------------------------

/// Crash after the row names the new Variant but before the Junction is
/// re-pointed.
///
/// The Junction still resolves into the *old* Variant's subfolder, so
/// the game loads content the UI says is not selected. The row is the
/// source of truth for which Variant is active, and the stale target is
/// inside this Mod's own Library path — nobody but GMM put it there —
/// so this is unambiguously repairable.
#[tokio::test]
async fn variant_switch_crashing_before_the_junction_moves_recovers() {
    let env = TestEnv::new();
    let core = env.restart().await;

    let src = env.tmp.path().join("variant-src");
    for variant in ["Red", "Blue"] {
        let d = src.join(variant);
        std::fs::create_dir_all(&d).expect("variant dir");
        std::fs::write(d.join("merged.ini"), format!("; {variant}\n")).expect("ini");
    }
    let m = core
        .adopt_folder(GameCode::Gimi, &src, "Variant Mod")
        .await
        .expect("adopt");
    let variants = core.list_variants(&m.id).await.expect("variants");
    assert_eq!(variants.len(), 2, "fixture has two Variants");
    core.set_enabled(&m.id, true, &env.game_mods)
        .await
        .expect("enable");

    // Switch to whichever Variant is not currently active.
    let active = core
        .active_variant_id(&m.id)
        .await
        .expect("active")
        .expect("an active variant");
    let target_variant = variants
        .iter()
        .find(|v| v.id != active)
        .expect("the other variant");
    let target_name = target_variant.name.clone();
    let target_id = target_variant.id.clone();
    drop(core);

    env.crash_during(
        crash_points::SET_ACTIVE_VARIANT_AFTER_DB_UPDATE,
        &[
            "set-active-variant",
            "--mod-id",
            &m.id,
            "--variant-id",
            &target_id,
            "--mods-dir",
            &env.game_mods.display().to_string(),
        ],
    );

    // Precondition: the Junction still resolves into the old Variant.
    let resolved = std::fs::canonicalize(env.link("Variant Mod")).expect("resolve junction");
    assert!(
        !resolved.ends_with(&target_name),
        "precondition: the Junction has not moved to {target_name} yet, got {resolved:?}",
    );

    let core = recover_and_assert(&env, "variant switch crashed before the junction moved").await;

    let resolved = std::fs::canonicalize(env.link("Variant Mod")).expect("resolve junction");
    assert!(
        resolved.ends_with(&target_name),
        "reconcile must re-point the Junction at the Variant the row selects; \
         expected it to end with {target_name:?}, got {resolved:?}",
    );
    assert_rows_match_disk(&core, &env, "variant switch recovered").await;
}

/// Crash after the old Junction is removed and before the new one is
/// created. The row says enabled with no Junction at all — the plain
/// case reconcile already handled.
#[tokio::test]
async fn variant_switch_crashing_between_junctions_recovers() {
    let env = TestEnv::new();
    let core = env.restart().await;

    let src = env.tmp.path().join("variant-src-2");
    for variant in ["Red", "Blue"] {
        let d = src.join(variant);
        std::fs::create_dir_all(&d).expect("variant dir");
        std::fs::write(d.join("merged.ini"), format!("; {variant}\n")).expect("ini");
    }
    let m = core
        .adopt_folder(GameCode::Gimi, &src, "Between Mod")
        .await
        .expect("adopt");
    let variants = core.list_variants(&m.id).await.expect("variants");
    core.set_enabled(&m.id, true, &env.game_mods)
        .await
        .expect("enable");
    let active = core
        .active_variant_id(&m.id)
        .await
        .expect("active")
        .expect("active variant");
    let target_id = variants
        .iter()
        .find(|v| v.id != active)
        .expect("other variant")
        .id
        .clone();
    drop(core);

    env.crash_during(
        crash_points::SET_ACTIVE_VARIANT_AFTER_JUNCTION_REMOVE,
        &[
            "set-active-variant",
            "--mod-id",
            &m.id,
            "--variant-id",
            &target_id,
            "--mods-dir",
            &env.game_mods.display().to_string(),
        ],
    );

    assert!(
        std::fs::symlink_metadata(env.link("Between Mod")).is_err(),
        "precondition: the crash left no Junction",
    );

    let core = recover_and_assert(&env, "variant switch crashed between junctions").await;
    assert_rows_match_disk(&core, &env, "variant switch crashed between junctions").await;
}

// ---------------------------------------------------------------------
// recover_unreferenced_library_dir
// ---------------------------------------------------------------------

/// Crash after the rename, before the row insert.
///
/// Nothing is lost and nothing is duplicated: the bytes sit under the
/// fresh ULID, no row claims them, and the audit reports them again — so
/// the user's next click recovers exactly the same folder. That is the
/// whole reason the rename goes first.
#[tokio::test]
async fn recovery_crashing_after_the_rename_leaves_the_folder_recoverable_again() {
    let env = TestEnv::new();
    let game_root = env.library.join("gimi");
    let dropped = game_root.join("Dropped In By Hand");
    std::fs::create_dir_all(&dropped).expect("dropped dir");
    std::fs::write(dropped.join("merged.ini"), b"hash=11\n").expect("ini");

    env.crash_during(
        crash_points::RECOVER_AFTER_LIBRARY_MOVE,
        &[
            "recover",
            "--path",
            &dropped.display().to_string(),
            "--name",
            "Recovered By Hand",
        ],
    );

    let core = recover_and_assert(&env, "recovery crashed after the rename").await;
    assert!(
        core.list_mods(GameCode::Gimi)
            .await
            .expect("list")
            .is_empty(),
        "no Mod row should exist — the insert never ran",
    );
    assert!(
        !dropped.exists(),
        "the rename committed before the crash, so the old name is gone",
    );

    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit after the crashed recovery");
    assert_eq!(
        report.unreferenced.len(),
        1,
        "the renamed folder must still be reported: {report:?}",
    );
    let orphan = &report.unreferenced[0];
    assert_eq!(
        std::fs::read(orphan.path.join("merged.ini")).expect("the user's bytes"),
        b"hash=11\n",
        "an interrupted recovery must not have cost the user their files",
    );

    // And a second attempt now succeeds against the reported path.
    let recovered = core
        .recover_unreferenced_library_dir(GameCode::Gimi, &orphan.path, "Recovered By Hand")
        .await
        .expect("recover the folder the crashed attempt renamed");
    assert_eq!(recovered.library_path, orphan.path);
    assert!(core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit after the second attempt")
        .unreferenced
        .is_empty(),);
}

// ---------------------------------------------------------------------
// delete_unreferenced_library_dir
// ---------------------------------------------------------------------

/// A crash after the durable intent but before the same-volume rename leaves
/// the user's directory untouched. Startup removes only the stranded marker,
/// so the ordinary Library audit can still offer recovery or deletion.
#[tokio::test]
async fn delete_crashing_after_intent_write_keeps_the_orphan_and_cleans_the_intent() {
    let env = TestEnv::new();
    let core = env.restart().await;
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game Library root");
    let orphan = root.join(ulid::Ulid::new().to_string());
    std::fs::create_dir_all(&orphan).expect("orphan");
    std::fs::write(orphan.join("precious.buf"), b"keep me").expect("orphan bytes");
    let orphan_s = orphan.display().to_string();

    env.crash_during(
        crash_points::DELETE_AFTER_INTENT_WRITE,
        &["delete-library-dir", "--path", &orphan_s],
    );
    assert_eq!(
        std::fs::read(orphan.join("precious.buf")).expect("orphan survives pre-rename crash"),
        b"keep me",
    );
    assert!(
        std::fs::read_dir(&root)
            .expect("root after crash")
            .any(|entry| entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .ends_with(".intent")),
        "the crash seam must leave the durable intent before restart",
    );

    let restarted = env.restart().await;
    assert!(
        std::fs::read_dir(&root)
            .expect("root after restart")
            .all(|entry| !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .ends_with(".intent")),
        "startup must remove an intent whose quarantine rename never happened",
    );
    assert_eq!(
        std::fs::read(orphan.join("precious.buf")).expect("startup preserves orphan"),
        b"keep me",
    );
    assert!(
        restarted
            .audit_library(GameCode::Gimi)
            .await
            .expect("audit after restart")
            .unreferenced
            .iter()
            .any(|entry| entry.path == orphan),
        "the intact orphan remains available for explicit user action",
    );
}

/// Delete first atomically removes the proven orphan from the user-visible
/// Library namespace. A crash before recursive purge leaves a reserved
/// quarantine, and constructing the Core on restart must finish that purge.
/// This proves both the durable state and the real startup wiring: removing
/// cleanup from `Core::new` makes the quarantine assertion fail.
#[tokio::test]
async fn delete_crashing_after_quarantine_is_finished_on_restart() {
    let env = TestEnv::new();
    let core = env.restart().await;
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game Library root");
    let orphan = root.join(ulid::Ulid::new().to_string());
    std::fs::create_dir_all(orphan.join("nested")).expect("orphan tree");
    std::fs::write(orphan.join("nested/precious.buf"), b"delete me").expect("orphan bytes");
    let orphan_s = orphan.display().to_string();

    env.crash_during(
        crash_points::DELETE_AFTER_QUARANTINE_MOVE,
        &["delete-library-dir", "--path", &orphan_s],
    );

    assert!(
        !orphan.exists(),
        "the atomic quarantine rename committed before the crash",
    );
    let quarantines: Vec<_> = std::fs::read_dir(&root)
        .expect("Library root after crash")
        .filter_map(std::result::Result::ok)
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_dir())
                && entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".gmm-delete-")
        })
        .collect();
    assert_eq!(
        quarantines.len(),
        1,
        "the crash must leave exactly one resumable delete quarantine",
    );
    assert_eq!(
        std::fs::read(quarantines[0].path().join("nested/precious.buf"))
            .expect("quarantined bytes"),
        b"delete me",
        "the crash point fires before recursive purge begins",
    );

    let restarted = env.restart().await;
    assert!(
        std::fs::read_dir(&root)
            .expect("Library root after restart")
            .all(|entry| !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".gmm-delete-")),
        "Core startup must finish every interrupted delete quarantine",
    );
    assert!(
        restarted
            .audit_library(GameCode::Gimi)
            .await
            .expect("audit after cleanup")
            .unreferenced
            .is_empty(),
        "the completed delete leaves neither a Mod row nor an orphan",
    );
}

/// A quarantined delete remains owned after its SQLite claim commits and
/// before recursive removal finishes. Recovery must not rename those bytes
/// back into the Mod namespace while the delete worker may still be walking
/// them.
#[tokio::test]
async fn recover_refuses_a_live_interrupted_delete_quarantine() {
    let env = TestEnv::new();
    let core = env.restart().await;
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game Library root");
    let orphan = root.join(ulid::Ulid::new().to_string());
    std::fs::create_dir_all(orphan.join("nested")).expect("orphan tree");
    std::fs::write(orphan.join("nested/precious.buf"), b"still being deleted")
        .expect("orphan bytes");
    let orphan_s = orphan.display().to_string();

    env.crash_during(
        crash_points::DELETE_AFTER_QUARANTINE_MOVE,
        &["delete-library-dir", "--path", &orphan_s],
    );
    let quarantine = std::fs::read_dir(&root)
        .expect("Library root after crash")
        .filter_map(std::result::Result::ok)
        .find(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_dir())
                && entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".gmm-delete-")
        })
        .expect("the crash left a delete quarantine")
        .path();

    let recovered = core
        .recover_unreferenced_library_dir(GameCode::Gimi, &quarantine, "Must Stay Quarantined")
        .await;
    assert!(
        matches!(
            recovered,
            Err(gmm_lib::core::Error::NotAnUnreferencedLibraryDir { .. })
        ),
        "recover must not reclaim bytes still owned by a committed delete, got {recovered:?}",
    );
    assert_eq!(
        std::fs::read(quarantine.join("nested/precious.buf"))
            .expect("refused recovery leaves quarantine bytes in place"),
        b"still being deleted",
    );
    assert!(
        core.list_mods(GameCode::Gimi)
            .await
            .expect("list after refusal")
            .is_empty(),
        "a refused recovery must not create a Mod row",
    );
}

// ---------------------------------------------------------------------
// Coverage
// ---------------------------------------------------------------------

/// Every crash point must be exercised by a crash-recovery or deterministic
/// concurrency test. Relocation's snapshot point is a rendezvous seam rather
/// than a process-abort seam, so its coverage belongs in `concurrency.rs`.
///
/// Without this, adding a point to `crash_points::ALL` and forgetting to
/// cover it looks exactly like covering it — the constant compiles, the
/// suite is green, and the new durable step has no crash test at all.
#[test]
fn every_crash_point_is_covered_by_a_test() {
    let source = format!(
        "{}\n{}",
        include_str!("crash_recovery.rs"),
        include_str!("concurrency.rs"),
    );
    let uncovered: Vec<_> = crash_points::ALL
        .iter()
        .filter(|point| {
            // Match on the constant's Rust name, which is what the
            // tests actually reference — `set_enabled.after_x` is
            // declared as `SET_ENABLED_AFTER_X`. Matching the string
            // literal instead would only ever find `crash_points.rs`.
            let const_name = point.replace('.', "_").to_uppercase();
            !source.contains(&const_name)
        })
        .collect();
    assert!(
        uncovered.is_empty(),
        "crash points with no test in this file: {uncovered:?}",
    );
}
