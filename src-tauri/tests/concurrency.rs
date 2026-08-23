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

use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
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
    pause_at: Option<&'static str>,
    crash_at: Option<&'static str>,
    args: Vec<String>,
}

fn probe(env: &TestEnv) -> Probe {
    Probe {
        data_dir: env.data_dir.clone(),
        db_url: env.db_url.clone(),
        library: env.library.clone(),
        take_lock: false,
        at: None,
        pause_at: None,
        crash_at: None,
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

    /// Stop at one of the production crash-point seams until the parent
    /// explicitly releases the child. Unlike a rendezvous timestamp, this
    /// proves the mutation has reached the exact durable step under test.
    fn pausing_at(mut self, point: &'static str) -> Self {
        self.pause_at = Some(point);
        self
    }

    fn crashing_at(mut self, point: &'static str) -> Self {
        self.crash_at = Some(point);
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
        if let Some(point) = self.pause_at {
            cmd.arg("--pause-at").arg(point);
        }
        if let Some(point) = self.crash_at {
            cmd.arg("--crash-at").arg(point);
        }
        cmd.args(&self.args);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
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
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        RunningProbe {
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
        }
    }
}

struct RunningProbe {
    child: Child,
    stdin: Option<ChildStdin>,
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

    fn wait_for_pause(&mut self, expected: &str) {
        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .expect("read probe pause line");
        assert!(
            !line.trim().is_empty(),
            "probe closed stdout before pausing at {expected}"
        );
        let event: serde_json::Value = serde_json::from_str(line.trim())
            .unwrap_or_else(|error| panic!("probe printed non-JSON pause {line:?}: {error}"));
        assert_eq!(
            event["pausedAt"].as_str(),
            Some(expected),
            "probe paused at the wrong crash point: {event}",
        );
    }

    fn resume(&mut self) {
        let stdin = self.stdin.as_mut().expect("probe stdin still open");
        writeln!(stdin, "resume").expect("release paused probe");
        stdin.flush().expect("flush probe release");
    }

    fn wait_for_crash(&mut self) {
        self.stdin.take();
        let status = self.child.wait().expect("wait for crashed probe");
        assert!(!status.success(), "probe completed instead of crashing");
    }

    /// Kill without letting the process unwind — the point is to prove
    /// the *kernel* releases the lock, not that a Drop impl does.
    fn kill(&mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for RunningProbe {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            self.kill();
        }
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

/// Relocation has snapshotted the rows it will rewrite but has not moved the
/// old root yet. Recovery must not commit a row after that snapshot, because
/// relocation could never discover and rewrite it.
#[tokio::test]
async fn relocation_with_an_open_snapshot_transaction_excludes_recovery_commit() {
    let env = TestEnv::new();
    let core = env.core().await;
    let old_root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("old Library root");
    let new_root = env._tmp.path().join("relocated-gimi");
    let orphan = old_root.join(Ulid::new().to_string());
    std::fs::create_dir_all(&orphan).expect("orphan");
    std::fs::write(orphan.join("merged.ini"), b"hash=relocate-first\n").expect("marker");

    let mut relocating = probe(&env)
        .pausing_at(gmm_lib::core::crash_points::RELOCATE_AFTER_MOD_SNAPSHOT)
        .op([
            "set-library-path",
            "--path",
            &new_root.display().to_string(),
        ])
        .spawn();
    relocating.wait_for_pause(gmm_lib::core::crash_points::RELOCATE_AFTER_MOD_SNAPSHOT);

    let recovered = probe(&env)
        .op([
            "recover",
            "--path",
            &orphan.display().to_string(),
            "--name",
            "Must Not Be Stranded",
        ])
        .run();
    assert!(
        !recovered.ok,
        "recover_unreferenced_library_dir must not commit after relocation's row snapshot; \
         it succeeded and would leave a stale library_path: {recovered:?}",
    );

    relocating.resume();
    relocating
        .wait_for_outcome()
        .expect_ok("the fenced relocation after recovery was refused");

    assert!(
        core.list_mods(GameCode::Gimi)
            .await
            .expect("list")
            .is_empty(),
        "the refused recovery must not leave a Mod row",
    );
    assert!(
        new_root
            .join(orphan.file_name().expect("orphan name"))
            .join("merged.ini")
            .is_file(),
        "relocation must still move the orphan's bytes intact",
    );
}

/// Recovery has validated the orphan and is paused immediately before its row
/// insert. Relocation must be refused before touching the filesystem, leaving
/// the path recovery commits both present and identity-stable.
#[tokio::test]
async fn recovery_started_before_relocation_cannot_be_stranded() {
    let env = TestEnv::new();
    let core = env.core().await;
    let old_root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("old Library root");
    let new_root = env._tmp.path().join("relocated-gimi");
    let orphan = old_root.join(Ulid::new().to_string());
    std::fs::create_dir_all(&orphan).expect("orphan");
    std::fs::write(orphan.join("merged.ini"), b"hash=recover-first\n").expect("marker");

    let mut recovering = probe(&env)
        .pausing_at(gmm_lib::core::crash_points::RECOVER_AFTER_LIBRARY_MOVE)
        .op([
            "recover",
            "--path",
            &orphan.display().to_string(),
            "--name",
            "Recovery Owns The Path",
        ])
        .spawn();
    recovering.wait_for_pause(gmm_lib::core::crash_points::RECOVER_AFTER_LIBRARY_MOVE);

    let relocated = probe(&env)
        .op([
            "set-library-path",
            "--path",
            &new_root.display().to_string(),
        ])
        .run();
    assert!(
        !relocated.ok,
        "set_library_path_for_game must be fenced before filesystem work while recovery owns \
         the writer claim; relocation unexpectedly succeeded: {relocated:?}",
    );
    assert!(
        orphan.is_dir(),
        "set_library_path_for_game moved recovery's validated directory before taking the \
         shared fence",
    );

    recovering.resume();
    recovering
        .wait_for_outcome()
        .expect_ok("recovery after the competing relocation was refused");
    let mods = core.list_mods(GameCode::Gimi).await.expect("list");
    assert_eq!(mods.len(), 1, "the recovery must commit exactly one Mod");
    assert_eq!(mods[0].library_path, orphan);
    assert!(
        mods[0].library_path.join("merged.ini").is_file(),
        "the recovered row must point at the directory recovery validated",
    );
}

/// The SQLite fence serializes GMM callers, but an external filesystem actor
/// can still rename and replace the pathname recovery validated. The final
/// identity check must fail closed instead of committing the replacement.
#[tokio::test]
async fn recovery_revalidates_the_directory_identity_before_commit() {
    let env = TestEnv::new();
    let core = env.core().await;
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("Library root");
    let orphan = root.join(Ulid::new().to_string());
    let original = root.join("original-held-aside");
    std::fs::create_dir_all(&orphan).expect("orphan");
    std::fs::write(orphan.join("merged.ini"), b"hash=validated\n").expect("marker");

    let mut recovering = probe(&env)
        .pausing_at(gmm_lib::core::crash_points::RECOVER_AFTER_LIBRARY_MOVE)
        .op([
            "recover",
            "--path",
            &orphan.display().to_string(),
            "--name",
            "Must Keep Its Identity",
        ])
        .spawn();
    recovering.wait_for_pause(gmm_lib::core::crash_points::RECOVER_AFTER_LIBRARY_MOVE);

    std::fs::rename(&orphan, &original).expect("move validated directory aside");
    std::fs::create_dir_all(&orphan).expect("replacement directory");
    std::fs::write(orphan.join("merged.ini"), b"hash=replacement\n").expect("replacement marker");

    recovering.resume();
    let recovered = recovering.wait_for_outcome();
    assert!(
        !recovered.ok,
        "recover_unreferenced_library_dir must refuse when its final library_path names a \
         replacement directory: {recovered:?}",
    );
    assert!(
        core.list_mods(GameCode::Gimi)
            .await
            .expect("list")
            .is_empty(),
        "identity revalidation failure must not commit a Mod row",
    );
    assert_eq!(
        std::fs::read(original.join("merged.ini")).expect("original bytes"),
        b"hash=validated\n",
    );
    assert_eq!(
        std::fs::read(orphan.join("merged.ini")).expect("replacement bytes"),
        b"hash=replacement\n",
    );
}

fn write_single_file_mod_zip(path: &Path) {
    let file = std::fs::File::create(path).expect("create zip");
    let mut archive = zip::ZipWriter::new(file);
    archive
        .start_file("merged.ini", zip::write::SimpleFileOptions::default())
        .expect("zip entry");
    archive.write_all(b"hash=zip-race\n").expect("zip bytes");
    archive.finish().expect("finish zip");
}

fn public_async_function<'a>(source: &'a str, function: &str) -> &'a str {
    let signature = format!("    pub async fn {function}(");
    let start = source.find(&signature).unwrap_or_else(|| {
        panic!("Library mutation {function} is missing from the implementation")
    });
    let rest = &source[start + signature.len()..];
    let end = rest
        .find("\n    pub async fn ")
        .or_else(|| rest.find("\n    async fn "))
        .or_else(|| rest.find("\n}"))
        .unwrap_or(rest.len());
    &source[start..start + signature.len() + end]
}

/// Best-effort inventory of today's known Library-content mutations. This is
/// intentionally not a compile-time boundary: a new module, helper, primitive
/// such as `std::fs::rename`, or differently formatted function can escape its
/// textual discovery. It still catches policy drift in the current call sites;
/// a real production boundary belongs in a focused follow-up rather than this
/// concurrency fix.
#[test]
fn current_known_library_content_mutations_declare_their_fence_policy() {
    let core = include_str!("../src/core/mod.rs");
    let recovery = include_str!("../src/core/library_recovery.rs");
    let sources = [core, recovery];

    let contracts: &[(&str, &[&str])] = &[
        (
            "finish_interrupted_library_deletes",
            &["LibraryMutation::FinishInterruptedDeletes"],
        ),
        (
            "set_library_root",
            &["begin_library_mutation", "LibraryMutation::SetLibraryRoot"],
        ),
        (
            "set_library_path_for_game",
            &[
                "begin_library_mutation",
                "LibraryMutation::SetLibraryPathForGame",
            ],
        ),
        (
            "adopt_folder",
            &[
                "snapshot_library_root_for_mutation",
                "revalidate_library_root_for_mutation",
                "LibraryMutation::AdoptFolder",
            ],
        ),
        (
            "import_zip",
            &[
                "snapshot_library_root_for_mutation",
                "revalidate_library_root_for_mutation",
                "LibraryMutation::ImportZip",
            ],
        ),
        (
            "recover_unreferenced_library_dir",
            &[
                "begin_guarded_library_mutation",
                "LibraryMutation::RecoverUnreferencedLibraryDir",
            ],
        ),
        (
            "delete_unreferenced_library_dir",
            &[
                "begin_guarded_library_mutation",
                "LibraryMutation::DeleteUnreferencedLibraryDir",
            ],
        ),
        (
            "reinstall_gamebanana_mod_with_endpoints",
            &[
                "record_library_mutation_exemption",
                "LibraryMutation::ReinstallGamebananaMod",
                "166",
            ],
        ),
    ];

    let discovery_patterns = [
        "purge_delete_quarantines(",
        ".move_root(",
        "copy_dir_recursive(",
        "zip_import::extract(",
        "begin_guarded_library_mutation(",
        "std::fs::remove_dir_all(&library_path)",
    ];
    let mut discovered = Vec::new();
    for source in sources {
        for line in source.lines() {
            let Some(signature) = line.strip_prefix("    pub async fn ") else {
                continue;
            };
            let Some((function, _)) = signature.split_once('(') else {
                continue;
            };
            let body = public_async_function(source, function);
            if discovery_patterns
                .iter()
                .any(|pattern| body.contains(pattern))
            {
                discovered.push(function);
            }
        }
    }
    discovered.sort_unstable();
    discovered.dedup();

    for function in &discovered {
        assert!(
            contracts
                .iter()
                .any(|(registered, _)| registered == function),
            "Library mutation {function} has neither a shared fence policy nor a deliberate \
             issue-backed exemption",
        );
    }

    for (function, required_markers) in contracts {
        assert!(
            discovered.contains(function),
            "Library mutation {function} escaped filesystem-mutation discovery; add its \
             primitive to the enforcement test before changing its fence policy",
        );
        let body = sources
            .iter()
            .find_map(|source| {
                source
                    .contains(&format!("    pub async fn {function}("))
                    .then(|| public_async_function(source, function))
            })
            .expect("contract function exists in one source");
        for marker in *required_markers {
            assert!(
                body.contains(marker),
                "Library mutation {function} is outside its shared fence policy: missing \
                 {marker:?}",
            );
        }
    }
}

/// An adopt that has finished its unbounded copy must revalidate the root
/// under the shared fence before inserting its row. If relocation won in the
/// meantime, adopt fails closed without committing a stale row. Guarded cleanup
/// can deliberately preserve an orphan when relocation copied the directory
/// and therefore changed its filesystem identity (the Windows fallback).
#[tokio::test]
async fn adopt_revalidates_the_library_root_after_copy() {
    let env = TestEnv::new();
    let core = env.core().await;
    let new_root = env._tmp.path().join("relocated-gimi");
    let source = env._tmp.path().join("adopt-source");
    std::fs::create_dir_all(&source).expect("source");
    std::fs::write(source.join("merged.ini"), b"hash=adopt-race\n").expect("marker");

    let mut adopting = probe(&env)
        .pausing_at(gmm_lib::core::crash_points::ADOPT_AFTER_LIBRARY_COPY)
        .op([
            "adopt",
            "--from",
            &source.display().to_string(),
            "--name",
            "Adopt Root Race",
        ])
        .spawn();
    adopting.wait_for_pause(gmm_lib::core::crash_points::ADOPT_AFTER_LIBRARY_COPY);
    probe(&env)
        .op([
            "set-library-path",
            "--path",
            &new_root.display().to_string(),
        ])
        .run()
        .expect_ok("relocation while adopt is between copy and commit");
    adopting.resume();
    let adopted = adopting.wait_for_outcome();
    assert!(
        !adopted.ok,
        "adopt_folder must fail closed when relocation changes its resolved root; \
         it committed a stale row: {adopted:?}",
    );
    assert!(
        core.list_mods(GameCode::Gimi)
            .await
            .expect("list")
            .is_empty(),
        "a refused adopt must not commit a Mod row",
    );
}

/// A failed staged commit is not proof that its ULID is still unowned. After
/// relocation carries the staged bytes to the new root, recovery may commit a
/// row for them before the original adopt reaches cleanup. Cleanup must re-take
/// the writer fence and preserve the now-owned directory.
#[tokio::test]
async fn failed_adopt_cleanup_preserves_a_concurrently_recovered_mod() {
    let env = TestEnv::new();
    let core = env.core().await;
    let new_root = env._tmp.path().join("relocated-gimi");
    let source = env._tmp.path().join("adopt-cleanup-source");
    std::fs::create_dir_all(&source).expect("source");
    std::fs::write(source.join("merged.ini"), b"hash=cleanup-owner-race\n").expect("marker");

    let mut adopting = probe(&env)
        .pausing_at(gmm_lib::core::crash_points::ADOPT_AFTER_LIBRARY_COPY)
        .op([
            "adopt",
            "--from",
            &source.display().to_string(),
            "--name",
            "Original Staged Adopt",
        ])
        .spawn();
    adopting.wait_for_pause(gmm_lib::core::crash_points::ADOPT_AFTER_LIBRARY_COPY);

    probe(&env)
        .op([
            "set-library-path",
            "--path",
            &new_root.display().to_string(),
        ])
        .run()
        .expect_ok("relocation while adopt is between copy and commit");

    let staged = std::fs::read_dir(&new_root)
        .expect("relocated root")
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.is_dir())
        .expect("relocated staged directory");
    probe(&env)
        .op([
            "recover",
            "--path",
            &staged.display().to_string(),
            "--name",
            "Recovered Staged Mod",
        ])
        .run()
        .expect_ok("recovery that wins before failed-adopt cleanup");

    adopting.resume();
    let adopted = adopting.wait_for_outcome();
    assert!(
        !adopted.ok,
        "the original adopt must still fail after its root changed: {adopted:?}",
    );

    let mods = core.list_mods(GameCode::Gimi).await.expect("list");
    assert_eq!(mods.len(), 1, "recovery must commit exactly one Mod");
    assert!(
        mods[0].library_path.join("merged.ini").is_file(),
        "failed staged cleanup must not delete a directory a concurrent recovery now owns",
    );
}

