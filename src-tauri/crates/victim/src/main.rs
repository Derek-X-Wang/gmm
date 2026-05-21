//! Controlled victim process for the loader smoke test.
//!
//! Creates a window with the class name `GMM-LOADER-TEST-VICTIM`, pumps
//! messages, and exits cleanly on `WM_CLOSE` or after `VICTIM_TIMEOUT_SECS`
//! seconds — whichever comes first. The window is intentionally hidden so
//! the test doesn't flash a window onto the CI runner's desktop.
//!
//! On non-Windows hosts the bin compiles to a tiny "Windows-only" stub so
//! the workspace builds end-to-end on the Linux CI runner and on a macOS
//! dev host. The real smoke test runs only on the Windows matrix job.

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!(
        "victim is a Windows-only test target. Run `cargo xtask test-loader` on a Windows host."
    );
    std::process::ExitCode::from(64) // EX_USAGE
}

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::ptr;

#[cfg(windows)]
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
#[cfg(windows)]
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, KillTimer, PeekMessageW, PostQuitMessage,
    RegisterClassExW, SetTimer, TranslateMessage, MSG, PM_REMOVE, WM_CLOSE, WM_DESTROY, WM_TIMER,
    WNDCLASSEXW, WS_OVERLAPPED,
};

#[cfg(windows)]
const VICTIM_WINDOW_CLASS: &str = "GMM-LOADER-TEST-VICTIM";
#[cfg(windows)]
const VICTIM_TIMEOUT_SECS: u32 = 30;
#[cfg(windows)]
const VICTIM_TIMER_ID: usize = 1;

#[cfg(windows)]
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: every branch returns; nothing leaks out except via WM_DESTROY's
    // PostQuitMessage which is the documented teardown path.
    unsafe {
        match msg {
            WM_TIMER if wparam == VICTIM_TIMER_ID as WPARAM => {
                // Fallback timeout — exit if nobody has closed us.
                KillTimer(hwnd, VICTIM_TIMER_ID);
                PostQuitMessage(0);
                0
            }
            WM_CLOSE => {
                PostQuitMessage(0);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(windows)]
fn to_wide_nul(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = OsStr::new(s).encode_wide().collect();
    v.push(0);
    v
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    let class_name = to_wide_nul(VICTIM_WINDOW_CLASS);

    // SAFETY: GetModuleHandleW(NULL) returns the current process module.
    let hinstance = unsafe { GetModuleHandleW(ptr::null()) };
    if hinstance.is_null() {
        eprintln!("victim: GetModuleHandleW failed");
        return std::process::ExitCode::FAILURE;
    }

    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: 0,
        lpfnWndProc: Some(wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance as _,
        hIcon: ptr::null_mut(),
        hCursor: ptr::null_mut(),
        hbrBackground: ptr::null_mut(),
        lpszMenuName: ptr::null(),
        lpszClassName: class_name.as_ptr(),
        hIconSm: ptr::null_mut(),
    };

    // SAFETY: the WNDCLASSEXW above is well-formed and lives for the call.
    let atom = unsafe { RegisterClassExW(&wc) };
    if atom == 0 {
        eprintln!("victim: RegisterClassExW failed");
        return std::process::ExitCode::FAILURE;
    }

    // SAFETY: class_name lives for the call. The window is intentionally
    // not shown (WS_OVERLAPPED without WS_VISIBLE).
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            100,
            100,
            ptr::null_mut(),
            ptr::null_mut(),
            hinstance as _,
            ptr::null(),
        )
    };
    if hwnd.is_null() {
        eprintln!("victim: CreateWindowExW failed");
        return std::process::ExitCode::FAILURE;
    }

    // SAFETY: hwnd is a freshly-created valid window handle.
    let timer_id = unsafe { SetTimer(hwnd, VICTIM_TIMER_ID, VICTIM_TIMEOUT_SECS * 1000, None) };
    if timer_id == 0 {
        eprintln!("victim: SetTimer failed");
        return std::process::ExitCode::FAILURE;
    }

    let mut msg = MSG {
        hwnd: ptr::null_mut(),
        message: 0,
        wParam: 0,
        lParam: 0,
        time: 0,
        pt: windows_sys::Win32::Foundation::POINT { x: 0, y: 0 },
    };

    // SAFETY: PeekMessageW + TranslateMessage + DispatchMessageW expect a
    // valid MSG buffer; `msg` is on this stack frame.
    unsafe {
        loop {
            while PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == windows_sys::Win32::UI::WindowsAndMessaging::WM_QUIT {
                    return std::process::ExitCode::SUCCESS;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            // Yield briefly so we don't burn a core.
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }
}
