//! Issue #58: two GMM processes against one `gmm.db` and one Library.
//!
//! `tests/session.rs` covers concurrent calls inside one Tokio runtime,
//! which proves nothing about SQLite's cross-process locking or about
//! filesystem races between two GMM processes. This suite spawns real
//! child processes — `crates/probe`, built as `concurrency-probe` — so
//! the operating system, not a mock, decides who wins.
//!
//! # The policy under test
//!
//! GMM is **single-instance per data directory**. Two instances sharing
//! one `gmm.db` and one Library is not supported; the second process is
//! detected and refused. The rationale is in `core::instance_lock`.
//!
//! So the suite has two halves, and they are testing different things:
//!
//! 1. **The gate works.** A second real process is refused, and the lock
//!    is released when the holder dies however it dies.
//! 2. **The gate is not load-bearing for corruption.** Each pairing the
//!    issue names is then run *with the gate deliberately bypassed*, and
//!    the enabled-state invariant is asserted afterwards. The lock is
//!    advisory: a user with a portable copy, a developer running a debug
//!    build alongside the installed one, or anyone whose lock file an
//!    antivirus is holding (`core::instance_lock` fails open there) gets
//!    through it. "We refuse the second instance" must not be the only
//!    thing standing between a user and a corrupt Library.
//!
//! # The invariant
//!
//! *Never a DB row marked enabled with a missing or wrong Junction.*
//!
//! [`assert_enabled_state_invariant`] checks it through `reconcile_junctions`,
//! which reports exactly the two failure modes by name, plus a direct
//! scan of `<Game>/Mods/` for the inverse case reconcile does not cover.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gmm_lib::core::{Core, GameCode};
use tempfile::TempDir;
use ulid::Ulid;

// ---------------------------------------------------------------------
// Probe process harness
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

/// What a probe printed on stdout: one JSON line, always, whether the
/// operation succeeded or failed. Exit codes are deliberately not the
/// channel — a probe that fails to *start* and a probe whose operation
/// was correctly refused must be distinguishable.
#[derive(Debug)]
struct ProbeOutcome {
    ok: bool,
    error: String,
}

impl ProbeOutcome {
    fn parse(line: &str) -> Self {
        let v: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("probe printed non-JSON {line:?}: {e}"));
        Self {
            ok: v["ok"].as_bool().expect("probe JSON has an `ok` field"),
            error: v["error"].as_str().unwrap_or_default().to_string(),
        }
    }

    fn expect_ok(&self, what: &str) {
        assert!(
            self.ok,
            "{what} should have succeeded, got error: {}",
            self.error
        );
    }

    fn expect_refused(&self, what: &str, expected_fragment: &str) {
        assert!(
            !self.ok,
            "{what} should have been refused, but it succeeded"
        );
        let lowered = self.error.to_lowercase();
        assert!(
            lowered.contains(&expected_fragment.to_lowercase()),
            "{what} was refused for the wrong reason — expected something containing \
             {expected_fragment:?}, got: {}",
            self.error,
        );
    }
}

/// Builder for one probe invocation. Every probe gets the same three
/// global paths; the differences are the operation and its arguments.
struct Probe {
    data_dir: PathBuf,
    db_url: String,
    library: PathBuf,
    take_lock: bool,
    at: Option<u128>,
    args: Vec<String>,
}

fn probe(env: &TestEnv) -> Probe {
    Probe {
        data_dir: env.data_dir.clone(),
        db_url: env.db_url.clone(),
        library: env.library.clone(),
        take_lock: false,
        at: None,
        args: Vec::new(),
    }
}

impl Probe {
    /// Make this probe honour the single-instance policy. Off by default:
    /// most tests here are deliberately bypassing the gate to exercise the
    /// layer underneath it.
    fn honouring_the_lock(mut self) -> Self {
        self.take_lock = true;
        self
    }

    /// Delay the operation until `at` so two probes collide instead of
    /// running in sequence. A wall-clock rendezvous rather than a shared
    /// synchronisation primitive: it needs no IPC, and being a few
    /// milliseconds out only makes the test weaker, never flaky.
    fn at(mut self, at: u128) -> Self {
        self.at = Some(at);
        self
    }