/// A path name is not proof of ownership. An external actor can move the
/// staged directory aside and put a different filesystem object at the same
/// ULID before failed cleanup runs. Cleanup must retain the staged identity and
/// refuse to delete the replacement.
#[tokio::test]
async fn failed_adopt_cleanup_preserves_a_replacement_directory() {
    let env = TestEnv::new();
    let new_root = env._tmp.path().join("relocated-gimi");
    let source = env._tmp.path().join("adopt-cleanup-identity-source");
    std::fs::create_dir_all(&source).expect("source");
    std::fs::write(source.join("merged.ini"), b"hash=original-staged\n").expect("marker");

    let mut adopting = probe(&env)
        .pausing_at(gmm_lib::core::crash_points::ADOPT_AFTER_LIBRARY_COPY)
        .op([
            "adopt",
            "--from",
            &source.display().to_string(),
            "--name",
            "Identity Staged Adopt",
        ])
        .spawn();
    adopting.wait_for_pause(gmm_lib::core::crash_points::ADOPT_AFTER_LIBRARY_COPY);

    probe(&env)
        .op([
            "set-library-path",
            "--path",
            &new_root.display().to_string(),
        ])
        .run()
        .expect_ok("relocation while adopt is between copy and commit");

    let staged = std::fs::read_dir(&new_root)
        .expect("relocated root")
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.is_dir())
        .expect("relocated staged directory");
    let original = new_root.join("original-staged-held-aside");
    std::fs::rename(&staged, &original).expect("move staged object aside");
    std::fs::create_dir(&staged).expect("replacement directory");
    std::fs::write(staged.join("merged.ini"), b"hash=replacement\n").expect("replacement marker");

    adopting.resume();
    let adopted = adopting.wait_for_outcome();
    assert!(
        !adopted.ok,
        "the original adopt must still fail after its root changed: {adopted:?}",
    );
    assert_eq!(
        std::fs::read(staged.join("merged.ini")).expect("replacement survives"),
        b"hash=replacement\n",
        "failed staged cleanup must not delete a replacement with a different filesystem identity",
    );
    assert_eq!(
        std::fs::read(original.join("merged.ini")).expect("original survives aside"),
        b"hash=original-staged\n",
    );
}

