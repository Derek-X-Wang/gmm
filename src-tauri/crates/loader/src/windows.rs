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
//!            int     timeout_secs);
//! ```
//!
//! `HookLibrary` installs a CBT hook that watches for windows being created
//! in *any* process; when a window appears, 3dmloader calls
//! `LoadLibraryW(dll_to_inject_path)` inside that process. Its status codes
//! are:
//!
//! | status | meaning |
//! |-------:|---------|
//! | 0      | success |
//! | 100    | another instance of the loader is already hooked |
//! | 200    | failed to LoadLibraryW the supplied DLL |
//! | 300    | DLL missing the entry point upstream expects |
//! | 400    | failed to install the CBT hook |
//!
//! `Inject` starts a remote `LoadLibraryW` thread and waits up to the supplied
//! timeout. Its status table is separate from `HookLibrary`'s:
//!
//! | status | meaning |
//! |-------:|---------|
//! | 0      | success |
//! | 100    | process not found or could not be opened |
//! | 110    | invalid DLL path |
//! | 120    | failed to resolve kernel32 |
//! | 130    | failed to resolve LoadLibraryW |
//! | 200    | VirtualAllocEx failed |
//! | 300    | WriteProcessMemory failed |
//! | 400    | CreateRemoteThread failed |
//! | 500    | injection thread timed out |
//! | 510    | waiting for the injection thread failed |
//! | 600    | LoadLibraryW returned NULL |
//! | 700    | unknown failure |
//!
//! The upstream default is 15 seconds. Passing zero makes the wait time out
//! immediately after the remote thread starts; upstream then frees the DLL
//! path buffer while that thread may still be reading it. [`Loader::inject`]
//! therefore always supplies the non-zero upstream default.
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

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
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

