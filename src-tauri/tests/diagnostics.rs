//! Slice NEW-LOG: local diagnostics + bundle export.
//!
//! These tests exercise pure I/O in `core::diagnostics`. We don't touch
//! the global tracing subscriber except in the writer smoke test, which
//! installs the subscriber locally via `tracing::subscriber::with_default`
//! so other tests aren't affected.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime};

use gmm_lib::core::diagnostics::{
    build_bundle, build_writer, install_panic_hook, install_subscriber, prune_old_logs,
    record_instance_lock_fail_open, record_ipc_ready, record_main_window_shown,
    record_manifest_refresh_started, record_network_signal, record_startup_handoff,
    SettingsSnapshot, DEFAULT_BUNDLE_LOG_DAYS, DEFAULT_LOG_RETENTION_DAYS,
    INSTANCE_LOCK_FAIL_OPEN_MARKER, IPC_READY_COMMAND, IPC_READY_MARKER, LOG_FILE_PREFIX,
    LOG_FILE_SUFFIX, MAIN_WINDOW_SHOWN_MARKER, MANIFEST_REFRESH_STARTED_MARKER, PANIC_MARKER,
    STARTUP_HANDOFF_MARKER,
};
use tempfile::TempDir;
use tracing_subscriber::layer::SubscriberExt as _;

fn set_mtime(path: &Path, age_days: i64) {
    let when = SystemTime::now()
        .checked_sub(Duration::from_secs(age_days.max(0) as u64 * 86_400))
        .expect("subtract days");
    let ft = filetime::FileTime::from_system_time(when);
    filetime::set_file_mtime(path, ft).expect("set mtime");
}

#[test]
fn tracing_event_lands_in_dated_log_file() {
    let tmp = TempDir::new().expect("tmp");
    let log_dir = tmp.path().join("logs");

    let (writer, guard) = build_writer(&log_dir).expect("writer");
    let layer = tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(false)
        .with_span_list(false)
        .with_writer(writer);

    let subscriber = tracing_subscriber::registry().with(layer);
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(target: "gmm_test", marker = "hello-world", "smoke event");
    });
    // Drop the guard to flush.
    drop(guard);

    // Find the rotated file (gmm.log or gmm.log.YYYY-MM-DD).
    let mut found = false;
    for entry in fs::read_dir(&log_dir).expect("read log_dir") {
        let entry = entry.expect("entry");
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(LOG_FILE_PREFIX) || !name.contains(LOG_FILE_SUFFIX) {
            continue;
        }
        let mut contents = String::new();
        fs::File::open(entry.path())
            .expect("open log")
            .read_to_string(&mut contents)
            .expect("read log");
        if contents.contains("\"marker\":\"hello-world\"") {
            found = true;
            assert!(
                contents.contains("\"level\":\"INFO\""),
                "JSON line should include the level field, got: {contents}",
            );
            break;
        }
    }
    assert!(
        found,
        "rolling appender must produce a log file containing the smoke event",
    );
}

#[test]
fn prune_old_logs_removes_files_past_retention() {
    let tmp = TempDir::new().expect("tmp");
    let log_dir = tmp.path().join("logs");
    fs::create_dir_all(&log_dir).expect("logs dir");

    let old_file = log_dir.join("gmm.log.2026-04-01");
    let middle_file = log_dir.join("gmm.log.2026-05-15");
    let new_file = log_dir.join("gmm.log");
    fs::write(&old_file, b"older than retention").expect("write old");
    fs::write(&middle_file, b"middle aged").expect("write middle");
    fs::write(&new_file, b"fresh").expect("write fresh");

    set_mtime(&old_file, DEFAULT_LOG_RETENTION_DAYS + 5);
    set_mtime(&middle_file, 3);
    set_mtime(&new_file, 0);

    let removed = prune_old_logs(&log_dir, DEFAULT_LOG_RETENTION_DAYS).expect("prune");
    assert_eq!(removed, 1, "only the >14d file should have been pruned");
    assert!(!old_file.exists(), "old file must be removed");
    assert!(middle_file.exists(), "middle-aged file stays");
    assert!(new_file.exists(), "fresh file stays");
}

#[cfg(unix)]
#[test]
fn pruning_propagates_log_directory_uncertainty() {
    use std::os::unix::fs::symlink;

    let tmp = TempDir::new().expect("tmp");
    let log_dir = tmp.path().join("logs");
    symlink(&log_dir, &log_dir).expect("self-referential log directory");

    let prune = prune_old_logs(&log_dir, DEFAULT_LOG_RETENTION_DAYS);
    assert!(
        matches!(prune, Err(gmm_lib::core::Error::Io { ref path, .. }) if path == &log_dir),
        "an unreadable log directory must not be reported as empty, got {prune:?}",
    );
}