/// Identity handles are evidence, not exclusion locks. Even after cleanup has
/// checked both identity and database ownership under the writer fence, an
/// external actor can rename that object away and put new bytes at the same
/// pathname. Cleanup must anchor deletion to the reserved quarantine name it
/// creates, then re-check which object the rename actually moved.
///
/// Mutation oracle: replacing the quarantine rename with direct
/// `remove_dir_all(path)` deletes `replacement-marker` after this seam swaps
/// it into the validated pathname, and the replacement-survival assertion
/// goes red.
#[tokio::test]
async fn staged_cleanup_quarantine_preserves_a_post_validation_replacement() {
    let env = TestEnv::new();
    let core = env.core().await;
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game Library root");
    let missing_source = env._tmp.path().join("missing-adopt-source");
    let saved_original = root.join("validated-staged-moved-away");
    let swapped_path = std::sync::Arc::new(std::sync::Mutex::new(None));
    let hook_swapped_path = std::sync::Arc::clone(&swapped_path);
    let hook_root = root.clone();
    let hook_saved_original = saved_original.clone();
    let cleaning = core
        .clone()
        .with_crash_hook(std::sync::Arc::new(move |point| {
            if point == gmm_lib::core::crash_points::STAGED_CLEANUP_BEFORE_QUARANTINE_MOVE {
                let staged = std::fs::read_dir(&hook_root)
                    .expect("Library root at staged-cleanup seam")
                    .filter_map(std::result::Result::ok)
                    .map(|entry| entry.path())
                    .find(|path| {
                        path.is_dir()
                            && path
                                .file_name()
                                .and_then(|name| name.to_str())
                                .is_some_and(|name| Ulid::from_string(name).is_ok())
                    })
                    .expect("staged ULID directory before quarantine move");
                std::fs::write(staged.join("original-marker"), b"validated bytes")
                    .expect("mark validated directory");
                std::fs::rename(&staged, &hook_saved_original)
                    .expect("move the validated directory away after proof");
                std::fs::create_dir(&staged).expect("replacement directory");
                std::fs::write(staged.join("replacement-marker"), b"replacement bytes")
                    .expect("mark replacement directory");
                *hook_swapped_path.lock().expect("record swapped path") = Some(staged);
            }
        }));

    let adopted = cleaning
        .adopt_folder(GameCode::Gimi, &missing_source, "Must Fail Copy")
        .await;
    assert!(
        adopted.is_err(),
        "the missing source must fail the staged adopt"
    );
    let staged = swapped_path
        .lock()
        .expect("read swapped path")
        .clone()
        .expect("cleanup reached the post-validation seam");
    assert_eq!(
        std::fs::read(staged.join("replacement-marker")).expect("replacement survives"),
        b"replacement bytes",
        "staged cleanup must not delete bytes swapped into the validated pathname",
    );
    assert_eq!(
        std::fs::read(saved_original.join("original-marker")).expect("original survives aside"),
        b"validated bytes",
    );
}

