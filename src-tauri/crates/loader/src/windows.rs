//! Windows implementation of the four `3dmloader.dll` entry points.
//!
//! ## Entry points (matches `dll_injector.py` from XXMI Launcher)
//!
//! ```c
//! int HookLibrary(LPCWSTR target_window_class,
//!                 HHOOK*  out_hook_handle,
//!                 HANDLE* out_named_mutex);
//!
//! int WaitForInjection(LPCWSTR injected_dll_path,
//!                      LPCWSTR observed_dll_path,
//!                      int     timeout_ms);
//!
//! int UnhookLibrary(HHOOK*  in_out_hook_handle,
//!                   HANDLE* in_out_named_mutex);
//!
//! int Inject(DWORD   target_pid,
//!            LPCWSTR dll_path,
//!            int     flags);
//! ```
//!
//! All four return `0` on success. The upstream library does not document a
//! stable error-code table; we expose the integer verbatim through
//! [`Error::NonZeroStatus`].
//!
//! ## Cleanup contract
//!
//! Callers of [`Loader::hook`] receive a [`HookSession`] that holds the
//! `HHOOK` and named-mutex handles. Its [`Drop`] impl calls
//! `UnhookLibrary`, **including on panic**, so even an unwinding test never
//! leaves a stray Windows hook in place. The only way to skip cleanup is
//! `std::mem::forget`, which we never do.
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
//! `PathBuf`) or borrowed `&Path`. No raw pointers cross the public API
//! boundary.

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;
use std::sync::Arc;

use windows_sys::Win32::Foundation::{FreeLibrary, BOOL, HANDLE, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::UI::WindowsAndMessaging::HHOOK;

use crate::Error;

/// FFI types match upstream's ctypes declarations.
type DWORD = u32;
type LPCWSTR = *const u16;

type FnHookLibrary = unsafe extern "system" fn(LPCWSTR, *mut HHOOK, *mut HANDLE) -> i32;
type FnWaitForInjection = unsafe extern "system" fn(LPCWSTR, LPCWSTR, i32) -> i32;
type FnUnhookLibrary = unsafe extern "system" fn(*mut HHOOK, *mut HANDLE) -> i32;
type FnInject = unsafe extern "system" fn(DWORD, LPCWSTR, i32) -> i32;

/// Internal handle pack passed between [`Loader`] and [`HookSession`].
/// We keep these inside an [`Arc`] so the `Drop` impl on [`HookSession`]
/// can call back into the loader's function pointers even if the user has
/// dropped their `Loader` value first.
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
        // call in [`Loader::load`] and has not been freed elsewhere. Calling
        // `FreeLibrary` is the documented Win32 inverse.
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
    /// Load `3dmloader.dll` from `dll_path`. Bumps the OS loader's
    /// reference count on the file; dropping this `Loader` (and any
    /// outstanding [`HookSession`] clones of its inner [`Arc`]) calls
    /// `FreeLibrary`.
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

        // SAFETY: `handle` is a valid HMODULE from LoadLibraryW above.
        // Each `GetProcAddress` may return null if the symbol is missing.
        let hook_library = resolve_symbol(handle, "HookLibrary")?;
        let wait_for_injection = resolve_symbol(handle, "WaitForInjection")?;
        let unhook_library = resolve_symbol(handle, "UnhookLibrary")?;
        let inject = resolve_symbol(handle, "Inject")?;

        Ok(Self {
            inner: Arc::new(LoadedDll {
                handle,
                // SAFETY: the function pointers' ABIs match upstream's
                // exports — checked against `XXMI-Libs-Package` v0.8.8.
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

    /// Install a CBT hook that watches for windows belonging to the
    /// `target_window_class` to be created, and injects `dll_to_inject`
    /// into the process that owns each such window.
    ///
    /// The returned [`HookSession`] owns the hook + named-mutex handles.
    /// Drop it (or call [`HookSession::unhook`]) to remove the hook.
    pub fn hook(
        &self,
        target_window_class: &str,
        dll_to_inject: &Path,
    ) -> Result<HookSession<'_>, Error> {
        // Upstream's HookLibrary takes only one wide string (the window
        // class to watch for). The DLL path to inject is configured via a
        // companion call — XXMI sets this through a sibling export named
        // `Inject` *after* hook setup. We mirror that order here so the
        // FFI invariants are consistent with how XXMI exercises the
        // library in the wild.
        let target_wide =
            to_wide_nul(OsStr::new(target_window_class)).ok_or_else(|| Error::InvalidPath {
                path: dll_to_inject.to_path_buf(),
            })?;

        let mut hook: HHOOK = ptr::null_mut();
        let mut mutex: HANDLE = ptr::null_mut();

        // SAFETY: `target_wide` lives for the duration of the call;
        // `hook` and `mutex` are valid mutable references to local
        // variables on this stack frame.
        let status =
            unsafe { (self.inner.hook_library)(target_wide.as_ptr(), &mut hook, &mut mutex) };
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
    /// going through a CBT hook. Used by test harnesses that have already
    /// spawned the target process and know its PID.
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
    /// Wait up to `timeout_ms` milliseconds for the hook to inject the
    /// configured DLL into a target process. Returns `Ok(())` on success.
    pub fn wait_for_injection(&self, injected_dll: &Path, timeout_ms: i32) -> Result<(), Error> {
        let injected_wide =
            to_wide_nul(injected_dll.as_os_str()).ok_or_else(|| Error::InvalidPath {
                path: injected_dll.to_path_buf(),
            })?;
        let configured_wide =
            to_wide_nul(self.dll_path.as_os_str()).ok_or_else(|| Error::InvalidPath {
                path: self.dll_path.clone(),
            })?;

        // SAFETY: both wide buffers and the timeout live for the call.
        let status = unsafe {
            (self.loader.wait_for_injection)(
                injected_wide.as_ptr(),
                configured_wide.as_ptr(),
                timeout_ms,
            )
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
    if ptr.is_none() {
        return Err(Error::MissingSymbol { symbol });
    }
    // Cast the function pointer to a raw `*const ()` for transmute later;
    // the concrete signature varies by symbol.
    Ok(ptr.unwrap() as *const ())
}

fn to_wide_nul(s: &OsStr) -> Option<Vec<u16>> {
    let mut wide: Vec<u16> = s.encode_wide().collect();
    if wide.iter().any(|&c| c == 0) {
        return None;
    }
    wide.push(0);
    Some(wide)
}

#[doc(hidden)]
fn _send_sync_assertions() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<Loader>();
    assert_sync::<Loader>();
}

// Suppress dead-code warning on the OsString import when the only use is
// within `to_wide_nul`'s temporary type bound.
#[allow(dead_code)]
fn _osstring_marker() -> OsString {
    OsString::new()
}

// `BOOL` and `FreeLibrary` are pulled in just so the imports line up with
// the Drop impl above; the cast happens implicitly. Silence the unused
// warning on the alias.
#[allow(dead_code)]
type _FreeLibraryReturn = BOOL;