#[cfg(unix)]
#[test]
fn bundle_propagates_log_directory_uncertainty() {
    use std::os::unix::fs::symlink;

    let tmp = TempDir::new().expect("tmp");
    let log_dir = tmp.path().join("logs");
    symlink(&log_dir, &log_dir).expect("self-referential log directory");

    let bundle = tmp.path().join("bundle.zip");
    let result = build_bundle(
        &log_dir,
        &SettingsSnapshot::default(),
        &bundle,
        DEFAULT_BUNDLE_LOG_DAYS,
    );
    assert!(
        matches!(result, Err(gmm_lib::core::Error::Io { ref path, .. }) if path == &log_dir),
        "an unreadable log directory must not produce a confidently complete bundle, got {result:?}",
    );
}

#[test]
fn bundle_includes_recent_logs_and_redacts_settings() {
    let tmp = TempDir::new().expect("tmp");
    let log_dir = tmp.path().join("logs");
    fs::create_dir_all(&log_dir).expect("logs dir");

    let recent = log_dir.join("gmm.log.2026-05-20");
    let stale = log_dir.join("gmm.log.2026-05-01");
    fs::write(&recent, b"{\"level\":\"INFO\",\"msg\":\"recent\"}\n").expect("write recent");
    fs::write(&stale, b"{\"level\":\"INFO\",\"msg\":\"stale\"}\n").expect("write stale");
    set_mtime(&recent, 1);
    set_mtime(&stale, DEFAULT_BUNDLE_LOG_DAYS + 5);

    let mut settings = SettingsSnapshot {
        library_root: Some(tmp.path().join("library")),
        proxy_url: Some("http://user:hunter2@proxy.local:8080".to_string()),
        ..SettingsSnapshot::default()
    };
    settings
        .game_install_paths
        .insert("gimi".to_string(), Some("D:/Games/Genshin".into()));

    let dest = tmp.path().join("bundle.zip");
    build_bundle(&log_dir, &settings, &dest, DEFAULT_BUNDLE_LOG_DAYS).expect("bundle");

    // Read the bundle back and assert its contents.
    let file = fs::File::open(&dest).expect("open bundle");
    let mut archive = zip::ZipArchive::new(file).expect("parse bundle");

    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    assert!(
        names.iter().any(|n| n == "settings.json"),
        "bundle must contain settings.json — got {names:?}",
    );
    assert!(
        names.iter().any(|n| n == "logs/gmm.log.2026-05-20"),
        "recent log must be in the bundle — got {names:?}",
    );
    assert!(
        !names.iter().any(|n| n == "logs/gmm.log.2026-05-01"),
        "stale log must be excluded — got {names:?}",
    );

    let mut settings_contents = String::new();
    archive
        .by_name("settings.json")
        .expect("settings.json present")
        .read_to_string(&mut settings_contents)
        .expect("read settings.json");

    assert!(
        settings_contents.contains("REDACTED"),
        "proxy userinfo must be redacted: {settings_contents}",
    );
    assert!(
        !settings_contents.contains("hunter2"),
        "the literal password must not appear: {settings_contents}",
    );
    assert!(
        settings_contents.contains("proxy.local:8080"),
        "the host:port stays so the user can confirm what they configured: {settings_contents}",
    );
    assert!(
        settings_contents.contains("D:/Games/Genshin")
            || settings_contents.contains(r"D:\\Games\\Genshin"),
        "game install paths are preserved for repro: {settings_contents}",
    );
}

/// The installer smoke greps the app's own log for this marker to prove
/// the WebView actually reached the Rust side (issue #54). The line has
/// to survive the JSON layer intact, or the smoke silently stops
/// checking anything.
#[test]
fn ipc_ready_marker_lands_in_the_log_file() {
    let tmp = TempDir::new().expect("tmp");
    let log_dir = tmp.path().join("logs");

    let (writer, guard) = build_writer(&log_dir).expect("writer");
    let layer = tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(false)
        .with_span_list(false)
        .with_writer(writer);

    let subscriber = tracing_subscriber::registry().with(layer);
    tracing::subscriber::with_default(subscriber, || {
        record_ipc_ready();
    });
    drop(guard);

    let mut body = String::new();
    for entry in fs::read_dir(&log_dir).expect("read log_dir") {
        let path = entry.expect("entry").path();
        let mut f = fs::File::open(&path).expect("open log");
        f.read_to_string(&mut body).expect("read log");
    }
    assert!(
        body.contains(IPC_READY_MARKER),
        "log must carry the IPC readiness marker {IPC_READY_MARKER}, got:\n{body}",
    );
    assert!(
        body.contains(IPC_READY_COMMAND),
        "the marker must name the command that produced it, got:\n{body}",
    );
}