/// Staged cleanup uses the exact same intent-backed quarantine as explicit
/// orphan deletion. If the process dies after the rename, ordinary Core
/// startup must recognize and finish it without a staged-cleanup special case.
///
/// Mutation oracle: removing the shared intent before this crash point makes
/// the durable-intent assertion fail, and startup can no longer prove it owns
/// the stranded quarantine.
#[tokio::test]
async fn startup_finishes_an_interrupted_staged_cleanup_quarantine() {
    let env = TestEnv::new();
    let core = env.core().await;
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game Library root");
    let missing_source = env._tmp.path().join("missing-crashing-adopt-source");

    let mut cleaning = probe(&env)
        .crashing_at(gmm_lib::core::crash_points::STAGED_CLEANUP_AFTER_QUARANTINE_MOVE)
        .op([
            "adopt",
            "--from",
            &missing_source.display().to_string(),
            "--name",
            "Crash During Cleanup",
        ])
        .spawn();
    cleaning.wait_for_crash();

    let quarantines: Vec<_> = std::fs::read_dir(&root)
        .expect("Library root after cleanup crash")
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
        "the crash must leave one resumable staged-cleanup quarantine",
    );
    let quarantine_name = quarantines[0].file_name();
    let intent = root.join(format!("{}.intent", quarantine_name.to_string_lossy()));
    assert!(
        intent.exists(),
        "staged cleanup quarantine must retain the shared durable ownership intent",
    );

    probe(&env)
        .op(["migrate"])
        .run()
        .expect_ok("startup finishing interrupted staged cleanup");
    assert!(
        std::fs::read_dir(&root)
            .expect("Library root after restart")
            .all(|entry| {
                !entry
                    .expect("Library entry")
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".gmm-delete-")
            }),
        "the ordinary startup delete-quarantine recovery must finish staged cleanup",
    );
}

