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

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use gmm_lib::core::{crash_points, Core, Error, GameCode, SessionInfo};
use sqlx::Connection;
use tempfile::TempDir;
use ulid::Ulid;

// ---------------------------------------------------------------------
// Probe process harness
// ---------------------------------------------------------------------

/// Long enough for a cold Windows runner to start the probe and open SQLite,
/// but short enough to report the missing crash point instead of burning the
/// CI job's timeout.
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Cleanup should be nearly immediate after a forceful process termination.
/// Keep it separate from the operation deadline so a pathological child or
/// pipe reader cannot consume another full probe timeout while reporting the
/// original failure.
const PROBE_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_POLL_INTERVAL: Duration = Duration::from_millis(10);

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
    ready_before_op: bool,
    at: Option<u128>,
    pause_at: Option<&'static str>,
    crash_at: Option<&'static str>,
    timeout: Duration,
    cleanup_timeout: Duration,
    stdout_reader_delay: Duration,
    stderr_reader_delay: Duration,
    force_reap_timeout: bool,
    args: Vec<String>,
}

fn probe(env: &TestEnv) -> Probe {
    Probe {
        data_dir: env.data_dir.clone(),
        db_url: env.db_url.clone(),
        library: env.library.clone(),
        take_lock: false,
        ready_before_op: false,
        at: None,
        pause_at: None,
        crash_at: None,
        timeout: PROBE_TIMEOUT,
        cleanup_timeout: PROBE_CLEANUP_TIMEOUT,
        stdout_reader_delay: Duration::ZERO,
        stderr_reader_delay: Duration::ZERO,
        force_reap_timeout: false,
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

    /// Initialize the real Core and stop immediately before invoking the
    /// selected public operation. This keeps startup recovery outside the
    /// interleaving while the vulnerable boundaries remain production crash
    /// points inside the two operations under test.
    fn ready_before_operation(mut self) -> Self {
        self.ready_before_op = true;
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

    fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    fn with_cleanup_timeout(mut self, timeout: Duration) -> Self {
        self.cleanup_timeout = timeout;
        self
    }

    fn with_stdout_reader_delay(mut self, delay: Duration) -> Self {
        self.stdout_reader_delay = delay;
        self
    }

    fn with_stderr_reader_delay(mut self, delay: Duration) -> Self {
        self.stderr_reader_delay = delay;
        self
    }

    fn forcing_reap_timeout(mut self) -> Self {
        self.force_reap_timeout = true;
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
        if self.ready_before_op {
            cmd.arg("--ready-before-op");
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
        let mut running = self.spawn();
        // `Command::output` closes the piped stdin before waiting. Preserve
        // that behaviour so a mistakenly paused `run` probe reaches its next
        // step instead of being held open by the harness itself.
        running.stdin.take();
        let expected_crash_point = running.expected_crash_point();
        let status = running.wait_for_exit("finish", expected_crash_point);
        let lines = running.finish_stdout().unwrap_or_else(|error| {
            running.fail_after_kill(
                format!("failed while waiting for probe stdout reader to finish: {error}"),
                expected_crash_point,
            )
        });
        let line = lines.last().unwrap_or_else(|| {
            let stderr = running.finish_stderr();
            panic!("probe printed nothing on stdout; exit status {status}; stderr was:\n{stderr}")
        });
        ProbeOutcome::parse(line.trim())
    }

    /// Spawn without waiting. Used for the probe that holds a lock open
    /// while the test does something else.
    fn spawn(self) -> RunningProbe {
        let mut child = self.command().spawn().expect("spawn probe");
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let stdout_reader_delay = self.stdout_reader_delay;
        let stderr_reader_delay = self.stderr_reader_delay;

        // Pipe reads are blocking on both Unix and Windows. Dedicated readers
        // let the harness enforce its own deadline while continuously draining
        // both streams so a noisy child cannot block on a full pipe.
        let (stdout_tx, stdout_rx) = mpsc::channel();
        let stdout_reader = std::thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match stdout.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if stdout_tx.send(Ok(line)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = stdout_tx.send(Err(error));
                        break;
                    }
                }
            }
            std::thread::sleep(stdout_reader_delay);
        });
        let stderr_reader = std::thread::spawn(move || {
            let mut stderr = stderr;
            let mut bytes = Vec::new();
            let result = stderr.read_to_end(&mut bytes).map(|_| bytes);
            std::thread::sleep(stderr_reader_delay);
            result
        });

        let operation = self
            .args
            .first()
            .cloned()
            .unwrap_or_else(|| "<missing operation>".to_string());
        RunningProbe {
            child,
            stdin: Some(stdin),
            stdout: stdout_rx,
            stdout_reader: Some(stdout_reader),
            stderr_reader: Some(stderr_reader),
            operation,
            pause_at: self.pause_at,
            crash_at: self.crash_at,
            timeout: self.timeout,
            cleanup_timeout: self.cleanup_timeout,
            force_reap_timeout: self.force_reap_timeout,
            cleanup_attempted: false,
            cleanup_deadline: None,
        }
    }
}

struct RunningProbe {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Receiver<std::io::Result<String>>,
    stdout_reader: Option<JoinHandle<()>>,
    stderr_reader: Option<JoinHandle<std::io::Result<Vec<u8>>>>,
    operation: String,
    pause_at: Option<&'static str>,
    crash_at: Option<&'static str>,
    timeout: Duration,
    cleanup_timeout: Duration,
    force_reap_timeout: bool,
    cleanup_attempted: bool,
    cleanup_deadline: Option<Instant>,
}

impl RunningProbe {
    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn expected_crash_point(&self) -> Option<&'static str> {
        self.crash_at.or(self.pause_at)
    }

    fn recv_stdout_line(&mut self, action: &str, expected_crash_point: Option<&str>) -> String {
        match self.stdout.recv_timeout(self.timeout) {
            Ok(Ok(line)) => line,
            Ok(Err(error)) => self.fail_after_kill(
                format!("failed to read probe stdout while waiting to {action}: {error}"),
                expected_crash_point,
            ),
            Err(RecvTimeoutError::Timeout) => self.fail_after_kill(
                format!(
                    "timed out after {:?} waiting for probe to {action}",
                    self.timeout
                ),
                expected_crash_point,
            ),
            Err(RecvTimeoutError::Disconnected) => self.fail_after_kill(
                format!("probe closed stdout before it could {action}"),
                expected_crash_point,
            ),
        }
    }

    fn wait_for_exit(&mut self, action: &str, expected_crash_point: Option<&str>) -> ExitStatus {
        let deadline = Instant::now() + self.timeout;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(PROBE_POLL_INTERVAL);
                }
                Ok(None) => self.fail_after_kill(
                    format!(
                        "timed out after {:?} waiting for probe to {action}",
                        self.timeout
                    ),
                    expected_crash_point,
                ),
                Err(error) => self.fail_after_kill(
                    format!("failed while waiting for probe to {action}: {error}"),
                    expected_crash_point,
                ),
            }
        }
    }

    fn fail_after_kill(&mut self, message: String, expected_crash_point: Option<&str>) -> ! {
        let cleanup_deadline = self.begin_cleanup();
        self.fail_after_kill_until(message, expected_crash_point, cleanup_deadline)
    }

    fn fail_after_kill_until(
        &mut self,
        message: String,
        expected_crash_point: Option<&str>,
        cleanup_deadline: Instant,
    ) -> ! {
        let status = self
            .kill_and_reap_until(cleanup_deadline)
            .map(|status| status.to_string())
            .unwrap_or_else(|error| format!("<{error}>"));
        let stderr = self.finish_stderr_until(cleanup_deadline);
        let stdout_cleanup = self
            .finish_stdout_until(cleanup_deadline)
            .err()
            .map(|error| format!("; stdout cleanup: {error}"))
            .unwrap_or_default();
        let expected_crash_point = expected_crash_point
            .map(|point| format!(" at crash point {point}"))
            .unwrap_or_default();
        panic!(
            "{message}{expected_crash_point} (operation {:?}); child cleanup: {status}; \
             stderr:\n{stderr}{stdout_cleanup}",
            self.operation,
        );
    }

    fn finish_stdout(&mut self) -> Result<Vec<String>, String> {
        let cleanup_deadline = self.begin_cleanup();
        self.finish_stdout_until(cleanup_deadline)
    }

    fn finish_stderr(&mut self) -> String {
        let cleanup_deadline = self.begin_cleanup();
        self.finish_stderr_until(cleanup_deadline)
    }

    fn begin_cleanup(&mut self) -> Instant {
        if let Some(deadline) = self.cleanup_deadline {
            return deadline;
        }
        let deadline = Instant::now() + self.cleanup_timeout;
        self.cleanup_deadline = Some(deadline);
        deadline
    }

    fn finish_stdout_until(&mut self, deadline: Instant) -> Result<Vec<String>, String> {
        if let Some(reader) = self.stdout_reader.take() {
            finish_reader(reader, deadline, "probe stdout reader")?;
        }
        self.stdout
            .try_iter()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("failed to read probe stdout: {error}"))
    }

    fn finish_stderr_until(&mut self, deadline: Instant) -> String {
        let Some(reader) = self.stderr_reader.take() else {
            return String::new();
        };
        match finish_reader(reader, deadline, "probe stderr reader") {
            Ok(Ok(bytes)) => String::from_utf8_lossy(&bytes).into_owned(),
            Ok(Err(error)) => format!("<failed to read probe stderr: {error}>"),
            Err(error) => format!("<{error}>"),
        }
    }

    fn kill_and_reap(&mut self) -> Result<ExitStatus, String> {
        let cleanup_deadline = self.begin_cleanup();
        self.kill_and_reap_until(cleanup_deadline)
    }

    fn kill_and_reap_until(&mut self, deadline: Instant) -> Result<ExitStatus, String> {
        self.stdin.take();
        self.cleanup_attempted = true;
        match self.child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => {
                let kill = self
                    .child
                    .kill()
                    .map(|()| "kill requested".to_string())
                    .unwrap_or_else(|kill_error| format!("kill failed: {kill_error}"));
                return Err(format!(
                    "failed to inspect probe before kill: {error}; {kill}"
                ));
            }
        }

        if let Err(error) = self.child.kill() {
            return Err(format!("failed to kill timed-out probe: {error}"));
        }
        loop {
            let status = if self.force_reap_timeout {
                Ok(None)
            } else {
                self.child.try_wait()
            };
            match status {
                Ok(Some(status)) => return Ok(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(
                        PROBE_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
                Ok(None) => {
                    return Err(format!(
                        "probe did not exit within {:?} after kill; left for OS cleanup",
                        self.cleanup_timeout
                    ));
                }
                Err(error) => return Err(format!("failed to reap killed probe: {error}")),
            }
        }
    }

    /// Block until the probe reports its operation's result. Removes the
    /// need for a sleep before the test's own half of the race.
    fn wait_for_outcome(&mut self) -> ProbeOutcome {
        let line = self.recv_stdout_line("report its outcome", None);
        assert!(
            !line.trim().is_empty(),
            "probe closed stdout without reporting"
        );
        ProbeOutcome::parse(line.trim())
    }

    fn wait_for_pause(&mut self, expected_crash_point: &str) {
        let line = self.recv_stdout_line("pause", Some(expected_crash_point));
        assert!(
            !line.trim().is_empty(),
            "probe closed stdout before pausing at {expected_crash_point}"
        );
        let event: serde_json::Value = serde_json::from_str(line.trim())
            .unwrap_or_else(|error| panic!("probe printed non-JSON pause {line:?}: {error}"));
        assert_eq!(
            event["pausedAt"].as_str(),
            Some(expected_crash_point),
            "probe paused at the wrong crash point: {event}",
        );
    }

    fn wait_until_ready_before_operation(&mut self) {
        let line = self.recv_stdout_line("become ready before its operation", None);
        let event: serde_json::Value = serde_json::from_str(line.trim())
            .unwrap_or_else(|error| panic!("probe printed non-JSON readiness {line:?}: {error}"));
        assert_eq!(
            event["readyBeforeOp"].as_bool(),
            Some(true),
            "probe reported the wrong pre-operation event: {event}",
        );
    }

    fn resume(&mut self) {
        let stdin = self.stdin.as_mut().expect("probe stdin still open");
        writeln!(stdin, "resume").expect("release paused probe");
        stdin.flush().expect("flush probe release");
    }

    fn wait_for_crash(&mut self) {
        self.stdin.take();
        let expected_crash_point = self.expected_crash_point();
        let status = self.wait_for_exit("crash", expected_crash_point);
        if status.success() {
            let stderr = self.finish_stderr();
            panic!(
                "probe completed instead of crashing at crash point {}; \
                 child exit status: {status}; stderr:\n{stderr}",
                expected_crash_point.unwrap_or("<unspecified>"),
            );
        }
    }

    /// Kill without letting the process unwind — the point is to prove
    /// the *kernel* releases the lock, not that a Drop impl does.
    fn kill(&mut self) {
        if let Err(error) = self.kill_and_reap() {
            panic!(
                "failed to kill and reap probe for operation {:?}: {error}",
                self.operation
            );
        }
    }
}

fn finish_reader<T>(
    reader: JoinHandle<T>,
    deadline: Instant,
    reader_name: &str,
) -> Result<T, String> {
    while !reader.is_finished() {
        if Instant::now() >= deadline {
            return Err(format!(
                "{reader_name} did not finish before the cleanup deadline"
            ));
        }
        std::thread::sleep(
            PROBE_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    reader.join().map_err(|_| format!("{reader_name} panicked"))
}

impl Drop for RunningProbe {
    fn drop(&mut self) {
        let cleanup_deadline = self.begin_cleanup();
        if !self.cleanup_attempted && self.child.try_wait().ok().flatten().is_none() {
            let _ = self.kill_and_reap_until(cleanup_deadline);
        } else {
            let _ = self.child.try_wait();
        }
        let _ = self.finish_stdout_until(cleanup_deadline);
        let _ = self.finish_stderr_until(cleanup_deadline);
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

fn write_reinstall_zip(path: &Path, body: &[u8]) {
    let file = std::fs::File::create(path).expect("create reinstall ZIP");
    let mut archive = zip::ZipWriter::new(file);
    archive
        .start_file("merged.ini", zip::write::SimpleFileOptions::default())
        .expect("start reinstall ZIP entry");
    archive.write_all(body).expect("write reinstall ZIP body");
    archive.finish().expect("finish reinstall ZIP");
}

async fn obstruct_reinstall_recovery(
    env: &TestEnv,
    imported: &gmm_lib::core::Mod,
) -> (Ulid, PathBuf, PathBuf, PathBuf) {
    obstruct_reinstall_recovery_with_token(env, imported, Ulid::new()).await
}

async fn obstruct_reinstall_recovery_with_token(
    env: &TestEnv,
    imported: &gmm_lib::core::Mod,
    token: Ulid,
) -> (Ulid, PathBuf, PathBuf, PathBuf) {
    let root = imported.library_path.parent().expect("game Library root");
    let stage = root.join(format!(".gmm-reinstall-{token}"));
    let held_stage = root.join(format!(".held-reinstall-{token}"));
    let quarantine = root.join(format!(".gmm-delete-{token}"));
    std::fs::create_dir(&stage).expect("reinstall stage");
    std::fs::write(stage.join("replacement.ini"), b"witnessed replacement")
        .expect("replacement bytes");
    let old_identity = durable_directory_key(&imported.library_path);
    let staged_identity = durable_directory_key(&stage);

    let pool = sqlx::SqlitePool::connect(&env.db_url)
        .await
        .expect("open DB for recovery witness");
    sqlx::query(
        "INSERT INTO reinstall_swaps (
            token, mod_id, game_code, library_path, staged_path,
            quarantine_path, old_identity, staged_identity, created_at
         ) VALUES (?, ?, 'gimi', ?, ?, ?, ?, ?, ?)",
    )
    .bind(token.to_string())
    .bind(&imported.id)
    .bind(imported.library_path.to_string_lossy().as_ref())
    .bind(stage.to_string_lossy().as_ref())
    .bind(quarantine.to_string_lossy().as_ref())
    .bind(old_identity)
    .bind(staged_identity)
    .bind("2026-08-23T00:00:00Z")
    .execute(&pool)
    .await
    .expect("insert reinstall witness");
    pool.close().await;

    std::fs::rename(&stage, &held_stage).expect("hold witnessed stage aside");
    std::fs::create_dir(&stage).expect("substitute reserved stage name");
    std::fs::write(stage.join("unknown.ini"), b"unproved stage bytes")
        .expect("unproved stage bytes");
    (token, stage, held_stage, quarantine)
}

#[cfg(unix)]
fn durable_directory_key(path: &Path) -> String {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = std::fs::metadata(path).expect("directory metadata for recovery witness");
    format!("{:016x}:{:016x}", metadata.dev(), metadata.ino())
}

#[cfg(windows)]
fn durable_directory_key(path: &Path) -> String {
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
        .expect("open directory for recovery witness identity");
    let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    let ok = unsafe { GetFileInformationByHandle(directory.as_raw_handle(), info.as_mut_ptr()) };
    assert_ne!(
        ok,
        0,
        "read directory identity for recovery witness: {}",
        std::io::Error::last_os_error(),
    );
    let info = unsafe { info.assume_init() };
    let file = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    format!("{:016x}:{:016x}", info.dwVolumeSerialNumber, file)
}

async fn seed_enabled_gamebanana_mod(
    env: &TestEnv,
    core: &Core,
    gamebanana_id: u64,
) -> (gmm_lib::core::Mod, String) {
    core.set_game_install_path(
        GameCode::Gimi,
        env.game_mods.parent().expect("game install path"),
    )
    .await
    .expect("record game install path");
    let zip_path = env._tmp.path().join(format!("{gamebanana_id}-old.zip"));
    let file = std::fs::File::create(&zip_path).expect("create old Variant ZIP");
    let mut archive = zip::ZipWriter::new(file);
    for (path, body) in [
        (
            "Blue/merged.ini",
            b"[TextureOverrideBlue]\nhash=old-blue\n".as_slice(),
        ),
        (
            "Red/merged.ini",
            b"[TextureOverrideRed]\nhash=old-red\n".as_slice(),
        ),
    ] {
        archive
            .start_file(path, zip::write::SimpleFileOptions::default())
            .expect("start old Variant entry");
        archive.write_all(body).expect("write old Variant entry");
    }
    archive.finish().expect("finish old Variant ZIP");
    let zip_bytes = std::fs::read(&zip_path).expect("old ZIP bytes");
    let mut server = mockito::Server::new_async().await;
    let api_path = format!("/apiv11/Mod/{gamebanana_id}");
    let file_path = format!("/file/{gamebanana_id}/old.zip");
    let _api = server
        .mock("GET", mockito::Matcher::Regex(format!("{api_path}.*")))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
                "_idRow": {gamebanana_id},
                "_sName": "Crash Safe Mod",
                "_sProfileUrl": "https://gamebanana.com/mods/{gamebanana_id}",
                "_sVersion": "1.0.0",
                "_aSubmitter": {{ "_sName": "Author" }},
                "_aPreviewMedia": {{ "_aImages": [] }},
                "_aFiles": [{{ "_sFile": "old.zip", "_sDownloadUrl": "{base}{file_path}" }}]
            }}"#,
            base = server.url(),
        ))
        .create_async()
        .await;
    let _file = server
        .mock("GET", file_path.as_str())
        .with_status(200)
        .with_body(zip_bytes)
        .create_async()
        .await;
    let imported = core
        .import_gamebanana_with_endpoints(
            GameCode::Gimi,
            &format!("https://gamebanana.com/mods/{gamebanana_id}"),
            &gmm_lib::core::gamebanana::Endpoints {
                api_base: server.url(),
            },
        )
        .await
        .expect("seed GameBanana Mod");
    let variants = core
        .list_variants(&imported.id)
        .await
        .expect("seeded Variants");
    let red = variants
        .iter()
        .find(|variant| variant.name == "Red")
        .expect("Red Variant");
    core.set_active_variant(&imported.id, &red.id, &env.game_mods)
        .await
        .expect("select Red Variant before reinstall");
    core.set_enabled(&imported.id, true, &env.game_mods)
        .await
        .expect("enable seeded GameBanana Mod");
    (imported, red.id.clone())
}

