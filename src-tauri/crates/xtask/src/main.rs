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
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use std::path::Path;
        use std::process::{Child, Command, Stdio};
        use std::time::{Duration, Instant};

        use gmm_loader::Loader;
        use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

        // 3dmloader does case-insensitive exact-filename match against
        // the running process's image name (per Injector.cpp's
        // wait_for_target / check_for_running_target). Must include the
        // .exe extension.
        const TARGET_PROCESS: &str = "victim.exe";
        const WAIT_TIMEOUT_SECS: i32 = 30;

        type RawInject = unsafe extern "system" fn(u32, *const u16, i32) -> i32;

        struct RawLoader(HMODULE);

        impl Drop for RawLoader {
            fn drop(&mut self) {
                // SAFETY: the handle came from LoadLibraryW and this guard
                // owns the matching FreeLibrary call.
                unsafe {
                    let _ = FreeLibrary(self.0);
                }
            }
        }

        fn wide_nul(value: &OsStr) -> Vec<u16> {
            value.encode_wide().chain(std::iter::once(0)).collect()
        }

        /// Call the pinned DLL's export directly so the smoke test records
        /// what timeout zero means empirically. This deliberately bypasses
        /// the safe wrapper and is used only against a disposable victim.
        fn raw_inject_status(
            loader_dll: &Path,
            pid: u32,
            injected_dll: &Path,
            timeout_secs: i32,
        ) -> Result<i32, String> {
            let loader_wide = wide_nul(loader_dll.as_os_str());
            // SAFETY: loader_wide is NUL-terminated and remains alive for
            // the call.
            let handle = unsafe { LoadLibraryW(loader_wide.as_ptr()) };
            if handle.is_null() {
                return Err(format!(
                    "load raw injector {}: {}",
                    loader_dll.display(),
                    std::io::Error::last_os_error()
                ));
            }
            let handle = RawLoader(handle);
            // SAFETY: handle is a live DLL module and the byte string is
            // NUL-terminated.
            let symbol = unsafe { GetProcAddress(handle.0, c"Inject".as_ptr().cast()) }
                .ok_or_else(|| "raw Inject export missing".to_string())?;
            // SAFETY: the signature is the upstream Inject ABI. The vendored
            // binary is pinned specifically so this smoke test catches drift.
            let inject = unsafe {
                std::mem::transmute::<unsafe extern "system" fn() -> isize, RawInject>(symbol)
            };
            let dll_wide = wide_nul(injected_dll.as_os_str());
            // SAFETY: dll_wide is live for the call, pid names a disposable
            // victim, and timeout_secs is the upstream timeout in seconds.
            Ok(unsafe { inject(pid, dll_wide.as_ptr(), timeout_secs) })
        }

        fn spawn_victim(victim_exe: &Path) -> Result<Child, String> {
            Command::new(victim_exe)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("spawn victim: {e}"))
        }

        fn kill_and_reap(victim: &mut Child) {
            let _ = victim.kill();
            let _ = victim.wait();
        }

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

            // Pin down the failure mode against the exact v0.9.2 binary we
            // ship. A zero timeout returns before the remote LoadLibraryW
            // thread completes and upstream reports status 500.
            let mut zero_timeout_victim = spawn_victim(&victim_exe)?;
            std::thread::sleep(Duration::from_millis(300));
            let zero_status = raw_inject_status(vendor_dll, zero_timeout_victim.id(), &noop_dll, 0);
            kill_and_reap(&mut zero_timeout_victim);
            let zero_status = zero_status?;
            if zero_status != 500 {
                return Err(format!(
                    "Inject(timeout_secs=0) returned {zero_status}, expected pinned v0.9.2 status 500"
                ));
            }

            // The public wrapper must use a real timeout and complete the
            // same injection successfully.
            let mut direct_victim = spawn_victim(&victim_exe)?;
            std::thread::sleep(Duration::from_millis(300));
            let direct_result = loader.inject(direct_victim.id(), &noop_dll);
            kill_and_reap(&mut direct_victim);
            direct_result.map_err(|e| format!("direct inject: {e}"))?;

            // Install the hook before spawning victim — the CBT hook must
            // be in place when victim's window is created.
            let session = loader
                .hook(&noop_dll)
                .map_err(|e| format!("install hook: {e}"))?;

            // Spawn victim.
            let mut victim = spawn_victim(&victim_exe)?;

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
