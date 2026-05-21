//! Windows implementation of the four `3dmloader.dll` entry points.
//!
//! ## Entry points (matches `dll_injector.py` from XXMI Launcher)
//!
//! ```c
//! int HookLibrary(LPCWSTR dll_to_inject_path,
//!                 HHOOK*  out_hook_handle,
//!                 HANDLE* out_named_mutex);
//!
//! int WaitForInjection(LPCWSTR dll_to_inject_path,
//!                      LPCWSTR target_process_name,
//!                      int     timeout_secs);
//!
//! int UnhookLibrary(HHOOK*  in_out_hook_handle,
//!                   HANDLE* in_out_named_mutex);
//!
//! int Inject(DWORD   target_pid,
//!            LPCWSTR dll_path,
//!            int     flags);
//! ```
//!
//! `HookLibrary` installs a CBT hook that watches for windows being created
//! in *any* process; when a window appears, 3dmloader calls
//! `LoadLibraryW(dll_to_inject_path)` inside that process. The status
//! integers we surface verbatim through [`Error::NonZeroStatus`]; XXMI's
//! Python wrapper documents the meaningful ones:
//!
//! | status | meaning |
//! |-------:|---------|
//! | 0      | success |
//! | 100    | another instance of the loader is already hooked |
//! | 200    | failed to LoadLibraryW the supplied DLL |
//! | 300    | DLL missing the entry point upstream expects |
//! | 400    | failed to install the CBT hook |
//!
//! `WaitForInjection` blocks until a process whose name contains
//! `target_process_name` has loaded `dll_to_inject_path`, or until
//! `timeout_secs` elapses (returns non-zero on timeout).
//!
//! ## Cleanup contract
//!
//! Callers of [`Loader::hook`] receive a [`HookSession`] that holds the
//! `HHOOK` and named-mutex handles plus the DLL path. Its [`Drop`] impl
//! calls `UnhookLibrary`, **including on panic**, so even an unwinding
//! test never leaves a stray Windows hook in place. The only way to skip
//! cleanup is `std::mem::forget`, which we never do.
//!
//! ## Audit guidance
//!
//! Unsafe blocks in this file are limited to:
//!
//! - The four `extern "system" fn` typedefs and the `GetProcAddress` casts
//!   that produce them. These are the FFI surface and cannot be made safe.
//! - The actual function-pointer invocations, which receive only
//!   well-formed Rust-owned UTF-16 buffers and out-pointers to local
//!   stack variables.
//! - The `FreeLibrary` call inside `Drop for Loader`, which receives the
//!   HMODULE we got from `LoadLibraryW`. No user-supplied data here.
//!
//! There is no unsafe surface reachable from public methods *except* via
//! these FFI calls. Public APIs hand out only owned values (`Vec<u16>`,
//! `PathBuf`) or borrowed `&Path` / `&str`. No raw pointers cross the
//! public API boundary.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;
use std::sync::Arc;