/// ZIP import has the same filesystem-first shape as adopt. Extraction may be
/// unbounded, so the fence is reacquired only for identity revalidation and
/// commit; a relocation winner makes import fail without a stale row. Cleanup
/// may preserve an identity-mismatched copy as an auditable orphan.
#[tokio::test]
async fn import_zip_revalidates_the_library_root_after_extract() {
    let env = TestEnv::new();
    let core = env.core().await;
    let new_root = env._tmp.path().join("relocated-gimi");
    let archive = env._tmp.path().join("race.zip");
    write_single_file_mod_zip(&archive);

    let mut importing = probe(&env)
        .pausing_at(gmm_lib::core::crash_points::IMPORT_ZIP_AFTER_EXTRACT)
        .op([
            "import-zip",
            "--zip",
            &archive.display().to_string(),
            "--name",
            "Import Root Race",
        ])
        .spawn();
    importing.wait_for_pause(gmm_lib::core::crash_points::IMPORT_ZIP_AFTER_EXTRACT);
    probe(&env)
        .op([
            "set-library-path",
            "--path",
            &new_root.display().to_string(),
        ])
        .run()
        .expect_ok("relocation while import is between extract and commit");
    importing.resume();
    let imported = importing.wait_for_outcome();
    assert!(
        !imported.ok,
        "import_zip must fail closed when relocation changes its resolved root; \
         it committed a stale row: {imported:?}",
    );
    assert!(
        core.list_mods(GameCode::Gimi)
            .await
            .expect("list")
            .is_empty(),
        "a refused ZIP import must not commit a Mod row",
    );
}

