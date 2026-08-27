//! The one error boundary shared by every Tauri command.
//!
//! Core failures arrive here while they are still typed, so their stable
//! classification and existing user-facing message cross IPC together.

use serde::Serialize;

use crate::core::error::SurfaceFailureKind;
use crate::core::Error;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub kind: SurfaceFailureKind,
    pub message: String,
}

impl CommandError {
    /// Construct an explicitly unclassified command failure.
    ///
    /// Typed [`Error`] values should use [`From`] instead. Calling this is the
    /// deliberate escape hatch for command-local and third-party failures that
    /// do not have a GMM surface classification.
    pub fn other(message: impl Into<String>) -> Self {
        Self {
            kind: SurfaceFailureKind::Other,
            message: message.into(),
        }
    }
}

impl From<Error> for CommandError {
    fn from(error: Error) -> Self {
        Self {
            kind: error.surface_failure_kind(),
            message: error.to_string(),
        }
    }
}

pub type CommandResult<T> = std::result::Result<T, CommandError>;