async fn assert_reinstall_rolled_back(
    env: &TestEnv,
    imported: &gmm_lib::core::Mod,
    old_active_variant_id: &str,
    context: &str,
) {
    let recovered = env.core().await;
    let original = std::fs::read(imported.library_path.join("Red/merged.ini"))
        .expect("old Library bytes after startup recovery");
    assert_eq!(
        original, b"[TextureOverrideRed]\nhash=old-red\n",
        "{context}: startup must restore the complete old Library tree",
    );
    let listed = recovered
        .list_mods(GameCode::Gimi)
        .await
        .expect("list recovered Mod");
    let found = listed
        .iter()
        .find(|candidate| candidate.id == imported.id)
        .expect("recovered Mod row");
    assert!(found.enabled, "{context}: enabled state must roll back");
    assert_eq!(found.name, "Crash Safe Mod", "{context}: metadata rollback");
    assert_eq!(found.version.as_deref(), Some("1.0.0"));
    assert_eq!(
        recovered
            .active_variant_id(&imported.id)
            .await
            .expect("active Variant after recovery")
            .as_deref(),
        Some(old_active_variant_id),
        "{context}: active Variant must roll back exactly",
    );
    assert_eq!(
        std::fs::read(env.game_mods.join("Crash Safe Mod/merged.ini"))
            .expect("working Junction after startup recovery"),
        original,
        "{context}: Junction must again resolve to the old tree",
    );
    let root = imported.library_path.parent().expect("game Library root");
    assert!(
        std::fs::read_dir(root)
            .expect("Library root after reinstall recovery")
            .all(|entry| {
                let name = entry.expect("Library entry").file_name();
                let name = name.to_string_lossy();
                !name.starts_with(".gmm-reinstall-") && !name.starts_with(".gmm-delete-")
            }),
        "{context}: recovery must leave neither staging nor quarantine state",
    );
    let pool = sqlx::SqlitePool::connect(&env.db_url)
        .await
        .expect("open DB to inspect reinstall witness");
    let swaps: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reinstall_swaps")
        .fetch_one(&pool)
        .await
        .expect("count reinstall witnesses");
    assert_eq!(swaps, 0, "{context}: rollback must retire its witness");
}

async fn assert_reinstall_committed(env: &TestEnv, imported: &gmm_lib::core::Mod, context: &str) {
    let recovered = env.core().await;
    let installed = std::fs::read(imported.library_path.join("merged.ini"))
        .expect("new Library bytes after committed reinstall restart");
    assert_eq!(
        installed, b"[TextureOverrideNew]\nhash=new\n",
        "{context}: committed replacement must remain live",
    );
    let listed = recovered
        .list_mods(GameCode::Gimi)
        .await
        .expect("list committed Mod");
    let found = listed
        .iter()
        .find(|candidate| candidate.id == imported.id)
        .expect("committed Mod row");
    assert!(found.enabled, "{context}: enabled state remains true");
    assert_eq!(found.name, "Crash Safe Mod v2");
    assert_eq!(found.version.as_deref(), Some("2.0.0"));
    assert_eq!(
        recovered
            .active_variant_id(&imported.id)
            .await
            .expect("active Variant after committed reinstall"),
        None,
        "{context}: committed single-root replacement clears the old active Variant",
    );
    assert_eq!(
        std::fs::read(env.game_mods.join("Crash Safe Mod/merged.ini"))
            .expect("working Junction after committed reinstall restart"),
        installed,
        "{context}: Junction must resolve to the committed replacement",
    );
    let root = imported.library_path.parent().expect("game Library root");
    assert!(
        std::fs::read_dir(root)
            .expect("Library root after committed reinstall recovery")
            .all(|entry| {
                let name = entry.expect("Library entry").file_name();
                let name = name.to_string_lossy();
                !name.starts_with(".gmm-reinstall-") && !name.starts_with(".gmm-delete-")
            }),
        "{context}: startup must reclaim the committed swap's old quarantine",
    );
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

fn catch_probe_failure(action: impl FnOnce()) -> (Duration, String) {
    let started = Instant::now();
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(action))
        .expect_err("the deliberately stalled probe must hit its deadline");
    let elapsed = started.elapsed();
    let message = failure
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| failure.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .unwrap_or_else(|| "<non-string panic>".to_string());
    (elapsed, message)
}

/// A missing crash point must identify itself quickly instead of consuming the
/// surrounding CI job timeout. `migrate` never reaches the delete crash point;
/// the far-future `--at` keeps the child alive so this exercises the harness's
/// deadline rather than an ordinary early exit.
#[test]
fn missing_pause_point_fails_fast_with_expected_crash_point() {
    let env = TestEnv::new();
    let expected_crash_point = gmm_lib::core::crash_points::DELETE_AFTER_INTENT_WRITE;
    let mut running = probe(&env)
        .at(rendezvous_in(Duration::from_secs(30)))
        .pausing_at(expected_crash_point)
        .with_timeout(Duration::from_millis(250))
        .op(["migrate"])
        .spawn();

    let (elapsed, message) = catch_probe_failure(|| {
        running.wait_for_pause(expected_crash_point);
    });

    assert!(
        elapsed < Duration::from_secs(5),
        "probe deadline did not fail fast: elapsed {elapsed:?}; failure: {message}",
    );
    assert!(
        message.contains(expected_crash_point),
        "probe deadline failure did not name expected crash point \
         {expected_crash_point:?}: {message}",
    );
}

#[test]
fn missing_outcome_fails_fast_with_expected_operation() {
    let env = TestEnv::new();
    let mut running = probe(&env)
        .at(rendezvous_in(Duration::from_secs(30)))
        .with_timeout(Duration::from_millis(250))
        .op(["migrate"])
        .spawn();

    let (elapsed, message) = catch_probe_failure(|| {
        running.wait_for_outcome();
    });

    assert!(
        elapsed < Duration::from_secs(2),
        "outcome deadline did not fail fast: elapsed {elapsed:?}; failure: {message}",
    );
    assert!(
        message.contains("waiting for probe to report its outcome")
            && message.contains("operation \"migrate\""),
        "outcome deadline failure did not name the expected operation: {message}",
    );
}

#[test]
fn missing_crash_fails_fast_with_expected_crash_point() {
    let env = TestEnv::new();
    let expected_crash_point = gmm_lib::core::crash_points::DELETE_AFTER_INTENT_WRITE;
    let mut running = probe(&env)
        .at(rendezvous_in(Duration::from_secs(30)))
        .crashing_at(expected_crash_point)
        .with_timeout(Duration::from_millis(250))
        .op(["migrate"])
        .spawn();

    let (elapsed, message) = catch_probe_failure(|| {
        running.wait_for_crash();
    });

    assert!(
        elapsed < Duration::from_secs(2),
        "crash deadline did not fail fast: elapsed {elapsed:?}; failure: {message}",
    );
    assert!(
        message.contains("waiting for probe to crash") && message.contains(expected_crash_point),
        "crash deadline failure did not name expected crash point \
         {expected_crash_point:?}: {message}",
    );
}

#[test]
fn blocking_run_fails_fast_with_expected_operation() {
    let env = TestEnv::new();

    let (elapsed, message) = catch_probe_failure(|| {
        let _ = probe(&env)
            .at(rendezvous_in(Duration::from_secs(30)))
            .with_timeout(Duration::from_millis(250))
            .op(["migrate"])
            .run();
    });

    assert!(
        elapsed < Duration::from_secs(2),
        "blocking run deadline did not fail fast: elapsed {elapsed:?}; failure: {message}",
    );
    assert!(
        message.contains("waiting for probe to finish")
            && message.contains("operation \"migrate\""),
        "blocking run deadline failure did not name the expected operation: {message}",
    );
}

#[test]
fn stalled_stdout_reader_fails_fast_with_expected_operation() {
    let env = TestEnv::new();

    let (elapsed, message) = catch_probe_failure(|| {
        let _ = probe(&env)
            .with_cleanup_timeout(Duration::from_millis(250))
            .with_stdout_reader_delay(Duration::from_secs(3))
            .op(["hold-lock", "--ms", "0"])
            .run();
    });

    assert!(
        elapsed < Duration::from_secs(2),
        "stdout-reader deadline did not fail fast: elapsed {elapsed:?}; failure: {message}",
    );
    assert!(
        message.contains("probe stdout reader did not finish")
            && message.contains("operation \"hold-lock\""),
        "stdout-reader deadline failure did not name the expected operation: {message}",
    );
}

#[test]
fn stalled_stderr_reader_fails_fast_with_expected_crash_point() {
    let env = TestEnv::new();
    let expected_crash_point = gmm_lib::core::crash_points::DELETE_AFTER_INTENT_WRITE;
    let mut running = probe(&env)
        .at(rendezvous_in(Duration::from_secs(30)))
        .pausing_at(expected_crash_point)
        .with_timeout(Duration::from_millis(250))
        .with_cleanup_timeout(Duration::from_millis(250))
        .with_stderr_reader_delay(Duration::from_secs(3))
        .op(["migrate"])
        .spawn();

    let (elapsed, message) = catch_probe_failure(|| {
        running.wait_for_pause(expected_crash_point);
    });

    assert!(
        elapsed < Duration::from_secs(2),
        "stderr-reader deadline did not fail fast: elapsed {elapsed:?}; failure: {message}",
    );
    assert!(
        message.contains("probe stderr reader did not finish")
            && message.contains(expected_crash_point),
        "stderr-reader deadline failure did not name expected crash point \
         {expected_crash_point:?}: {message}",
    );
}

#[test]
fn unreaped_child_fails_fast_with_expected_crash_point() {
    let env = TestEnv::new();
    let expected_crash_point = gmm_lib::core::crash_points::DELETE_AFTER_INTENT_WRITE;
    let mut running = probe(&env)
        .at(rendezvous_in(Duration::from_secs(30)))
        .pausing_at(expected_crash_point)
        .with_timeout(Duration::from_millis(250))
        .with_cleanup_timeout(Duration::from_millis(250))
        .forcing_reap_timeout()
        .op(["migrate"])
        .spawn();

    let (elapsed, message) = catch_probe_failure(|| {
        running.wait_for_pause(expected_crash_point);
    });

    assert!(
        elapsed < Duration::from_secs(2),
        "reap deadline did not fail fast: elapsed {elapsed:?}; failure: {message}",
    );
    assert!(
        message.contains("left for OS cleanup") && message.contains(expected_crash_point),
        "reap deadline failure did not name expected crash point \
         {expected_crash_point:?}: {message}",
    );
}

#[test]
fn drop_bounds_both_reader_joins() {
    let env = TestEnv::new();

    for (reader_name, stdout_delay, stderr_delay) in [
        ("stdout", Duration::from_secs(3), Duration::ZERO),
        ("stderr", Duration::ZERO, Duration::from_secs(3)),
    ] {
        let mut running = probe(&env)
            .with_cleanup_timeout(Duration::from_millis(250))
            .with_stdout_reader_delay(stdout_delay)
            .with_stderr_reader_delay(stderr_delay)
            .op(["hold-lock", "--ms", "0"])
            .spawn();
        running.wait_for_exit("finish", None);

        let started = Instant::now();
        drop(running);
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "Drop waited for stalled {reader_name} reader on operation \"hold-lock\": \
             elapsed {elapsed:?}",
        );
    }
}

#[test]
fn both_stalled_readers_share_one_cleanup_deadline() {
    let env = TestEnv::new();
    let cleanup_timeout = Duration::from_secs(1);
    let mut running = probe(&env)
        .with_cleanup_timeout(cleanup_timeout)
        .with_stdout_reader_delay(Duration::from_secs(5))
        .with_stderr_reader_delay(Duration::from_secs(5))
        .op(["hold-lock", "--ms", "0"])
        .spawn();
    running.wait_for_exit("finish", None);

    let (elapsed, message) = catch_probe_failure(|| {
        let _ = running.finish_stdout().unwrap_or_else(|error| {
            running.fail_after_kill(
                format!("failed while waiting for probe stdout reader to finish: {error}"),
                None,
            )
        });
    });

    assert!(
        elapsed < cleanup_timeout + Duration::from_millis(500),
        "both-reader cleanup exceeded its single shared deadline: elapsed {elapsed:?}; \
         failure: {message}",
    );
    assert!(
        message.contains("probe stdout reader did not finish")
            && message.contains("probe stderr reader did not finish"),
        "both-reader cleanup did not report both stalled readers: {message}",
    );

    // Also cover the successful-stdout path: stdout consumes most of the
    // budget, then `Drop` gets only the remainder for stderr.
    let cleanup_timeout = Duration::from_secs(2);
    let mut running = probe(&env)
        .with_cleanup_timeout(cleanup_timeout)
        .with_stdout_reader_delay(Duration::from_millis(1500))
        .with_stderr_reader_delay(Duration::from_secs(5))
        .op(["hold-lock", "--ms", "0"])
        .spawn();
    running.wait_for_exit("finish", None);

    let started = Instant::now();
    running
        .finish_stdout()
        .expect("stdout reader should finish within the shared cleanup deadline");
    drop(running);
    let elapsed = started.elapsed();
    assert!(
        elapsed < cleanup_timeout + Duration::from_millis(750),
        "Drop renewed the cleanup deadline after stdout used part of it: \
         elapsed {elapsed:?}",
    );
}

#[test]
fn drop_bounds_and_kills_a_live_unreaped_child_during_unwinding() {
    let env = TestEnv::new();
    let mut running = probe(&env)
        .honouring_the_lock()
        .with_cleanup_timeout(Duration::from_millis(250))
        .forcing_reap_timeout()
        // The hold must outlast anything the rest of this test can wait for.
        // At three seconds a contended runner could let the child exit on its
        // own between the unwind and the lock probe below, freeing the lock
        // without Drop ever killing anything — the test would then pass with
        // Drop's kill removed. Thirty seconds is the same hold the rest of the
        // suite uses, and is far longer than the one-second unwind budget plus
        // the fresh probe's startup.
        .op(["hold-lock", "--ms", "30000"])
        .spawn();
    running
        .wait_for_outcome()
        .expect_ok("the live child taking the instance lock before Drop");

    let (finished_tx, finished_rx) = mpsc::channel();
    let started = Instant::now();
    std::thread::spawn(move || {
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _running = running;
            panic!("live-child Drop unwind sentinel");
        }))
        .expect_err("the sentinel panic should unwind through RunningProbe::drop");
        let message = failure
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| failure.downcast_ref::<&str>().map(|s| (*s).to_string()))
            .unwrap_or_else(|| "<non-string panic>".to_string());
        let _ = finished_tx.send((started.elapsed(), message));
    });

    let (elapsed, message) = finished_rx.recv_timeout(Duration::from_secs(2)).expect(
        "live-child Drop did not finish within its bounded cleanup deadline during unwinding",
    );
    assert_eq!(message, "live-child Drop unwind sentinel");
    assert!(
        elapsed < Duration::from_secs(1),
        "live-child Drop exceeded its cleanup deadline during unwinding: elapsed {elapsed:?}",
    );

    probe(&env)
        .honouring_the_lock()
        .op(["hold-lock", "--ms", "0"])
        .run()
        .expect_ok("a fresh process after live-child Drop");
}

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

/// Recovery has finished its bounded ownership step and released the writer
/// fence before recursive Variant detection. Relocation can therefore win
/// while recovery is paused; recovery must then revalidate the effective root
/// and refuse to commit a stale row, leaving the moved orphan actionable.
#[tokio::test]
async fn recovery_releases_the_writer_fence_before_variant_detection() {
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

    probe(&env)
        .op([
            "set-library-path",
            "--path",
            &new_root.display().to_string(),
        ])
        .run()
        .expect_ok("relocation while recovery is outside the writer fence");
    assert!(
        !orphan.exists(),
        "relocation must move the old-root orphan while recovery performs staged detection",
    );

    recovering.resume();
    let recovered = recovering.wait_for_outcome();
    assert!(
        !recovered.ok,
        "recovery must fail closed after relocation changes its effective Library root: \
         {recovered:?}",
    );
    assert!(
        core.list_mods(GameCode::Gimi)
            .await
            .expect("list")
            .is_empty(),
        "the refused recovery must not commit a stale Mod row",
    );
    let moved_orphan = new_root.join(orphan.file_name().expect("orphan name"));
    assert!(
        moved_orphan.join("merged.ini").is_file(),
        "relocation must preserve the refused recovery's orphan bytes",
    );
    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit after staged recovery loses the root race");
    assert!(
        report
            .unreferenced
            .iter()
            .any(|entry| entry.path == moved_orphan),
        "the moved orphan must remain visible for retry: {report:?}",
    );
}

/// Recovery releases its first transaction before Variant detection. A
/// concurrent writer can therefore claim a case-variant of the orphan's ULID
/// while detection is in flight. SQLite's binary primary key accepts that
/// spelling, so recovery must explicitly repeat its case-insensitive
/// availability check after reacquiring the writer fence.
#[tokio::test]
async fn recovery_rechecks_case_insensitive_mod_id_availability_after_variant_detection() {
    let env = TestEnv::new();
    let core = env.core().await;
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game Library root");
    let id = loop {
        let candidate = Ulid::new().to_string();
        if candidate.to_ascii_lowercase() != candidate {
            break candidate;
        }
    };
    let orphan = root.join(&id);
    for variant in ["Blue", "Red"] {
        let dir = orphan.join(variant);
        std::fs::create_dir_all(&dir).expect("Variant directory");
        std::fs::write(dir.join("merged.ini"), format!("hash={variant}\n")).expect("Variant INI");
    }

    let mut recovering = probe(&env)
        .pausing_at(gmm_lib::core::crash_points::RECOVER_AFTER_LIBRARY_MOVE)
        .op([
            "recover",
            "--path",
            &orphan.display().to_string(),
            "--name",
            "Case-Variant Race",
        ])
        .spawn();
    recovering.wait_for_pause(gmm_lib::core::crash_points::RECOVER_AFTER_LIBRARY_MOVE);

    let claimed_id = id.to_ascii_lowercase();
    let mut writer = sqlx::SqliteConnection::connect(&env.db_url)
        .await
        .expect("open competing writer");
    sqlx::query(
        "INSERT INTO mods (
            id, game_code, name, source, library_path,
            junction_dir_name, enabled, created_at
         ) VALUES (?, 'gimi', 'Concurrent Claimant', 'manual', ?, ?, 0, ?)",
    )
    .bind(&claimed_id)
    .bind(
        root.join("missing-case-variant-claimant")
            .to_string_lossy()
            .as_ref(),
    )
    .bind(format!("claimant-{claimed_id}"))
    .bind(Utc::now().to_rfc3339())
    .execute(&mut writer)
    .await
    .expect("SQLite's binary primary key accepts the case-variant ID");
    writer.close().await.expect("close competing writer");

    recovering.resume();
    let recovered = recovering.wait_for_outcome();
    recovered.expect_refused(
        "recovery after a case-variant ID claim during Variant detection",
        "ignoring ASCII case while Variant detection ran",
    );
    let mods = core.list_mods(GameCode::Gimi).await.expect("list Mods");
    assert_eq!(
        mods.iter()
            .map(|mod_record| mod_record.id.as_str())
            .collect::<Vec<_>>(),
        [claimed_id],
        "the competing claimant must be the only committed Mod row",
    );
    assert!(
        orphan.is_dir(),
        "refused recovery must preserve the orphan for user action",
    );
}

