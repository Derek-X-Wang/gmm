use std::path::PathBuf;

use thiserror::Error;

/// Errors returned by [`Loader`](crate::Loader) and [`HookSession`](crate::HookSession).
///
/// Every variant is constructed by the safe wrapper around the FFI call;
/// callers never see raw Win32 error codes or pointer values.
#[derive(Debug, Error)]
pub enum Error {
    /// The host operating system is not Windows. Every non-Windows call
    /// short-circuits with this variant so the rest of the codebase can
    /// compile.
    #[error("loader is only supported on Windows")]
    UnsupportedPlatform,

    /// `3dmloader.dll` could not be loaded from the path supplied to
    /// [`Loader::load`].
    #[error("failed to load {path:?}: {source}")]
    LoadLibrary {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// One of the four expected entry points was not exported by the
    /// supplied DLL. Almost always means the wrong DLL was vendored —
    /// see `vendor/3dmloader/README.md` for the pinned version.
    #[error("entry point {symbol:?} not found in loaded library")]
    MissingSymbol { symbol: &'static str },

    /// The underlying FFI call returned a non-zero status code. The
    /// upstream library does not document a stable error-code → meaning
    /// table, so we expose the integer verbatim along with the symbol
    /// that produced it.
    #[error("{symbol} returned status {status}")]
    NonZeroStatus { symbol: &'static str, status: i32 },

    /// A path was supplied that contained interior NUL bytes or could not
    /// be encoded as UTF-16. Both are misuses of the API, not failures
    /// from the upstream library.
    #[error("path is not valid UTF-16: {path:?}")]
    InvalidPath { path: PathBuf },
}
