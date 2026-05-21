//! Local diagnostics (slice NEW-LOG): JSON-lines logs + bundle export.
//!
//! GMM does not phone home. Every line of structured output written by
//! GMM lives on the user's disk and never leaves it. The diagnostics
//! bundle is an explicit, user-initiated `.zip` they can attach to a
//! bug report.
//!
//! The module exposes three concerns that are independently testable:
//!
//! 1. [`build_writer`] returns a non-blocking JSON-lines writer that
//!    rotates daily and writes to `gmm-YYYY-MM-DD.log`. Used by
//!    [`install_subscriber`] in production, and by integration tests
//!    that want to point the writer at a tempdir.
//! 2. [`prune_old_logs`] deletes log files whose modified time is older
//!    than `max_age_days`. We call this on startup so the user's log
//!    directory cannot grow unbounded.
//! 3. [`build_bundle`] zips the last N days of logs plus a redacted
//!    settings snapshot into a destination path the user picks.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt, EnvFilter};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use super::error::{Error, Result};

/// Prefix used for every log file the rolling appender writes.
pub const LOG_FILE_PREFIX: &str = "gmm";
/// Suffix used for every log file the rolling appender writes.
pub const LOG_FILE_SUFFIX: &str = "log";
/// Default retention window for [`prune_old_logs`]. Matches the slice
/// NEW-LOG acceptance criterion.
pub const DEFAULT_LOG_RETENTION_DAYS: i64 = 14;
/// Default age window for [`build_bundle`]: only logs modified within
/// this many days are included.
pub const DEFAULT_BUNDLE_LOG_DAYS: i64 = 7;

/// Build a non-blocking JSON-lines writer that rotates daily inside
/// `log_dir`. The returned [`WorkerGuard`] **must** outlive every
/// `tracing` event you want to flush.
pub fn build_writer(
    log_dir: &Path,
) -> Result<(tracing_appender::non_blocking::NonBlocking, WorkerGuard)> {
    fs::create_dir_all(log_dir).map_err(|source| Error::Io {
        path: log_dir.to_path_buf(),
        source,
    })?;

    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .filename_suffix(LOG_FILE_SUFFIX)
        .build(log_dir)
        .map_err(|source| {
            Error::Diagnostics(format!(
                "rolling appender init failed at {}: {source}",
                log_dir.display()
            ))
        })?;

    let (non_blocking, guard) = tracing_appender::non_blocking(appender);
    Ok((non_blocking, guard))
}

/// Install the JSON-lines subscriber globally. Production entry point —
/// integration tests should compose their own subscriber via
/// [`build_writer`] so they do not collide with the global default.
pub fn install_subscriber(log_dir: &Path) -> Result<WorkerGuard> {
    let (writer, guard) = build_writer(log_dir)?;

    let layer = fmt::layer()
        .json()
        .with_current_span(false)
        .with_span_list(false)
        .with_writer(writer);

    let filter = EnvFilter::try_from_env("GMM_LOG").unwrap_or_else(|_| EnvFilter::new("info"));

    use tracing_subscriber::layer::SubscriberExt as _;
    let subscriber = tracing_subscriber::registry().with(filter).with(layer);
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|e| Error::Diagnostics(format!("install subscriber: {e}")))?;
    Ok(guard)
}