/// A real process death after the recovery row insert but before the staged
/// Variant set is persisted must roll back that still-open transaction. On
/// restart, the directory therefore remains in the orphan report and the same
/// recovery operation can finish it, including its Variant rows.
#[tokio::test]
async fn recovery_crash_before_variant_persistence_leaves_a_retryable_orphan() {
    let env = TestEnv::new();
    let core = env.core().await;
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game Library root");
    let orphan = root.join(Ulid::new().to_string());
    for variant in ["Blue", "Red"] {
        let dir = orphan.join(variant);
        std::fs::create_dir_all(&dir).expect("Variant directory");
        std::fs::write(dir.join("merged.ini"), format!("hash={variant}\n")).expect("Variant INI");
    }

    let mut recovering = probe(&env)
        .crashing_at(gmm_lib::core::crash_points::RECOVER_AFTER_ROW_INSERT)
        .op([
            "recover",
            "--path",
            &orphan.display().to_string(),
            "--name",
            "Crash-Retry Variants",
        ])
        .spawn();
    recovering.wait_for_crash();
    drop(core);

    let restarted = env.core().await;
    assert!(
        restarted
            .list_mods(GameCode::Gimi)
            .await
            .expect("list after recovery crash")
            .is_empty(),
        "the uncommitted recovery row must roll back when the process dies",
    );
    let report = restarted
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit after recovery crash");
    assert!(
        report.unreferenced.iter().any(|entry| entry.path == orphan),
        "the rolled-back recovery directory must remain visible for user action: {report:?}",
    );

    let recovered = restarted
        .recover_unreferenced_library_dir(GameCode::Gimi, &orphan, "Crash-Retry Variants")
        .await
        .expect("retry recovery after process death");
    let variants = restarted
        .list_variants(&recovered.id)
        .await
        .expect("Variants after retry");
    assert_eq!(
        variants
            .iter()
            .map(|variant| variant.name.as_str())
            .collect::<Vec<_>>(),
        ["Blue", "Red"],
        "the retry must finish the same Variant detection that the crash interrupted",
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

/// The Mod row's Library path is durable database structure, not an observation
/// about the filesystem. A root path has no parent, and a differently named
/// child violates the schema-level `<root>/<mod-id>` invariant; neither may be
/// downgraded to a retryable per-Mod filesystem quarantine.
///
/// Mutation oracle: changing either direct-child guard in reinstall preparation
/// back to `ReinstallRecoveryUncertain` fires that case's typed-corruption
/// assertion below.
#[tokio::test]
async fn reinstall_rejects_structurally_impossible_mod_paths_as_corruption() {
    for (case, corrupt_path, expected_reason) in [
        (
            "parentless",
            PathBuf::from(std::path::MAIN_SEPARATOR_STR),
            "has no parent",
        ),
        (
            "wrong filename",
            PathBuf::from("not-the-mod-id"),
            "not the expected direct child",
        ),
    ] {
        let env = TestEnv::new();
        let core = env.core().await;
        let gamebanana_id = if case == "parentless" {
            166_101_u64
        } else {
            166_102_u64
        };
        let (imported, _) = seed_enabled_gamebanana_mod(&env, &core, gamebanana_id).await;
        let corrupt_path = if case == "wrong filename" {
            imported
                .library_path
                .parent()
                .expect("game Library root")
                .join(corrupt_path)
        } else {
            corrupt_path
        };
        let pool = sqlx::SqlitePool::connect(&env.db_url)
            .await
            .expect("open DB for structurally corrupt Mod path");
        sqlx::query("UPDATE mods SET library_path = ? WHERE id = ?")
            .bind(corrupt_path.to_string_lossy().as_ref())
            .bind(&imported.id)
            .execute(&pool)
            .await
            .expect("plant structurally impossible Mod path");
        pool.close().await;

        let update_zip = env._tmp.path().join(format!("{case}-update.zip"));
        write_reinstall_zip(&update_zip, b"[TextureOverrideNew]\nhash=new\n");
        let update_bytes = std::fs::read(&update_zip).expect("update ZIP bytes");
        let mut server = mockito::Server::new_async().await;
        let api_path = format!("/apiv11/Mod/{gamebanana_id}");
        let file_path = format!("/file/{gamebanana_id}/new.zip");
        let _api = server
            .mock("GET", mockito::Matcher::Regex(format!("{api_path}.*")))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"{{
                    "_idRow": {gamebanana_id}, "_sName": "Corrupt Path Update",
                    "_sProfileUrl": "https://gamebanana.com/mods/{gamebanana_id}", "_sVersion": "2.0.0",
                    "_aSubmitter": {{ "_sName": "Author" }},
                    "_aPreviewMedia": {{ "_aImages": [] }},
                    "_aFiles": [{{ "_sFile": "new.zip", "_sDownloadUrl": "{base}{file_path}" }}]
                }}"#,
                base = server.url(),
            ))
            .create_async()
            .await;
        let _file = server
            .mock("GET", file_path.as_str())
            .with_status(200)
            .with_body(update_bytes)
            .create_async()
            .await;

        let error = core
            .reinstall_gamebanana_mod_with_endpoints(
                &imported.id,
                &gmm_lib::core::gamebanana::Endpoints {
                    api_base: server.url(),
                },
            )
            .await
            .expect_err("a structurally impossible Mod path must stop reinstall");
        assert!(
            matches!(
                error,
                Error::ReinstallWitnessCorrupt { ref mod_id, ref reason }
                    if mod_id == &imported.id && reason.contains(expected_reason)
            ),
            "{case} Mod path must be classified as database corruption, got: {error}",
        );
        assert!(
            imported.library_path.join("Red/merged.ini").is_file(),
            "{case} corruption must leave the installed Library bytes untouched",
        );
    }
}

/// The first rename has hidden the old tree under the shared durable
/// quarantine, but the replacement has not taken the live name. A real process
/// death must leave the witness for ordinary startup to restore bytes, row
/// state, and Junction without choosing between candidates.
///
/// Mutation oracle: deleting the `reinstall_swaps` insert makes startup treat
/// the old tree as an ordinary delete quarantine; the old-byte assertion fails.
#[tokio::test]
async fn reinstall_crash_after_old_quarantine_rolls_back_the_whole_mod() {
    let env = TestEnv::new();
    let core = env.core().await;
    let id = 166_001_u64;
    let (imported, old_active_variant_id) = seed_enabled_gamebanana_mod(&env, &core, id).await;
    let update_zip = env._tmp.path().join("update-after-old-quarantine.zip");
    write_reinstall_zip(&update_zip, b"[TextureOverrideNew]\nhash=new\n");
    let update_bytes = std::fs::read(&update_zip).expect("update ZIP bytes");
    let mut server = mockito::Server::new_async().await;
    let api_path = format!("/apiv11/Mod/{id}");
    let file_path = format!("/file/{id}/new.zip");
    let _api = server
        .mock("GET", mockito::Matcher::Regex(format!("{api_path}.*")))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
                "_idRow": {id}, "_sName": "Crash Safe Mod v2",
                "_sProfileUrl": "https://gamebanana.com/mods/{id}", "_sVersion": "2.0.0",
                "_aSubmitter": {{ "_sName": "Author" }},
                "_aPreviewMedia": {{ "_aImages": [] }},
                "_aFiles": [{{ "_sFile": "new.zip", "_sDownloadUrl": "{base}{file_path}" }}]
            }}"#,
            base = server.url(),
        ))
        .create_async()
        .await;
    let _file = server
        .mock("GET", file_path.as_str())
        .with_status(200)
        .with_body(update_bytes)
        .create_async()
        .await;

    let mut reinstalling = probe(&env)
        .crashing_at(gmm_lib::core::crash_points::REINSTALL_AFTER_OLD_QUARANTINE_MOVE)
        .op([
            "reinstall",
            "--mod-id",
            &imported.id,
            "--api-base",
            &server.url(),
        ])
        .spawn();
    reinstalling.wait_for_crash();
    assert_reinstall_rolled_back(
        &env,
        &imported,
        &old_active_variant_id,
        "crash after old quarantine",
    )
    .await;
}

/// The complete replacement already occupies the live Mod name, but the
/// metadata transaction did not commit. Witness presence still means rollback;
/// startup must move the new tree aside, restore old, and rebuild the Junction.
///
/// Mutation oracle: ignoring witness presence when a live tree exists leaves
/// the new bytes installed and fires the old-byte assertion.
#[tokio::test]
async fn reinstall_crash_after_replacement_move_still_rolls_back_the_whole_mod() {
    let env = TestEnv::new();
    let core = env.core().await;
    let id = 166_002_u64;
    let (imported, old_active_variant_id) = seed_enabled_gamebanana_mod(&env, &core, id).await;
    let update_zip = env._tmp.path().join("update-after-replacement-move.zip");
    write_reinstall_zip(&update_zip, b"[TextureOverrideNew]\nhash=new\n");
    let update_bytes = std::fs::read(&update_zip).expect("update ZIP bytes");
    let mut server = mockito::Server::new_async().await;
    let api_path = format!("/apiv11/Mod/{id}");
    let file_path = format!("/file/{id}/new.zip");
    let _api = server
        .mock("GET", mockito::Matcher::Regex(format!("{api_path}.*")))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
                "_idRow": {id}, "_sName": "Crash Safe Mod v2",
                "_sProfileUrl": "https://gamebanana.com/mods/{id}", "_sVersion": "2.0.0",
                "_aSubmitter": {{ "_sName": "Author" }},
                "_aPreviewMedia": {{ "_aImages": [] }},
                "_aFiles": [{{ "_sFile": "new.zip", "_sDownloadUrl": "{base}{file_path}" }}]
            }}"#,
            base = server.url(),
        ))
        .create_async()
        .await;
    let _file = server
        .mock("GET", file_path.as_str())
        .with_status(200)
        .with_body(update_bytes)
        .create_async()
        .await;

    let mut reinstalling = probe(&env)
        .crashing_at(gmm_lib::core::crash_points::REINSTALL_AFTER_REPLACEMENT_MOVE)
        .op([
            "reinstall",
            "--mod-id",
            &imported.id,
            "--api-base",
            &server.url(),
        ])
        .spawn();
    reinstalling.wait_for_crash();
    assert_reinstall_rolled_back(
        &env,
        &imported,
        &old_active_variant_id,
        "crash after replacement move",
    )
    .await;
}

/// Once metadata/Variants and witness deletion commit in one SQLite
/// transaction, the new tree is authoritative. A crash before old-byte purge
/// must not roll back; ordinary startup only finishes the verified quarantine.
///
/// Mutation oracle: moving witness deletion out of the metadata transaction
/// leaves it present at this crash point, so startup restores old and the
/// new-byte assertion fails.
#[tokio::test]
async fn reinstall_crash_after_metadata_commit_keeps_the_complete_new_mod() {
    let env = TestEnv::new();
    let core = env.core().await;
    let id = 166_003_u64;
    let (imported, _old_active_variant_id) = seed_enabled_gamebanana_mod(&env, &core, id).await;
    let update_zip = env._tmp.path().join("update-after-metadata-commit.zip");
    write_reinstall_zip(&update_zip, b"[TextureOverrideNew]\nhash=new\n");
    let update_bytes = std::fs::read(&update_zip).expect("update ZIP bytes");
    let mut server = mockito::Server::new_async().await;
    let api_path = format!("/apiv11/Mod/{id}");
    let file_path = format!("/file/{id}/new.zip");
    let _api = server
        .mock("GET", mockito::Matcher::Regex(format!("{api_path}.*")))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
                "_idRow": {id}, "_sName": "Crash Safe Mod v2",
                "_sProfileUrl": "https://gamebanana.com/mods/{id}", "_sVersion": "2.0.0",
                "_aSubmitter": {{ "_sName": "Author" }},
                "_aPreviewMedia": {{ "_aImages": [] }},
                "_aFiles": [{{ "_sFile": "new.zip", "_sDownloadUrl": "{base}{file_path}" }}]
            }}"#,
            base = server.url(),
        ))
        .create_async()
        .await;
    let _file = server
        .mock("GET", file_path.as_str())
        .with_status(200)
        .with_body(update_bytes)
        .create_async()
        .await;

    let mut reinstalling = probe(&env)
        .crashing_at(gmm_lib::core::crash_points::REINSTALL_AFTER_METADATA_COMMIT)
        .op([
            "reinstall",
            "--mod-id",
            &imported.id,
            "--api-base",
            &server.url(),
        ])
        .spawn();
    reinstalling.wait_for_crash();
    assert_reinstall_committed(&env, &imported, "crash after metadata commit").await;
}

/// A non-empty destination forces `move_subtree` past rename and through its
/// recursive copy/delete fallback. Copying gives both live and staged trees new
/// filesystem identities, so relocation must refuse before touching bytes
/// while the reinstall witness exists. Once reinstall commits, retrying the
/// exact same relocation is safe, and a fresh startup must find no witness or
/// reserved swap state left to retry forever.
///
/// Mutation oracle: removing the active-witness guard from `move_root` lets the
/// first relocation copy the in-flight stage; the explicit refusal assertion
/// fires because relocation incorrectly succeeds.
#[tokio::test]
async fn copy_based_relocation_waits_for_reinstall_then_startup_settles() {
    let env = TestEnv::new();
    let core = env.core().await;
    let id = 166_004_u64;
    let (imported, _old_active_variant_id) = seed_enabled_gamebanana_mod(&env, &core, id).await;
    let update_zip = env._tmp.path().join("update-before-copy-relocation.zip");
    write_reinstall_zip(&update_zip, b"[TextureOverrideNew]\nhash=new\n");
    let update_bytes = std::fs::read(&update_zip).expect("update ZIP bytes");
    let mut server = mockito::Server::new_async().await;
    let api_path = format!("/apiv11/Mod/{id}");
    let file_path = format!("/file/{id}/new.zip");
    let _api = server
        .mock("GET", mockito::Matcher::Regex(format!("{api_path}.*")))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
                "_idRow": {id}, "_sName": "Crash Safe Mod v2",
                "_sProfileUrl": "https://gamebanana.com/mods/{id}", "_sVersion": "2.0.0",
                "_aSubmitter": {{ "_sName": "Author" }},
                "_aPreviewMedia": {{ "_aImages": [] }},
                "_aFiles": [{{ "_sFile": "new.zip", "_sDownloadUrl": "{base}{file_path}" }}]
            }}"#,
            base = server.url(),
        ))
        .create_async()
        .await;
    let _file = server
        .mock("GET", file_path.as_str())
        .with_status(200)
        .with_body(update_bytes)
        .create_async()
        .await;

    let mut reinstalling = probe(&env)
        .pausing_at(gmm_lib::core::crash_points::REINSTALL_AFTER_WITNESS_COMMIT)
        .op([
            "reinstall",
            "--mod-id",
            &imported.id,
            "--api-base",
            &server.url(),
        ])
        .spawn();
    reinstalling.wait_for_pause(gmm_lib::core::crash_points::REINSTALL_AFTER_WITNESS_COMMIT);

    let new_root = env._tmp.path().join("copy-relocated-gimi");
    std::fs::create_dir_all(&new_root).expect("pre-populated relocation destination");
    std::fs::write(new_root.join("forces-copy-fallback"), b"keep").expect("copy-fallback sentinel");
    let relocation = core
        .set_library_path_for_game(GameCode::Gimi, Some(&new_root))
        .await
        .expect_err("copy-based Library relocation during reinstall must be refused");
    assert!(
        relocation.to_string().contains("Let the reinstall finish"),
        "relocation must tell the user how to proceed, got: {relocation}",
    );
    assert!(
        imported.library_path.is_dir(),
        "refused relocation must not touch the installed Mod",
    );

    reinstalling.resume();
    reinstalling
        .wait_for_outcome()
        .expect_ok("reinstall after competing relocation was refused");

    let report = core
        .set_library_path_for_game(GameCode::Gimi, Some(&new_root))
        .await
        .expect("retry copy-based relocation after reinstall settles");
    assert_eq!(report.relocated, vec![imported.id.clone()]);
    assert_eq!(
        std::fs::read(new_root.join("forces-copy-fallback")).expect("fallback sentinel"),
        b"keep",
        "the non-empty destination must survive, proving rename could not replace it",
    );

    drop(core);
    let restarted = env.core().await;
    let listed = restarted
        .list_mods(GameCode::Gimi)
        .await
        .expect("list after copy relocation startup");
    let relocated = listed
        .iter()
        .find(|candidate| candidate.id == imported.id)
        .expect("relocated Mod row");
    assert_eq!(relocated.library_path, new_root.join(&imported.id));
    assert_eq!(
        std::fs::read(relocated.library_path.join("merged.ini"))
            .expect("replacement bytes after copy relocation startup"),
        b"[TextureOverrideNew]\nhash=new\n",
    );
    assert_eq!(relocated.version.as_deref(), Some("2.0.0"));
    assert_eq!(
        std::fs::read(env.game_mods.join("Crash Safe Mod/merged.ini"))
            .expect("working Junction after copy relocation startup"),
        b"[TextureOverrideNew]\nhash=new\n",
    );
    let pool = sqlx::SqlitePool::connect(&env.db_url)
        .await
        .expect("open DB after copy relocation startup");
    let witnesses: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reinstall_swaps")
        .fetch_one(&pool)
        .await
        .expect("count settled reinstall witnesses");
    assert_eq!(
        witnesses, 0,
        "startup after copy relocation must have no reinstall witness left to retry",
    );
}

