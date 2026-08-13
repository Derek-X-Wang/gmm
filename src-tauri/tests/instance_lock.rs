//! Issue #58: GMM is single-instance *per data directory*.
//!
//! The hazard multi-instance creates is not "two GMM windows" — it is two
//! writers against one `gmm.db` and one Library. So the lock is scoped to
//! the data directory that holds them, not to the executable. Two builds
//! (a dev build and the installed one) pointed at the same `%APPDATA%\GMM`
//! are the dangerous case, and an exe-identity lock would wave them
//! through.
//!
//! These tests pin the lock's contract. The cross-process pairings that
//! motivate it live in `tests/concurrency.rs`.

use gmm_lib::core::instance_lock::{self, InstanceLockError};
use tempfile::TempDir;

#[test]
fn a_second_instance_is_refused_while_the_first_holds_the_lock() {
    let tmp = TempDir::new().expect("tmp");

    let first = instance_lock::acquire(tmp.path()).expect("first instance acquires the lock");

    match instance_lock::acquire(tmp.path()) {
        Err(InstanceLockError::AlreadyRunning { path }) => {
            assert_eq!(
                path,
                tmp.path().join(instance_lock::LOCK_FILE_NAME),
                "the error names the lock file so the log line is actionable",
            );
        }
        other => panic!("expected AlreadyRunning while the first lock is held, got {other:?}"),
    }

    drop(first);

    let _reacquired =
        instance_lock::acquire(tmp.path()).expect("the lock is released when the holder drops");
}

#[test]
fn locks_are_scoped_to_the_data_directory() {
    let a = TempDir::new().expect("tmp a");
    let b = TempDir::new().expect("tmp b");

    let _lock_a = instance_lock::acquire(a.path()).expect("data dir A");
    let _lock_b =
        instance_lock::acquire(b.path()).expect("a different data dir is independently lockable");
}

#[test]
fn a_missing_data_directory_is_created_rather_than_erroring() {
    let tmp = TempDir::new().expect("tmp");
    let nested = tmp.path().join("GMM/nope/not/yet");

    let lock = instance_lock::acquire(&nested).expect("acquire creates the data dir");

    assert!(nested.is_dir(), "data dir created");
    assert_eq!(lock.path(), nested.join(instance_lock::LOCK_FILE_NAME));
}

/// A lock file left behind by a killed process must not wedge the next
/// launch. Both backing primitives (`flock` on unix, exclusive share mode
/// on Windows) are released by the kernel when the handle closes, so a
/// stale *file* carries no stale *lock* — unlike a PID file, which is why
/// we don't use one.
#[test]
fn a_leftover_lock_file_from_a_crashed_instance_does_not_wedge_startup() {
    let tmp = TempDir::new().expect("tmp");
    let lock_path = tmp.path().join(instance_lock::LOCK_FILE_NAME);

    std::fs::write(&lock_path, b"leftover").expect("seed a stale lock file");
    assert!(lock_path.exists(), "precondition: stale file present");

    let _lock = instance_lock::acquire(tmp.path()).expect("a stale lock file is not a held lock");
}