/// Match the default in XXMI-Libs-Package v0.9.2 and XXMI Launcher's
/// `dll_injector.py`. Zero is unsafe: status 500 frees the remote path buffer
/// before the live LoadLibraryW thread is guaranteed to have finished.
const INJECT_TIMEOUT_SECS: i32 = 15;

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
                // exports — checked against `XXMI-Libs-Package` v0.9.2 and
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
        // Expand to the long form up front. WaitForInjection compares
        // this exact string against the module paths Windows reports,
        // and Windows always reports long form — see `to_long_path`.
        let dll_to_inject = to_long_path(dll_to_inject);
        let dll_wide =
            to_wide_nul(dll_to_inject.as_os_str()).ok_or_else(|| Error::InvalidPath {
                path: dll_to_inject.clone(),
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
            dll_path: dll_to_inject,
            _phantom: std::marker::PhantomData,
        })
    }

    /// Inject `dll_path` directly into the process with `pid`, without
    /// installing a CBT hook. Used by harnesses that already have a PID.
    pub fn inject(&self, pid: u32, dll_path: &Path) -> Result<(), Error> {
        // Same long-path normalisation as `hook` — callers may verify
        // the result against the target's module list.
        let dll_path = to_long_path(dll_path);
        let dll_wide = to_wide_nul(dll_path.as_os_str()).ok_or_else(|| Error::InvalidPath {
            path: dll_path.clone(),
        })?;

        // SAFETY: `dll_wide` lives for the call. INJECT_TIMEOUT_SECS is the
        // timeout in seconds, not flags; it keeps the buffer alive while the
        // remote LoadLibraryW thread runs.
        let status =
            unsafe { (self.inner.inject)(pid as DWORD, dll_wide.as_ptr(), INJECT_TIMEOUT_SECS) };
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

// HHOOK / HANDLE are Win32 opaque pointers backed by kernel objects.
// Win32 documents both as thread-safe; the only state the user code
// touches is the `&mut` outparams during UnhookLibrary, which take place
// behind the session's exclusive borrow.
unsafe impl Send for HookSession<'_> {}
unsafe impl Sync for HookSession<'_> {}

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

/// Expand a path to its **long** (non-8.3) form.
///
/// `WaitForInjection` verifies injection by walking the target
/// process's module list and doing a literal `_wcsicmp` of each
/// module's `szExePath` against the DLL path we handed to
/// `HookLibrary` — no normalisation on either side. Windows reports
/// module paths in long form, so passing a short path (`C:\PROGRA~1\…`,
/// or any profile directory whose name exceeds 8 characters, e.g.
/// `C:\Users\RUNNER~1\…`) makes that comparison fail forever. The
/// injection itself succeeds; only the verification never fires, so the
/// caller sits until the timeout and then reports a failure that did
/// not happen.
///
/// Paths already in long form come back unchanged, so this is safe to
/// apply unconditionally. If the expansion fails for any reason we fall
/// back to the input rather than erroring — a wrong-but-present path is
/// no worse than what we had before.
fn to_long_path(path: &Path) -> PathBuf {
    // Two normalisations, in this order:
    //
    //   1. GetFullPathNameW — rewrites `/` to `\`, collapses `.`/`..`,
    //      and makes the path absolute. Windows APIs accept forward
    //      slashes, but the module list always reports backslashes, so
    //      a path built with `join("a/b")` fails the comparison even
    //      when every component is already long-form.
    //   2. GetLongPathNameW — expands 8.3 aliases (`RUNNER~1` ->
    //      `runneradmin`). This *fails outright* on a mixed-separator
    //      path, which is why it has to run second.
    //
    // Skipping step 1 was a real bug: the injected DLL was present in
    // the target process the whole time and only the verification
    // string-compare failed.
    let normalised = to_full_path(path);
    let expanded = to_expanded_path(&normalised).unwrap_or(normalised);
    strip_verbatim_prefix(expanded)
}

/// Drop a `\\?\` extended-length prefix.
///
/// `MODULEENTRY32.szExePath` is reported in ordinary drive-letter form,
/// so a verbatim path would fail the comparison for the same reason an
/// 8.3 or forward-slash path does. `std::fs::canonicalize` produces this
/// form, and a caller could reasonably hand us one.
///
/// Only the plain `\\?\C:\...` shape is unwrapped; `\\?\UNC\...` is left
/// alone because there is no equivalent drive-letter spelling to fall
/// back to.
fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest.to_string()),
        _ => path,
    }
}