/// Remove log files matching `gmm-*.log` whose modified time is older
/// than `max_age_days`. Returns the number of files removed.
pub fn prune_old_logs(log_dir: &Path, max_age_days: i64) -> Result<u32> {
    if !log_dir.exists() {
        return Ok(0);
    }
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs((max_age_days.max(0) as u64) * 86_400))
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let mut removed = 0_u32;
    for entry in fs::read_dir(log_dir).map_err(|source| Error::Io {
        path: log_dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| Error::Io {
            path: log_dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if !is_gmm_log(&path) {
            continue;
        }
        let modified = match entry.metadata().and_then(|m| m.modified()) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if modified < cutoff {
            // Best-effort removal — we'd rather not crash on a locked file
            // mid-startup.
            if fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

/// Whether `path` matches our rolling-appender naming scheme:
/// `gmm-YYYY-MM-DD.log` or, for the latest, `gmm.log`.
fn is_gmm_log(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if !name.starts_with(LOG_FILE_PREFIX) {
        return false;
    }
    // Either `gmm.log` (current) or `gmm.log.YYYY-MM-DD` (rotated by
    // tracing-appender's filename pattern).
    name.contains(LOG_FILE_SUFFIX)
}

/// Snapshot of the bits of GMM's settings we include in a diagnostics
/// bundle. Sensitive fields are marked here so the redactor at the
/// boundary cannot forget them.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct SettingsSnapshot {
    /// Library root (file paths are not considered secret).
    pub library_root: Option<PathBuf>,
    /// Per-game install paths keyed by lowercased game code.
    pub game_install_paths: HashMap<String, Option<PathBuf>>,
    /// Configured proxy URL, if any. The host/port stays; the
    /// `userinfo` portion is always redacted before the snapshot is
    /// serialised. See [`Self::redacted`].
    pub proxy_url: Option<String>,
}

impl SettingsSnapshot {
    /// Return a copy of `self` with the proxy URL's `userinfo` (e.g.
    /// `user:password`) replaced with `REDACTED`. Library + game paths
    /// are preserved because they're necessary for reproducing
    /// path-dependent bugs.
    pub fn redacted(&self) -> Self {
        let proxy_url = self
            .proxy_url
            .as_ref()
            .map(|raw| redact_proxy_userinfo(raw));
        Self {
            library_root: self.library_root.clone(),
            game_install_paths: self.game_install_paths.clone(),
            proxy_url,
        }
    }
}

/// Replace the `user:password@` portion of `proxy_url` with
/// `REDACTED@`, leaving scheme + host + port + path intact. Non-URL
/// inputs are returned unchanged (we don't want a parse failure to
/// silently drop the value).
fn redact_proxy_userinfo(raw: &str) -> String {
    let Some(scheme_end) = raw.find("://") else {
        return raw.to_string();
    };
    let after_scheme = scheme_end + 3;
    let rest = &raw[after_scheme..];
    let Some(at_idx) = rest.find('@') else {
        return raw.to_string();
    };
    let mut out = String::with_capacity(raw.len());
    out.push_str(&raw[..after_scheme]);
    out.push_str("REDACTED");
    out.push_str(&rest[at_idx..]);
    out
}

/// Build a diagnostics bundle ZIP at `dest`. Includes every log file in
/// `log_dir` modified within the last `log_age_days` days, plus a
/// redacted `settings.json`. Pure I/O — no global state, no tracing
/// init required.
pub fn build_bundle(
    log_dir: &Path,
    settings: &SettingsSnapshot,
    dest: &Path,
    log_age_days: i64,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
    }

    let file = File::create(dest).map_err(|source| Error::Io {
        path: dest.to_path_buf(),
        source,
    })?;
    let mut writer = ZipWriter::new(BufWriter::new(file));
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    // Redacted settings snapshot.
    let redacted = settings.redacted();
    let settings_json = serde_json::to_vec_pretty(&redacted)
        .map_err(|e| Error::Diagnostics(format!("serialise settings: {e}")))?;
    writer
        .start_file("settings.json", opts)
        .map_err(|e| Error::Diagnostics(format!("zip start settings.json: {e}")))?;
    writer
        .write_all(&settings_json)
        .map_err(|source| Error::Io {
            path: dest.to_path_buf(),
            source,
        })?;

    // Logs younger than the cutoff.
    if log_dir.exists() {
        let cutoff = SystemTime::now()
            .checked_sub(Duration::from_secs((log_age_days.max(0) as u64) * 86_400))
            .unwrap_or(SystemTime::UNIX_EPOCH);

        let entries = fs::read_dir(log_dir).map_err(|source| Error::Io {
            path: log_dir.to_path_buf(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| Error::Io {
                path: log_dir.to_path_buf(),
                source,
            })?;
            let path = entry.path();
            if !path.is_file() || !is_gmm_log(&path) {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            if modified < cutoff {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| Error::Diagnostics("log filename not utf-8".to_string()))?;
            let zip_name = format!("logs/{name}");
            writer
                .start_file(&zip_name, opts)
                .map_err(|e| Error::Diagnostics(format!("zip start {zip_name}: {e}")))?;
            let mut log_file = File::open(&path).map_err(|source| Error::Io {
                path: path.clone(),
                source,
            })?;
            io::copy(&mut log_file, &mut writer).map_err(|source| Error::Io {
                path: path.clone(),
                source,
            })?;
        }
    }

    writer
        .finish()
        .map_err(|e| Error::Diagnostics(format!("finalise bundle: {e}")))?;
    Ok(())
}

/// Record a structured event sourced from the frontend. The Tauri
/// command shell calls this; nothing else should.
pub fn record_frontend_error(message: &str, stack: Option<&str>, route: Option<&str>) {
    tracing::error!(
        source = "frontend",
        stack = stack.unwrap_or(""),
        route = route.unwrap_or(""),
        message,
    );
}