/// The packaged startup smoke waits for this marker while holding the
/// manifest response open. If the event does not survive the JSON layer,
/// that runtime guard can no longer distinguish a refresh that started
/// from one that was never wired into startup.
#[test]
fn manifest_refresh_started_marker_lands_in_the_log_file() {
    let tmp = TempDir::new().expect("tmp");
    let log_dir = tmp.path().join("logs");

    let (writer, guard) = build_writer(&log_dir).expect("writer");
    let layer = tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(false)
        .with_span_list(false)
        .with_writer(writer);

    let subscriber = tracing_subscriber::registry().with(layer);
    tracing::subscriber::with_default(subscriber, || {
        record_manifest_refresh_started();
    });
    drop(guard);

    let mut body = String::new();
    for entry in fs::read_dir(&log_dir).expect("read log_dir") {
        let path = entry.expect("entry").path();
        let mut f = fs::File::open(&path).expect("open log");
        f.read_to_string(&mut body).expect("read log");
    }
    assert!(
        body.contains(MANIFEST_REFRESH_STARTED_MARKER),
        "log must carry the manifest refresh marker {MANIFEST_REFRESH_STARTED_MARKER}, got:\n{body}",
    );
}

/// The installer smoke accepts this line as proof that a running GMM
/// legitimately holds no instance lock. If it does not survive the JSON layer
/// at the default `info` filter, a healthy fail-open GMM fails the smoke.
#[test]
fn instance_lock_fail_open_marker_lands_in_the_log_file() {
    let tmp = TempDir::new().expect("tmp");
    let log_dir = tmp.path().join("logs");

    let (writer, guard) = build_writer(&log_dir).expect("writer");
    let layer = tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(false)
        .with_span_list(false)
        .with_writer(writer);

    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("info"))
        .with(layer);
    tracing::subscriber::with_default(subscriber, || {
        record_instance_lock_fail_open();
    });
    drop(guard);

    let mut body = String::new();
    for entry in fs::read_dir(&log_dir).expect("read log_dir") {
        let path = entry.expect("entry").path();
        let mut f = fs::File::open(&path).expect("open log");
        f.read_to_string(&mut body).expect("read log");
    }
    assert!(
        body.contains(INSTANCE_LOCK_FAIL_OPEN_MARKER),
        "log must carry the fail-open marker {INSTANCE_LOCK_FAIL_OPEN_MARKER}, got:\n{body}",
    );
}

/// The installer smoke reports the elapsed time of these two markers to
/// bracket the otherwise unlogged stretch before the window appears (#450).
/// Both lines have to survive the JSON layer, or the smoke silently loses
/// the phase split.
#[test]
fn startup_bracket_markers_land_in_the_log_file() {
    let tmp = TempDir::new().expect("tmp");
    let log_dir = tmp.path().join("logs");

    let (writer, guard) = build_writer(&log_dir).expect("writer");
    let layer = tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(false)
        .with_span_list(false)
        .with_writer(writer);

    let subscriber = tracing_subscriber::registry().with(layer);
    tracing::subscriber::with_default(subscriber, || {
        record_startup_handoff();
        record_main_window_shown();
    });
    drop(guard);

    let mut body = String::new();
    for entry in fs::read_dir(&log_dir).expect("read log_dir") {
        let path = entry.expect("entry").path();
        let mut f = fs::File::open(&path).expect("open log");
        f.read_to_string(&mut body).expect("read log");
    }
    for marker in [STARTUP_HANDOFF_MARKER, MAIN_WINDOW_SHOWN_MARKER] {
        assert!(
            body.contains(marker),
            "log must carry the startup marker {marker}, got:\n{body}",
        );
    }
}

/// Pin the public grep contract independently of the production constant.
#[test]
fn network_signal_marker_and_info_fields_land_in_the_log_file() {
    let tmp = TempDir::new().expect("tmp");
    let log_dir = tmp.path().join("logs");
    let (writer, guard) = build_writer(&log_dir).expect("writer");
    let layer = tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(false)
        .with_span_list(false)
        .with_writer(writer);
    let subscriber = tracing_subscriber::registry().with(layer);
    tracing::subscriber::with_default(subscriber, || {
        record_network_signal(true, "startup");
        record_network_signal(true, "online");
        record_network_signal(false, "offline");
    });
    drop(guard);

    let mut body = String::new();
    for entry in fs::read_dir(&log_dir).expect("read log_dir") {
        fs::File::open(entry.expect("entry").path())
            .expect("open log")
            .read_to_string(&mut body)
            .expect("read log");
    }
    let events: Vec<serde_json::Value> = body
        .lines()
        .map(|line| serde_json::from_str(line).expect("JSON log line"))
        .collect();
    assert_eq!(events.len(), 3);
    for (event, (online, reason)) in
        events
            .iter()
            .zip([(true, "startup"), (true, "online"), (false, "offline")])
    {
        assert_eq!(event["fields"]["marker"], "gmm-network-signal");
        assert_eq!(event["level"], "INFO");
        assert_eq!(event["fields"]["online"], online);
        assert_eq!(event["fields"]["reason"], reason);
        assert!(event["timestamp"].as_str().is_some_and(|s| !s.is_empty()));
    }
}

