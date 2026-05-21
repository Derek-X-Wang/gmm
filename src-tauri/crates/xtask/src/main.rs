//! `cargo xtask <subcommand>` — project-internal task runner.
//!
//! Subcommands:
//! - `test-loader` — smoke-test the `gmm-loader` FFI binding against
//!   `vendor/3dmloader/3dmloader.dll`. Requires Windows.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let cmd = match args.next() {
        Some(c) => c,
        None => {
            eprintln!("usage: cargo xtask <subcommand>");
            eprintln!("subcommands:");
            eprintln!("  test-loader   smoke-test the 3dmloader.dll FFI binding (Windows only)");
            return ExitCode::FAILURE;
        }
    };

    match cmd.as_str() {
        "test-loader" => match test_loader::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(msg) => {
                eprintln!("test-loader: {msg}");
                ExitCode::FAILURE
            }
        },
        other => {
            eprintln!("unknown subcommand: {other}");
            ExitCode::FAILURE
        }
    }
}

fn workspace_root() -> PathBuf {
    // The xtask binary lives at <workspace>/target/<profile>/xtask. Walk
    // up to the workspace root. We rely on Cargo's CARGO_MANIFEST_DIR
    // being set when invoked through cargo, and fall back to CWD
    // traversal otherwise.
    if let Ok(manifest) = env::var("CARGO_MANIFEST_DIR") {
        // CARGO_MANIFEST_DIR points at the xtask crate; go up two levels.
        let p = PathBuf::from(manifest);
        if let Some(parent) = p.parent().and_then(|p| p.parent()) {
            return parent.to_path_buf();
        }
    }
    env::current_dir().expect("cwd")
}

mod test_loader {
    use super::workspace_root;

    pub fn run() -> Result<(), String> {
        let ws = workspace_root();
        let vendor_dll = ws
            .parent()
            .unwrap_or(&ws)
            .join("vendor/3dmloader/3dmloader.dll");
        if !vendor_dll.exists() {
            return Err(format!(
                "vendor binary not found at {vendor_dll:?} — see vendor/3dmloader/README.md"
            ));
        }

        #[cfg(not(windows))]
        {
            let _ = ws;
            println!("test-loader: skipped (host is not Windows)");
            println!("test-loader: vendor binary located at {vendor_dll:?}");
            Ok(())
        }

        #[cfg(windows)]
        windows_impl::run(&ws, &vendor_dll)
    }

    #[cfg(windows)]
    mod windows_impl {
        use std::path::Path;
        use std::process::{Child, Command, Stdio};
        use std::time::{Duration, Instant};

        use gmm_loader::Loader;

        // 3dmloader keys WaitForInjection off a substring match against the
        // target process name. "victim" matches victim.exe regardless of
        // path.
        const TARGET_PROCESS: &str = "victim";
        const WAIT_TIMEOUT_SECS: i32 = 15;

        pub fn run(workspace: &Path, vendor_dll: &Path) -> Result<(), String> {
            let target_dir = workspace.join("target");
            // Prefer release if it exists, else debug.
            let profile = if target_dir.join("release/victim.exe").exists() {
                "release"
            } else {
                "debug"
            };

            let victim_exe = target_dir.join(profile).join("victim.exe");
            let noop_dll = target_dir.join(profile).join("noop_dll.dll");
            if !victim_exe.exists() || !noop_dll.exists() {
                return Err(format!(
                    "victim.exe or noop_dll.dll not built. Run `cargo build -p victim -p noop_dll` first ({victim_exe:?}, {noop_dll:?})"
                ));
            }

            // Load 3dmloader.dll
            let loader = Loader::load(vendor_dll).map_err(|e| format!("load loader: {e}"))?;

            // Install the hook before spawning victim — the CBT hook must
            // be in place when victim's window is created.
            let session = loader
                .hook(&noop_dll)
                .map_err(|e| format!("install hook: {e}"))?;

            // Spawn victim.
            let mut victim: Child = Command::new(&victim_exe)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("spawn victim: {e}"))?;

            // Wait for injection.
            let inject_result = session.wait_for_injection(TARGET_PROCESS, WAIT_TIMEOUT_SECS);
            // Drop the hook session regardless (covers panic path too).
            drop(session);

            inject_result.map_err(|e| {
                let _ = victim.kill();
                format!("wait_for_injection: {e}")
            })?;

            // Tell victim to exit by killing it; in a richer harness we'd
            // post WM_CLOSE, but kill() is enough to prove the hook → inject
            // round-trip works and the unhook tore down cleanly.
            let start = Instant::now();
            let exit_status = loop {
                if let Some(status) = victim
                    .try_wait()
                    .map_err(|e| format!("try_wait victim: {e}"))?
                {
                    break status;
                }
                if start.elapsed() > Duration::from_secs(45) {
                    let _ = victim.kill();
                    break victim.wait().map_err(|e| format!("wait victim: {e}"))?;
                }
                std::thread::sleep(Duration::from_millis(200));
            };

            if !exit_status.success() && exit_status.code() != Some(0) {
                return Err(format!("victim exited non-zero: {exit_status:?}"));
            }

            println!("test-loader: ok (vendor {})", vendor_dll.display());
            Ok(())
        }
    }
}