/// Relocation must finish restoring every previously-enabled Mod before it
/// releases the writer fence that excludes Game Session claims. A session
/// claimed immediately after commit must observe enabled rows with their
/// junctions already restored, never persistently disable them.
#[tokio::test]
async fn session_claim_after_relocation_commit_cannot_disable_a_relocated_mod() {
    let env = TestEnv::new();
    let core = env.core().await;
    core.set_game_install_path(
        GameCode::Gimi,
        env.game_mods.parent().expect("game install path"),
    )
    .await
    .expect("set install path");
    let seeded = env.seed_mod(&core, "Relocation Session Race").await;
    core.set_enabled(&seeded.id, true, &env.game_mods)
        .await
        .expect("enable seeded Mod");

    let new_root = env._tmp.path().join("relocated-gimi");
    let mut relocating = probe(&env)
        .pausing_at(gmm_lib::core::crash_points::RELOCATE_AFTER_FENCE_COMMIT)
        .op([
            "set-library-path",
            "--path",
            &new_root.display().to_string(),
        ])
        .spawn();
    relocating.wait_for_pause(gmm_lib::core::crash_points::RELOCATE_AFTER_FENCE_COMMIT);

    probe(&env)
        .op(["start-session", "--pid", &std::process::id().to_string()])
        .run()
        .expect_ok("session claim immediately after relocation commit");
    relocating.resume();
    let relocated = relocating.wait_for_outcome();

    let mods = core.list_mods(GameCode::Gimi).await.expect("list");
    assert_eq!(mods.len(), 1, "the seeded Mod must remain present");
    assert!(
        mods[0].enabled,
        "a session claimed after relocation commit must not leave the previously-enabled Mod persisted disabled",
    );
    relocated.expect_ok("relocation whose post-commit session claim is safe");
    assert!(
        env.game_mods
            .join("Relocation Session Race")
            .join("merged.ini")
            .is_file(),
        "the relocated Mod's junction must already be restored before the session claim",
    );
    core.end_session().await.expect("end session");
}