/// A failed startup rollback quarantines only the affected Mod. Its witness
/// remains the owner of every uncertain byte tree, while an in-app retry uses
/// the exact same verified rollback once the obstruction is corrected.
///
/// Mutation oracle: removing `recover_interrupted_reinstalls_at_startup` from
/// `Core::new` leaves `reinstall_recovery` empty, so the named assertion fails.
#[tokio::test]
async fn failed_reinstall_recovery_quarantines_one_mod_and_in_app_retry_settles_it() {
    let env = TestEnv::new();
    let core = env.core().await;
    let imported = env.seed_mod(&core, "Stale Reinstall Recovery").await;
    core.set_game_install_path(
        GameCode::Gimi,
        env.game_mods.parent().expect("game install path"),
    )
    .await
    .expect("record game install path");
    core.set_enabled(&imported.id, true, &env.game_mods)
        .await
        .expect("enable Mod before its reinstall is quarantined");
    let junction = env.game_mods.join("Stale Reinstall Recovery");
    assert!(
        junction.join("merged.ini").is_file(),
        "the enabled Mod must begin deployed"
    );
    let root = imported.library_path.parent().expect("game Library root");
    let token = Ulid::new();
    let stage = root.join(format!(".gmm-reinstall-{token}"));
    let held_stage = root.join(format!(".held-reinstall-{token}"));
    let quarantine = root.join(format!(".gmm-delete-{token}"));
    std::fs::create_dir(&stage).expect("original reinstall stage");
    std::fs::write(stage.join("replacement.ini"), b"staged replacement")
        .expect("staged replacement bytes");

    let pool = sqlx::SqlitePool::connect(&env.db_url)
        .await
        .expect("open DB for recovery witness");
    sqlx::query(
        "INSERT INTO reinstall_swaps (
            token, mod_id, game_code, library_path, staged_path,
            quarantine_path, old_identity, staged_identity, created_at
         ) VALUES (?, ?, 'gimi', ?, ?, ?, ?, ?, ?)",
    )
    .bind(token.to_string())
    .bind(&imported.id)
    .bind(imported.library_path.to_string_lossy().as_ref())
    .bind(stage.to_string_lossy().as_ref())
    .bind(quarantine.to_string_lossy().as_ref())
    .bind(durable_directory_key(&imported.library_path))
    .bind(durable_directory_key(&stage))
    .bind("2026-08-23T00:00:00Z")
    .execute(&pool)
    .await
    .expect("insert recoverable reinstall witness");
    pool.close().await;
    drop(core);

    // Simulate an external actor replacing the reserved stage after the
    // witness committed. Recovery must not delete the unowned replacement.
    std::fs::rename(&stage, &held_stage).expect("hold witnessed stage aside");
    std::fs::create_dir(&stage).expect("replacement at reserved stage name");
    std::fs::write(stage.join("unknown.ini"), b"unowned bytes").expect("unowned replacement bytes");

    let started = Core::new(env.library.clone(), &env.db_url)
        .await
        .expect("one Mod's failed filesystem recovery must not stop GMM");
    assert!(
        std::fs::symlink_metadata(&junction).is_err(),
        "quarantine must withdraw its live Junction before Core starts",
    );
    assert!(
        imported.library_path.join("merged.ini").is_file(),
        "withdrawing a Junction must not touch the Mod's Library bytes",
    );
    let listed = started
        .list_mods(GameCode::Gimi)
        .await
        .expect("list Mods after quarantined startup");
    let quarantined = listed
        .iter()
        .find(|mod_| mod_.id == imported.id)
        .and_then(|mod_| mod_.reinstall_recovery.as_ref())
        .expect("the failed witness must visibly quarantine its Mod");
    assert_eq!(quarantined.attempts, 1);
    assert_eq!(quarantined.library_path, imported.library_path);
    assert_eq!(quarantined.staged_path, stage);
    assert_eq!(quarantined.quarantine_path, quarantine);
    assert!(
        listed
            .iter()
            .find(|mod_| mod_.id == imported.id)
            .expect("quarantined Mod remains listed")
            .enabled,
        "quarantine must preserve the user's enabled intent",
    );
    assert!(
        quarantined.reason.contains("unrelated directory"),
        "the durable quarantine must retain the specific intervention evidence: {quarantined:?}",
    );
    assert_eq!(
        std::fs::read(stage.join("unknown.ini")).expect("unowned bytes after refused startup"),
        b"unowned bytes",
        "quarantined startup recovery must leave an unproved directory untouched",
    );

    let toggle = started
        .set_enabled(&imported.id, false, &env.game_mods)
        .await
        .expect_err("a quarantined Mod must be unusable");
    assert!(
        toggle.to_string().contains("Retry recovery"),
        "the refusal must point at the in-app escape, got: {toggle}",
    );
    let variant_switch = started
        .set_active_variant(
            &imported.id,
            "unavailable-while-quarantined",
            &env.game_mods,
        )
        .await
        .expect_err("a quarantined Mod must reject Variant changes before validation");
    assert!(
        variant_switch.to_string().contains("Retry recovery"),
        "the Variant refusal must identify reinstall quarantine, got: {variant_switch}",
    );

    // The blast radius is exactly one Mod. Unrelated Library work and the
    // per-game conflict scan remain available in the same running Core.
    let other_source = env.data_dir.join("other-game-source");
    std::fs::create_dir_all(&other_source).expect("other game source");
    std::fs::write(other_source.join("merged.ini"), b"hash=other\n").expect("other game bytes");
    let other = started
        .adopt_folder(GameCode::Srmi, &other_source, "Other Game Mod")
        .await
        .expect("another Game remains manageable");
    let other_mods = env.data_dir.join("StarRail/Mods");
    std::fs::create_dir_all(&other_mods).expect("other game Mods path");
    started
        .set_enabled(&other.id, true, &other_mods)
        .await
        .expect("another Game's Mod remains toggleable");
    started
        .detect_conflicts(GameCode::Gimi)
        .await
        .expect("a quarantined Mod cannot break the game's conflict report");
    let reconciled = started
        .reconcile_junctions(GameCode::Gimi, &env.game_mods)
        .await
        .expect("reconcile leaves only the quarantined Mod untouched");
    assert_eq!(
        reconciled.quarantined.as_slice(),
        std::slice::from_ref(&imported.id),
        "reconcile must name the Mod as quarantined rather than disabled",
    );
    assert!(
        std::fs::symlink_metadata(&junction).is_err(),
        "reconcile must keep a quarantined Mod withdrawn from the game",
    );

    // Undo the external substitution and use the in-app retry. Recovery
    // retires the same witness without a restart or database editing.
    std::fs::remove_dir_all(&stage).expect("remove external replacement");
    std::fs::rename(&held_stage, &stage).expect("restore witnessed stage identity");
    started
        .start_session(&SessionInfo {
            game: GameCode::Gimi,
            pid: std::process::id(),
            started_at: Utc::now(),
        })
        .await
        .expect("start a Game Session before retry");
    let session_refusal = started
        .retry_reinstall_recovery(&imported.id)
        .await
        .expect_err("retry must not move Mod bytes during a Game Session");
    assert!(
        session_refusal.to_string().contains("session"),
        "the retry refusal must explain the active Game Session: {session_refusal}",
    );
    assert!(
        stage.join("replacement.ini").is_file(),
        "a session-refused retry must leave the staged replacement untouched",
    );
    started.end_session().await.expect("end Game Session");
    let outcome = started
        .retry_reinstall_recovery(&imported.id)
        .await
        .expect("retry the verified rollback");
    assert!(
        matches!(outcome, gmm_lib::core::ReinstallRecoveryOutcome::Recovered),
        "correcting the obstruction must settle the quarantined witness: {outcome:?}",
    );
    assert!(
        imported.library_path.join("merged.ini").is_file(),
        "successful retry must keep the original Mod bytes",
    );
    assert!(
        junction.join("merged.ini").is_file(),
        "successful retry must restore the enabled Mod's Junction before returning",
    );
    let recovered = started
        .list_mods(GameCode::Gimi)
        .await
        .expect("list after in-app recovery");
    assert!(
        recovered
            .iter()
            .find(|mod_| mod_.id == imported.id)
            .expect("recovered Mod remains listed")
            .reinstall_recovery
            .is_none(),
        "the Mod must become usable when the witness is retired",
    );
    assert!(
        recovered
            .iter()
            .find(|mod_| mod_.id == imported.id)
            .expect("recovered Mod remains listed")
            .enabled,
        "recovery must preserve the user's enabled intent",
    );
    drop(started);
    let pool = sqlx::SqlitePool::connect(&env.db_url)
        .await
        .expect("open DB after recovered startup");
    let witnesses: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reinstall_swaps")
        .fetch_one(&pool)
        .await
        .expect("count recovery witnesses after retry");
    assert_eq!(
        witnesses, 0,
        "the successful in-app retry must retire the witness",
    );
}

/// Quarantine is durable even when GMM cannot withdraw the recorded
/// deployment entry. A non-link directory is a deterministic cross-platform
/// stand-in for a locked Junction or permission refusal: the guard must refuse
/// to delete it, startup must continue, and the UI model must say the Mod may
/// still be loading.
///
/// Mutation oracle: deleting the non-link refusal in
/// `withdraw_reinstall_junction` changes the durable withdrawal reason and
/// fires the named user-visible evidence assertion below.
#[tokio::test]
async fn junction_withdrawal_failure_quarantines_as_possibly_deployed_without_aborting_startup() {
    let env = TestEnv::new();
    let core = env.core().await;
    let imported = env.seed_mod(&core, "Possibly Deployed Recovery").await;
    core.set_game_install_path(
        GameCode::Gimi,
        env.game_mods.parent().expect("game install path"),
    )
    .await
    .expect("record game install path");
    core.set_enabled(&imported.id, true, &env.game_mods)
        .await
        .expect("deploy Mod before interrupted reinstall");
    let deployment = env.game_mods.join("Possibly Deployed Recovery");
    gmm_lib::core::junction::remove(&deployment).expect("replace the Junction with a directory");
    std::fs::create_dir(&deployment).expect("non-link deployment directory");
    std::fs::write(deployment.join("still-loading.ini"), b"deployed bytes")
        .expect("possibly loaded deployment bytes");
    let (_token, _stage, _held_stage, _quarantine) =
        obstruct_reinstall_recovery(&env, &imported).await;
    drop(core);

    let started = Core::new(env.library.clone(), &env.db_url)
        .await
        .expect("a failed Junction withdrawal must not abort startup");
    let listed = started
        .list_mods(GameCode::Gimi)
        .await
        .expect("list possibly deployed quarantine");
    let recovery = listed[0]
        .reinstall_recovery
        .as_ref()
        .expect("the failed rollback remains quarantined");
    assert!(
        !recovery.junction_withdrawn,
        "the durable state must not claim Junction withdrawal succeeded",
    );
    assert!(
        recovery
            .junction_withdrawal_error
            .as_deref()
            .is_some_and(|error| error.contains("not a Junction")),
        "the user-visible state must retain why the Mod may still load: {recovery:?}",
    );
    assert_eq!(
        std::fs::read(deployment.join("still-loading.ini"))
            .expect("guarded deployment bytes survive"),
        b"deployed bytes",
        "refusing a non-Junction must never delete its bytes",
    );

    let reconciled = started
        .reconcile_junctions(GameCode::Gimi, &env.game_mods)
        .await
        .expect("reconcile must report rather than propagate withdrawal failure");
    assert_eq!(reconciled.quarantined, vec![imported.id.clone()]);
    let rebuilt = started
        .rebuild_junctions(GameCode::Gimi, &env.game_mods)
        .await
        .expect("rebuild must report rather than propagate withdrawal failure");
    assert_eq!(rebuilt.quarantined, vec![imported.id]);
}

async fn mark_reinstall_recovery_as_quarantined(db_url: &str, token: Ulid) {
    let pool = sqlx::SqlitePool::connect(db_url)
        .await
        .expect("open DB to mark reinstall recovery quarantined");
    sqlx::query(
        "UPDATE reinstall_swaps
         SET recovery_error = 'fixture recovery obstruction',
             recovery_attempted_at = '2026-08-23T00:01:00Z', recovery_attempts = 1,
             junction_withdrawn = 0, junction_withdrawal_error = NULL
         WHERE token = ?",
    )
    .bind(token.to_string())
    .execute(&pool)
    .await
    .expect("mark reinstall recovery quarantined");
    pool.close().await;
}

/// Reconcile snapshots no quarantine, then a real recovery retry fails closed,
/// durably quarantines the enabled Mod, and withdraws its Junction. Resuming the
/// stale pass must not put that deployment entry back into the game.
///
/// Mutation oracle: removing the fresh in-fence quarantine revalidation before
/// Junction creation makes the named final-entry assertion fail.
#[tokio::test]
async fn reconcile_cannot_redeploy_mod_quarantined_after_snapshot() {
    let env = TestEnv::new();
    let core = env.core().await;
    core.set_game_install_path(
        GameCode::Gimi,
        env.game_mods.parent().expect("game install path"),
    )
    .await
    .expect("record game install path");
    let imported = env.seed_mod(&core, "Reconcile Quarantine Race").await;
    core.set_enabled(&imported.id, true, &env.game_mods)
        .await
        .expect("deploy enabled Mod before reconcile race");
    let deployment = env.game_mods.join("Reconcile Quarantine Race");
    assert!(
        deployment.join("merged.ini").is_file(),
        "the enabled Mod must begin the race deployed",
    );

    let pause = gmm_lib::core::crash_points::RECONCILE_AFTER_QUARANTINE_SNAPSHOT;
    let mut reconciling = probe(&env)
        .ready_before_operation()
        .pausing_at(pause)
        .op([
            "reconcile",
            "--mods-dir",
            &env.game_mods.display().to_string(),
        ])
        .spawn();
    reconciling.wait_until_ready_before_operation();
    reconciling.resume();
    reconciling.wait_for_pause(pause);

    let (_token, _stage, _held_stage, _quarantine) =
        obstruct_reinstall_recovery(&env, &imported).await;
    let recovery = core
        .retry_reinstall_recovery(&imported.id)
        .await
        .expect("retry must quarantine the obstructed reinstall");
    assert!(
        matches!(
            recovery,
            gmm_lib::core::ReinstallRecoveryOutcome::Quarantined { .. }
        ),
        "the controlled retry must durably quarantine the Mod: {recovery:?}",
    );
    assert!(
        matches!(
            std::fs::symlink_metadata(&deployment),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        ),
        "recovery must withdraw the Mod before the stale reconcile resumes",
    );

    reconciling.resume();
    reconciling
        .wait_for_outcome()
        .expect_ok("reconcile after concurrent quarantine");

    assert!(
        matches!(
            std::fs::symlink_metadata(&deployment),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        ),
        "stale reconcile recreated the quarantined Mod Junction",
    );
}

/// Rebuild snapshots no quarantine, then a real recovery retry fails closed,
/// durably quarantines the enabled Mod, and withdraws its Junction. Resuming the
/// stale pass must not put that deployment entry back into the game.
///
/// Mutation oracle: bypassing the guarded Junction creation in
/// `rebuild_junctions` makes the named final-entry assertion fail.
#[tokio::test]
async fn rebuild_cannot_redeploy_mod_quarantined_after_snapshot() {
    let env = TestEnv::new();
    let core = env.core().await;
    core.set_game_install_path(
        GameCode::Gimi,
        env.game_mods.parent().expect("game install path"),
    )
    .await
    .expect("record game install path");
    let imported = env.seed_mod(&core, "Rebuild Quarantine Race").await;
    core.set_enabled(&imported.id, true, &env.game_mods)
        .await
        .expect("deploy enabled Mod before rebuild race");
    let deployment = env.game_mods.join("Rebuild Quarantine Race");
    assert!(
        deployment.join("merged.ini").is_file(),
        "the enabled Mod must begin the race deployed",
    );

    let pause = gmm_lib::core::crash_points::REBUILD_AFTER_QUARANTINE_SNAPSHOT;
    let mut rebuilding = probe(&env)
        .ready_before_operation()
        .pausing_at(pause)
        .op([
            "rebuild",
            "--mods-dir",
            &env.game_mods.display().to_string(),
        ])
        .spawn();
    rebuilding.wait_until_ready_before_operation();
    rebuilding.resume();
    rebuilding.wait_for_pause(pause);

    let (_token, _stage, _held_stage, _quarantine) =
        obstruct_reinstall_recovery(&env, &imported).await;
    let recovery = core
        .retry_reinstall_recovery(&imported.id)
        .await
        .expect("retry must quarantine the obstructed reinstall");
    assert!(
        matches!(
            recovery,
            gmm_lib::core::ReinstallRecoveryOutcome::Quarantined { .. }
        ),
        "the controlled retry must durably quarantine the Mod: {recovery:?}",
    );
    assert!(
        matches!(
            std::fs::symlink_metadata(&deployment),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        ),
        "recovery must withdraw the Mod before the stale rebuild resumes",
    );

    rebuilding.resume();
    rebuilding
        .wait_for_outcome()
        .expect_ok("rebuild after concurrent quarantine");

    assert!(
        matches!(
            std::fs::symlink_metadata(&deployment),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        ),
        "stale rebuild recreated the quarantined Mod Junction",
    );
}

async fn assert_enable_survives_stale_disabled_pass(
    operation: &str,
    snapshot_pause: &'static str,
    display_name: &str,
) {
    let env = TestEnv::new();
    let core = env.core().await;
    let imported = env.seed_mod(&core, display_name).await;
    let deployment = env.game_mods.join(display_name);

    let mut stale_pass = probe(&env)
        .ready_before_operation()
        .pausing_at(snapshot_pause)
        .op([
            operation,
            "--mods-dir",
            &env.game_mods.display().to_string(),
        ])
        .spawn();
    stale_pass.wait_until_ready_before_operation();
    stale_pass.resume();
    stale_pass.wait_for_pause(snapshot_pause);

    let junction_pause = gmm_lib::core::crash_points::SET_ENABLED_AFTER_JUNCTION_CREATE;
    let mut enabling = probe(&env)
        .pausing_at(junction_pause)
        .op([
            "set-enabled",
            "--mod-id",
            &imported.id,
            "--enabled",
            "1",
            "--mods-dir",
            &env.game_mods.display().to_string(),
        ])
        .spawn();
    enabling.wait_for_pause(junction_pause);
    assert!(
        deployment.join("merged.ini").is_file(),
        "set_enabled must create the Junction before the stale pass resumes",
    );

    enabling.resume();
    enabling
        .wait_for_outcome()
        .expect_ok("enable after the stale row snapshot");
    stale_pass.resume();
    stale_pass
        .wait_for_outcome()
        .expect_ok(&format!("stale {operation} pass"));

    let enabled = core
        .list_mods(GameCode::Gimi)
        .await
        .expect("list Mods after race")
        .into_iter()
        .find(|candidate| candidate.id == imported.id)
        .expect("raced Mod")
        .enabled;
    assert!(enabled, "the raced Mod must commit enabled");
    assert!(
        deployment.join("merged.ini").is_file(),
        "the enabled Mod must remain actually deployed after stale {operation}",
    );
}

/// Reconcile caches `enabled = 0`, then `set_enabled(true)` creates the
/// Junction under its second writer fence. The stale pass must reacquire the
/// fence and revalidate before removing that newly-created deployment entry.
///
/// Mutation oracle: replacing the guarded disabled-Junction removal with a
/// direct `junction::remove` fires the named final-deployment assertion.
#[tokio::test]
async fn enabled_mod_remains_deployed_after_stale_reconcile() {
    assert_enable_survives_stale_disabled_pass(
        "reconcile",
        gmm_lib::core::crash_points::RECONCILE_AFTER_MOD_SNAPSHOT,
        "Reconcile Enable Transition Race",
    )
    .await;
}

/// Rebuild has the same stale disabled-row window as reconcile and must use
/// the same bounded post-traversal fence discipline.
///
/// Mutation oracle: replacing rebuild's guarded disabled-Junction removal
/// with a direct `junction::remove` fires the named final-deployment assertion.
#[tokio::test]
async fn enabled_mod_remains_deployed_after_stale_rebuild() {
    assert_enable_survives_stale_disabled_pass(
        "rebuild",
        gmm_lib::core::crash_points::REBUILD_AFTER_MOD_SNAPSHOT,
        "Rebuild Enable Transition Race",
    )
    .await;
}

/// Reconcile is a recovery path, not only a reporting path: a prior failed or
/// interrupted withdrawal must be retried before the quarantined Mod is
/// returned to the caller.
///
/// Mutation oracle: replacing the guarded withdrawal branch in
/// `reconcile_junctions` with classification-only bookkeeping leaves the live
/// deployment entry behind and fires the named entry-survival assertion.
#[tokio::test]
async fn reconcile_retries_quarantined_reinstall_junction_withdrawal() {
    let env = TestEnv::new();
    let core = env.core().await;
    let imported = env.seed_mod(&core, "Reconcile Withdrawal Guard").await;
    core.set_enabled(&imported.id, true, &env.game_mods)
        .await
        .expect("deploy Mod before quarantine");
    let deployment = env.game_mods.join("Reconcile Withdrawal Guard");
    let (token, _stage, _held_stage, _quarantine) =
        obstruct_reinstall_recovery(&env, &imported).await;
    mark_reinstall_recovery_as_quarantined(&env.db_url, token).await;

    let result = core
        .reconcile_junctions(GameCode::Gimi, &env.game_mods)
        .await
        .expect("reconcile quarantined Mod");

    assert_eq!(result.quarantined, vec![imported.id]);
    assert!(
        matches!(
            std::fs::symlink_metadata(&deployment),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        ),
        "reconcile must withdraw the quarantined Mod's surviving deployment entry",
    );
}

