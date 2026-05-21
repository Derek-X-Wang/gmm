//! Non-Windows stub. Every public method returns [`Error::UnsupportedPlatform`].
//! The shape mirrors `windows::*` so call sites compile identically across
//! platforms.

use std::path::Path;

use crate::Error;

/// Owns a loaded `3dmloader.dll`. On non-Windows this is a unit struct.
#[derive(Clone)]
pub struct Loader {
    _private: (),
}

impl Loader {
    /// Pretends to load the library; always errors with
    /// [`Error::UnsupportedPlatform`] on non-Windows hosts.
    pub fn load(_dll_path: &Path) -> Result<Self, Error> {
        Err(Error::UnsupportedPlatform)
    }

    /// See [`crate::Loader::hook`] for the real signature.
    pub fn hook(
        &self,
        _target_window_class: &str,
        _dll_to_inject: &Path,
    ) -> Result<HookSession<'_>, Error> {
        Err(Error::UnsupportedPlatform)
    }

    /// See [`crate::Loader::inject`] for the real signature.
    pub fn inject(&self, _pid: u32, _dll_path: &Path) -> Result<(), Error> {
        Err(Error::UnsupportedPlatform)
    }
}

/// Lifetime token for a hook installed via [`Loader::hook`]. Dropping it
/// removes the hook. On non-Windows this is uninhabited at runtime.
pub struct HookSession<'loader> {
    _marker: std::marker::PhantomData<&'loader Loader>,
}

impl HookSession<'_> {
    /// See [`crate::HookSession::wait_for_injection`].
    pub fn wait_for_injection(&self, _injected_dll: &Path, _timeout_ms: i32) -> Result<(), Error> {
        Err(Error::UnsupportedPlatform)
    }
}
