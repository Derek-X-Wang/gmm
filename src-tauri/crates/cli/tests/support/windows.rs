use super::{result, Fixture};
use gmm_lib::core::{session::is_pid_alive, Core, GameCode};
use gmm_lib::runtime::{
    launch::{launch_headless, LaunchOptions},
    SessionRuntime,
};
use serde_json::Value;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use windows_sys::Win32::System::Console::{
    AllocConsole, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler, CTRL_C_EVENT,
};
use windows_sys::Win32::System::Threading::DETACHED_PROCESS;

struct Reap(Child);
impl Drop for Reap {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn fake_endfield(env: &Fixture) -> (Core, gmm_lib::core::Mod) {
    let artifacts = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug");
    let install = env.tmp.path().join("Game");
    std::fs::create_dir(&install).unwrap();
    std::fs::copy(
        artifacts.join("victim.exe"),
        install.join("Endfield-Win64-Shipping.exe"),
    )
    .expect("build the workspace before native launch tests");
    std::fs::copy(artifacts.join("noop_dll.dll"), install.join("d3d11.dll")).unwrap();
    let core = env.core().await;
    core.set_game_install_path(GameCode::Efmi, &install)
        .await
        .unwrap();
    let item = core
        .adopt_folder(GameCode::Efmi, &env.source(), "Example")
        .await
        .unwrap();
    (core, item)
}

#[tokio::test(flavor = "multi_thread")]
async fn headless_success_injects_and_watcher_releases_session() {
    let env = Fixture::new();
    let (core, _) = fake_endfield(&env).await;
    let runtime = SessionRuntime::new();
    let options = LaunchOptions {
        inject_settle: Duration::from_millis(300),
        watch_poll_interval: Duration::from_millis(50),
        ..LaunchOptions::default()
    };
    let launched = launch_headless(&core, &runtime, GameCode::Efmi, &options)
        .await
        .expect("headless inject-plus-watcher launch must succeed");
    assert!(
        runtime.has_session(),
        "headless success must retain the Loader and child"
    );
    assert_eq!(
        core.session_info().await.unwrap(),
        Some(launched.info.clone()),
        "headless success must persist the Game Session"
    );
    assert!(core
        .interrupted_session_launches()
        .await
        .unwrap()
        .is_empty());
    let status = Command::new("taskkill")
        .args(["/PID", &launched.info.pid.to_string(), "/F"])
        .output()
        .unwrap();
    assert!(status.status.success());
    tokio::time::timeout(Duration::from_secs(10), launched.watcher)
        .await
        .expect("headless watcher must finish after Game exit")
        .unwrap();
    assert!(
        !runtime.has_session(),
        "headless watcher must release the Loader and child"
    );
    assert!(
        core.session_info().await.unwrap().is_none(),
        "headless watcher must clear the persisted session"
    );
    core.close().await;
}

// Keep CTRL_C_EVENT away from Cargo, its terminal, and concurrent tests. The
// detached helper owns a new console; only its CLI and game inherit it.
#[test]
fn ctrl_c_interrupt_leaves_cli_able_to_launch_enable_and_disable() {
    if std::env::var_os("GMM_CLI_TEST_ISOLATED_CONSOLE").is_some() {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(ctrl_c_console_helper());
        return;
    }
    let temp = tempfile::TempDir::new().unwrap();
    let log = temp.path().join("console-helper.log");
    let output = std::fs::File::create(&log).unwrap();
    let mut helper = Reap(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "windows::ctrl_c_interrupt_leaves_cli_able_to_launch_enable_and_disable",
                "--nocapture",
            ])
            .env("GMM_CLI_TEST_ISOLATED_CONSOLE", "1")
            .creation_flags(DETACHED_PROCESS)
            .stdout(output.try_clone().unwrap())
            .stderr(output)
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(status) = helper.0.try_wait().unwrap() {
            let evidence = std::fs::read_to_string(&log).unwrap();
            assert!(
                status.success(),
                "native Ctrl+C helper must pass: {evidence}"
            );
            assert!(
                evidence.contains("CTRL_C_EVENT terminated the blocking CLI"),
                "helper must execute a real console Ctrl+C: {evidence}"
            );
            println!("{evidence}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "native Ctrl+C helper timed out: {}",
            std::fs::read_to_string(&log).unwrap()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

unsafe extern "system" fn preserve_helper(_event: u32) -> i32 {
    1
}

async fn ctrl_c_console_helper() {
    // SAFETY: this test runs in a detached helper. The handler is static and
    // affects only that helper; child processes do not inherit registered handlers.
    unsafe {
        assert_ne!(AllocConsole(), 0, "helper must own an isolated console");
        assert_ne!(SetConsoleCtrlHandler(Some(preserve_helper), 1), 0);
    }
    let env = Fixture::new();
    let (core, item) = fake_endfield(&env).await;
    core.close().await;
    let pool = sqlx::SqlitePool::connect(&env.url).await.unwrap();
    let mut cli = Reap(
        Command::new(env!("CARGO_BIN_EXE_gmm-cli"))
            .args([
                "--data-dir",
                env.data.to_str().unwrap(),
                "launch",
                "--game",
                "efmi",
            ])
            .env("GMM_VICTIM_TIMEOUT_SECS", "20")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let game_pid: i64 = loop {
        if let Some(pid) = sqlx::query_scalar("SELECT pid FROM active_session WHERE id = 1")
            .fetch_optional(&pool)
            .await
            .unwrap()
        {
            break pid;
        }
        assert!(
            cli.0.try_wait().unwrap().is_none(),
            "CLI must reach a blocking Game Session"
        );
        assert!(
            Instant::now() < deadline,
            "CLI never persisted its Game Session"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    // SAFETY: console group zero is confined to this helper's allocated console.
    unsafe {
        assert_ne!(GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0), 0);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    let interrupted = loop {
        if let Some(status) = cli.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "Ctrl+C must stop the blocking CLI"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert_eq!(
        interrupted.code(),
        Some(-1073741510),
        "Ctrl+C must terminate the CLI with STATUS_CONTROL_C_EXIT"
    );
    println!("CTRL_C_EVENT terminated the blocking CLI: {interrupted}");
    while is_pid_alive(game_pid as u32) {
        assert!(
            Instant::now() < deadline,
            "the interrupted console's Game must exit"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let persisted: Option<i64> = sqlx::query_scalar("SELECT pid FROM active_session WHERE id = 1")
        .fetch_optional(&pool)
        .await
        .unwrap();
    assert_eq!(
        persisted,
        Some(game_pid),
        "Ctrl+C baseline must leave the dead session for write-path recovery"
    );
    pool.close().await;
    for (command, enabled) in [("enable", true), ("disable", false)] {
        let outcome = env
            .invoke(&[command, "--mod-id", &item.id, "--confirm"])
            .await;
        assert!(
            outcome.ok,
            "after native Ctrl+C, CLI must {command} Mods: {:?}",
            outcome.error
        );
        assert_eq!(result(outcome)["mod"]["enabled"], enabled);
    }
    // Recreate the exact interrupted record, so launch proves recovery too.
    let core = Core::new_without_recovery(env.data.join("library"), &env.url)
        .await
        .unwrap();
    core.start_session(&gmm_lib::core::SessionInfo {
        game: GameCode::Efmi,
        pid: game_pid as u32,
        started_at: "2026-08-24T00:00:00Z".parse().unwrap(),
    })
    .await
    .unwrap();
    core.close().await;
    let resumed = Command::new(env!("CARGO_BIN_EXE_gmm-cli"))
        .args([
            "--data-dir",
            env.data.to_str().unwrap(),
            "launch",
            "--game",
            "efmi",
        ])
        .env("GMM_VICTIM_TIMEOUT_SECS", "3")
        .output()
        .unwrap();
    let json: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert!(
        resumed.status.success(),
        "after native Ctrl+C, CLI must launch again: {json}"
    );
    assert_eq!(json["result"]["ended"], true);
    assert_eq!(
        result(env.invoke(&["status"]).await)["activeSession"],
        Value::Null
    );
}