/// Rebuild independently retries a pending quarantine withdrawal. It must not
/// rely on startup or reconcile having removed the deployment first.
///
/// Mutation oracle: replacing the guarded withdrawal branch in
/// `rebuild_junctions` with classification-only bookkeeping leaves the live
/// deployment entry behind and fires the named entry-survival assertion.
#[tokio::test]
async fn rebuild_retries_quarantined_reinstall_junction_withdrawal() {
    let env = TestEnv::new();
    let core = env.core().await;
    let imported = env.seed_mod(&core, "Rebuild Withdrawal Guard").await;
    core.set_enabled(&imported.id, true, &env.game_mods)
        .await
        .expect("deploy Mod before quarantine");
    let deployment = env.game_mods.join("Rebuild Withdrawal Guard");
    let (token, _stage, _held_stage, _quarantine) =
        obstruct_reinstall_recovery(&env, &imported).await;
    mark_reinstall_recovery_as_quarantined(&env.db_url, token).await;

    let result = core
        .rebuild_junctions(GameCode::Gimi, &env.game_mods)
        .await
        .expect("rebuild quarantined Mod");

    assert_eq!(result.quarantined, vec![imported.id]);
    assert!(
        matches!(
            std::fs::symlink_metadata(&deployment),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        ),
        "rebuild must withdraw the quarantined Mod's surviving deployment entry",
    );
}

/// `read_link` accepts a Windows directory symlink, while the junction crate
/// initially attempts to delete only MOUNT_POINT reparse tags. A dangling
/// target must not turn the target-following existence check into false proof
/// that the link entry disappeared. Keeping this native case in the
/// concurrency target ensures it still runs when another boundary regression
/// in the same suite also fails.
///
/// Mutation oracle: restoring the target-following `Path::exists` checks in
/// `junction::remove` fires the named false-success assertion on Windows CI.
#[cfg(windows)]
#[tokio::test]
async fn dangling_directory_symlink_cannot_survive_a_successful_quarantine_withdrawal() {
    let env = TestEnv::new();
    let core = env.core().await;
    let imported = env.seed_mod(&core, "Dangling Symlink Withdrawal").await;
    core.set_enabled(&imported.id, true, &env.game_mods)
        .await
        .expect("deploy Mod before quarantine");
    let deployment = env.game_mods.join("Dangling Symlink Withdrawal");
    gmm_lib::core::junction::remove(&deployment).expect("remove real Junction");
    let missing_target = env.data_dir.join("missing-symlink-target");
    std::os::windows::fs::symlink_dir(&missing_target, &deployment)
        .expect("create dangling directory symlink");
    let (token, _stage, _held_stage, _quarantine) =
        obstruct_reinstall_recovery(&env, &imported).await;
    mark_reinstall_recovery_as_quarantined(&env.db_url, token).await;

    core.reconcile_junctions(GameCode::Gimi, &env.game_mods)
        .await
        .expect("reconcile quarantined dangling symlink");
    let recovery = core
        .list_mods(GameCode::Gimi)
        .await
        .expect("list quarantined Mod")[0]
        .reinstall_recovery
        .clone()
        .expect("reinstall recovery state");
    let entry_survives = std::fs::symlink_metadata(&deployment).is_ok();

    assert!(
        !recovery.junction_withdrawn || !entry_survives,
        "withdrawal reported success while the dangling directory-symlink entry survived",
    );
}

/// Models a process death after the quarantine record committed but before
/// Junction withdrawal. The default false/null state is intentionally
/// conservative; startup retries the failed rollback and then resolves the
/// pending withdrawal without treating the missing entry as an error.
#[tokio::test]
async fn startup_resumes_pending_withdrawal_after_quarantine_record_commit() {
    let env = TestEnv::new();
    let core = env.core().await;
    let imported = env.seed_mod(&core, "Pending Withdrawal Recovery").await;
    let (token, _stage, _held_stage, _quarantine) =
        obstruct_reinstall_recovery(&env, &imported).await;
    let pool = sqlx::SqlitePool::connect(&env.db_url)
        .await
        .expect("open DB for crash-state fixture");
    sqlx::query(
        "UPDATE reinstall_swaps
         SET recovery_error = 'previous recovery obstruction',
             recovery_attempted_at = '2026-08-23T00:01:00Z', recovery_attempts = 1
         WHERE token = ?",
    )
    .bind(token.to_string())
    .execute(&pool)
    .await
    .expect("commit the pre-withdrawal crash state");
    let pending: (i64, Option<String>) = sqlx::query_as(
        "SELECT junction_withdrawn, junction_withdrawal_error
         FROM reinstall_swaps WHERE token = ?",
    )
    .bind(token.to_string())
    .fetch_one(&pool)
    .await
    .expect("read pending withdrawal state");
    assert_eq!(pending, (0, None));
    pool.close().await;
    drop(core);

    let started = Core::new(env.library.clone(), &env.db_url)
        .await
        .expect("startup must resume the committed pre-withdrawal state");
    let listed = started
        .list_mods(GameCode::Gimi)
        .await
        .expect("list resumed quarantine");
    let recovery = listed[0]
        .reinstall_recovery
        .as_ref()
        .expect("obstructed recovery remains quarantined");
    assert!(
        recovery.junction_withdrawn,
        "startup must resolve the pending withdrawal when no deployment entry exists",
    );
    assert!(recovery.junction_withdrawal_error.is_none());
}

/// Both real processes observe the witness before either enters the serialized
/// recovery fence. The winner retires it; the later caller must report success
/// rather than turning the winner's recovery into a false intervention alert.
///
/// Mutation oracle: restoring `fetch_one`/RowNotFound propagation inside
/// `attempt_reinstall_recovery` makes the later outcome fail the named
/// assertion below.
#[tokio::test]
async fn concurrent_reinstall_retries_report_the_later_success_honestly() {
    let env = TestEnv::new();
    let core = env.core().await;
    let imported = env.seed_mod(&core, "Concurrent Recovery Retry").await;
    let (_token, stage, held_stage, _quarantine) =
        obstruct_reinstall_recovery(&env, &imported).await;
    drop(core);

    let pause = gmm_lib::core::crash_points::RETRY_REINSTALL_AFTER_WITNESS_LOOKUP;
    let mut first = probe(&env)
        .pausing_at(pause)
        .op(["retry-reinstall-recovery", "--mod-id", imported.id.as_str()])
        .spawn();
    first.wait_for_pause(pause);
    let mut later = probe(&env)
        .pausing_at(pause)
        .op(["retry-reinstall-recovery", "--mod-id", imported.id.as_str()])
        .spawn();
    later.wait_for_pause(pause);

    std::fs::remove_dir_all(&stage).expect("remove unproved stage replacement");
    std::fs::rename(&held_stage, &stage).expect("restore witnessed stage identity");

    first.resume();
    first
        .wait_for_outcome()
        .expect_ok("first concurrent recovery retry");
    later.resume();
    later
        .wait_for_outcome()
        .expect_ok("later concurrent retry must recognize recovery already succeeded");

    let pool = sqlx::SqlitePool::connect(&env.db_url)
        .await
        .expect("open DB after concurrent retries");
    let witnesses: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reinstall_swaps")
        .fetch_one(&pool)
        .await
        .expect("count witnesses after concurrent retries");
    assert_eq!(
        witnesses, 0,
        "the winner must retire the witness exactly once"
    );
}

/// An absent reinstall witness does not itself prove recovery. If an enabled
/// Mod's deployed Junction is missing, retry must repair the deployment before
/// it can report that recovery had already completed.
///
/// Mutation oracle: restoring the unconditional `AlreadyRecovered` return for
/// an absent witness makes the named outcome assertion fail.
#[tokio::test]
async fn retry_without_witness_does_not_report_missing_enabled_deployment_as_already_recovered() {
    let env = TestEnv::new();
    let core = env.core().await;
    core.set_game_install_path(
        GameCode::Gimi,
        env.game_mods.parent().expect("game install path"),
    )
    .await
    .expect("record game install path");
    let imported = env.seed_mod(&core, "Missing Recovered Deployment").await;
    core.set_enabled(&imported.id, true, &env.game_mods)
        .await
        .expect("deploy enabled Mod");
    let deployment = env.game_mods.join("Missing Recovered Deployment");
    gmm_lib::core::junction::remove(&deployment).expect("remove enabled Mod deployment");

    let outcome = core
        .retry_reinstall_recovery(&imported.id)
        .await
        .expect("retry recovery without a witness");

    assert!(
        !matches!(
            outcome,
            gmm_lib::core::ReinstallRecoveryOutcome::AlreadyRecovered
        ),
        "missing enabled deployment was reported as AlreadyRecovered",
    );
    assert!(
        deployment.join("merged.ini").is_file(),
        "retry did not restore the enabled Mod's deployed Junction",
    );
}

/// One retry fails its rollback and pauses after reading the quarantined
/// witness for Junction withdrawal. A competing retry has already observed
/// the same witness and can restore it only after the withdrawal releases the
/// shared Library writer fence. The final enabled flag and Junction must agree.
///
/// Mutation oracle: removing the writer fence from
/// `withdraw_quarantined_reinstall_junction` lets the competing retry restore
/// and retire the witness while withdrawal is paused. The stale withdrawal
/// then removes that restored Junction, and the named final-deployment
/// assertion fails.
#[tokio::test]
async fn quarantined_withdrawal_racing_retry_leaves_enabled_mod_deployed() {
    let env = TestEnv::new();
    let core = env.core().await;
    core.set_game_install_path(
        GameCode::Gimi,
        env.game_mods.parent().expect("game install path"),
    )
    .await
    .expect("record game install path");
    let imported = env.seed_mod(&core, "Withdrawal Retry Race").await;
    core.set_enabled(&imported.id, true, &env.game_mods)
        .await
        .expect("deploy enabled Mod before recovery race");

    // Construct both real processes before the witness exists so startup
    // recovery cannot consume the fixture before the controlled operations.
    let mut withdrawing = probe(&env)
        .ready_before_operation()
        .pausing_at(gmm_lib::core::crash_points::WITHDRAW_REINSTALL_AFTER_WITNESS_LOOKUP)
        .op(["retry-reinstall-recovery", "--mod-id", imported.id.as_str()])
        .spawn();
    withdrawing.wait_until_ready_before_operation();
    let mut retrying = probe(&env)
        .ready_before_operation()
        .pausing_at(gmm_lib::core::crash_points::RETRY_REINSTALL_AFTER_WITNESS_LOOKUP)
        .op(["retry-reinstall-recovery", "--mod-id", imported.id.as_str()])
        .spawn();
    retrying.wait_until_ready_before_operation();

    let (_token, stage, held_stage, _quarantine) =
        obstruct_reinstall_recovery(&env, &imported).await;
    withdrawing.resume();
    withdrawing
        .wait_for_pause(gmm_lib::core::crash_points::WITHDRAW_REINSTALL_AFTER_WITNESS_LOOKUP);

    std::fs::remove_dir_all(&stage).expect("remove unproved stage replacement");
    std::fs::rename(&held_stage, &stage).expect("restore witnessed stage identity");
    retrying.resume();
    retrying.wait_for_pause(gmm_lib::core::crash_points::RETRY_REINSTALL_AFTER_WITNESS_LOOKUP);
    retrying.resume();
    let first_retry = retrying.wait_for_outcome();

    withdrawing.resume();
    withdrawing
        .wait_for_outcome()
        .expect_ok("quarantining retry after serialized Junction withdrawal");

    if !first_retry.ok {
        assert!(
            first_retry
                .error
                .to_lowercase()
                .contains("database is locked"),
            "the competing retry must be excluded by the withdrawal writer fence, got: {}",
            first_retry.error,
        );
        core.retry_reinstall_recovery(&imported.id)
            .await
            .expect("retry recovery after serialized withdrawal");
    }

    let listed = core
        .list_mods(GameCode::Gimi)
        .await
        .expect("list Mod after recovery race");
    let recovered = listed
        .iter()
        .find(|candidate| candidate.id == imported.id)
        .expect("recovered Mod remains listed");
    assert!(recovered.enabled, "the race must preserve enabled intent");
    assert!(
        env.game_mods
            .join("Withdrawal Retry Race")
            .join("merged.ini")
            .is_file(),
        "successful recovery left the enabled Mod without its deployed Junction",
    );
}

/// Junction withdrawal and retry recovery serialize on the same Library
/// writer fence. This narrower oracle invokes withdrawal directly, so the
/// absent-witness repair in `retry_reinstall_recovery` cannot conceal a stale
/// withdrawal that runs after the competing retry retires the witness.
///
/// Mutation oracle: releasing the withdrawal writer fence across its witness
/// read and Junction removal makes the final-deployment assertion fail.
#[tokio::test]
async fn direct_quarantined_withdrawal_cannot_remove_a_recovered_deployment() {
    let env = TestEnv::new();
    let core = env.core().await;
    core.set_game_install_path(
        GameCode::Gimi,
        env.game_mods.parent().expect("game install path"),
    )
    .await
    .expect("record game install path");
    let imported = env.seed_mod(&core, "Direct Withdrawal Fence").await;
    core.set_enabled(&imported.id, true, &env.game_mods)
        .await
        .expect("deploy enabled Mod before recovery race");
    let deployment = env.game_mods.join("Direct Withdrawal Fence");
    let token = Ulid::new();

    // Construct both real processes before the witness exists so startup
    // recovery cannot consume the fixture before the controlled operations.
    let mut withdrawing = probe(&env)
        .ready_before_operation()
        .pausing_at(gmm_lib::core::crash_points::WITHDRAW_REINSTALL_AFTER_WITNESS_LOOKUP)
        .op([
            "withdraw-quarantined-reinstall-junction",
            "--token",
            token.to_string().as_str(),
            "--link",
            deployment.to_string_lossy().as_ref(),
        ])
        .spawn();
    withdrawing.wait_until_ready_before_operation();
    let mut retrying = probe(&env)
        .ready_before_operation()
        .pausing_at(gmm_lib::core::crash_points::RETRY_REINSTALL_AFTER_WITNESS_LOOKUP)
        .op(["retry-reinstall-recovery", "--mod-id", imported.id.as_str()])
        .spawn();
    retrying.wait_until_ready_before_operation();

    let (_token, stage, held_stage, _quarantine) =
        obstruct_reinstall_recovery_with_token(&env, &imported, token).await;
    let pool = sqlx::SqlitePool::connect(&env.db_url)
        .await
        .expect("open DB to quarantine recovery witness");
    sqlx::query(
        "UPDATE reinstall_swaps
         SET recovery_error = 'fixture recovery obstruction',
             recovery_attempted_at = '2026-08-27T00:00:00Z',
             recovery_attempts = 1
         WHERE token = ?",
    )
    .bind(token.to_string())
    .execute(&pool)
    .await
    .expect("quarantine recovery witness");
    pool.close().await;

    retrying.resume();
    retrying.wait_for_pause(gmm_lib::core::crash_points::RETRY_REINSTALL_AFTER_WITNESS_LOOKUP);
    withdrawing.resume();
    withdrawing
        .wait_for_pause(gmm_lib::core::crash_points::WITHDRAW_REINSTALL_AFTER_WITNESS_LOOKUP);

    std::fs::remove_dir_all(&stage).expect("remove unproved stage replacement");
    std::fs::rename(&held_stage, &stage).expect("restore witnessed stage identity");
    retrying.resume();
    let first_retry = retrying.wait_for_outcome();

    withdrawing.resume();
    withdrawing
        .wait_for_outcome()
        .expect_ok("direct serialized Junction withdrawal");

    if !first_retry.ok {
        assert!(
            first_retry
                .error
                .to_lowercase()
                .contains("database is locked"),
            "the competing retry must be excluded by the withdrawal writer fence, got: {}",
            first_retry.error,
        );
        core.retry_reinstall_recovery(&imported.id)
            .await
            .expect("retry witness recovery after serialized withdrawal");
    }

    assert!(
        deployment.join("merged.ini").is_file(),
        "stale direct withdrawal removed the recovered enabled Mod deployment",
    );
}

/// Ordinary delete reclamation runs after reinstall recovery. If the old tree
/// is already at its intent-backed quarantine name when recovery becomes
/// uncertain, that second phase must not erase the user's rollback copy.
///
/// Mutation oracle: removing the old identity arm from
/// `LibraryOwnershipSnapshot::load` lets ordinary cleanup purge `quarantine`,
/// and the `old rollback bytes` assertion fails.
#[tokio::test]
async fn quarantined_reinstall_preserves_old_and_unproved_byte_trees() {
    let env = TestEnv::new();
    let core = env.core().await;
    let imported = env.seed_mod(&core, "Preserved Recovery Trees").await;
    let root = imported.library_path.parent().expect("game Library root");
    let token = Ulid::new();
    let stage = root.join(format!(".gmm-reinstall-{token}"));
    let held_replacement = root.join(format!(".held-replacement-{token}"));
    let quarantine = root.join(format!(".gmm-delete-{token}"));
    let intent = root.join(format!(".gmm-delete-{token}.intent"));
    std::fs::create_dir(&stage).expect("reinstall stage");
    std::fs::write(stage.join("replacement.ini"), b"witnessed replacement")
        .expect("replacement bytes");
    let old_identity = durable_directory_key(&imported.library_path);
    let staged_identity = durable_directory_key(&stage);

    let pool = sqlx::SqlitePool::connect(&env.db_url)
        .await
        .expect("open DB for recovery witness");
    sqlx::query(
        "INSERT INTO reinstall_swaps (
            token, mod_id, game_code, library_path, staged_path,
            quarantine_path, old_identity, staged_identity, created_at
         ) VALUES (?, ?, 'gimi', ?, ?, ?, ?, ?, ?)",
    )
    .bind(token.to_string())
    .bind(&imported.id)
    .bind(imported.library_path.to_string_lossy().as_ref())
    .bind(stage.to_string_lossy().as_ref())
    .bind(quarantine.to_string_lossy().as_ref())
    .bind(&old_identity)
    .bind(&staged_identity)
    .bind("2026-08-23T00:00:00Z")
    .execute(&pool)
    .await
    .expect("insert reinstall witness");
    pool.close().await;
    drop(core);

    std::fs::write(&intent, &old_identity).expect("old-tree ownership intent");
    std::fs::rename(&imported.library_path, &quarantine).expect("old tree to quarantine");
    std::fs::rename(&stage, &imported.library_path).expect("replacement to live");
    std::fs::rename(&imported.library_path, &held_replacement)
        .expect("hold witnessed replacement aside");
    std::fs::create_dir(&imported.library_path).expect("unproved live replacement");
    std::fs::write(
        imported.library_path.join("unknown.ini"),
        b"unproved live bytes",
    )
    .expect("unproved live bytes");

    let started = Core::new(env.library.clone(), &env.db_url)
        .await
        .expect("uncertain reinstall must quarantine only its Mod");
    assert_eq!(
        std::fs::read(quarantine.join("merged.ini")).expect("old rollback bytes survive"),
        b"[TextureOverride]\nhash=42\n",
        "ordinary cleanup must not purge the witnessed old rollback bytes",
    );
    assert_eq!(
        std::fs::read(held_replacement.join("replacement.ini"))
            .expect("witnessed replacement bytes survive"),
        b"witnessed replacement",
    );
    assert_eq!(
        std::fs::read(imported.library_path.join("unknown.ini"))
            .expect("unproved live bytes survive"),
        b"unproved live bytes",
    );
    assert!(
        intent.is_file(),
        "the old-tree ownership intent must survive"
    );
    let listed = started
        .list_mods(GameCode::Gimi)
        .await
        .expect("list quarantined Mod");
    assert!(listed[0].reinstall_recovery.is_some());
}

