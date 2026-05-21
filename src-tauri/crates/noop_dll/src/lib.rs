//! No-op DLL. Exports nothing, hooks nothing, just succeeds.
//! Loaded into `victim.exe` by `cargo xtask test-loader` so we can prove
//! that `3dmloader.dll`'s hook → inject pipeline actually fires.

#![cfg(windows)]

use windows_sys::Win32::Foundation::{BOOL, HINSTANCE};

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
