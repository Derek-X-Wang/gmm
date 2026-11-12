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
use std::future::Future;

use crate::core::error::SurfaceFailureKind;
use crate::core::{Core, Error};

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

    /// Convert a typed core failure, naming every Mod its message mentions.
    ///
    /// Resolution happens here, at the command boundary, because a core path
    /// that holds a mutation fence must not read through a second connection,
    /// and a lookup whose only job is to explain a failure must not be able to
    /// replace that failure. [`Core::mod_names`] reads `self.pool`, a second
    /// connection, so it can only run once the fence is gone; it logs its own
    /// error and returns no names, which leaves the bare identifier. Reads
    /// through the fence's own transaction are unaffected. The failure kind
    /// and the identifier both survive: logs and database rows carry the
    /// identifier, so the name is added next to it, never in place of it.
    pub async fn resolved(error: Error, core: &Core) -> Self {
        let ids = error.mod_ids();
        if ids.is_empty() {
            return Self::from(error);
        }
        let names = core.mod_names(&ids).await;
        let kind = error.surface_failure_kind();
        let message = names.iter().fold(error.to_string(), |message, (id, name)| {
            name_mod(&message, id, name)
        });
        Self { kind, message }
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

impl From<crate::core::instance_lock::InstanceLockError> for CommandError {
    fn from(error: crate::core::instance_lock::InstanceLockError) -> Self {
        use crate::core::instance_lock::InstanceLockError;
        Self {
            kind: match &error {
                InstanceLockError::AlreadyRunning { .. } => SurfaceFailureKind::AlreadyRunning,
                InstanceLockError::Io { .. } => SurfaceFailureKind::Other,
            },
            message: error.to_string(),
        }
    }
}

/// Replace each whole-token occurrence of `id` with `"<name>" (<id>)`.
///
/// An occurrence glued to a path separator or an identifier character is part
/// of something else, usually the Library path whose last component is the
/// Mod ID, and rewriting it would corrupt a path the user may need to open.
fn name_mod(message: &str, id: &str, name: &str) -> String {
    let continues_a_token = |c: char| c.is_alphanumeric() || matches!(c, '/' | '\\');
    let replacement = format!("\"{name}\" ({id})");
    let mut resolved = String::with_capacity(message.len());
    let mut copied_to = 0;
    for (start, _) in message.match_indices(id) {
        let end = start + id.len();
        let glued = message[..start]
            .chars()
            .next_back()
            .is_some_and(continues_a_token)
            || message[end..].chars().next().is_some_and(continues_a_token);
        if glued {
            continue;
        }
        resolved.push_str(&message[copied_to..start]);
        resolved.push_str(&replacement);
        copied_to = end;
    }
    resolved.push_str(&message[copied_to..]);
    resolved
}

/// The one way a command turns a core failure into [`CommandError`].
///
/// `tests/command_error_boundary.rs` rejects `CommandError::from` inside any
/// `#[tauri::command]` body, so a command cannot skip Mod-name resolution.
pub trait SurfaceErrors<T> {
    fn surface(self, core: &Core) -> impl Future<Output = CommandResult<T>> + Send;
}

impl<T: Send> SurfaceErrors<T> for crate::core::Result<T> {
    async fn surface(self, core: &Core) -> CommandResult<T> {
        match self {
            Ok(value) => Ok(value),
            Err(error) => Err(CommandError::resolved(error, core).await),
        }
    }
}

pub type CommandResult<T> = std::result::Result<T, CommandError>;