/// A malformed witness is database corruption, not evidence about one Mod's
/// filesystem bytes. This fixture is deliberately artificial: it disables
/// SQLite foreign keys on one connection to model a corrupt or incorrectly
/// migrated row that the normal application can never write.
///
/// Mutation oracle: making `quarantinable_reinstall_failure` return true for
/// every error lets Core construction succeed, and the named startup-fatal
/// assertion fails.
#[tokio::test]
async fn corrupt_reinstall_witness_still_aborts_startup() {
    let env = TestEnv::new();
    let core = env.core().await;
    let imported = env.seed_mod(&core, "Corrupt Recovery Witness").await;
    let root = imported.library_path.parent().expect("game Library root");
    let token = Ulid::new();
    let stage = root.join(format!(".gmm-reinstall-{token}"));
    let quarantine = root.join(format!(".gmm-delete-{token}"));
    std::fs::create_dir(&stage).expect("reinstall stage");
    let old_identity = durable_directory_key(&imported.library_path);
    let staged_identity = durable_directory_key(&stage);
    drop(core);

    let mut connection = sqlx::SqliteConnection::connect(&env.db_url)
        .await
        .expect("open connection for corrupt witness fixture");
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut connection)
        .await
        .expect("disable foreign keys only for artificial corruption");
    sqlx::query(
        "INSERT INTO reinstall_swaps (
            token, mod_id, game_code, library_path, staged_path,
            quarantine_path, old_identity, staged_identity, created_at
         ) VALUES (?, ?, 'corrupt-game-code', ?, ?, ?, ?, ?, ?)",
    )
    .bind(token.to_string())
    .bind(&imported.id)
    .bind(imported.library_path.to_string_lossy().as_ref())
    .bind(stage.to_string_lossy().as_ref())
    .bind(quarantine.to_string_lossy().as_ref())
    .bind(old_identity)
    .bind(staged_identity)
    .bind("2026-08-23T00:00:00Z")
    .execute(&mut connection)
    .await
    .expect("insert artificial corrupt recovery witness");
    connection.close().await.expect("close fixture connection");

    let startup = Core::new(env.library.clone(), &env.db_url).await;
    let error = match startup {
        Ok(_) => panic!("database-corrupt recovery state must keep startup fatal"),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("invalid game code"),
        "startup must report the database value rather than blame filesystem recovery: {error}",
    );
}

/// Structural witness paths are database corruption, even when the token,
/// Mod, and Game all identify real rows. None of these values may become a
/// per-Mod filesystem quarantine or reach path-based relocation decisions.
///
/// Mutation oracle: classifying `ReinstallWitnessCorrupt` as quarantinable
/// makes Core construction succeed and fires the case-specific startup-fatal
/// assertion below.
#[tokio::test]
async fn corrupt_reinstall_witness_paths_still_abort_startup() {
    for corrupt_field in ["library_path", "staged_path", "quarantine_path"] {
        let env = TestEnv::new();
        let core = env.core().await;
        let imported = env.seed_mod(&core, "Corrupt Recovery Witness Path").await;
        let root = imported.library_path.parent().expect("game Library root");
        let token = Ulid::new();
        let stage = root.join(format!(".gmm-reinstall-{token}"));
        let quarantine = root.join(format!(".gmm-delete-{token}"));
        std::fs::create_dir(&stage).expect("reinstall stage");
        let old_identity = durable_directory_key(&imported.library_path);
        let staged_identity = durable_directory_key(&stage);
        let mut library_path = imported.library_path.clone();
        let mut staged_path = stage.clone();
        let mut quarantine_path = quarantine.clone();
        match corrupt_field {
            "library_path" => library_path = root.join("not-the-mod-id"),
            "staged_path" => staged_path = root.join(".gmm-reinstall-wrong-token"),
            "quarantine_path" => quarantine_path = root.join(".gmm-delete-wrong-token"),
            _ => unreachable!(),
        }

        let pool = sqlx::SqlitePool::connect(&env.db_url)
            .await
            .expect("open DB for corrupt path fixture");
        sqlx::query(
            "INSERT INTO reinstall_swaps (
                token, mod_id, game_code, library_path, staged_path,
                quarantine_path, old_identity, staged_identity, created_at
             ) VALUES (?, ?, 'gimi', ?, ?, ?, ?, ?, ?)",
        )
        .bind(token.to_string())
        .bind(&imported.id)
        .bind(library_path.to_string_lossy().as_ref())
        .bind(staged_path.to_string_lossy().as_ref())
        .bind(quarantine_path.to_string_lossy().as_ref())
        .bind(old_identity)
        .bind(staged_identity)
        .bind("2026-08-23T00:00:00Z")
        .execute(&pool)
        .await
        .expect("insert corrupt path witness");
        pool.close().await;
        drop(core);

        let startup = Core::new(env.library.clone(), &env.db_url).await;
        let error = match startup {
            Ok(_) => panic!(
                "corrupt {corrupt_field} witness must keep startup fatal rather than quarantine one Mod"
            ),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("database corruption"),
            "corrupt {corrupt_field} must be reported as database state, got: {error}",
        );
    }
}

/// Durable directory identities have one canonical fixed-width format. A
/// malformed value is corrupt database state, not evidence that one Mod's
/// filesystem bytes changed or disappeared.
///
/// Mutation oracle: accepting either identity as an unvalidated `String`
/// lets startup quarantine the Mod and fires the case-specific startup-fatal
/// assertion.
#[tokio::test]
async fn corrupt_reinstall_witness_identities_abort_startup_before_filesystem_recovery() {
    for corrupt_field in ["old_identity", "staged_identity"] {
        let env = TestEnv::new();
        let core = env.core().await;
        let imported = env.seed_mod(&core, "Corrupt Recovery Identity").await;
        let root = imported.library_path.parent().expect("game Library root");
        let token = Ulid::new();
        let stage = root.join(format!(".gmm-reinstall-{token}"));
        let quarantine = root.join(format!(".gmm-delete-{token}"));
        std::fs::create_dir(&stage).expect("reinstall stage");
        let mut old_identity = durable_directory_key(&imported.library_path);
        let mut staged_identity = durable_directory_key(&stage);
        match corrupt_field {
            "old_identity" => old_identity = "not-a-durable-identity".to_string(),
            "staged_identity" => staged_identity = "not-a-durable-identity".to_string(),
            _ => unreachable!(),
        }

        let pool = sqlx::SqlitePool::connect(&env.db_url)
            .await
            .expect("open DB for corrupt identity fixture");
        sqlx::query(
            "INSERT INTO reinstall_swaps (
                token, mod_id, game_code, library_path, staged_path,
                quarantine_path, old_identity, staged_identity, created_at
             ) VALUES (?, ?, 'gimi', ?, ?, ?, ?, ?, ?)",
        )
        .bind(token.to_string())
        .bind(&imported.id)
        .bind(imported.library_path.to_string_lossy().as_ref())
        .bind(stage.to_string_lossy().as_ref())
        .bind(quarantine.to_string_lossy().as_ref())
        .bind(old_identity)
        .bind(staged_identity)
        .bind("2026-08-23T00:00:00Z")
        .execute(&pool)
        .await
        .expect("insert corrupt identity witness");
        pool.close().await;
        drop(core);

        let startup = Core::new(env.library.clone(), &env.db_url).await;
        let error = match startup {
            Ok(_) => panic!(
                "corrupt {corrupt_field} must abort startup as database corruption before filesystem recovery"
            ),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("database corruption"),
            "corrupt {corrupt_field} must be reported as database state, got: {error}",
        );
    }
}

/// Relocation must validate every witness against the effective Library root
/// before using its recorded path to decide whether that witness is in scope.
/// Sibling-only validation is insufficient: a self-consistent witness rooted
/// somewhere else can place itself outside the move and silently bypass the
/// active-reinstall refusal.
///
/// Mutation oracle: deleting the recorded-root comparison from
/// `rebase_reinstall_swap_witness` lets this move succeed and fires the named
/// database-corruption assertion.
#[tokio::test]
async fn relocation_validates_the_complete_reinstall_witness_before_scope_decision() {
    let env = TestEnv::new();
    let core = env.core().await;
    let imported = env.seed_mod(&core, "Relocation Witness Validation").await;
    let real_root = imported.library_path.parent().expect("game Library root");
    let token = Ulid::new();
    let corrupt_root = env.data_dir.join("outside-relocation-scope");
    let corrupt_library_path = corrupt_root.join(&imported.id);
    let corrupt_stage = corrupt_root.join(format!(".gmm-reinstall-{token}"));
    let corrupt_quarantine = corrupt_root.join(format!(".gmm-delete-{token}"));
    let actual_stage = real_root.join(format!(".gmm-reinstall-{token}"));
    std::fs::create_dir(&actual_stage).expect("actual stage for identity evidence");

    let pool = sqlx::SqlitePool::connect(&env.db_url)
        .await
        .expect("open DB for relocation witness fixture");
    sqlx::query(
        "INSERT INTO reinstall_swaps (
            token, mod_id, game_code, library_path, staged_path,
            quarantine_path, old_identity, staged_identity, created_at
         ) VALUES (?, ?, 'gimi', ?, ?, ?, ?, ?, ?)",
    )
    .bind(token.to_string())
    .bind(&imported.id)
    .bind(corrupt_library_path.to_string_lossy().as_ref())
    .bind(corrupt_stage.to_string_lossy().as_ref())
    .bind(corrupt_quarantine.to_string_lossy().as_ref())
    .bind(durable_directory_key(&imported.library_path))
    .bind(durable_directory_key(&actual_stage))
    .bind("2026-08-23T00:00:00Z")
    .execute(&pool)
    .await
    .expect("insert corrupt relocation witness");
    pool.close().await;

    let destination = env.data_dir.join("relocated-library");
    let relocation = core
        .set_library_path_for_game(GameCode::Gimi, Some(&destination))
        .await
        .expect_err("relocation must validate a witness before deciding it is out of scope");
    assert!(
        relocation.to_string().contains("database corruption"),
        "relocation must reject the corrupt witness row before moving bytes, got: {relocation}",
    );
    assert!(
        imported.library_path.join("merged.ini").is_file(),
        "a corrupt witness must stop relocation before Library bytes move",
    );
}

/// The typed witness shape is not the database boundary: a migration can add
/// a column while named-column SELECTs keep compiling. Loading a witness must
/// compare the actual SQLite row shape with the column registry derived from
/// the raw witness declaration so a future column cannot silently bypass a
/// validation decision.
///
/// Mutation oracle: removing the row-shape comparison lets relocation reach
/// the ordinary active reinstall refusal and fires the named schema-boundary
/// assertion.
#[tokio::test]
async fn reinstall_witness_loader_rejects_a_column_without_a_validation_rule() {
    let env = TestEnv::new();
    let core = env.core().await;
    let imported = env.seed_mod(&core, "Unruled Witness Column").await;
    let root = imported.library_path.parent().expect("game Library root");
    let token = Ulid::new();
    let stage = root.join(format!(".gmm-reinstall-{token}"));
    let quarantine = root.join(format!(".gmm-delete-{token}"));
    std::fs::create_dir(&stage).expect("reinstall stage");

    let pool = sqlx::SqlitePool::connect(&env.db_url)
        .await
        .expect("open DB for future-column fixture");
    sqlx::query(
        "INSERT INTO reinstall_swaps (
            token, mod_id, game_code, library_path, staged_path,
            quarantine_path, old_identity, staged_identity, created_at
         ) VALUES (?, ?, 'gimi', ?, ?, ?, ?, ?, ?)",
    )
    .bind(token.to_string())
    .bind(&imported.id)
    .bind(imported.library_path.to_string_lossy().as_ref())
    .bind(stage.to_string_lossy().as_ref())
    .bind(quarantine.to_string_lossy().as_ref())
    .bind(durable_directory_key(&imported.library_path))
    .bind(durable_directory_key(&stage))
    .bind("2026-08-23T00:00:00Z")
    .execute(&pool)
    .await
    .expect("insert valid reinstall witness");
    sqlx::query("ALTER TABLE reinstall_swaps ADD COLUMN unruled_future_state TEXT")
        .execute(&pool)
        .await
        .expect("simulate a future migration adding an unruled column");
    pool.close().await;

    let destination = env.data_dir.join("future-column-relocation");
    let error = core
        .set_library_path_for_game(GameCode::Gimi, Some(&destination))
        .await
        .expect_err("an unknown reinstall_swaps column must fail at the row boundary");
    assert!(
        error.to_string().contains("schema columns changed"),
        "the unknown column must demand a validation decision before relocation scope is evaluated, got: {error}",
    );
    assert!(
        imported.library_path.join("merged.ini").is_file(),
        "an unruled witness column must stop relocation before Library bytes move",
    );
}

/// Reinstall rollback and ordinary delete-quarantine reclamation are separate
/// startup phases. If rollback succeeds but a later root cannot be inspected,
/// the committed witness deletion must survive so routine cleanup remains
/// best-effort and does not prevent the whole application from starting.
#[tokio::test]
async fn purge_failure_after_successful_reinstall_rollback_does_not_stop_startup() {
    let env = TestEnv::new();
    let core = env.core().await;
    let imported = env.seed_mod(&core, "Recovered Before Purge Failure").await;
    let root = imported.library_path.parent().expect("game Library root");
    let token = Ulid::new();
    let stage = root.join(format!(".gmm-reinstall-{token}"));
    let quarantine = root.join(format!(".gmm-delete-{token}"));
    std::fs::create_dir(&stage).expect("reinstall stage");
    std::fs::write(stage.join("replacement.ini"), b"staged replacement")
        .expect("staged replacement bytes");

    let pool = sqlx::SqlitePool::connect(&env.db_url)
        .await
        .expect("open DB for reinstall witness");
    sqlx::query(
        "INSERT INTO reinstall_swaps (
            token, mod_id, game_code, library_path, staged_path,
            quarantine_path, old_identity, staged_identity, created_at
         ) VALUES (?, ?, 'gimi', ?, ?, ?, ?, ?, ?)",
    )
    .bind(token.to_string())
    .bind(&imported.id)
    .bind(imported.library_path.to_string_lossy().as_ref())
    .bind(stage.to_string_lossy().as_ref())
    .bind(quarantine.to_string_lossy().as_ref())
    .bind(durable_directory_key(&imported.library_path))
    .bind(durable_directory_key(&stage))
    .bind("2026-08-23T00:00:00Z")
    .execute(&pool)
    .await
    .expect("commit recoverable reinstall witness");
    pool.close().await;
    drop(core);

    // `read_dir` on a regular file fails on every supported platform. Use an
    // otherwise-unused game root so rollback itself succeeds before ordinary
    // quarantine scanning reaches this routine cleanup failure.
    let unreadable_root = env.library.join(GameCode::Srmi.as_str());
    std::fs::write(&unreadable_root, b"not a directory").expect("failing purge root");

    let restarted = Core::new(env.library.clone(), &env.db_url)
        .await
        .expect("ordinary purge failure after successful reinstall rollback must not stop startup");
    assert_eq!(
        std::fs::read(imported.library_path.join("merged.ini"))
            .expect("installed bytes after successful rollback"),
        b"[TextureOverride]\nhash=42\n",
        "startup must keep the restored installed Mod bytes",
    );
    drop(restarted);
    let pool = sqlx::SqlitePool::connect(&env.db_url)
        .await
        .expect("open DB after best-effort purge failure");
    let witnesses: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reinstall_swaps")
        .fetch_one(&pool)
        .await
        .expect("count witnesses after best-effort purge failure");
    assert_eq!(
        witnesses, 0,
        "successful rollback must stay committed when ordinary purge later fails",
    );
}

