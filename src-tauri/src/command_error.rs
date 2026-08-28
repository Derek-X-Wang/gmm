//! The shared error boundary for Tauri commands authored in this crate.
//!
//! Core failures arrive here while they are still typed, so their stable
//! classification and existing user-facing message cross IPC together.
//!
//! `tests/command_error_boundary.rs` structurally checks literal
//! `#[tauri::command]` functions under this crate's `src/` tree and resolves
//! direct imports of [`CommandResult`] to this module. Like any source parser,
//! that gate cannot see macro-generated commands, an aliased command attribute,
//! commands outside `src/`, or a type routed through an arbitrary re-export;
//! those remain review responsibilities at the Tauri registration boundary.
//! Conditional imports deliberately do not establish a shared binding: a
//! cfg-gated command must name the fully qualified alias so another target
//! cannot replace its one-segment result type.

use serde::Serialize;
use std::fmt;

use crate::core::error::SurfaceFailureKind;
use crate::core::Error;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub kind: SurfaceFailureKind,
    pub message: String,
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CommandError {}

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

    /// Rewrite presentation text without discarding the stable failure kind.
    pub fn map_message(self, map: impl FnOnce(String) -> String) -> Self {
        Self {
            kind: self.kind,
            message: map(self.message),
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