/// Junctions are reconstructible projections of committed Mod rows. If one
/// restore fails after an earlier Mod was restored, relocation must therefore
/// commit the moved paths and report the partial restore instead of rolling
/// the transaction back around an already-completed filesystem move.
///
/// The repeated crash-point seam fires after the first successful Junction
/// restore. The hook places an ordinary directory at the other Mod's link
/// path, deterministically forcing that later `junction::create` to fail.
///
/// Mutation oracle: propagating the per-Mod restore error (`?`) out of
/// relocation rolls back the rewritten rows after the bytes moved. The
/// `row must name its relocated bytes` assertion then fails on the old path.
#[tokio::test]
async fn partial_junction_restore_keeps_relocated_rows_and_bytes_in_agreement() {
    let env = TestEnv::new();
    let core = env.core().await;
    core.set_game_install_path(
        GameCode::Gimi,
        env.game_mods.parent().expect("game install path"),
    )
    .await
    .expect("set install path");
    let first = env.seed_mod(&core, "Relocation Restore First").await;
    let second = env.seed_mod(&core, "Relocation Restore Second").await;
    core.set_enabled(&first.id, true, &env.game_mods)
        .await
        .expect("enable first Mod");
    core.set_enabled(&second.id, true, &env.game_mods)
        .await
        .expect("enable second Mod");

    let first_link = env.game_mods.join("Relocation Restore First");
    let second_link = env.game_mods.join("Relocation Restore Second");
    let injected = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hook_injected = std::sync::Arc::clone(&injected);
    let hook_first_link = first_link.clone();
    let hook_second_link = second_link.clone();
    let relocating = core
        .clone()
        .with_crash_hook(std::sync::Arc::new(move |point| {
            if point == gmm_lib::core::crash_points::RELOCATE_AFTER_JUNCTION_RESTORE
                && !hook_injected.swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                let blocked = if hook_first_link.join("merged.ini").is_file() {
                    &hook_second_link
                } else {
                    &hook_first_link
                };
                std::fs::create_dir(blocked).expect("block the later Junction restore");
            }
        }));

    let new_root = env._tmp.path().join("relocated-partial-restore");
    let result = relocating
        .set_library_path_for_game(GameCode::Gimi, Some(&new_root))
        .await;

    assert!(
        injected.load(std::sync::atomic::Ordering::SeqCst),
        "the test must reach the post-first-restore crash-point seam",
    );
    let mods = core
        .list_mods(GameCode::Gimi)
        .await
        .expect("list after move");
    assert_eq!(mods.len(), 2, "both seeded Mods remain recorded");
    for seeded in [&first, &second] {
        let row = mods
            .iter()
            .find(|candidate| candidate.id == seeded.id)
            .expect("seeded Mod row after relocation");
        assert_eq!(
            row.library_path,
            new_root.join(&seeded.id),
            "row must name its relocated bytes after a partial Junction restore",
        );
        assert!(
            row.library_path.join("merged.ini").is_file(),
            "the filesystem must contain the bytes at the path committed in the row",
        );
    }

    let report = result.expect("Junction restore failure is a reportable partial outcome");
    assert_eq!(
        report.failed_junction_restores.len(),
        1,
        "exactly the obstructed Junction restore must be reported",
    );
    assert!(
        first_link.join("merged.ini").is_file() ^ second_link.join("merged.ini").is_file(),
        "one Junction restored before the injected failure and one remains for Rebuild Junctions",
    );
}