/// A relocation process is initialized before an interrupted reinstall is
/// seeded, so its own startup cannot consume the fixture. Real startup then
/// pauses after rollback committed and before ordinary quarantine cleanup.
/// A copy-fallback relocation in that gap must refuse; otherwise it gives the
/// rolled-back replacement a new filesystem identity while copying its old
/// intent marker.
///
/// Mutation oracle: removing the owned-delete-quarantine relocation guard
/// makes `relocation between startup recovery phases` succeed and fail the
/// explicit refusal assertion below.
#[tokio::test]
async fn copy_relocation_between_startup_recovery_phases_waits_for_cleanup() {
    let env = TestEnv::new();
    let core = env.core().await;
    let imported = env
        .seed_mod(&core, "Relocation Between Startup Phases")
        .await;
    let new_root = env._tmp.path().join("copy-relocated-after-rollback");
    std::fs::create_dir(&new_root).expect("pre-existing relocation destination");
    std::fs::write(new_root.join("force-copy-fallback"), b"destination marker")
        .expect("make rename replacement impossible");
    let mut relocating = probe(&env)
        .ready_before_operation()
        .op([
            "set-library-path",
            "--path",
            &new_root.display().to_string(),
        ])
        .spawn();
    relocating.wait_until_ready_before_operation();

    let root = imported.library_path.parent().expect("game Library root");
    let token = Ulid::new();
    let stage = root.join(format!(".gmm-reinstall-{token}"));
    let quarantine = root.join(format!(".gmm-delete-{token}"));
    std::fs::create_dir(&stage).expect("reinstall stage");
    std::fs::write(stage.join("replacement.ini"), b"rolled-back replacement")
        .expect("replacement bytes");
    let pool = sqlx::SqlitePool::connect(&env.db_url)
        .await
        .expect("open DB for startup witness");
    sqlx::query(
        "INSERT INTO reinstall_swaps (
            token, mod_id, game_code, library_path, staged_path,
            quarantine_path, old_identity, staged_identity, created_at
         ) VALUES (?, ?, 'gimi', ?, ?, ?, ?, ?, ?)",
    )
    .bind(token.to_string())
    .bind(&imported.id)
    .bind(imported.library_path.to_string_lossy().as_ref())
    .bind(stage.to_string_lossy().as_ref())
    .bind(quarantine.to_string_lossy().as_ref())
    .bind(durable_directory_key(&imported.library_path))
    .bind(durable_directory_key(&stage))
    .bind("2026-08-24T00:00:00Z")
    .execute(&pool)
    .await
    .expect("commit interrupted reinstall witness");
    pool.close().await;
    drop(core);

    let startup_pause = gmm_lib::core::crash_points::STARTUP_AFTER_REINSTALL_RECOVERY;
    let mut startup = probe(&env)
        .pausing_at(startup_pause)
        .op(["startup"])
        .spawn();
    startup.wait_for_pause(startup_pause);

    relocating.resume();
    relocating.wait_for_outcome().expect_refused(
        "relocation between startup recovery phases",
        "cleanup is pending",
    );

    startup.resume();
    startup
        .wait_for_outcome()
        .expect_ok("startup quarantine cleanup after refused relocation");

    probe(&env)
        .op([
            "set-library-path",
            "--path",
            &new_root.display().to_string(),
        ])
        .run()
        .expect_ok("relocation after startup cleanup completed");
    assert_eq!(
        std::fs::read(new_root.join(&imported.id).join("merged.ini"))
            .expect("installed Mod bytes after relocation retry"),
        b"[TextureOverride]\nhash=42\n",
    );
    assert!(
        std::fs::read_dir(&new_root)
            .expect("relocated root")
            .filter_map(std::result::Result::ok)
            .all(|entry| !entry
                .file_name()
                .to_string_lossy()
                .starts_with(".gmm-delete-")),
        "cleanup must retire every rollback quarantine before relocation retries",
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

fn staged_adopt_source(env: &TestEnv, name: &str) -> PathBuf {
    let source = env._tmp.path().join(name);
    for variant in ["Red", "Blue"] {
        let directory = source.join(variant);
        std::fs::create_dir_all(&directory).expect("staged adopt Variant directory");
        std::fs::write(
            directory.join("merged.ini"),
            format!("hash=staged-{variant}\n"),
        )
        .expect("staged adopt Variant file");
    }
    source
}

/// The audit pauses after releasing its first ownership snapshot, then a real
/// producer commits its witness and pauses after its first copied file. The
/// audit scans that live staging directory and must drop it after rechecking
/// ownership under a second, short writer fence.
///
/// Mutation oracle: removing the second ownership snapshot makes `audit whose
/// producer began after the snapshot` fail with the live staged directory in
/// the probe's error.
#[tokio::test]
async fn audit_excludes_a_staging_operation_that_begins_during_its_scan() {
    let env = TestEnv::new();
    let core = env.core().await;
    drop(core);
    let source = staged_adopt_source(&env, "audit-overlapping-staged-adopt");
    let copy_pause = gmm_lib::core::crash_points::ADOPT_DURING_LIBRARY_COPY;
    let mut producer = probe(&env)
        .ready_before_operation()
        .pausing_at(copy_pause)
        .op([
            "adopt",
            "--from",
            &source.display().to_string(),
            "--name",
            "Audit Overlap Stage",
        ])
        .spawn();
    producer.wait_until_ready_before_operation();

    let audit_pause = gmm_lib::core::crash_points::AUDIT_AFTER_OWNERSHIP_SNAPSHOT;
    let mut auditing = probe(&env).pausing_at(audit_pause).op(["audit"]).spawn();
    auditing.wait_for_pause(audit_pause);

    producer.resume();
    producer.wait_for_pause(copy_pause);
    auditing.resume();
    auditing
        .wait_for_outcome()
        .expect_ok("audit whose producer began after the ownership snapshot");

    producer.resume();
    producer
        .wait_for_outcome()
        .expect_ok("producer after the audit released its fence");
}

async fn only_staged_directory(core: &Core) -> PathBuf {
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("resolve staging Library root");
    let directories: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("read staging Library root")
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    assert_eq!(
        directories.len(),
        1,
        "the paused adopt should own exactly one staged directory: {directories:?}",
    );
    directories
        .into_iter()
        .next()
        .expect("one staged directory")
}

/// A real adopt producer pauses after committing its durable witness and
/// copying its first file, while the remaining copy is still outside the
/// writer fence. Relocation must observe that witness under its own fence and
/// refuse before moving the subtree. The unchanged staged identity must remain
/// owned by the producer, so the same public Delete guard still refuses it.
///
/// Mutation oracle: removing the staged-witness relocation guard makes the
/// explicit relocation refusal fail while the producer is paused in its
/// unbounded copy window.
#[tokio::test]
async fn relocation_waits_for_a_staged_adopt_and_preserves_its_owned_bytes() {
    let env = TestEnv::new();
    let core = env.core().await;
    let source = staged_adopt_source(&env, "relocation-refused-staged-adopt");
    let new_root = env._tmp.path().join("relocated-after-staged-adopt");
    let copy_pause = gmm_lib::core::crash_points::ADOPT_DURING_LIBRARY_COPY;
    let mut adopting = probe(&env)
        .pausing_at(copy_pause)
        .op([
            "adopt",
            "--from",
            &source.display().to_string(),
            "--name",
            "Relocation Guarded Stage",
        ])
        .spawn();
    adopting.wait_for_pause(copy_pause);
    let staged = only_staged_directory(&core).await;
    assert!(
        std::fs::read_dir(&staged)
            .expect("read partially copied stage")
            .next()
            .is_some(),
        "the pause must sit after the first copied entry, not before the bytes under test",
    );

    let relocation = core
        .set_library_path_for_game(GameCode::Gimi, Some(&new_root))
        .await
        .expect_err("Library relocation during a staged import must be refused");
    assert!(
        matches!(
            relocation,
            gmm_lib::core::Error::LibraryRelocationBlockedByStaging
        ),
        "relocation must refuse specifically because the producer owns the stage: {relocation}",
    );
    assert!(
        relocation.to_string().contains("Let the import finish"),
        "relocation refusal must tell the user how to proceed: {relocation}",
    );
    assert!(
        staged.is_dir(),
        "refused relocation must leave the partially copied stage at its witnessed path",
    );

    let deleted = core
        .delete_unreferenced_library_dir(GameCode::Gimi, &staged)
        .await;
    assert!(
        matches!(
            deleted,
            Err(gmm_lib::core::Error::NotAnUnreferencedLibraryDir { ref reason, .. })
                if reason.contains("active staging operation")
        ),
        "Delete must still refuse the producer-owned bytes after relocation is refused: {deleted:?}",
    );
    assert!(
        staged.is_dir(),
        "Delete refusal must preserve the producer-owned bytes",
    );

    adopting.resume();
    adopting
        .wait_for_outcome()
        .expect_ok("staged adopt after the competing relocation was refused");
    let report = core
        .set_library_path_for_game(GameCode::Gimi, Some(&new_root))
        .await
        .expect("retry relocation after the staged adopt settles");
    assert_eq!(report.relocated.len(), 1);
    let mods = core
        .list_mods(GameCode::Gimi)
        .await
        .expect("list relocated Mod");
    assert_eq!(mods.len(), 1, "the producer must commit exactly one Mod");
    assert!(
        mods[0].library_path.starts_with(&new_root)
            && mods[0].library_path.join("Red/merged.ini").is_file()
            && mods[0].library_path.join("Blue/merged.ini").is_file(),
        "the completed producer bytes must relocate only after its witness retires",
    );
}

/// A real adopt producer pauses after its first file copy. Recovery observes
/// the committed staging identity through the shared ownership snapshot and
/// must refuse before it can claim a partial Variant tree.
#[tokio::test]
async fn recovery_refuses_a_directory_while_adopt_is_copying() {
    let env = TestEnv::new();
    let core = env.core().await;
    let source = staged_adopt_source(&env, "recovery-refused-staged-adopt");
    let mut adopting = probe(&env)
        .pausing_at(gmm_lib::core::crash_points::ADOPT_DURING_LIBRARY_COPY)
        .op([
            "adopt",
            "--from",
            &source.display().to_string(),
            "--name",
            "Recovery Guarded Stage",
        ])
        .spawn();
    adopting.wait_for_pause(gmm_lib::core::crash_points::ADOPT_DURING_LIBRARY_COPY);
    let staged = only_staged_directory(&core).await;

    let recovered = core
        .recover_unreferenced_library_dir(GameCode::Gimi, &staged, "Must Not Recover")
        .await;
    assert!(
        matches!(
            recovered,
            Err(gmm_lib::core::Error::NotAnUnreferencedLibraryDir { ref reason, .. })
                if reason.contains("active staging operation")
        ),
        "recovery must refuse specifically because the producer owns the stage: {recovered:?}",
    );
    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit the live stage");
    assert!(
        report.unreferenced.is_empty(),
        "the audit must not offer the live stage as an orphan: {report:?}",
    );

    adopting.resume();
    adopting
        .wait_for_outcome()
        .expect_ok("the witnessed adopt after recovery refusal");
    let mods = core
        .list_mods(GameCode::Gimi)
        .await
        .expect("list adopted Mod");
    assert_eq!(mods.len(), 1, "the original adopt must commit one Mod");
    let variants = core
        .list_variants(&mods[0].id)
        .await
        .expect("list completed staged Variants");
    let mut names: Vec<&str> = variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["Blue", "Red"], "the Variant set must be complete");
}

/// Delete shares the exact ownership snapshot used by audit and recovery. It
/// must reject the same live staged identity without moving or purging bytes,
/// then allow the producer to finish normally.
#[tokio::test]
async fn delete_refuses_a_directory_while_adopt_is_copying() {
    let env = TestEnv::new();
    let core = env.core().await;
    let source = staged_adopt_source(&env, "delete-refused-staged-adopt");
    let mut adopting = probe(&env)
        .pausing_at(gmm_lib::core::crash_points::ADOPT_DURING_LIBRARY_COPY)
        .op([
            "adopt",
            "--from",
            &source.display().to_string(),
            "--name",
            "Delete Guarded Stage",
        ])
        .spawn();
    adopting.wait_for_pause(gmm_lib::core::crash_points::ADOPT_DURING_LIBRARY_COPY);
    let staged = only_staged_directory(&core).await;

    let deleted = core
        .delete_unreferenced_library_dir(GameCode::Gimi, &staged)
        .await;
    assert!(
        matches!(
            deleted,
            Err(gmm_lib::core::Error::NotAnUnreferencedLibraryDir { ref reason, .. })
                if reason.contains("active staging operation")
        ),
        "delete must refuse specifically because the producer owns the stage",
    );
    assert!(
        staged.is_dir(),
        "a refused delete must leave the live stage in place"
    );

    adopting.resume();
    adopting
        .wait_for_outcome()
        .expect_ok("the witnessed adopt after delete refusal");
    let mods = core
        .list_mods(GameCode::Gimi)
        .await
        .expect("list adopted Mod");
    assert_eq!(mods.len(), 1, "the original adopt must commit one Mod");
    let variants = core
        .list_variants(&mods[0].id)
        .await
        .expect("list completed staged Variants");
    let mut names: Vec<&str> = variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["Blue", "Red"], "the Variant set must be complete");
}

/// A staging witness owns the filesystem object it recorded, not whatever
/// later appears at the same pathname. Replacing a live stage must therefore
/// make the replacement visible to audit and eligible for explicit deletion.
#[tokio::test]
async fn replacement_at_witnessed_staging_path_remains_unowned() {
    let env = TestEnv::new();
    let core = env.core().await;
    let source = staged_adopt_source(&env, "replaced-witnessed-staged-adopt");
    let mut adopting = probe(&env)
        .pausing_at(gmm_lib::core::crash_points::ADOPT_DURING_LIBRARY_COPY)
        .op([
            "adopt",
            "--from",
            &source.display().to_string(),
            "--name",
            "Replaced Witnessed Stage",
        ])
        .spawn();
    adopting.wait_for_pause(gmm_lib::core::crash_points::ADOPT_DURING_LIBRARY_COPY);
    let staged = only_staged_directory(&core).await;
    let displaced = staged.with_extension("producer-displaced");
    std::fs::rename(&staged, &displaced).expect("move the witnessed producer identity away");
    std::fs::create_dir(&staged).expect("create a replacement at the witnessed spelling");
    std::fs::write(staged.join("replacement.ini"), b"hash=replacement\n")
        .expect("write replacement bytes");

    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit the replacement identity");
    let deletion = core
        .delete_unreferenced_library_dir(GameCode::Gimi, &staged)
        .await;
    let reported_paths: HashSet<_> = report
        .unreferenced
        .iter()
        .map(|directory| directory.path.as_path())
        .collect();
    assert!(
        reported_paths.contains(staged.as_path())
            && reported_paths.contains(displaced.as_path())
            && matches!(
                deletion,
                Ok(ref deleted)
                    if deleted.reclamation
                        == gmm_lib::core::LibraryReclamationOutcome::Reclaimed
            ),
        "a stale path/identity pair must hide neither object and must not guard the replacement: report={report:?}, deletion={deletion:?}",
    );
    assert!(
        displaced.is_dir(),
        "acting on the replacement must not remove the witnessed producer identity",
    );

    // The producer is deliberately not resumed: it has lost its staged name,
    // so RunningProbe's bounded Drop terminates it without manufacturing a
    // second pathname replacement and testing a different commit question.
    drop(adopting);
}

/// ZIP import uses the same setup boundary as adopt. At the shared pause the
/// witness is committed, extraction has not written a byte, and the ownership
/// snapshot already hides the empty stage from the audit.
#[tokio::test]
async fn import_zip_witness_commits_before_extraction_starts() {
    let env = TestEnv::new();
    let core = env.core().await;
    let archive = env._tmp.path().join("witnessed-import.zip");
    write_reinstall_zip(&archive, b"hash=witnessed-zip\n");
    let mut importing = probe(&env)
        .pausing_at(gmm_lib::core::crash_points::STAGING_AFTER_WITNESS_COMMIT)
        .op([
            "import-zip",
            "--zip",
            &archive.display().to_string(),
            "--name",
            "Witnessed ZIP Stage",
        ])
        .spawn();
    importing.wait_for_pause(gmm_lib::core::crash_points::STAGING_AFTER_WITNESS_COMMIT);
    let staged = only_staged_directory(&core).await;
    assert!(
        std::fs::read_dir(&staged)
            .expect("read pre-extract stage")
            .next()
            .is_none(),
        "the witness boundary must fire before extraction writes its first entry",
    );
    let pool = sqlx::SqlitePool::connect(&env.db_url)
        .await
        .expect("open DB for staging witness inspection");
    let operation: String = sqlx::query_scalar("SELECT operation FROM staged_library_operations")
        .fetch_one(&pool)
        .await
        .expect("ZIP staging witness exists");
    pool.close().await;
    assert_eq!(operation, "import_zip");
    let report = core
        .audit_library(GameCode::Gimi)
        .await
        .expect("audit pre-extract stage");
    assert!(
        report.unreferenced.is_empty(),
        "the committed pre-extract witness must hide the live stage: {report:?}",
    );

    importing.resume();
    importing
        .wait_for_outcome()
        .expect_ok("the witnessed ZIP import after inspection");
    let pool = sqlx::SqlitePool::connect(&env.db_url)
        .await
        .expect("open DB after ZIP import");
    let witnesses: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM staged_library_operations")
        .fetch_one(&pool)
        .await
        .expect("count ZIP staging witnesses after commit");
    pool.close().await;
    assert_eq!(
        witnesses, 0,
        "the successful ZIP Mod commit must retire its witness",
    );
}

/// Retiring a failed producer's database witness is not permission to delete
/// whatever later occupies its pathname. Cleanup retains the original open
/// identity, reopens the name after witness retirement, and preserves both
/// the moved original and the user's replacement when they differ.
#[tokio::test]
async fn rollback_witness_retirement_preserves_a_replacement_directory() {
    let env = TestEnv::new();
    let core = env.core().await;
    let missing_source = env._tmp.path().join("missing-rollback-source");
    let mut adopting = probe(&env)
        .pausing_at(gmm_lib::core::crash_points::STAGED_CLEANUP_AFTER_WITNESS_RETIRE)
        .op([
            "adopt",
            "--from",
            &missing_source.display().to_string(),
            "--name",
            "Rollback Identity Guard",
        ])
        .spawn();
    adopting.wait_for_pause(gmm_lib::core::crash_points::STAGED_CLEANUP_AFTER_WITNESS_RETIRE);
    let staged = only_staged_directory(&core).await;
    let original = staged.with_extension("producer-moved-away");
    std::fs::rename(&staged, &original).expect("move producer's empty stage away");
    std::fs::create_dir(&staged).expect("create user replacement at staged spelling");
    std::fs::write(staged.join("precious.buf"), b"user-owned replacement")
        .expect("write replacement bytes");

    adopting.resume();
    let outcome = adopting.wait_for_outcome();
    assert!(
        !outcome.ok,
        "the missing source must still report its original failure: {outcome:?}",
    );
    assert_eq!(
        std::fs::read(staged.join("precious.buf")).expect("replacement survives cleanup"),
        b"user-owned replacement",
        "witness retirement must not turn the pathname into deletion authority",
    );
    assert!(
        original.is_dir(),
        "cleanup must also leave the moved producer identity untouched",
    );
    let pool = sqlx::SqlitePool::connect(&env.db_url)
        .await
        .expect("open DB after rollback cleanup");
    let witnesses: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM staged_library_operations")
        .fetch_one(&pool)
        .await
        .expect("count rollback staging witnesses");
    pool.close().await;
    assert_eq!(
        witnesses, 0,
        "the returned-error rollback must still retire its durable witness",
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
/// pathname. Cleanup must move the candidate to the reserved quarantine name
/// it creates, then re-check which object the rename actually moved. The later
/// path-based recursive purge remains tracked separately in #172.
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

/// Staged cleanup has released the identity handles that proved the reserved
/// quarantine and is about to purge it. If an external actor swaps in another
/// directory at that name, cleanup must leave the replacement and intent.
/// Startup cannot find the moved original and must not promise to reclaim it.
///
/// Mutation oracle: removing `open_owned_delete_quarantine` from the shared
/// purge deletes the replacement and fires the replacement-survival assertion.
#[tokio::test]
async fn staged_cleanup_purge_refuses_replacement_without_promising_reclamation() {
    let env = TestEnv::new();
    let core = env.core().await;
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game Library root");
    let missing_source = env._tmp.path().join("missing-staged-purge-source");

    let mut cleaning = probe(&env)
        .pausing_at(gmm_lib::core::crash_points::STAGED_CLEANUP_BEFORE_QUARANTINE_PURGE)
        .op([
            "adopt",
            "--from",
            &missing_source.display().to_string(),
            "--name",
            "Swap During Staged Purge",
        ])
        .spawn();
    cleaning.wait_for_pause(gmm_lib::core::crash_points::STAGED_CLEANUP_BEFORE_QUARANTINE_PURGE);

    let (quarantine, intent) = single_delete_quarantine(&root, "paused staged cleanup");
    let proven = root.join("staged-proven-quarantine-held-aside");
    std::fs::rename(&quarantine, &proven).expect("move proven staged quarantine aside");
    std::fs::create_dir(&quarantine).expect("create staged-cleanup replacement");
    std::fs::write(quarantine.join("replacement-marker"), b"staged replacement")
        .expect("write staged-cleanup replacement marker");

    cleaning.resume();
    let adopted = cleaning.wait_for_outcome();
    assert!(
        !adopted.ok,
        "the adopt with a missing source must still report its original failure",
    );
    assert!(
        quarantine.is_dir(),
        "staged cleanup purge deleted the replacement placed at its proven quarantine pathname",
    );
    assert!(
        intent.is_file(),
        "staged cleanup purge removed the intent after refusing the replacement",
    );

    probe(&env)
        .op(["migrate"])
        .run()
        .expect_ok("startup while the staged quarantine identity still mismatches");
    assert!(
        quarantine.join("replacement-marker").is_file() && intent.is_file() && proven.is_dir(),
        "startup must preserve the mismatch but cannot reclaim the moved staged bytes",
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

/// Explicit Library delete has committed its durable quarantine and released
/// the identity proof before recursive purge. A replacement at the reserved
/// pathname must survive and the accepted delete must still report success.
/// Startup preserves the mismatch but cannot locate or reclaim the moved
/// original, so this outcome must not be described as retryable.
///
/// Mutation oracle: removing `open_owned_delete_quarantine` from the shared
/// purge deletes the replacement and fires the replacement-survival assertion.
/// Returning the identity mismatch as a hard command error fires the
/// successful-delete assertion.
#[tokio::test]
async fn explicit_delete_purge_refuses_replacement_without_promising_reclamation() {
    let env = TestEnv::new();
    let core = env.core().await;
    let root = core
        .resolved_library_root_for(GameCode::Gimi)
        .await
        .expect("game Library root");
    std::fs::create_dir_all(&root).expect("create game Library root");
    let orphan = root.join(Ulid::new().to_string());
    std::fs::create_dir(&orphan).expect("orphan");
    std::fs::write(orphan.join("proven-marker"), b"explicit proven bytes")
        .expect("write explicit-delete marker");

    let mut deleting = probe(&env)
        .pausing_at(gmm_lib::core::crash_points::DELETE_BEFORE_QUARANTINE_PURGE)
        .op([
            "delete-library-dir",
            "--path",
            &orphan.display().to_string(),
        ])
        .spawn();
    deleting.wait_for_pause(gmm_lib::core::crash_points::DELETE_BEFORE_QUARANTINE_PURGE);

    let (quarantine, intent) = single_delete_quarantine(&root, "paused explicit delete");
    let proven = root.join("explicit-proven-quarantine-held-aside");
    std::fs::rename(&quarantine, &proven).expect("move proven explicit quarantine aside");
    std::fs::create_dir(&quarantine).expect("create explicit-delete replacement");
    std::fs::write(
        quarantine.join("replacement-marker"),
        b"explicit replacement",
    )
    .expect("write explicit-delete replacement marker");

    deleting.resume();
    let deleted = deleting.wait_for_outcome();
    assert!(
        deleted.ok,
        "the Library delete was already committed and must stay successful when byte reclamation fails: {}",
        deleted.error,
    );
    assert!(
        quarantine.join("replacement-marker").is_file(),
        "explicit Library delete purge removed the replacement after its identity changed",
    );
    assert!(
        intent.is_file(),
        "explicit Library delete purge removed the intent after refusing the replacement",
    );

    probe(&env)
        .op(["migrate"])
        .run()
        .expect_ok("startup while the explicit quarantine identity still mismatches");
    assert!(
        quarantine.join("replacement-marker").is_file()
            && intent.is_file()
            && proven.join("proven-marker").is_file(),
        "startup must preserve the mismatch but cannot reclaim the moved explicit-delete bytes",
    );
}

fn single_delete_quarantine(root: &Path, context: &str) -> (PathBuf, PathBuf) {
    let quarantines: Vec<_> = std::fs::read_dir(root)
        .unwrap_or_else(|error| panic!("read Library root during {context}: {error}"))
        .filter_map(std::result::Result::ok)
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_dir())
                && entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".gmm-delete-")
        })
        .map(|entry| entry.path())
        .collect();
    assert_eq!(
        quarantines.len(),
        1,
        "{context} must expose exactly one delete quarantine, got {quarantines:?}",
    );
    let quarantine = quarantines.into_iter().next().expect("one quarantine");
    let intent = quarantine.with_file_name(format!(
        "{}.intent",
        quarantine
            .file_name()
            .expect("quarantine name")
            .to_string_lossy()
    ));
    assert!(
        intent.is_file(),
        "{context} must retain the durable delete intent",
    );
    (quarantine, intent)
}