use windows_sys::Win32::Foundation::{FreeLibrary, HANDLE, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::UI::WindowsAndMessaging::HHOOK;

use crate::Error;

/// FFI types match upstream's ctypes declarations.
#[allow(clippy::upper_case_acronyms)]
type DWORD = u32;
#[allow(clippy::upper_case_acronyms)]
type LPCWSTR = *const u16;

type FnHookLibrary = unsafe extern "system" fn(LPCWSTR, *mut HHOOK, *mut HANDLE) -> i32;
type FnWaitForInjection = unsafe extern "system" fn(LPCWSTR, LPCWSTR, i32) -> i32;
type FnUnhookLibrary = unsafe extern "system" fn(*mut HHOOK, *mut HANDLE) -> i32;
type FnInject = unsafe extern "system" fn(DWORD, LPCWSTR, i32) -> i32;

struct LoadedDll {
    handle: HMODULE,
    hook_library: FnHookLibrary,
    wait_for_injection: FnWaitForInjection,
    unhook_library: FnUnhookLibrary,
    inject: FnInject,
}

// HMODULE is just an opaque pointer to the OS loader's record for the DLL.
// Win32 documents LoadLibrary/FreeLibrary as thread-safe; sending/sharing
// the handle across threads is allowed.
unsafe impl Send for LoadedDll {}
unsafe impl Sync for LoadedDll {}

impl Drop for LoadedDll {
    fn drop(&mut self) {
        // SAFETY: `self.handle` was returned by a successful `LoadLibraryW`
        // call in [`Loader::load`] and has not been freed elsewhere.
        unsafe {
            let _ = FreeLibrary(self.handle);
        }
    }
}

/// Owns a loaded `3dmloader.dll`. Multiple [`HookSession`]s may be derived
/// from the same `Loader` — the inner `LoadedDll` is reference-counted so
/// the DLL stays mapped until every hook session has been dropped.
#[derive(Clone)]
pub struct Loader {
    inner: Arc<LoadedDll>,
}

impl Loader {
    /// Load `3dmloader.dll` from `dll_path`.
    pub fn load(dll_path: &Path) -> Result<Self, Error> {
        let path_wide = to_wide_nul(dll_path.as_os_str()).ok_or_else(|| Error::InvalidPath {
            path: dll_path.to_path_buf(),
        })?;

        // SAFETY: `path_wide` is a NUL-terminated UTF-16 buffer owned by
        // this function for the entire duration of the call.
        let handle = unsafe { LoadLibraryW(path_wide.as_ptr()) };
        if handle.is_null() {
            return Err(Error::LoadLibrary {
                path: dll_path.to_path_buf(),
                source: std::io::Error::last_os_error(),
            });
        }

        let hook_library = resolve_symbol(handle, "HookLibrary")?;
        let wait_for_injection = resolve_symbol(handle, "WaitForInjection")?;
        let unhook_library = resolve_symbol(handle, "UnhookLibrary")?;
        let inject = resolve_symbol(handle, "Inject")?;

        Ok(Self {
            inner: Arc::new(LoadedDll {
                handle,
                // SAFETY: the function pointers' ABIs match upstream's
                // exports — checked against `XXMI-Libs-Package` v0.8.8 and
                // its `dll_injector.py` consumer.
                hook_library: unsafe {
                    std::mem::transmute::<*const (), FnHookLibrary>(hook_library)
                },
                wait_for_injection: unsafe {
                    std::mem::transmute::<*const (), FnWaitForInjection>(wait_for_injection)
                },
                unhook_library: unsafe {
                    std::mem::transmute::<*const (), FnUnhookLibrary>(unhook_library)
                },
                inject: unsafe { std::mem::transmute::<*const (), FnInject>(inject) },
            }),
        })
    }

    /// Install the CBT hook. 3dmloader watches for window creation in
    /// every process and calls `LoadLibraryW(dll_to_inject)` inside each
    /// such process. Use [`HookSession::wait_for_injection`] to block
    /// until a specific target process has loaded the DLL, then drop the
    /// session to remove the hook.
    pub fn hook(&self, dll_to_inject: &Path) -> Result<HookSession<'_>, Error> {
        let dll_wide =
            to_wide_nul(dll_to_inject.as_os_str()).ok_or_else(|| Error::InvalidPath {
                path: dll_to_inject.to_path_buf(),
            })?;

        let mut hook: HHOOK = ptr::null_mut();
        let mut mutex: HANDLE = ptr::null_mut();

        // SAFETY: `dll_wide` lives for the duration of the call; `hook`
        // and `mutex` are valid mutable references to local variables on
        // this stack frame.
        let status = unsafe { (self.inner.hook_library)(dll_wide.as_ptr(), &mut hook, &mut mutex) };
        if status != 0 {
            return Err(Error::NonZeroStatus {
                symbol: "HookLibrary",
                status,
            });
        }

        Ok(HookSession {
            loader: self.inner.clone(),
            hook,
            mutex,
            dll_path: dll_to_inject.to_path_buf(),
            _phantom: std::marker::PhantomData,
        })
    }

    /// Inject `dll_path` directly into the process with `pid`, without
    /// installing a CBT hook. Used by harnesses that already have a PID.
    pub fn inject(&self, pid: u32, dll_path: &Path) -> Result<(), Error> {
        let dll_wide = to_wide_nul(dll_path.as_os_str()).ok_or_else(|| Error::InvalidPath {
            path: dll_path.to_path_buf(),
        })?;

        // SAFETY: `dll_wide` lives for the call.
        let status = unsafe { (self.inner.inject)(pid as DWORD, dll_wide.as_ptr(), 0) };
        if status != 0 {
            return Err(Error::NonZeroStatus {
                symbol: "Inject",
                status,
            });
        }
        Ok(())
    }
}