/// Startup cleanup must not classify an intent as stranded while the delete
/// that owns it is paused immediately before its quarantine rename.
///
/// The delete's `BEGIN IMMEDIATE` remains held while a second process performs
/// a real `Core::new`. SQLite's bounded busy timeout makes that startup attempt
/// return without a timing guess: the guarded cleaner cannot enter its purge,
/// so it leaves the intent intact. The delete is then released and killed at
/// the next durable crash point; restart must still recognize and purge the
/// owned quarantine.
///
/// Mutation oracle: removing or weakening the cleaner's transaction lets the
/// concurrent startup remove the intent, and the assertion before release goes
/// red with an in-flight intent missing.
#[tokio::test]
async fn startup_cleanup_cannot_strand_a_delete_paused_before_quarantine_rename() {
    let env = TestEnv::new();
    let core = env.core().await;
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game Library root");
    let orphan = root.join(Ulid::new().to_string());
    std::fs::create_dir_all(orphan.join("nested")).expect("orphan tree");
    std::fs::write(orphan.join("nested/precious.buf"), b"delete me").expect("orphan bytes");
    let orphan_s = orphan.display().to_string();

    let mut deleting = probe(&env)
        .pausing_at(gmm_lib::core::crash_points::DELETE_AFTER_INTENT_WRITE)
        .crashing_at(gmm_lib::core::crash_points::DELETE_AFTER_QUARANTINE_MOVE)
        .op(["delete-library-dir", "--path", &orphan_s])
        .spawn();
    deleting.wait_for_pause(gmm_lib::core::crash_points::DELETE_AFTER_INTENT_WRITE);

    // This is a second process doing the real startup path, not a direct call
    // to the purge helper. It waits for SQLite's configured busy timeout and
    // then starts successfully after logging that cleanup could not take the
    // writer claim held by `deleting`.
    probe(&env)
        .op(["migrate"])
        .run()
        .expect_ok("startup overlapping an in-flight Library delete");

    let intents: Vec<_> = std::fs::read_dir(&root)
        .expect("Library root while delete is paused")
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".intent"))
        .collect();
    assert_eq!(
        intents.len(),
        1,
        "startup cleanup must not remove the in-flight delete intent before its quarantine rename",
    );

    deleting.resume();
    deleting.wait_for_crash();
    assert!(
        !orphan.exists(),
        "the delete reached its post-quarantine crash point",
    );

    let quarantines: Vec<_> = std::fs::read_dir(&root)
        .expect("Library root after delete crash")
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
        "the crash must leave one resumable quarantine",
    );
    let quarantine_name = quarantines[0].file_name();
    let intent = root.join(format!("{}.intent", quarantine_name.to_string_lossy()));
    assert!(
        intent.exists(),
        "the crashed quarantine must retain its durable ownership intent",
    );

    probe(&env)
        .op(["migrate"])
        .run()
        .expect_ok("restart finishing the interrupted Library delete");
    assert!(
        std::fs::read_dir(&root)
            .expect("Library root after recovery startup")
            .all(|entry| !entry
                .expect("Library entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".gmm-delete-")),
        "restart must purge the owned quarantine and its intent",
    );
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