/// Concatenate every `gmm*.log*` file in `log_dir`. Callers must drop the
/// writer guard first or the non-blocking appender may not have flushed.
fn read_all_logs(log_dir: &Path) -> String {
    let mut out = String::new();
    for entry in fs::read_dir(log_dir).expect("read log_dir") {
        let entry = entry.expect("entry");
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(LOG_FILE_PREFIX) || !name.contains(LOG_FILE_SUFFIX) {
            continue;
        }
        fs::File::open(entry.path())
            .expect("open log")
            .read_to_string(&mut out)
            .expect("read log");
    }
    out
}

/// A panic caught on a thread with a scoped subscriber must carry its
/// message and source location into the log. This proves the hook's
/// formatting; the process-exit test below covers the production path.
#[test]
fn panic_lands_in_the_log_with_message_and_location() {
    let tmp = TempDir::new().expect("tmp");
    let log_dir = tmp.path().join("logs");

    let (writer, guard) = build_writer(&log_dir).expect("writer");
    let layer = tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(false)
        .with_span_list(false)
        .with_writer(writer);
    let subscriber = tracing_subscriber::registry().with(layer);

    install_panic_hook();

    tracing::subscriber::with_default(subscriber, || {
        // `catch_unwind` still runs the panic hook; it only stops the
        // unwind from taking the test process with it.
        let _ = std::panic::catch_unwind(|| panic!("boom-marker-9f3c"));
    });
    drop(guard);

    let body = read_all_logs(&log_dir);
    assert!(
        body.contains(PANIC_MARKER),
        "log must carry the panic marker {PANIC_MARKER}, got:\n{body}",
    );
    assert!(
        body.contains("boom-marker-9f3c"),
        "log must carry the panic payload, got:\n{body}",
    );
    assert!(
        body.contains("diagnostics.rs"),
        "log must carry the panic source location, got:\n{body}",
    );
}

/// Environment variable through which the parent test hands the child
/// its log directory.
const PANIC_CHILD_LOG_DIR: &str = "GMM_TEST_PANIC_CHILD_LOG_DIR";

/// The dying half of [`panic_in_the_production_setup_survives_process_exit`].
/// It repeats what `run()` does — global subscriber, panic hook, then an
/// unwinding panic that drops the writer guard on the way out. It does
/// nothing unless the parent launches it in its own process, where a
/// global subscriber cannot collide with other tests.
#[test]
fn panic_child_process() {
    let Some(log_dir) = std::env::var_os(PANIC_CHILD_LOG_DIR) else {
        return;
    };
    let _guard = install_subscriber(Path::new(&log_dir)).expect("global subscriber");
    // Twice on purpose: nested hooks would write the record twice.
    install_panic_hook();
    install_panic_hook();
    panic!("child-boom-marker-4be1");
}

/// The production setup — global subscriber, installed hook — must leave
/// the panic in the log file after the panicking process has exited.
///
/// Scope of the claim: a panic that unwinds, which drops the writer
/// guard and flushes the non-blocking appender. A hard abort or a native
/// fault skips that flush and is not exercised here.
#[test]
fn panic_in_the_production_setup_survives_process_exit() {
    let tmp = TempDir::new().expect("tmp");
    let log_dir = tmp.path().join("logs");

    let output = Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "panic_child_process", "--nocapture"])
        .env(PANIC_CHILD_LOG_DIR, &log_dir)
        .env_remove("GMM_LOG")
        .output()
        .expect("spawn child test process");
    assert!(
        !output.status.success(),
        "the child must die from its panic, got: {:?}",
        output.status,
    );

    let body = read_all_logs(&log_dir);
    assert!(
        body.contains("child-boom-marker-4be1"),
        "the log must hold the panic after the process exited, got:\n{body}",
    );
    assert!(
        body.contains("diagnostics.rs"),
        "the log must carry the panic source location, got:\n{body}",
    );
    let records = body.lines().filter(|l| l.contains(PANIC_MARKER)).count();
    assert_eq!(
        records, 1,
        "installing the hook twice must still record the panic once, got:\n{body}",
    );
}
