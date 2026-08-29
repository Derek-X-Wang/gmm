//! Test-only GMM stand-in. Not shipped: `tauri build` bundles only the
//! `gmm` binary, and nothing in the app depends on this crate.
//!
//! One invocation performs exactly one `Core` operation against a given
//! `gmm.db` and Library, then prints a single JSON line and exits. That
//! is the whole design: `tests/concurrency.rs` (issue #58) needs two
//! *operating-system processes* contending for one SQLite file and one
//! Library, which no amount of Tokio concurrency inside one test binary
//! can simulate. SQLite's locking is per-process; so are file handles.
//!
//! ```text
//! concurrency-probe --data-dir D --db URL --library L \
//!     [--take-lock] [--at EPOCH_MILLIS] <op> [op args…]
//! ```
//!
//! `--take-lock` opts into the single-instance policy. It is opt-in
//! rather than default because most of the suite deliberately bypasses
//! the gate to test the layer underneath it.
//!
//! `--at` is a wall-clock rendezvous: the probe spins until that instant
//! before touching anything, so two probes started sequentially still
//! collide. Both sides being a few milliseconds out only weakens the
//! test; it cannot make it flaky, because every assertion is on the
//! final state rather than on who won.
//!
//! Exit status: 0 when the operation succeeded, 2 when it failed, 1 on a
//! usage error, and 3 when a pause event cannot be written or flushed. The
//! JSON line is printed on operation success or failure — a probe that was
//! *correctly refused* and a probe that failed to start must not look alike
//! to the test.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gmm_lib::core::{instance_lock, Core, GameCode, SessionInfo};