async fn assert_set_enabled_excludes_relocation(
    start_enabled: bool,
    requested_enabled: bool,
    junction_pause_point: &'static str,
    display_name: &str,
) {
    for pause_point in [
        gmm_lib::core::crash_points::SET_ENABLED_AFTER_REINSTALL_GUARD,
        gmm_lib::core::crash_points::SET_ENABLED_AFTER_WITNESS_COMMIT,
        junction_pause_point,
        gmm_lib::core::crash_points::SET_ENABLED_AFTER_DB_UPDATE,
    ] {
        assert_set_enabled_excludes_relocation_at(
            start_enabled,
            requested_enabled,
            pause_point,
            display_name,
        )
        .await;
    }
}

/// The reinstall-quarantine guard for a Variant switch must run after the
/// Library writer fence is acquired. Otherwise recovery can quarantine the Mod
/// after the guard passes but before the Variant and Junction transition.
///
/// Mutation oracle: moving the guard and its adjacent crash point above
/// `begin_library_mutation` lets the relocation enter while this probe is
/// paused, and the named lock-refusal assertion fails.
#[tokio::test]
async fn active_variant_quarantine_guard_is_inside_library_writer_fence() {
    let env = TestEnv::new();
    let core = env.core().await;
    let source = env._tmp.path().join("variant-quarantine-guard");
    for name in ["Red", "Blue"] {
        let variant = source.join(name);
        std::fs::create_dir_all(&variant).expect("create Variant fixture");
        std::fs::write(variant.join("merged.ini"), format!("; {name}\n"))
            .expect("write Variant fixture");
    }
    let imported = core
        .adopt_folder(GameCode::Gimi, &source, "Guarded Variant")
        .await
        .expect("adopt Variant Mod");
    let variants = core
        .list_variants(&imported.id)
        .await
        .expect("list Variants");
    let active = core
        .active_variant_id(&imported.id)
        .await
        .expect("read active Variant");
    let selected = variants
        .iter()
        .find(|variant| Some(variant.id.as_str()) != active.as_deref())
        .expect("a different Variant");

    let mut switching = probe(&env)
        .pausing_at(gmm_lib::core::crash_points::SET_ACTIVE_VARIANT_AFTER_REINSTALL_GUARD)
        .op([
            "set-active-variant",
            "--mod-id",
            &imported.id,
            "--variant-id",
            &selected.id,
            "--mods-dir",
            &env.game_mods.display().to_string(),
        ])
        .spawn();
    switching.wait_for_pause(gmm_lib::core::crash_points::SET_ACTIVE_VARIANT_AFTER_REINSTALL_GUARD);

    let relocated_root = env._tmp.path().join("relocated-during-variant-guard");
    let relocation = probe(&env)
        .op([
            "set-library-path",
            "--path",
            &relocated_root.display().to_string(),
        ])
        .run();
    relocation.expect_refused(
        "relocation while set_active_variant paused after its quarantine guard",
        "database is locked",
    );

    switching.resume();
    switching
        .wait_for_outcome()
        .expect_ok("set_active_variant after the refused relocation");
    assert_eq!(
        core.active_variant_id(&imported.id)
            .await
            .expect("read committed Variant")
            .as_deref(),
        Some(selected.id.as_str()),
        "the fenced Variant transition must commit after it resumes",
    );
}

async fn assert_set_enabled_excludes_relocation_at(
    start_enabled: bool,
    requested_enabled: bool,
    pause_point: &'static str,
    display_name: &str,
) {
    let env = TestEnv::new();
    let core = env.core().await;
    core.set_game_install_path(
        GameCode::Gimi,
        env.game_mods.parent().expect("game install path"),
    )
    .await
    .expect("record game install path");
    let m = env.seed_mod(&core, display_name).await;
    if start_enabled {
        core.set_enabled(&m.id, true, &env.game_mods)
            .await
            .expect("set starting enabled state");
    }

    let mods_dir = env.game_mods.display().to_string();
    let enabled = if requested_enabled { "1" } else { "0" };
    let mut toggling = probe(&env)
        .pausing_at(pause_point)
        .op([
            "set-enabled",
            "--mod-id",
            &m.id,
            "--enabled",
            enabled,
            "--mods-dir",
            &mods_dir,
        ])
        .spawn();
    toggling.wait_for_pause(pause_point);

    // The toggle is paused after either its Junction mutation or its flag
    // update. Its writer fence must exclude relocation at both boundaries
    // until the complete deployment-state transition commits.
    let relocated_root = env._tmp.path().join(format!("relocated-{enabled}"));
    let relocation = probe(&env)
        .op([
            "set-library-path",
            "--path",
            &relocated_root.display().to_string(),
        ])
        .run();
    let expected_refusal =
        if pause_point == gmm_lib::core::crash_points::SET_ENABLED_AFTER_WITNESS_COMMIT {
            "interrupted enable/disable transition"
        } else {
            "database is locked"
        };
    relocation.expect_refused(
        &format!("relocation while set_enabled paused at {pause_point}"),
        expected_refusal,
    );

    toggling.resume();
    toggling
        .wait_for_outcome()
        .expect_ok("set_enabled while relocation was excluded");

    let listed_after_toggle = core.list_mods(GameCode::Gimi).await.expect("list Mods");
    let row_after_toggle = listed_after_toggle
        .iter()
        .find(|candidate| candidate.id == m.id)
        .expect("toggled Mod row before relocation retry");
    assert_eq!(
        row_after_toggle.enabled, requested_enabled,
        "set_enabled must commit the requested enabled flag before relocation retries",
    );
    let junction_loads_mod_after_toggle = env
        .game_mods
        .join(display_name)
        .join("merged.ini")
        .is_file();
    assert_eq!(
        junction_loads_mod_after_toggle, requested_enabled,
        "set_enabled itself left the enabled flag and Junction inconsistent before the \
         relocation retry: requested enabled={requested_enabled}, first relocation={relocation:?}",
    );

    let relocation_after_resume = probe(&env)
        .op([
            "set-library-path",
            "--path",
            &relocated_root.display().to_string(),
        ])
        .run();
    relocation_after_resume.expect_ok("relocation after set_enabled released its fence");

    let listed = core.list_mods(GameCode::Gimi).await.expect("list Mods");
    let row = listed
        .iter()
        .find(|candidate| candidate.id == m.id)
        .expect("toggled Mod row");
    assert_eq!(
        row.enabled, requested_enabled,
        "set_enabled must commit the requested enabled flag",
    );
    let junction_loads_mod = env
        .game_mods
        .join(display_name)
        .join("merged.ini")
        .is_file();
    assert_eq!(
        junction_loads_mod, requested_enabled,
        "set_enabled and a fenced Library relocation left the enabled flag and Junction \
         inconsistent: requested enabled={requested_enabled}, first relocation={relocation:?}, \
         relocation after resume={relocation_after_resume:?}",
    );
}

/// A disable paused after Junction removal must still own the writer fence.
/// Mutation oracle: releasing the fence before the Junction operation lets
/// relocation recreate the Junction from stale `enabled = 1`; the explicit
/// Junction/flag consistency assertion then fails after disable commits zero.
#[tokio::test]
async fn disable_excludes_relocation_until_junction_and_flag_agree() {
    assert_set_enabled_excludes_relocation(
        true,
        false,
        gmm_lib::core::crash_points::SET_ENABLED_AFTER_JUNCTION_REMOVE,
        "Fenced Disable",
    )
    .await;
}

/// An enable paused after Junction creation must still own the writer fence.
/// Mutation oracle: releasing the fence before the Junction operation lets
/// relocation act from stale `enabled = 0` and strand the row without the
/// Junction after enable commits one.
#[tokio::test]
async fn enable_excludes_relocation_until_junction_and_flag_agree() {
    assert_set_enabled_excludes_relocation(
        false,
        true,
        gmm_lib::core::crash_points::SET_ENABLED_AFTER_JUNCTION_CREATE,
        "Fenced Enable",
    )
    .await;
}

/// The manual escape hatch is only for an owner whose identity GMM cannot
/// establish. It must not cancel a transition still owned by the exact live
/// producer process.
///
/// Mutation oracle: removing the live-owner refusal makes the retirement call
/// succeed and fires the named safety assertion.
#[tokio::test]
async fn retire_refuses_transition_still_owned_by_original_process() {
    let env = TestEnv::new();
    let core = env.core().await;
    let m = env.seed_mod(&core, "Live Transition Owner").await;
    let pause = gmm_lib::core::crash_points::SET_ENABLED_AFTER_WITNESS_COMMIT;
    let mut enabling = probe(&env)
        .pausing_at(pause)
        .op([
            "set-enabled",
            "--mod-id",
            &m.id,
            "--enabled",
            "1",
            "--mods-dir",
            &env.game_mods.display().to_string(),
        ])
        .spawn();
    enabling.wait_for_pause(pause);

    let retirement = core.retire_interrupted_enabled_transition(&m.id).await;
    assert!(
        matches!(retirement, Err(Error::EnabledTransitionStillOwned)),
        "the recovery action must not cancel a transition still owned by its original process: {retirement:?}",
    );

    enabling.resume();
    enabling
        .wait_for_outcome()
        .expect_ok("live owner after refused retirement");
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
    let (zip_s, game_s) = (zip.display().to_string(), game_dir.display().to_string());

    // Sanity: with no session the install is allowed. Otherwise the
    // refusal below could be caused by anything at all.
    probe(&env)
        .op(["install-importer", "--zip", &zip_s, "--game-dir", &game_s])
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
        .op(["install-importer", "--zip", &zip_s, "--game-dir", &game_s])
        .run()
        .expect_refused(
            "installing an importer during another process's session",
            "running",
        );

    core.end_session().await.expect("end session");
    probe(&env)
        .op(["install-importer", "--zip", &zip_s, "--game-dir", &game_s])
        .run()
        .expect_ok("installing an importer once the session ended");
}

#[tokio::test]
async fn probe_importer_abort_leaves_a_witness_that_startup_replays() {
    let env = TestEnv::new();
    let game_dir = env.game_mods.parent().expect("game dir").to_path_buf();
    let zip = env._tmp.path().join("importer.zip");
    build_importer_zip(&zip);
    std::fs::create_dir_all(game_dir.join("Core")).expect("create old Core");
    std::fs::write(game_dir.join("d3dx.ini"), b"old d3dx bytes").expect("write old d3dx");
    std::fs::write(game_dir.join("Core/original.ini"), b"old Core bytes").expect("write old Core");
    let zip_s = zip.display().to_string();
    let game_s = game_dir.display().to_string();

    let mut interrupted = probe(&env)
        .crashing_at(gmm_lib::core::importer::BACKUP_AFTER_ENTRY_TEST_SEAM)
        .op(["install-importer", "--zip", &zip_s, "--game-dir", &game_s])
        .spawn();
    interrupted.wait_for_crash();

    let pool = sqlx::SqlitePool::connect(&env.db_url)
        .await
        .expect("inspect durable probe state");
    let witnesses: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM importer_evacuations")
        .fetch_one(&pool)
        .await
        .expect("count probe evacuation witnesses");
    assert_eq!(
        witnesses, 1,
        "the production-shaped probe must commit one durable importer evacuation witness before moving bytes",
    );
    pool.close().await;

    let recovered = env.core().await;
    assert_eq!(
        std::fs::read(game_dir.join("d3dx.ini")).expect("read restored importer entry"),
        b"old d3dx bytes",
        "startup must replay the probe-created witness and restore the evacuated entry",
    );
    assert!(
        recovered
            .importer_evacuation_recovery(GameCode::Gimi)
            .await
            .expect("read recovery state")
            .is_none(),
        "startup must retire the probe-created witness after restoring the importer",
    );
}

/// A content comparison on two live directory trees can never authorize a
/// delete: the live entry can change after its snapshot but before the backup
/// snapshot. Recovery may retire the witness after the stale snapshots compare
/// equal, but the backup must remain reachable by construction.
#[tokio::test]
async fn importer_recovery_retains_the_backup_when_the_live_entry_changes_between_snapshots() {
    let env = TestEnv::new();
    let game_dir = env.game_mods.parent().expect("game dir").to_path_buf();
    let zip = env._tmp.path().join("importer.zip");
    build_importer_zip(&zip);
    std::fs::create_dir_all(game_dir.join("Core")).expect("create old Core");
    std::fs::write(game_dir.join("d3dx.ini"), b"old d3dx bytes").expect("write old d3dx");
    std::fs::write(game_dir.join("Core/original.ini"), b"old Core bytes").expect("write old Core");
    let zip_s = zip.display().to_string();
    let game_s = game_dir.display().to_string();

    let mut interrupted = probe(&env)
        .crashing_at(gmm_lib::core::importer::BACKUP_AFTER_ENTRY_TEST_SEAM)
        .op(["install-importer", "--zip", &zip_s, "--game-dir", &game_s])
        .spawn();
    interrupted.wait_for_crash();

    let pool = sqlx::SqlitePool::connect(&env.db_url)
        .await
        .expect("inspect durable probe state");
    let backup_path: String =
        sqlx::query_scalar("SELECT backup_path FROM importer_evacuations WHERE game_code = 'gimi'")
            .fetch_one(&pool)
            .await
            .expect("load recorded backup path");
    pool.close().await;
    let backup = PathBuf::from(backup_path);
    assert_eq!(
        std::fs::read(backup.join("d3dx.ini")).expect("read published backup"),
        b"old d3dx bytes",
        "the fixture must start with the unique old entry in the published backup",
    );

    std::fs::write(game_dir.join("d3dx.ini"), b"old d3dx bytes")
        .expect("create an initially identical repaired entry");
    let pause = gmm_lib::core::importer::RECOVERY_BETWEEN_ENTRY_SNAPSHOTS_TEST_SEAM;
    let mut recovery = probe(&env).pausing_at(pause).op(["startup"]).spawn();
    recovery.wait_for_pause(pause);

    std::fs::write(game_dir.join("d3dx.ini"), b"changed during recovery")
        .expect("change the live entry between the two snapshots");
    recovery.resume();
    recovery
        .wait_for_outcome()
        .expect_ok("startup after the non-destructive comparison");

    assert_eq!(
        std::fs::read(game_dir.join("d3dx.ini")).expect("read concurrently repaired entry"),
        b"changed during recovery",
        "recovery must preserve the live importer entry changed after comparison",
    );
    assert_eq!(
        std::fs::read(backup.join("d3dx.ini")).expect("read retained unique backup"),
        b"old d3dx bytes",
        "recovery must retain the backup even when stale snapshots compare equal",
    );
    let pool = sqlx::SqlitePool::connect(&env.db_url)
        .await
        .expect("inspect completed recovery state");
    let recovery_state: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT recovery_error, recovery_action FROM importer_evacuations WHERE game_code = 'gimi'",
    )
    .fetch_optional(&pool)
    .await
    .expect("inspect importer evacuation witness");
    assert!(
        recovery_state.is_none(),
        "both locations remain usable, so recovery may retire the durable witness without deleting either entry: {recovery_state:?}",
    );
    pool.close().await;
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

/// A launch claim commits before the child exists. Pause on that exact
/// production boundary and prove a separately running process cannot enter a
/// fenced Library mutation while `active_session` is still empty.
#[tokio::test]
async fn launch_claim_blocks_library_mutation_before_child_pid_exists() {
    let env = TestEnv::new();
    let core = env.core().await;
    let source = env._tmp.path().join("launch-claim-source");
    std::fs::create_dir_all(&source).expect("launch-claim source");
    std::fs::write(source.join("merged.ini"), b"hash=launch-claim\n")
        .expect("launch-claim fixture");
    let source_s = source.to_string_lossy().to_string();

    let mut launch = probe(&env)
        .pausing_at(crash_points::SESSION_LAUNCH_AFTER_CLAIM)
        .op(["begin-session-launch"])
        .spawn();
    launch.wait_for_pause(crash_points::SESSION_LAUNCH_AFTER_CLAIM);

    assert_eq!(
        core.session_info().await.expect("active session query"),
        None,
        "the barrier must be before active_session, or it cannot prove issue #192",
    );
    probe(&env)
        .op([
            "adopt",
            "--from",
            &source_s,
            "--name",
            "Blocked During Launch",
        ])
        .run()
        .expect_refused(
            "a Library mutation after the launch claim committed",
            "launch is still in progress",
        );

    launch.resume();
    launch
        .wait_for_outcome()
        .expect_ok("abandoning the launch reservation");
    probe(&env)
        .op([
            "adopt",
            "--from",
            &source_s,
            "--name",
            "Allowed After Launch",
        ])
        .run()
        .expect_ok("the Library mutation after the launch was abandoned");
}

/// A process crash after spawn leaves a durable child PID. Startup and the
/// ordinary Library fence preserve that claim while the real child is alive,
/// then retire it without user intervention once the child exits.
#[tokio::test]
async fn crashed_launch_claim_blocks_until_its_recorded_child_exits() {
    let env = TestEnv::new();
    let _core = env.core().await;
    let source = env._tmp.path().join("orphaned-launch-source");
    std::fs::create_dir_all(&source).expect("orphaned-launch source");
    std::fs::write(source.join("merged.ini"), b"hash=orphaned-launch\n")
        .expect("orphaned-launch fixture");
    let source_s = source.to_string_lossy().to_string();

    let mut child = probe(&env).op(["hold-process"]).spawn();
    child
        .wait_for_outcome()
        .expect_ok("starting the stand-in game process");
    let child_pid = child.pid().to_string();

    let mut launch = probe(&env)
        .crashing_at(crash_points::SESSION_LAUNCH_AFTER_CHILD_RECORD)
        .op(["begin-session-launch", "--child-pid", &child_pid])
        .spawn();
    launch.wait_for_crash();

    probe(&env)
        .op(["adopt", "--from", &source_s, "--name", "Blocked By Orphan"])
        .run()
        .expect_refused(
            "a Library mutation while the orphaned launch child is alive",
            "cannot determine whether a game",
        );

    child.kill();
    probe(&env)
        .op([
            "adopt",
            "--from",
            &source_s,
            "--name",
            "Allowed After Child Exit",
        ])
        .run()
        .expect_ok("the first Library mutation after the orphaned child exited");
}
