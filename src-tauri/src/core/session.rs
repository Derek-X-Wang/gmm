//! Persisted GameSession state.
//!
//! The Tauri layer holds the live `std::process::Child` + `HookSession` for
//! the duration of the session; the Core holds the durable record so a
//! GMM crash mid-session can be detected on next startup. See ADR 0004
//! (conservative defaults) and `CONTEXT.md` § Game Session.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::games::GameCode;

/// Summary of the active GameSession as persisted in the `active_session`
/// table. The DB schema enforces at most one row (singleton via
/// `CHECK (id = 1)`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub game: GameCode,
    pub pid: u32,
    pub started_at: DateTime<Utc>,
}

/// Cross-platform "is this PID still alive" check.
///
/// On Unix: `kill(pid, 0)` returns 0 if a signal would be deliverable;
/// `EPERM` also means the process exists (we just lack permission).
/// On Windows: open the process with the lightest possible right and
/// query its exit code — `STILL_ACTIVE` (259) means it's running.
pub fn is_pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: libc::kill is safe to call with signal 0; it never
        // sends a real signal, only checks if one could be delivered.
        let result = unsafe { libc::kill(pid as i32, 0) };
        if result == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        const STILL_ACTIVE: u32 = 259;
        // SAFETY: OpenProcess returns a valid handle or null; we
        // CloseHandle whatever it gives us. GetExitCodeProcess writes
        // through a pointer to a local u32.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut exit_code: u32 = 0;
            let ok = GetExitCodeProcess(handle, &mut exit_code) != 0;
            CloseHandle(handle);
            ok && exit_code == STILL_ACTIVE
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}