fn main() {
    let args = match Args::parse() {
        Ok(a) => a,
        Err(msg) => {
            report(false, &msg);
            std::process::exit(1);
        }
    };

    // Held for the process lifetime when requested. Binding to `_` would
    // release it immediately and silently defeat the test.
    let _lock = if args.take_lock {
        match instance_lock::acquire(&args.data_dir) {
            Ok(lock) => Some(lock),
            Err(e) => {
                report(false, &e.to_string());
                std::process::exit(2);
            }
        }
    } else {
        None
    };

    // `hold-lock` reports before it blocks: the whole point is for the
    // parent to know the lock is held before it races against it.
    if args.op == "hold-lock" {
        report(true, "");
        let ms: u64 = args.get("--ms").and_then(|v| v.parse().ok()).unwrap_or(0);
        std::thread::sleep(Duration::from_millis(ms));
        return;
    }

    // A deterministic live PID for launch-claim recovery tests. Report only
    // after the process is ready, then let the parent close stdin or kill us;
    // no timing guess decides whether the witness is alive.
    if args.op == "hold-process" {
        report(true, "");
        let mut release = String::new();
        let _ = std::io::stdin().read_line(&mut release);
        return;
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build runtime");

    match rt.block_on(run(&args)) {
        Ok(()) => {
            report(true, "");
        }
        Err(msg) => {
            report(false, &msg);
            std::process::exit(2);
        }
    }
}

async fn run(args: &Args) -> Result<(), String> {
    // The rendezvous sits as close to the operation as possible so the
    // race is not diluted by pool-open jitter: `Core::new` runs
    // migrations and warms a connection pool, tens of milliseconds that
    // vary per process.
    //
    // Measured either way, a concurrent enable/disable tears roughly 5%
    // of rounds — moving the wait here did not obviously improve on
    // waiting before `Core::new`, so do not read this placement as a
    // tuned number. It is the principled spot, not a fast one. The
    // deterministic coverage of both torn directions lives in
    // `tests/reconcile.rs`; this race is evidence that the tear is real,
    // not the regression guard for it.
    //
    // `migrate` is the exception: opening the Core *is* the operation
    // under test there, so its rendezvous has to stay in front.
    let migrating = args.op == "migrate";
    if migrating {
        wait_until(args.at);
    }

    // Failure injection (issue #59). `abort` rather than `exit`: no
    // unwinding, no destructors, no buffered writes flushed — the point
    // is to model a process that stopped existing, not one that shut
    // down badly. sqlx has no chance to close the pool cleanly, which is
    // exactly the situation the recovery path has to survive.
    let crash_at = args.get("--crash-at").cloned();
    let pause_at = args.get("--pause-at").cloned();
    let crash_hook: Option<gmm_lib::core::CrashHook> = (crash_at.is_some() || pause_at.is_some())
        .then(|| {
            std::sync::Arc::new(move |reached: &str| {
                if pause_at.as_deref() == Some(reached) {
                    let line = serde_json::json!({ "pausedAt": reached });
                    let mut stdout = std::io::stdout();
                    if let Err(error) = writeln!(stdout, "{line}").and_then(|()| stdout.flush()) {
                        eprintln!("failed to report pause at crash point {reached}: {error}");
                        std::process::exit(3);
                    }

                    // The parent closes or writes to stdin to release this exact
                    // crash-point rendezvous. This is deliberately event-driven:
                    // concurrency tests must not guess that the child reached the
                    // vulnerable window after an arbitrary sleep.
                    let mut release = String::new();
                    let _ = std::io::stdin().read_line(&mut release);
                }
                if crash_at.as_deref() == Some(reached) {
                    std::process::abort();
                }
            }) as gmm_lib::core::CrashHook
        });
    let startup = args.op == "startup";
    let core = if startup {
        match crash_hook {
            Some(hook) => Core::new_with_crash_hook(args.library.clone(), &args.db_url, hook).await,
            None => Core::new(args.library.clone(), &args.db_url).await,
        }
        .map_err(|e| e.to_string())?
    } else {
        let core = Core::new(args.library.clone(), &args.db_url)
            .await
            .map_err(|e| e.to_string())?;
        match crash_hook {
            Some(hook) => core.with_crash_hook(hook),
            None => core,
        }
    };

    if !migrating {
        wait_until(args.at);
    }

    if args.ready_before_op {
        let line = serde_json::json!({ "readyBeforeOp": true });
        let mut stdout = std::io::stdout();
        writeln!(stdout, "{line}")
            .and_then(|()| stdout.flush())
            .map_err(|error| format!("failed to report pre-operation readiness: {error}"))?;
        let mut release = String::new();
        let _ = std::io::stdin().read_line(&mut release);
    }

    match args.op.as_str() {
        "migrate" => Ok(()),
        "startup" => Ok(()),

        "adopt" => {
            let from = args.req("--from")?;
            let name = args.req("--name")?;
            core.adopt_folder(args.game()?, &PathBuf::from(from), &name)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }

        "set-enabled" => {
            let id = args.req("--mod-id")?;
            let enabled = args.req("--enabled")? == "1";
            let mods_dir = PathBuf::from(args.req("--mods-dir")?);
            core.set_enabled(&id, enabled, &mods_dir)
                .await
                .map_err(|e| e.to_string())
        }

        "set-active-variant" => {
            let mod_id = args.req("--mod-id")?;
            let variant_id = args.req("--variant-id")?;
            let mods_dir = PathBuf::from(args.req("--mods-dir")?);
            core.set_active_variant(&mod_id, &variant_id, &mods_dir)
                .await
                .map_err(|e| e.to_string())
        }

        "import-zip" => {
            let zip = PathBuf::from(args.req("--zip")?);
            let name = args.req("--name")?;
            core.import_zip(
                args.game()?,
                &zip,
                &name,
                gmm_lib::core::ImportZipOptions::default(),
            )
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
        }

        "import-gamebanana" => {
            let id = args.req("--id")?;
            let api_base = args.req("--api-base")?;
            core.import_gamebanana_with_endpoints(
                args.game()?,
                &id,
                &gmm_lib::core::gamebanana::Endpoints { api_base },
            )
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
        }

        "reinstall" => {
            let mod_id = args.req("--mod-id")?;
            let api_base = args.req("--api-base")?;
            core.reinstall_gamebanana_mod_with_endpoints(
                &mod_id,
                &gmm_lib::core::gamebanana::Endpoints { api_base },
            )
            .await
            .map_err(|e| e.to_string())
        }

        "retry-reinstall-recovery" => {
            let mod_id = args.req("--mod-id")?;
            core.retry_reinstall_recovery(&mod_id)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }

        "withdraw-quarantined-reinstall-junction" => {
            let token = args.req("--token")?;
            let link = PathBuf::from(args.req("--link")?);
            core.withdraw_quarantined_reinstall_junction(&token, Some(&link))
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }

        "audit" => {
            let report = core
                .audit_library(args.game()?)
                .await
                .map_err(|e| e.to_string())?;
            if report.unreferenced.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "audit reported directories owned by the concurrent producer: {:?}",
                    report.unreferenced
                ))
            }
        }

        // Recovering an unreferenced Library directory (#72). Only the
        // fallback path — a directory whose name is not a usable ULID —
        // has a filesystem step to be interrupted; the ULID case moves
        // nothing at all.
        "recover" => {
            let path = PathBuf::from(args.req("--path")?);
            let name = args.req("--name")?;
            core.recover_unreferenced_library_dir(args.game()?, &path, &name)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }

        "delete-library-dir" => {
            let path = PathBuf::from(args.req("--path")?);
            core.delete_unreferenced_library_dir(args.game()?, &path)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }

        "set-library-path" => {
            let path = PathBuf::from(args.req("--path")?);
            core.set_library_path_for_game(args.game()?, Some(&path))
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }

        "reconcile" => {
            let mods_dir = PathBuf::from(args.req("--mods-dir")?);
            core.reconcile_junctions(args.game()?, &mods_dir)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }

        "rebuild" => {
            let mods_dir = PathBuf::from(args.req("--mods-dir")?);
            core.rebuild_junctions(args.game()?, &mods_dir)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }

        "start-session" => {
            let pid: u32 = args
                .req("--pid")?
                .parse()
                .map_err(|_| "--pid must be a number".to_string())?;
            core.start_session(&SessionInfo {
                game: args.game()?,
                pid,
                started_at: chrono::Utc::now(),
            })
            .await
            .map_err(|e| e.to_string())
        }

        "begin-session-launch" => {
            let claim = core
                .begin_session_launch(args.game()?)
                .await
                .map_err(|e| e.to_string())?;
            if let Some(child_pid) = args.get("--child-pid") {
                let child_pid = child_pid
                    .parse()
                    .map_err(|_| "--child-pid must be a number".to_string())?;
                core.record_session_launch_child(&claim, child_pid)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            core.abandon_session_launch(&claim)
                .await
                .map_err(|e| e.to_string())
        }

        // Mirrors what the `install_importer` command does after the
        // download: refuse during a Game Session, then lay the importer
        // into the game directory. The network half is not interesting
        // here — the contention is over the game directory and the
        // session row.
        "install-importer" => {
            let zip = PathBuf::from(args.req("--zip")?);
            let game_dir = PathBuf::from(args.req("--game-dir")?);
            let game = args.game()?;
            core.set_game_install_path(game, &game_dir)
                .await
                .map_err(|e| e.to_string())?;
            core.install_importer_from_local_zip(game, &zip, "GMM.exe")
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }

        other => Err(format!("unknown op {other:?}")),
    }
}

