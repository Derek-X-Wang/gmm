//! Safe Rust bindings for the four entry points `3dmloader.dll` exposes
//! to clients (XXMI Launcher uses the same four through Python ctypes, see
//! `XXMI-Launcher/src/xxmi_launcher/core/utils/dll_injector.py`).
//!
//! The actual `3dmloader.dll` is GPLv3 (see `vendor/3dmloader/README.md`)
//! and so is this crate — see ADR 0001 in the repo root.
//!
//! ## Platform support
//!
//! The loader only does anything on Windows. On other platforms the public
//! API is still present but every call returns [`Error::UnsupportedPlatform`]
//! so the rest of the codebase can compile and unit-test against the same
//! types regardless of host OS. Real coverage runs on the Windows CI matrix
//! job + on a developer's Windows host via `cargo xtask test-loader`.

#![deny(unsafe_op_in_unsafe_fn)]

mod error;

pub use error::Error;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{HookSession, Loader};

#[cfg(not(windows))]
mod stub;
#[cfg(not(windows))]
pub use stub::{HookSession, Loader};
