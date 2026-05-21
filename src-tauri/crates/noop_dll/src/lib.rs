//! No-op DLL. Loaded into `victim.exe` by `cargo xtask test-loader` so
//! we can prove that `3dmloader.dll`'s hook → inject pipeline actually
//! fires.
//!
//! 3dmloader's `Injector.cpp` calls `GetProcAddress(module, "CBTProc")`
//! after `LoadLibraryW`ing the configured DLL. If the symbol isn't
//! exported, HookLibrary returns status 300 ("missing expected entry
//! point"). So we expose a `CBTProc` that just returns 0 — enough for
//! the smoke test to confirm the round-trip.

#![cfg(windows)]

use windows_sys::Win32::Foundation::{BOOL, HINSTANCE, LPARAM, LRESULT, WPARAM};

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_PROCESS_DETACH: u32 = 0;
const TRUE: BOOL = 1;

#[no_mangle]
#[allow(non_snake_case, unsafe_op_in_unsafe_fn)]
pub extern "system" fn DllMain(
    _hinst: HINSTANCE,
    reason: u32,
    _reserved: *const std::ffi::c_void,
) -> BOOL {
    match reason {
        DLL_PROCESS_ATTACH | DLL_PROCESS_DETACH => TRUE,
        _ => TRUE,
    }
}

/// CBT hook procedure stub. 3dmloader resolves this symbol on the
/// injected DLL via GetProcAddress. The body intentionally does
/// nothing — for the smoke test we only need 3dmloader to find an
/// exported symbol of the right name, so it stops returning status 300.
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn CBTProc(_n_code: i32, _w_param: WPARAM, _l_param: LPARAM) -> LRESULT {
    0
}