/// `GetFullPathNameW` — separator + relative-component normalisation.
fn to_full_path(path: &Path) -> PathBuf {
    use windows_sys::Win32::Storage::FileSystem::GetFullPathNameW;

    let Some(wide) = to_wide_nul(path.as_os_str()) else {
        return path.to_path_buf();
    };

    // SAFETY: `wide` is NUL-terminated and alive for the call. A null
    // output buffer with length 0 is the documented length probe.
    let needed =
        unsafe { GetFullPathNameW(wide.as_ptr(), 0, std::ptr::null_mut(), std::ptr::null_mut()) };
    if needed == 0 {
        return path.to_path_buf();
    }

    let mut buf = vec![0u16; needed as usize];
    // SAFETY: `buf` holds `needed` code units, the size the probe asked
    // for. The final out-param (lpFilePart) is optional and unused.
    let written = unsafe {
        GetFullPathNameW(
            wide.as_ptr(),
            needed,
            buf.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    };
    if written == 0 || written >= needed {
        return path.to_path_buf();
    }

    buf.truncate(written as usize);
    PathBuf::from(OsString::from_wide(&buf))
}

/// `GetLongPathNameW` — 8.3 expansion. Returns `None` when the path
/// can't be expanded (most often because it doesn't exist yet).
fn to_expanded_path(path: &Path) -> Option<PathBuf> {
    use windows_sys::Win32::Storage::FileSystem::GetLongPathNameW;

    let wide = to_wide_nul(path.as_os_str())?;

    // SAFETY: as above — NUL-terminated input, documented length probe.
    let needed = unsafe { GetLongPathNameW(wide.as_ptr(), std::ptr::null_mut(), 0) };
    if needed == 0 {
        return None;
    }

    let mut buf = vec![0u16; needed as usize];
    // SAFETY: `buf` has room for `needed` code units including the
    // terminator, which is exactly what the probe above asked for.
    let written = unsafe { GetLongPathNameW(wide.as_ptr(), buf.as_mut_ptr(), needed) };
    // The probe counts the NUL terminator; this call does not.
    if written == 0 || written >= needed {
        return None;
    }

    buf.truncate(written as usize);
    Some(PathBuf::from(OsString::from_wide(&buf)))
}

#[cfg(test)]
mod tests {
    use super::{to_long_path, to_wide_nul};
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Storage::FileSystem::{
        SetFileShortNameW, DELETE, FILE_FLAG_BACKUP_SEMANTICS,
    };

    /// The ordinary drive-letter spelling Windows reports for a file.
    /// `canonicalize` supplies the OS-backed source of truth; removing its
    /// verbatim prefix mirrors the documented `MODULEENTRY32.szExePath`
    /// representation without calling the helper under test.
    fn module_list_spelling(path: &Path) -> PathBuf {
        let canonical = std::fs::canonicalize(path).expect("canonicalize fixture");
        let text = canonical.to_string_lossy();
        let ordinary = text
            .strip_prefix(r"\\?\")
            .unwrap_or_else(|| panic!("canonicalize returned a non-verbatim path: {canonical:?}"));
        PathBuf::from(ordinary)
    }

    fn assert_same_windows_path(actual: &Path, expected: &Path) {
        assert!(
            actual
                .to_string_lossy()
                .eq_ignore_ascii_case(&expected.to_string_lossy()),
            "expected {actual:?} to match Windows' spelling {expected:?}",
        );
    }

    /// Give an existing file an explicit 8.3 alias. Modern NTFS volumes
    /// often disable automatic short-name creation, so relying on
    /// `C:\\PROGRA~1` silently skipped the one case this helper exists for.
    fn set_short_name(path: &Path, short_name: &str) {
        let file = OpenOptions::new()
            .access_mode(DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
            .expect("open fixture with DELETE access for SetFileShortNameW");
        let short_wide =
            to_wide_nul(std::ffi::OsStr::new(short_name)).expect("short name contains no NUL");

        // SAFETY: `file` owns a live handle opened with the access and flags
        // SetFileShortNameW requires; `short_wide` is NUL-terminated and
        // remains alive for the call.
        let ok = unsafe { SetFileShortNameW(file.as_raw_handle(), short_wide.as_ptr()) };
        assert_ne!(
            ok,
            0,
            "SetFileShortNameW could not create the required 8.3 fixture: {}",
            std::io::Error::last_os_error(),
        );
    }

    /// A path already in long form must survive unchanged — this runs on
    /// every hook/inject call, so a mangling bug here would break
    /// injection everywhere rather than only on 8.3 paths.
    #[test]
    fn long_paths_pass_through_unchanged() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let file = tmp.path().join("some-file.dll");
        std::fs::write(&file, b"MZ").expect("write");

        let expected = module_list_spelling(&file);
        let actual = to_long_path(&expected);
        assert_same_windows_path(&actual, &expected);
    }

    /// The 8.3 short form of a path must expand back to the long form.
    /// This is the case that silently broke WaitForInjection: Windows
    /// reports module paths in long form, so a short path never matches.
    #[test]
    fn short_paths_expand_to_long_form() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let long = tmp.path().join("manifest-loader-probe.dll");
        std::fs::write(&long, b"MZ").expect("write");
        set_short_name(&long, "GMMLDR~1.DLL");

        let short = tmp.path().join("GMMLDR~1.DLL");
        assert!(
            short.exists(),
            "the explicit 8.3 fixture must resolve before normalisation: {short:?}",
        );

        let expected = module_list_spelling(&long);
        let expanded = to_long_path(&short);
        assert_same_windows_path(&expanded, &expected);
    }

    /// A path that doesn't exist can't have its 8.3 aliases expanded.
    /// It still comes back separator-normalised and absolute — the
    /// GetFullPathNameW half always applies — rather than erroring, so
    /// the caller gets a loader error rather than a path error.
    #[test]
    fn missing_paths_still_normalise_rather_than_erroring() {
        let missing = Path::new(r"C:\this\does\not\exist\anywhere\d3d11.dll");
        // Already absolute and backslash-separated, so it round-trips.
        assert_eq!(to_long_path(missing), missing.to_path_buf());

        // A missing path with forward slashes still gets normalised.
        let mixed = Path::new(r"C:/this/does/not/exist/d3d11.dll");
        let out = to_long_path(mixed);
        assert!(
            !out.to_string_lossy().contains('/'),
            "separator normalisation applies even when expansion fails, got {out:?}",
        );
    }

    /// `\\?\` verbatim paths must be unwrapped: the module list
    /// reports ordinary drive-letter paths, so a verbatim spelling
    /// fails the comparison exactly like an 8.3 one does.
    #[test]
    fn verbatim_prefix_is_stripped() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let file = tmp.path().join("probe.dll");
        std::fs::write(&file, b"MZ").expect("write");

        let verbatim = std::fs::canonicalize(&file).expect("canonicalize");
        assert!(
            verbatim.to_string_lossy().starts_with(r"\\?\"),
            "precondition: canonicalize should produce a verbatim path, got {verbatim:?}",
        );

        let out = to_long_path(&verbatim);
        assert!(
            !out.to_string_lossy().starts_with(r"\\?\"),
            "verbatim prefix must be stripped, got {out:?}",
        );
        let expected = module_list_spelling(&file);
        assert_same_windows_path(&out, &expected);
    }

    /// Forward slashes are legal input to most Windows APIs, but the
    /// module list always reports backslashes — so `WaitForInjection`'s
    /// literal comparison fails unless we normalise first. This is the
    /// exact shape that made injection *look* broken in CI while the
    /// DLL was in fact loaded in the target process the whole time.
    #[test]
    fn forward_slashes_are_rewritten_to_backslashes() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let nested = tmp.path().join("game");
        std::fs::create_dir_all(&nested).expect("dir");
        let file = nested.join("probe.dll");
        std::fs::write(&file, b"MZ").expect("write");

        // Same path spelled with a forward slash inside one join, the
        // way `join("game/Genshin Impact Game")` produces it.
        let mixed = PathBuf::from(format!("{}/game/probe.dll", tmp.path().display()));

        let normalised = to_long_path(&mixed);
        assert!(
            !normalised.to_string_lossy().contains('/'),
            "forward slashes must be rewritten, got {normalised:?}",
        );
        let expected = module_list_spelling(&file);
        assert_same_windows_path(&normalised, &expected);
    }

    /// The normalisation has to agree with what Windows itself reports
    /// for a loaded module — that string equality *is* the contract
    /// with WaitForInjection.
    #[test]
    fn normalised_path_matches_the_os_view_of_the_same_file() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let file = tmp.path().join("probe.dll");
        std::fs::write(&file, b"MZ").expect("write");

        let normalised = to_long_path(&file);
        let canonical = std::fs::canonicalize(&file).expect("canonicalize");
        // canonicalize returns a \\?\ verbatim path; the module list
        // does not use that form, so strip the prefix before comparing.
        let canonical = canonical.to_string_lossy().replace(r"\\?\", "");
        assert!(
            normalised
                .to_string_lossy()
                .eq_ignore_ascii_case(&canonical),
            "normalised {normalised:?} should match the OS view {canonical:?}",
        );
    }
}