    fn op<const N: usize>(mut self, args: [&str; N]) -> Self {
        self.args = args.iter().map(|s| s.to_string()).collect();
        self
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(probe_bin());
        cmd.arg("--data-dir")
            .arg(&self.data_dir)
            .arg("--db")
            .arg(&self.db_url)
            .arg("--library")
            .arg(&self.library);
        if self.take_lock {
            cmd.arg("--take-lock");
        }
        if let Some(at) = self.at {
            cmd.arg("--at").arg(at.to_string());
        }
        cmd.args(&self.args);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd
    }

    /// Run to completion and parse the outcome.
    fn run(self) -> ProbeOutcome {
        let out = self.command().output().expect("spawn probe");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let line = stdout.lines().last().unwrap_or_else(|| {
            panic!(
                "probe printed nothing on stdout; stderr was:\n{}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        ProbeOutcome::parse(line)
    }

    /// Spawn without waiting. Used for the probe that holds a lock open
    /// while the test does something else.
    fn spawn(self) -> RunningProbe {
        let mut child = self.command().spawn().expect("spawn probe");
        let stdout = child.stdout.take().expect("piped stdout");
        RunningProbe {
            child,
            stdout: BufReader::new(stdout),
        }
    }
}

struct RunningProbe {
    child: Child,
    stdout: BufReader<std::process::ChildStdout>,
}

impl RunningProbe {
    /// Block until the probe reports its operation's result. Removes the
    /// need for a sleep before the test's own half of the race.
    fn wait_for_outcome(&mut self) -> ProbeOutcome {
        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .expect("read probe outcome line");
        assert!(
            !line.trim().is_empty(),
            "probe closed stdout without reporting"
        );
        ProbeOutcome::parse(line.trim())
    }

    /// Kill without letting the process unwind — the point is to prove
    /// the *kernel* releases the lock, not that a Drop impl does.
    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------------------------------------------------------------------
// Test environment
// ---------------------------------------------------------------------

struct TestEnv {
    _tmp: TempDir,
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
        // `mode=rwc` matches `build_core`; the probes and the test all
        // open the same file the way the real app does.
        let db_url = format!("sqlite://{}/gmm.db?mode=rwc", data_dir.display());
        Self {
            _tmp: tmp,
            data_dir,
            db_url,
            library,
            game_mods,
        }
    }

    async fn core(&self) -> Core {
        Core::new(self.library.clone(), &self.db_url)
            .await
            .expect("init core")
    }

    /// A Mod in the Library, adopted and left disabled.
    async fn seed_mod(&self, core: &Core, name: &str) -> gmm_lib::core::Mod {
        let src = self._tmp.path().join("fixtures").join(name);
        std::fs::create_dir_all(&src).expect("fixture dir");
        std::fs::write(src.join("merged.ini"), b"[TextureOverride]\nhash=42\n")
            .expect("fixture ini");
        core.adopt_folder(GameCode::Gimi, &src, name)
            .await
            .expect("adopt")
    }
}

/// A rendezvous instant far enough out that both children are past
/// process start-up and Core init by the time it arrives.
fn rendezvous_in(d: Duration) -> u128 {
    (SystemTime::now() + d)
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_millis()
}

// ---------------------------------------------------------------------
// The invariant
// ---------------------------------------------------------------------

/// *Never a DB row marked enabled with a missing or wrong Junction.*
///
/// Checked through `reconcile_junctions`, which names both failure modes
/// directly: `recreated` is "the DB said enabled and the Junction was
/// missing", `conflicting` is "the Junction resolves somewhere other than
/// the Library path the row records". A run that repairs nothing is a run
/// that found nothing torn.
///
/// Reconcile is allowed to be the checker here even though it is also the
/// subject of one pairing: it only mutates when the invariant is already
/// broken, and if it mutates the assertion fails.
async fn assert_enabled_state_invariant(core: &Core, game_mods: &Path, context: &str) {
    let result = core
        .reconcile_junctions(GameCode::Gimi, game_mods)
        .await
        .expect("reconcile for invariant check");

    assert!(
        result.recreated.is_empty(),
        "{context}: {} enabled Mod(s) had no Junction — DB and Library disagree: {:?}",
        result.recreated.len(),
        result.recreated,
    );
    assert!(
        result.removed.is_empty(),
        "{context}: {} disabled Mod(s) had a Junction stranded in the game directory — \
         the Model Importer would keep loading a Mod the UI says is off: {:?}",
        result.removed.len(),
        result.removed,
    );
    assert!(
        result.conflicting.is_empty(),
        "{context}: {} Junction(s) resolve somewhere unexpected: {:?}",
        result.conflicting.len(),
        result.conflicting,
    );
}

/// Reconcile is the documented recovery path for a torn state. Run it,
/// then assert the invariant holds — i.e. a *second* pass finds nothing
/// left to repair. Two passes rather than one because a pass that
/// repaired something must leave nothing behind for the next one.
async fn reconcile_then_assert_invariant(core: &Core, game_mods: &Path, context: &str) {
    core.reconcile_junctions(GameCode::Gimi, game_mods)
        .await
        .expect("recovery reconcile");
    assert_enabled_state_invariant(core, game_mods, context).await;
}

// ---------------------------------------------------------------------
// 1. The gate works
// ---------------------------------------------------------------------

#[test]
fn a_second_gmm_process_is_refused_the_instance_lock() {
    let env = TestEnv::new();

    let mut holder = probe(&env)
        .honouring_the_lock()
        .op(["hold-lock", "--ms", "30000"])
        .spawn();
    holder
        .wait_for_outcome()
        .expect_ok("the first process taking the instance lock");

    probe(&env)
        .honouring_the_lock()
        .op(["hold-lock", "--ms", "0"])
        .run()
        .expect_refused(
            "a second process against the same data directory",
            "another GMM instance",
        );

    // SIGKILL, not a graceful exit: the lock must be released by the
    // kernel closing the handle, so that a crashed GMM never leaves the
    // user unable to start a new one. This is the whole reason the lock
    // is a file handle and not a PID file.
    holder.kill();

    probe(&env)
        .honouring_the_lock()
        .op(["hold-lock", "--ms", "0"])
        .run()
        .expect_ok("a fresh process after the holder was killed");
}

// ---------------------------------------------------------------------
// 2. The pairings, with the gate deliberately bypassed
// ---------------------------------------------------------------------

/// Recover and delete are two claims on the same Library directory. They
/// rendezvous in separate OS processes with the instance gate deliberately
/// bypassed; the large tree keeps delete between validation and removal long
/// enough for recover to exercise the old validate/act window. Exactly one
/// claim may succeed, and its database/filesystem outcome must be complete.
#[tokio::test]
async fn concurrent_recover_and_delete_of_one_library_directory_have_one_winner() {
    let env = TestEnv::new();
    let core = env.core().await;
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game Library root");
    let orphan = root.join(Ulid::new().to_string());
    std::fs::create_dir_all(&orphan).expect("orphan");
    std::fs::write(orphan.join("merged.ini"), b"hash=race\n").expect("marker");
    for n in 0..2_000 {
        std::fs::write(orphan.join(format!("texture-{n:04}.buf")), b"x").expect("race ballast");
    }
    let orphan_s = orphan.display().to_string();

    let at = rendezvous_in(Duration::from_millis(1500));
    let mut recover = probe(&env)
        .at(at)
        .op(["recover", "--path", &orphan_s, "--name", "Race Winner"])
        .spawn();
    let mut delete = probe(&env)
        .at(at)
        .op(["delete-library-dir", "--path", &orphan_s])
        .spawn();
    let (recovered, deleted) = (recover.wait_for_outcome(), delete.wait_for_outcome());

    assert_ne!(
        recovered.ok, deleted.ok,
        "exactly one guarded mutation may claim the directory, got {recovered:?} / {deleted:?}",
    );
    let mods = core.list_mods(GameCode::Gimi).await.expect("list");
    if recovered.ok {
        assert_eq!(mods.len(), 1, "the recovery winner records one Mod");
        assert_eq!(mods[0].library_path, orphan);
        assert_eq!(
            std::fs::read(mods[0].library_path.join("merged.ini")).expect("recovered bytes"),
            b"hash=race\n",
        );
    } else {
        assert!(mods.is_empty(), "the delete winner records no Mod row");
        assert!(!orphan.exists(), "the delete winner removes the directory");
    }
}

/// Two processes enable the same Mod at the same instant.
///
/// Both read `enabled = 0`, so both take the create-Junction branch, and
/// only one `junction::create` can win. The question is what the loser
/// leaves behind: an error is fine, an error *plus* a DB row that
/// disagrees with the Library is not.
#[tokio::test]
async fn concurrent_enable_of_the_same_mod_leaves_a_consistent_state() {
    let env = TestEnv::new();
    let core = env.core().await;
    let m = env.seed_mod(&core, "Contested Mod").await;
    let mods_dir = env.game_mods.display().to_string();

    let at = rendezvous_in(Duration::from_millis(1500));
    let mut a = probe(&env)
        .at(at)
        .op([
            "set-enabled",
            "--mod-id",
            &m.id,
            "--enabled",
            "1",
            "--mods-dir",
            &mods_dir,
        ])
        .spawn();
    let mut b = probe(&env)
        .at(at)
        .op([
            "set-enabled",
            "--mod-id",
            &m.id,
            "--enabled",
            "1",
            "--mods-dir",
            &mods_dir,
        ])
        .spawn();

    let (ra, rb) = (a.wait_for_outcome(), b.wait_for_outcome());
    assert!(
        ra.ok || rb.ok,
        "both processes failed to enable the Mod: {ra:?} / {rb:?}",
    );

    // Whoever won, the user asked for the Mod to be on, and it is on.
    let listed = core.list_mods(GameCode::Gimi).await.expect("list");
    assert!(
        listed[0].enabled,
        "the Mod should be enabled after two concurrent enables: {ra:?} / {rb:?}",
    );
    assert!(
        env.game_mods.join("Contested Mod").exists(),
        "the Junction should exist after two concurrent enables",
    );

    assert_enabled_state_invariant(&core, &env.game_mods, "after concurrent enable/enable").await;
}

/// The nastier interleaving: one process turning a Mod on while another
/// turns it off.
///
/// `Core::set_enabled` reads the `enabled` column, acts on the
/// filesystem, then writes the column back. Across two processes those
/// three steps interleave, and both sides can read the *pre*-state, take
/// a no-op branch, and still write. The result is a DB row and a
/// `<Game>/Mods/` directory that disagree, with **both processes
/// reporting success** — nothing surfaces an error for the user to act
/// on.
///
/// This is not hypothetical: it reproduced on roughly 1 run in 25 while
/// this test was being written, in both directions.
///
/// Given the single-instance policy, the contract asserted here is
/// deliberately *not* "the race cannot tear the state". It is:
///
/// * neither process corrupts the DB or the Library — both run to
///   completion and the pool stays usable, and
/// * a reconcile pass afterwards restores the invariant, whichever way
///   it tore.
///
/// Reconcile is GMM's declared repair path (ADR 0003 makes the Library
/// the source of truth), so "torn on disk, then reconcile fixes it" is
/// the honest guarantee for a configuration we do not support. The
/// guarantee that the race does not happen at all comes from the lock,
/// asserted in `the_instance_lock_makes_the_enable_disable_race_unreachable`.
///
/// Alternating the starting state matters: starting enabled tears toward
/// "row says enabled, Junction missing", starting disabled tears toward
/// "row says disabled, Junction stranded". Only the first was repairable
/// before this issue.
#[tokio::test]
async fn concurrent_enable_and_disable_is_always_recoverable_by_reconcile() {
    let env = TestEnv::new();
    let core = env.core().await;
    let m = env.seed_mod(&core, "Flipflop Mod").await;
    let mods_dir = env.game_mods.display().to_string();
    let link = env.game_mods.join("Flipflop Mod");

    // Several rounds because the interleaving is a genuine race: one
    // round would usually pass by luck rather than by correctness.
    for round in 0..6 {
        let start_enabled = round % 2 == 0;
        core.set_enabled(&m.id, start_enabled, &env.game_mods)
            .await
            .expect("set the round's starting state");

        let at = rendezvous_in(Duration::from_millis(800));
        let mut off = probe(&env)
            .at(at)
            .op([
                "set-enabled",
                "--mod-id",
                &m.id,
                "--enabled",
                "0",
                "--mods-dir",
                &mods_dir,
            ])
            .spawn();
        let mut on = probe(&env)
            .at(at)
            .op([
                "set-enabled",
                "--mod-id",
                &m.id,
                "--enabled",
                "1",
                "--mods-dir",
                &mods_dir,
            ])
            .spawn();

        let (r_off, r_on) = (off.wait_for_outcome(), on.wait_for_outcome());
        let context = format!(
            "round {round} (started {}): disable said {r_off:?}, enable said {r_on:?}",
            if start_enabled { "enabled" } else { "disabled" },
        );

        // Whatever the outcome, the DB has to still be readable and the
        // Library intact. A `list_mods` that errors here is the
        // corruption the issue is really about.
        let listed = core
            .list_mods(GameCode::Gimi)
            .await
            .unwrap_or_else(|e| panic!("{context}: the DB is unreadable after the race: {e}"));
        assert_eq!(listed.len(), 1, "{context}: the Mod row survived");
        assert!(
            listed[0].library_path.join("merged.ini").exists(),
            "{context}: the Library copy is never touched by enable/disable (ADR 0003)",
        );

        reconcile_then_assert_invariant(&core, &env.game_mods, &context).await;

        // Recovery means more than "reconcile reports nothing": the row
        // and the directory have to actually agree.
        let listed = core.list_mods(GameCode::Gimi).await.expect("list");
        assert_eq!(
            listed[0].enabled,
            std::fs::symlink_metadata(&link).is_ok(),
            "{context}: after reconcile the DB and the Library still disagree",
        );
    }
}

/// The other half of the policy: in a supported configuration the race
/// above is unreachable, because the second process never gets far enough
/// to open the DB.
///
/// Deterministic — no rendezvous, no retries. That is the point of
/// choosing "detect and refuse" over "make every path concurrency-safe".
#[test]
fn the_instance_lock_makes_the_enable_disable_race_unreachable() {
    let env = TestEnv::new();
    let mods_dir = env.game_mods.display().to_string();

    let mut first = probe(&env)
        .honouring_the_lock()
        .op(["hold-lock", "--ms", "30000"])
        .spawn();
    first
        .wait_for_outcome()
        .expect_ok("the running GMM instance");

    // A real second launch: same data directory, wants to toggle a Mod.
    // It is turned away before `Core::new` opens the pool, so there is
    // no second writer for anything to race against.
    probe(&env)
        .honouring_the_lock()
        .op([
            "set-enabled",
            "--mod-id",
            "any-mod-id",
            "--enabled",
            "1",
            "--mods-dir",
            &mods_dir,
        ])
        .run()
        .expect_refused(
            "a second instance trying to toggle a Mod",
            "another GMM instance",
        );

    first.kill();
}

/// One process enables a Mod while another runs a reconcile pass.
///
/// Reconcile reads every row and repairs what it finds; enable is
/// changing a row and the filesystem underneath it. Whichever order they
/// land in, both create the same Junction to the same target, so a
/// collision here should be benign — but "should be" is the reason to
/// test it. The failure this guards against is reconcile creating the
/// Junction first, `set_enabled` then failing on the already-existing
/// link, and the row never being written.
#[tokio::test]
async fn enable_racing_reconcile_leaves_the_mod_on_and_linked() {
    let env = TestEnv::new();
    let core = env.core().await;
    let m = env.seed_mod(&core, "Reconciled Mod").await;
    let mods_dir = env.game_mods.display().to_string();

    // Reconcile only has something to do if a row already says enabled,
    // so seed a second Mod that is on and whose Junction is missing.
    let other = env.seed_mod(&core, "Already On").await;
    core.set_enabled(&other.id, true, &env.game_mods)
        .await
        .expect("enable the second mod");
    gmm_lib::core::junction::remove(&env.game_mods.join("Already On"))
        .expect("tear the second mod's junction so reconcile has work");

    let at = rendezvous_in(Duration::from_millis(800));
    let mut enabling = probe(&env)
        .at(at)
        .op([
            "set-enabled",
            "--mod-id",
            &m.id,
            "--enabled",
            "1",
            "--mods-dir",
            &mods_dir,
        ])
        .spawn();
    let mut reconciling = probe(&env)
        .at(at)
        .op(["reconcile", "--mods-dir", &mods_dir])
        .spawn();

    let (r_enable, r_reconcile) = (enabling.wait_for_outcome(), reconciling.wait_for_outcome());
    let context = format!("enable said {r_enable:?}, reconcile said {r_reconcile:?}");

    reconcile_then_assert_invariant(&core, &env.game_mods, &context).await;

    let listed = core.list_mods(GameCode::Gimi).await.expect("list");
    for row in &listed {
        assert_eq!(
            row.enabled,
            std::fs::symlink_metadata(env.game_mods.join(&row.name)).is_ok(),
            "{context}: {} disagrees with its Junction",
            row.name,
        );
    }
    assert!(
        listed.iter().any(|r| r.id == other.id && r.enabled),
        "{context}: reconcile must not have turned the already-enabled Mod off",
    );
}

/// Two cold instances open the same brand-new `gmm.db` at the same
/// instant, both needing to run every migration.
///
/// `sqlx`'s `Migrate` impl for SQLite has a no-op `lock`, so there is no
/// cross-process mutex around the migration run — both processes can see
/// an empty `_sqlx_migrations` and both start applying `CREATE TABLE`.
/// What must not happen is a half-migrated database that neither process
/// can subsequently open.
#[tokio::test]
async fn two_cold_instances_migrating_at_once_leave_an_openable_database() {
    let env = TestEnv::new();

    let at = rendezvous_in(Duration::from_millis(800));
    let mut a = probe(&env).at(at).op(["migrate"]).spawn();
    let mut b = probe(&env).at(at).op(["migrate"]).spawn();
    let (ra, rb) = (a.wait_for_outcome(), b.wait_for_outcome());

    assert!(
        ra.ok || rb.ok,
        "neither cold instance could migrate the database: {ra:?} / {rb:?}",
    );

    // The real assertion: whoever lost, the next launch works. A DB left
    // half-migrated is unrecoverable for a user — GMM would refuse to
    // start every time from then on.
    let core = env.core().await;
    let listed = core.list_mods(GameCode::Gimi).await.unwrap_or_else(|e| {
        panic!("the DB is unusable after a migration race ({ra:?} / {rb:?}): {e}")
    });
    assert!(listed.is_empty(), "a freshly migrated DB has no Mods");

    // The seed data the initial migration installs has to be intact too,
    // not merely the schema — `installer-smoke.ps1` checks the same six
    // codes against the shipped MSI.
    for code in ["gimi", "srmi", "zzmi", "wwmi", "himi", "efmi"] {
        let game: GameCode = code.parse().expect("game code");
        core.game_install_path(game)
            .await
            .unwrap_or_else(|e| panic!("game row for {code} missing after migration race: {e}"));
    }
}

/// The single-instance lock also removes the migration race, and this is
/// the pairing where that matters most: the two others are recoverable by
/// reconcile, a half-applied schema is not.
#[test]
fn the_instance_lock_makes_the_migration_race_unreachable() {
    let env = TestEnv::new();

    let mut first = probe(&env)
        .honouring_the_lock()
        .op(["hold-lock", "--ms", "30000"])
        .spawn();
    first
        .wait_for_outcome()
        .expect_ok("the running GMM instance");

    probe(&env)
        .honouring_the_lock()
        .op(["migrate"])
        .run()
        .expect_refused(
            "a second cold instance trying to migrate",
            "another GMM instance",
        );

    first.kill();
}

/// Build a minimal Model Importer package: the shape
/// `install_from_local_zip` validates (#113) — `d3dx.ini` at the root,
/// `Core/`, `ShaderFixes/`, and no compiled binaries.
fn build_importer_zip(zip_path: &Path) {
    use std::io::Write as _;
    use zip::write::SimpleFileOptions;

    let file = std::fs::File::create(zip_path).expect("create zip");
    let mut zw = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    zw.start_file("d3dx.ini", opts).expect("d3dx.ini");
    zw.write_all(b"; 3dmigoto importer\n[Loader]\nloader = XXMI Launcher.exe\n")
        .expect("write d3dx");
    zw.add_directory("Core/", opts).expect("Core dir");
    zw.start_file("Core/library.ini", opts).expect("core ini");
    zw.write_all(b"; core library\n").expect("write core");
    zw.add_directory("ShaderFixes/", opts)
        .expect("ShaderFixes dir");
    zw.finish().expect("finish zip");
}

/// Installing a Model Importer rewrites files inside the game directory.
/// Doing that while a Game Session is live means overwriting a DLL the
/// running game has mapped — at best the install fails on a locked file,
/// at worst the user gets a half-swapped importer next launch.
///
/// The guard is the `active_session` row, and because that row lives in
/// the shared `gmm.db` it should hold across processes as well as within
/// one. This test is what makes "should" into "does": the session is
/// claimed by one process and the install attempted by a different one.
#[tokio::test]
async fn importer_install_is_refused_by_another_processs_game_session() {
    let env = TestEnv::new();
    let core = env.core().await;

    let game_dir = env.game_mods.parent().expect("game dir").to_path_buf();
    let zip = env._tmp.path().join("importer.zip");
    build_importer_zip(&zip);
    let backups = env.data_dir.join("backups");
    let (zip_s, game_s, backups_s) = (
        zip.display().to_string(),
        game_dir.display().to_string(),
        backups.display().to_string(),
    );

    // Sanity: with no session the install is allowed. Otherwise the
    // refusal below could be caused by anything at all.
    probe(&env)
        .op([
            "install-importer",
            "--zip",
            &zip_s,
            "--game-dir",
            &game_s,
            "--backups",
            &backups_s,
        ])
        .run()
        .expect_ok("installing an importer with no session active");

    // A different process claims a Game Session and exits; the row
    // outlives it, which is the point — the session belongs to the DB,
    // not to a process's memory.
    probe(&env)
        .op(["start-session", "--pid", &std::process::id().to_string()])
        .run()
        .expect_ok("claiming a game session from another process");

    probe(&env)
        .op([
            "install-importer",
            "--zip",
            &zip_s,
            "--game-dir",
            &game_s,
            "--backups",
            &backups_s,
        ])
        .run()
        .expect_refused(
            "installing an importer during another process's session",
            "running",
        );

    core.end_session().await.expect("end session");
    probe(&env)
        .op([
            "install-importer",
            "--zip",
            &zip_s,
            "--game-dir",
            &game_s,
            "--backups",
            &backups_s,
        ])
        .run()
        .expect_ok("installing an importer once the session ended");
}

/// Two processes launch a game at the same instant. Exactly one may win
/// the Game Session: `active_session` is a singleton row keyed on
/// `id = 1`, so the loser hits a primary-key conflict rather than
/// silently overwriting the winner's PID.
///
/// If both could claim it, `end_session` from one would clear the other's
/// mutation lock and the user could toggle Mods with the game running —
/// which is the thing a Game Session exists to prevent.
#[tokio::test]
async fn only_one_process_can_claim_the_game_session() {
    let env = TestEnv::new();
    let core = env.core().await;

    let at = rendezvous_in(Duration::from_millis(800));
    let mut a = probe(&env)
        .at(at)
        .op(["start-session", "--pid", "4001"])
        .spawn();
    let mut b = probe(&env)
        .at(at)
        .op(["start-session", "--pid", "4002"])
        .spawn();
    let (ra, rb) = (a.wait_for_outcome(), b.wait_for_outcome());

    assert_ne!(
        ra.ok, rb.ok,
        "exactly one process may claim the Game Session, got {ra:?} / {rb:?}",
    );

    let info = core
        .session_info()
        .await
        .expect("read session")
        .expect("a session is active");
    assert!(
        info.pid == 4001 || info.pid == 4002,
        "the surviving session belongs to one of the two claimants, got pid {}",
        info.pid,
    );
}