/// Spin until the rendezvous instant. A short sleep loop rather than a
/// busy wait so two probes on a one-core CI runner still both get there.
fn wait_until(at: Option<u128>) {
    let Some(at) = at else { return };
    loop {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis();
        if now >= at {
            return;
        }
        let remaining = (at - now) as u64;
        std::thread::sleep(Duration::from_millis(remaining.min(2)));
    }
}

fn report(ok: bool, error: &str) {
    let line = serde_json::json!({ "ok": ok, "error": error });
    let mut stdout = std::io::stdout();
    // Stdout is a pipe here, so it is block-buffered; the parent blocks
    // on this line, so an unflushed write is a deadlock.
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}

struct Args {
    data_dir: PathBuf,
    db_url: String,
    library: PathBuf,
    take_lock: bool,
    ready_before_op: bool,
    at: Option<u128>,
    op: String,
    flags: HashMap<String, String>,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let argv: Vec<String> = std::env::args().skip(1).collect();
        let mut flags = HashMap::new();
        let mut op = None;

        let mut i = 0;
        while i < argv.len() {
            let a = &argv[i];
            if let Some(name) = a.strip_prefix("--") {
                // These process-control flags carry no value.
                if matches!(name, "take-lock" | "ready-before-op") {
                    flags.insert(a.clone(), "1".to_string());
                    i += 1;
                    continue;
                }
                let value = argv
                    .get(i + 1)
                    .ok_or_else(|| format!("{a} needs a value"))?
                    .clone();
                flags.insert(a.clone(), value);
                i += 2;
            } else {
                if op.is_some() {
                    return Err(format!("unexpected positional argument {a:?}"));
                }
                op = Some(a.clone());
                i += 1;
            }
        }

        let take = |k: &str| -> Result<String, String> {
            flags
                .get(k)
                .cloned()
                .ok_or_else(|| format!("missing required {k}"))
        };

        Ok(Self {
            data_dir: PathBuf::from(take("--data-dir")?),
            db_url: take("--db")?,
            library: PathBuf::from(take("--library")?),
            take_lock: flags.contains_key("--take-lock"),
            ready_before_op: flags.contains_key("--ready-before-op"),
            at: flags.get("--at").and_then(|v| v.parse().ok()),
            op: op.ok_or_else(|| "missing operation".to_string())?,
            flags,
        })
    }

    fn get(&self, key: &str) -> Option<&String> {
        self.flags.get(key)
    }

    fn req(&self, key: &str) -> Result<String, String> {
        self.get(key)
            .cloned()
            .ok_or_else(|| format!("missing required {key}"))
    }

    fn game(&self) -> Result<GameCode, String> {
        let raw = self.get("--game").cloned().unwrap_or_else(|| "gimi".into());
        raw.parse::<GameCode>()
            .map_err(|_| format!("invalid game code {raw:?}"))
    }
}