/// Lifetime token for a hook installed via [`Loader::hook`]. Drop the
/// session to unhook. Calling [`HookSession::unhook`] explicitly returns
/// the unhook status; the drop impl swallows it.
pub struct HookSession<'loader> {
    loader: Arc<LoadedDll>,
    hook: HHOOK,
    mutex: HANDLE,
    dll_path: std::path::PathBuf,
    _phantom: std::marker::PhantomData<&'loader Loader>,
}

impl HookSession<'_> {
    /// Block until a process whose name contains `target_process` has
    /// loaded the hooked DLL, or `timeout_secs` seconds elapse.
    pub fn wait_for_injection(&self, target_process: &str, timeout_secs: i32) -> Result<(), Error> {
        let dll_wide =
            to_wide_nul(self.dll_path.as_os_str()).ok_or_else(|| Error::InvalidPath {
                path: self.dll_path.clone(),
            })?;
        let target_wide =
            to_wide_nul(OsStr::new(target_process)).ok_or_else(|| Error::InvalidPath {
                path: std::path::PathBuf::from(target_process),
            })?;

        // SAFETY: both wide buffers + the timeout live for the call.
        let status = unsafe {
            (self.loader.wait_for_injection)(dll_wide.as_ptr(), target_wide.as_ptr(), timeout_secs)
        };
        if status != 0 {
            return Err(Error::NonZeroStatus {
                symbol: "WaitForInjection",
                status,
            });
        }
        Ok(())
    }

    /// Explicit unhook. Returns the status from the underlying
    /// `UnhookLibrary` call. The same call runs again from [`Drop`] if
    /// it's never invoked manually, but the drop path can't return the
    /// status so prefer this when the caller needs to observe it.
    pub fn unhook(mut self) -> Result<(), Error> {
        run_unhook(&self.loader, &mut self.hook, &mut self.mutex)?;
        // Prevent Drop from running again on the now-cleared handles.
        std::mem::forget(self);
        Ok(())
    }
}

impl Drop for HookSession<'_> {
    fn drop(&mut self) {
        if self.hook.is_null() && self.mutex.is_null() {
            return;
        }
        // Drop-time best-effort. Swallow the status — there is no
        // sensible target to surface it to.
        let _ = run_unhook(&self.loader, &mut self.hook, &mut self.mutex);
    }
}

fn run_unhook(loaded: &LoadedDll, hook: &mut HHOOK, mutex: &mut HANDLE) -> Result<(), Error> {
    // SAFETY: both out-pointers point to fields of the live `HookSession`
    // value the caller still owns. After the call they are zeroed by the
    // upstream library to signal "no longer valid".
    let status = unsafe { (loaded.unhook_library)(hook, mutex) };
    if status != 0 {
        return Err(Error::NonZeroStatus {
            symbol: "UnhookLibrary",
            status,
        });
    }
    Ok(())
}

fn resolve_symbol(handle: HMODULE, symbol: &'static str) -> Result<*const (), Error> {
    let mut bytes = Vec::with_capacity(symbol.len() + 1);
    bytes.extend_from_slice(symbol.as_bytes());
    bytes.push(0);

    // SAFETY: `bytes` is a NUL-terminated ASCII buffer alive for the
    // call; `handle` is a valid HMODULE from `LoadLibraryW`.
    let ptr = unsafe { GetProcAddress(handle, bytes.as_ptr()) };
    match ptr {
        Some(p) => Ok(p as *const ()),
        None => Err(Error::MissingSymbol { symbol }),
    }
}

fn to_wide_nul(s: &OsStr) -> Option<Vec<u16>> {
    let mut wide: Vec<u16> = s.encode_wide().collect();
    if wide.contains(&0) {
        return None;
    }
    wide.push(0);
    Some(wide)
}
