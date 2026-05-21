//! Slice NEW-LOG: local diagnostics + bundle export.
//!
//! These tests exercise pure I/O in `core::diagnostics`. We don't touch
//! the global tracing subscriber except in the writer smoke test, which
//! installs the subscriber locally via `tracing::subscriber::with_default`
//! so other tests aren't affected.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, SystemTime};

use gmm_lib::core::diagnostics::{
    build_bundle, build_writer, prune_old_logs, SettingsSnapshot, DEFAULT_BUNDLE_LOG_DAYS,
    DEFAULT_LOG_RETENTION_DAYS, LOG_FILE_PREFIX, LOG_FILE_SUFFIX,
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
