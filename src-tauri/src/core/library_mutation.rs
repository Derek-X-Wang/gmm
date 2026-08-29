//! One writer-fence protocol for every mutation of Library-owned bytes or a
//! Mod's enabled deployment state.
//!
//! Root relocation is the exclusive form: it holds SQLite's writer claim
//! from the row snapshot through the filesystem move and the setting/row
//! commit. Potentially unbounded producers (`adopt_folder` and `import_zip`)
//! use the staged form: resolve and identify the root under a short claim,
//! perform their copy/extract without blocking SQLite, then reacquire the
//! claim and prove both the configured root name and its filesystem identity
//! are unchanged before inserting a row. Recovery/delete keep their bounded
//! filesystem ownership acts inside one claim. Enabling or disabling a Mod
//! likewise holds the claim across both its Junction mutation and `enabled`
//! update. It first commits a durable transition witness, then applies the
//! Junction, flag update, and witness retirement under a second short claim.
//! The shared ownership guard makes that committed witness the logical bridge
//! between claims, so no other Library mutation can enter in the gap.
//! Active-Variant retargeting uses the same fence so recovery quarantine and
//! Variant deployment cannot pass one another after either operation's guard.
//! Reconcile and rebuild keep their unbounded traversal outside the fence, then
//! take one short claim to reload quarantine state and protect each bounded
//! Junction create or retarget from a concurrent recovery decision.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono::{DateTime, FixedOffset, Utc};
use sqlx::{Column, Row, Sqlite, SqliteConnection};
use ulid::Ulid;

use super::filesystem::metadata_if_exists;
use super::library_audit::{load_duplicate_mod_records, DuplicateResolution, ReviewedDuplicateMod};
use super::library_identity::{DirectoryIdentity, IdentifiedDirectory};
use super::library_ownership::{LibraryDirectoryOwner, LibraryOwnershipSnapshot};
use super::mods::{EnabledTransitionRecovery, ReinstallRecovery, ReinstallRecoveryOutcome};
#[cfg(not(any(windows, unix)))]
use super::same_path;
use super::settings::{get as get_setting, keys};
use super::{
    crash_points, junction, link_exists, path_within, resolve_link, volume, Core, Error, GameCode,
    Result,
};

pub(super) const REINSTALL_STAGING_PREFIX: &str = ".gmm-reinstall-";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LibraryMutation {
    AuditLibrary,
    FinishInterruptedDeletes,
    ResolveInterruptedStaging,
    ResolveEnabledTransition,
    RetryReinstallRecovery,
    WithdrawQuarantinedReinstallJunction,
    SetLibraryRoot,
    SetLibraryPathForGame,
    AdoptFolder,
    ImportZip,
    RecoverUnreferencedLibraryDir,
    DeleteUnreferencedLibraryDir,
    ReinstallGamebananaMod,
    SetEnabled,
    SetActiveVariant,
    ReconcileJunction,
    ResolveDuplicateMods,
}

impl LibraryMutation {
    pub(super) const fn function_name(self) -> &'static str {
        match self {
            Self::AuditLibrary => "audit_library",
            Self::FinishInterruptedDeletes => "finish_interrupted_library_deletes",
            Self::ResolveInterruptedStaging => "resolve_interrupted_staging_at_startup",
            Self::ResolveEnabledTransition => "resolve_enabled_transition",
            Self::RetryReinstallRecovery => "retry_reinstall_recovery",
            Self::WithdrawQuarantinedReinstallJunction => "withdraw_quarantined_reinstall_junction",
            Self::SetLibraryRoot => "set_library_root",
            Self::SetLibraryPathForGame => "set_library_path_for_game",
            Self::AdoptFolder => "adopt_folder",
            Self::ImportZip => "import_zip",
            Self::RecoverUnreferencedLibraryDir => "recover_unreferenced_library_dir",
            Self::DeleteUnreferencedLibraryDir => "delete_unreferenced_library_dir",
            Self::ReinstallGamebananaMod => "reinstall_gamebanana_mod_with_endpoints",
            Self::SetEnabled => "set_enabled",
            Self::SetActiveVariant => "set_active_variant",
            Self::ReconcileJunction => "reconcile_junction",
            Self::ResolveDuplicateMods => "resolve_duplicate_mods",
        }
    }
}

pub(super) struct LibraryMutationFence {
    pub(super) transaction: sqlx::Transaction<'static, Sqlite>,
}

impl LibraryMutationFence {
    pub(super) async fn commit(self) -> Result<()> {
        self.transaction.commit().await?;
        Ok(())
    }
}

/// The root name and filesystem object a staged row committer selected before
/// its unbounded filesystem work.
pub(super) struct LibraryRootSnapshot {
    game: GameCode,
    root: IdentifiedDirectory,
}

impl LibraryRootSnapshot {
    pub(super) fn path(&self) -> &Path {
        self.root.path()
    }
}

/// Identity evidence for bytes staged outside the writer fence.
pub(super) struct StagedLibraryDirectory {
    id: String,
    directory: IdentifiedDirectory,
}

#[derive(Debug, Clone)]
pub(super) struct ReinstallSwapWitness {
    token: Ulid,
    mod_id: String,
    game: GameCode,
    library_path: PathBuf,
    staged_path: PathBuf,
    quarantine_path: PathBuf,
    old_identity: DirectoryIdentity,
    staged_identity: DirectoryIdentity,
    created_at: DateTime<FixedOffset>,
    recovery_error: Option<String>,
    recovery_attempted_at: Option<String>,
    recovery_attempts: u32,
    junction_withdrawn: bool,
    junction_withdrawal_error: Option<String>,
}

pub(super) struct NewReinstallSwapWitness<'a> {
    pub(super) token: Ulid,
    pub(super) mod_id: &'a str,
    pub(super) game: GameCode,
    pub(super) library_path: &'a Path,
    pub(super) staged_path: &'a Path,
    pub(super) quarantine_path: &'a Path,
    pub(super) old_identity: &'a DirectoryIdentity,
    pub(super) staged_identity: &'a DirectoryIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StagedLibraryOperation {
    AdoptFolder,
    ImportZip,
}

#[derive(Debug, Clone)]
pub(super) struct StagedLibraryOperationWitness {
    id: Ulid,
    staged_path: PathBuf,
    staged_identity: DirectoryIdentity,
    created_at: DateTime<FixedOffset>,
    recovery_error: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct EnabledTransitionWitness {
    mod_id: String,
    game: GameCode,
    intended_enabled: bool,
    junction_path: PathBuf,
    junction_target: Option<PathBuf>,
    junction_target_identity: DirectoryIdentity,
    library_identity: DirectoryIdentity,
    junction_parent_identity: DirectoryIdentity,
    junction_identity: Option<DirectoryIdentity>,
    owner_pid: u32,
    owner_started_at: Option<u64>,
    owner_active: bool,
    created_at: DateTime<FixedOffset>,
    recovery_error: Option<String>,
    recovery_attempted_at: Option<String>,
    recovery_attempts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReconciledJunctionMutation {
    Applied,
    Quarantined,
    Stale,
}

/// Declare every durable witness table once, deriving each raw row decoder,
/// its ordered column registry, and the table registry used by the structural
/// ownership test.
///
/// The exhaustive destructure in `validate` is deliberately outside the macro:
/// adding a field changes the accepted schema and raw type together, but still
/// cannot compile until the owning module gives that field a validation rule.
macro_rules! define_unvalidated_witness_tables {
    ($(
        table $table_const:ident = $table_name:literal;
        columns $columns:ident;
        raw $raw:ident;
        schema_error |$raw_value:ident, $actual_columns:ident| $schema_error:expr;
        fields { $($field:ident: $field_type:ty),+ $(,)? }
    )+) => {
        /// Durable witness tables whose SQL access belongs to this module.
        #[doc(hidden)]
        pub const DURABLE_WITNESS_TABLES: &[&str] = &[$($table_name),+];

        $(
            const $table_const: &str = $table_name;

            #[doc(hidden)]
            pub const $columns: &[&str] = &[$(stringify!($field)),+];

            struct $raw {
                $($field: $field_type),+
            }

            impl $raw {
                fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self> {
                    let $raw_value = Self {
                        $($field: row.try_get(stringify!($field))?),+
                    };
                    let $actual_columns: Vec<_> =
                        row.columns().iter().map(Column::name).collect();
                    if $actual_columns.as_slice() != $columns {
                        return Err($schema_error);
                    }
                    Ok($raw_value)
                }
            }
        )+
    };
}

define_unvalidated_witness_tables! {
    table REINSTALL_SWAPS_TABLE = "reinstall_swaps";
    columns REINSTALL_SWAP_COLUMNS;
    raw UnvalidatedReinstallSwapWitness;
    schema_error |raw, columns| Error::ReinstallWitnessCorrupt {
        mod_id: raw.mod_id.clone(),
        reason: format!(
            "the reinstall_swaps schema columns changed from the ruled set: {columns:?}"
        ),
    };
    fields {
        token: String,
        mod_id: String,
        game_code: String,
        library_path: String,
        staged_path: String,
        quarantine_path: String,
        old_identity: String,
        staged_identity: String,
        created_at: String,
        recovery_error: Option<String>,
        recovery_attempted_at: Option<String>,
        recovery_attempts: i64,
        junction_withdrawn: i64,
        junction_withdrawal_error: Option<String>,
    }

    table STAGED_LIBRARY_OPERATIONS_TABLE = "staged_library_operations";
    columns STAGED_LIBRARY_OPERATION_COLUMNS;
    raw UnvalidatedStagedLibraryOperationWitness;
    schema_error |raw, columns| Error::StagingWitnessCorrupt {
        id: raw.id.clone(),
        reason: format!(
            "the staged_library_operations schema columns changed from the ruled set: {columns:?}"
        ),
    };
    fields {
        id: String,
        game_code: String,
        operation: String,
        staged_path: String,
        staged_identity: String,
        created_at: String,
        recovery_error: Option<String>,
        recovery_attempted_at: Option<String>,
        recovery_attempts: i64,
    }

    table ENABLED_TRANSITIONS_TABLE = "enabled_transitions";
    columns ENABLED_TRANSITION_COLUMNS;
    raw UnvalidatedEnabledTransitionWitness;
    schema_error |raw, columns| Error::EnabledTransitionWitnessCorrupt {
        mod_id: raw.mod_id.clone(),
        reason: format!(
            "the enabled_transitions schema columns changed from the ruled set: {columns:?}"
        ),
    };
    fields {
        mod_id: String,
        game_code: String,
        intended_enabled: i64,
        junction_path: String,
        junction_target: Option<String>,
        junction_parent_identity: String,
        junction_identity: Option<String>,
        owner_pid: i64,
        owner_started_at: Option<i64>,
        owner_active: i64,
        created_at: String,
        recovery_error: Option<String>,
        recovery_attempted_at: Option<String>,
        recovery_attempts: i64,
        junction_target_identity: Option<String>,
        library_identity: Option<String>,
    }
}

impl UnvalidatedReinstallSwapWitness {
    fn validate(self) -> Result<ReinstallSwapWitness> {
        let Self {
            token,
            mod_id,
            game_code,
            library_path,
            staged_path,
            quarantine_path,
            old_identity,
            staged_identity,
            created_at,
            recovery_error,
            recovery_attempted_at,
            recovery_attempts,
            junction_withdrawn,
            junction_withdrawal_error,
        } = self;
        let corrupt = |reason| Error::ReinstallWitnessCorrupt {
            mod_id: mod_id.clone(),
            reason,
        };
        let token = Ulid::from_string(&token)
            .map_err(|_| corrupt(format!("the swap token {token:?} is not a ULID")))?;
        Ulid::from_string(&mod_id)
            .map_err(|_| corrupt(format!("the Mod ID {mod_id:?} is not a ULID")))?;
        let game = GameCode::from_str(&game_code).map_err(|_| {
            corrupt(format!(
                "the recorded value {game_code:?} is an invalid game code"
            ))
        })?;
        // Every durable field is decoded here before this row becomes trusted.
        // Recovery metadata is retained because it affects user-visible retry
        // and withdrawal status even though it is not filesystem identity
        // evidence.
        let old_identity = DirectoryIdentity::from_durable_key(&old_identity).ok_or_else(|| {
            corrupt(format!(
                "the old directory identity {old_identity:?} is not a canonical durable identity"
            ))
        })?;
        let staged_identity = DirectoryIdentity::from_durable_key(&staged_identity).ok_or_else(|| {
            corrupt(format!(
                "the staged directory identity {staged_identity:?} is not a canonical durable identity"
            ))
        })?;
        let created_at = DateTime::parse_from_rfc3339(&created_at).map_err(|_| {
            corrupt(format!(
                "the created-at value {created_at:?} is not an RFC 3339 timestamp"
            ))
        })?;
        let recovery_attempts = u32::try_from(recovery_attempts).map_err(|_| {
            corrupt(format!(
                "the recovery-attempt count {recovery_attempts} is outside the supported range"
            ))
        })?;
        let junction_withdrawn = match junction_withdrawn {
            0 => false,
            1 => true,
            value => {
                return Err(corrupt(format!(
                    "the junction-withdrawn flag {value} is not zero or one"
                )))
            }
        };
        if recovery_error.is_some() && recovery_attempted_at.is_none() {
            return Err(corrupt(
                "a recorded recovery error has no recovery-attempt timestamp".to_string(),
            ));
        }
        let witness = ReinstallSwapWitness {
            token,
            mod_id,
            game,
            library_path: PathBuf::from(library_path),
            staged_path: PathBuf::from(staged_path),
            quarantine_path: PathBuf::from(quarantine_path),
            old_identity,
            staged_identity,
            created_at,
            recovery_error,
            recovery_attempted_at,
            recovery_attempts,
            junction_withdrawn,
            junction_withdrawal_error,
        };
        #[allow(
            clippy::disallowed_methods,
            reason = "Library mutation policy exemption: ReinstallSwapWitness::validate_paths only validates the three recorded paths"
        )]
        witness.validate_paths()?;
        Ok(witness)
    }
}

impl ReinstallSwapWitness {
    pub(super) fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self> {
        UnvalidatedReinstallSwapWitness::from_row(row)?.validate()
    }

    pub(super) fn mod_id(&self) -> &str {
        &self.mod_id
    }

    pub(super) fn library_path(&self) -> &Path {
        &self.library_path
    }

    pub(super) fn staged_path(&self) -> &Path {
        &self.staged_path
    }

    pub(super) fn old_identity(&self) -> &DirectoryIdentity {
        &self.old_identity
    }

    pub(super) fn staged_identity(&self) -> &DirectoryIdentity {
        &self.staged_identity
    }

    pub(super) fn quarantine_path(&self) -> &Path {
        &self.quarantine_path
    }

    pub(super) fn token(&self) -> Ulid {
        self.token
    }

    fn created_at(&self) -> DateTime<FixedOffset> {
        self.created_at
    }

    pub(super) fn is_quarantined(&self) -> bool {
        self.recovery_error.is_some()
    }

    fn junction_withdrawn(&self) -> bool {
        self.junction_withdrawn
    }

    pub(super) fn recovery(&self) -> Option<ReinstallRecovery> {
        self.recovery_error
            .as_ref()
            .map(|reason| ReinstallRecovery {
                reason: reason.clone(),
                attempted_at: self
                    .recovery_attempted_at
                    .clone()
                    .expect("validated recovery errors have an attempt timestamp"),
                attempts: self.recovery_attempts,
                library_path: self.library_path.clone(),
                staged_path: self.staged_path.clone(),
                quarantine_path: self.quarantine_path.clone(),
                junction_withdrawn: self.junction_withdrawn,
                junction_withdrawal_error: self.junction_withdrawal_error.clone(),
            })
    }

    fn validate_paths(&self) -> Result<()> {
        let Some(root) = self.library_path.parent() else {
            return self.corrupt("the recorded live path has no Library root");
        };
        let Some(staged_root) = self.staged_path.parent() else {
            return self.corrupt("the recorded staging path has no Library root");
        };
        let Some(quarantine_root) = self.quarantine_path.parent() else {
            return self.corrupt("the recorded quarantine path has no Library root");
        };
        let expected_stage_name = format!("{REINSTALL_STAGING_PREFIX}{}", self.token);
        let expected_quarantine_name = format!(
            "{}{}",
            super::library_recovery::DELETE_QUARANTINE_PREFIX,
            self.token
        );
        if self.library_path.file_name().and_then(|name| name.to_str()) != Some(&self.mod_id)
            || self.staged_path.file_name().and_then(|name| name.to_str())
                != Some(expected_stage_name.as_str())
            || self
                .quarantine_path
                .file_name()
                .and_then(|name| name.to_str())
                != Some(expected_quarantine_name.as_str())
            || !super::same_path(staged_root, root)
            || !super::same_path(quarantine_root, root)
        {
            return self.corrupt("the recorded swap paths do not match the Mod ID and token");
        }
        Ok(())
    }

    fn corrupt<T>(&self, reason: impl Into<String>) -> Result<T> {
        Err(Error::ReinstallWitnessCorrupt {
            mod_id: self.mod_id.clone(),
            reason: reason.into(),
        })
    }

    fn uncertain<T>(&self, reason: impl Into<String>) -> Result<T> {
        Err(Error::ReinstallRecoveryUncertain {
            mod_id: self.mod_id.clone(),
            reason: reason.into(),
        })
    }
}

impl UnvalidatedStagedLibraryOperationWitness {
    fn validate(self) -> Result<StagedLibraryOperationWitness> {
        let Self {
            id,
            game_code,
            operation,
            staged_path,
            staged_identity,
            created_at,
            recovery_error,
            recovery_attempted_at,
            recovery_attempts,
        } = self;
        let corrupt = |reason| Error::StagingWitnessCorrupt {
            id: id.clone(),
            reason,
        };
        let parsed_id = Ulid::from_string(&id)
            .map_err(|_| corrupt(format!("the operation ID {id:?} is not a ULID")))?;
        let _game = GameCode::from_str(&game_code).map_err(|_| {
            corrupt(format!(
                "the recorded value {game_code:?} is an invalid game code"
            ))
        })?;
        let _operation = match operation.as_str() {
            "adopt" => StagedLibraryOperation::AdoptFolder,
            "import_zip" => StagedLibraryOperation::ImportZip,
            _ => {
                return Err(corrupt(format!(
                    "the operation value {operation:?} is not supported"
                )))
            }
        };
        let staged_path = PathBuf::from(staged_path);
        if staged_path.file_name().and_then(|name| name.to_str()) != Some(id.as_str()) {
            return Err(corrupt(
                "the staged path is not named by the operation ID".to_string(),
            ));
        }
        let staged_identity =
            DirectoryIdentity::from_durable_key(&staged_identity).ok_or_else(|| {
                corrupt(format!(
                    "the staged directory identity {staged_identity:?} is not a canonical durable identity"
                ))
            })?;
        let created_at = DateTime::parse_from_rfc3339(&created_at).map_err(|_| {
            corrupt(format!(
                "the created-at value {created_at:?} is not an RFC 3339 timestamp"
            ))
        })?;
        let _recovery_attempts = u32::try_from(recovery_attempts).map_err(|_| {
            corrupt(format!(
                "the recovery-attempt count {recovery_attempts} is outside the supported range"
            ))
        })?;
        if recovery_error.is_some() && recovery_attempted_at.is_none() {
            return Err(corrupt(
                "a recorded recovery error has no recovery-attempt timestamp".to_string(),
            ));
        }
        Ok(StagedLibraryOperationWitness {
            id: parsed_id,
            staged_path,
            staged_identity,
            created_at,
            recovery_error,
        })
    }
}

impl StagedLibraryOperationWitness {
    pub(super) fn id(&self) -> String {
        self.id.to_string()
    }

    pub(super) fn staged_path(&self) -> &Path {
        &self.staged_path
    }

    pub(super) fn staged_identity(&self) -> &DirectoryIdentity {
        &self.staged_identity
    }

    pub(super) fn is_active(&self) -> bool {
        self.recovery_error.is_none()
    }

    fn created_at(&self) -> DateTime<FixedOffset> {
        self.created_at
    }
}

impl UnvalidatedEnabledTransitionWitness {
    fn validate(self) -> Result<EnabledTransitionWitness> {
        let Self {
            mod_id,
            game_code,
            intended_enabled,
            junction_path,
            junction_target,
            junction_parent_identity,
            junction_identity,
            owner_pid,
            owner_started_at,
            owner_active,
            created_at,
            recovery_error,
            recovery_attempted_at,
            recovery_attempts,
            junction_target_identity,
            library_identity,
        } = self;
        let corrupt = |reason| Error::EnabledTransitionWitnessCorrupt {
            mod_id: mod_id.clone(),
            reason,
        };
        Ulid::from_string(&mod_id)
            .map_err(|_| corrupt(format!("the Mod ID {mod_id:?} is not a ULID")))?;
        let game = GameCode::from_str(&game_code).map_err(|_| {
            corrupt(format!(
                "the recorded value {game_code:?} is an invalid game code"
            ))
        })?;
        let intended_enabled = match intended_enabled {
            0 => false,
            1 => true,
            value => {
                return Err(corrupt(format!(
                    "the intended-enabled flag {value} is not zero or one"
                )))
            }
        };
        let junction_path = PathBuf::from(junction_path);
        if junction_path.file_name().is_none() {
            return Err(corrupt(
                "the recorded Junction path has no entry name".to_string(),
            ));
        }
        let junction_target = junction_target
            .map(PathBuf::from)
            .ok_or_else(|| corrupt("the recorded Junction target is missing".to_string()))?;
        let junction_target_identity = junction_target_identity
            .and_then(|identity| DirectoryIdentity::from_durable_key(&identity))
            .ok_or_else(|| {
                corrupt("the Junction-target identity is missing or not canonical".to_string())
            })?;
        let library_identity = library_identity
            .and_then(|identity| DirectoryIdentity::from_durable_key(&identity))
            .ok_or_else(|| {
                corrupt("the Mod Library identity is missing or not canonical".to_string())
            })?;
        let junction_parent_identity =
            DirectoryIdentity::from_durable_key(&junction_parent_identity).ok_or_else(|| {
                corrupt(format!(
                    "the Junction-parent identity {junction_parent_identity:?} is not a canonical durable identity"
                ))
            })?;
        let junction_identity = junction_identity
            .map(|identity| {
                DirectoryIdentity::from_durable_key(&identity).ok_or_else(|| {
                    corrupt(format!(
                        "the Junction identity {identity:?} is not a canonical durable identity"
                    ))
                })
            })
            .transpose()?;
        if intended_enabled == junction_identity.is_some() {
            return Err(corrupt(
                "only a disable transition may carry the original Junction identity".to_string(),
            ));
        }
        let owner_pid = u32::try_from(owner_pid).map_err(|_| {
            corrupt(format!(
                "the owner PID {owner_pid} is outside the supported range"
            ))
        })?;
        let owner_started_at = owner_started_at
            .map(|value| {
                u64::try_from(value).map_err(|_| {
                    corrupt(format!(
                        "the owner start time {value} is outside the supported range"
                    ))
                })
            })
            .transpose()?;
        let owner_active = match owner_active {
            0 => false,
            1 => true,
            value => {
                return Err(corrupt(format!(
                    "the owner-active flag {value} is not zero or one"
                )))
            }
        };
        let created_at = DateTime::parse_from_rfc3339(&created_at).map_err(|_| {
            corrupt(format!(
                "the created-at value {created_at:?} is not an RFC 3339 timestamp"
            ))
        })?;
        let recovery_attempts = u32::try_from(recovery_attempts).map_err(|_| {
            corrupt(format!(
                "the recovery-attempt count {recovery_attempts} is outside the supported range"
            ))
        })?;
        if recovery_error.is_some() && recovery_attempted_at.is_none() {
            return Err(corrupt(
                "a recorded recovery error has no recovery-attempt timestamp".to_string(),
            ));
        }
        Ok(EnabledTransitionWitness {
            mod_id,
            game,
            intended_enabled,
            junction_path,
            junction_target: Some(junction_target),
            junction_target_identity,
            library_identity,
            junction_parent_identity,
            junction_identity,
            owner_pid,
            owner_started_at,
            owner_active,
            created_at,
            recovery_error,
            recovery_attempted_at,
            recovery_attempts,
        })
    }
}

impl EnabledTransitionWitness {
    pub(super) fn mod_id(&self) -> &str {
        &self.mod_id
    }

    pub(super) fn game(&self) -> GameCode {
        self.game
    }

    fn intended_enabled(&self) -> bool {
        self.intended_enabled
    }

    fn junction_path(&self) -> &Path {
        &self.junction_path
    }

    fn junction_target(&self) -> Option<&Path> {
        self.junction_target.as_deref()
    }

    fn junction_target_identity(&self) -> &DirectoryIdentity {
        &self.junction_target_identity
    }

    pub(super) fn library_identity(&self) -> &DirectoryIdentity {
        &self.library_identity
    }

    fn junction_parent_identity(&self) -> &DirectoryIdentity {
        &self.junction_parent_identity
    }

    fn junction_identity(&self) -> Option<&DirectoryIdentity> {
        self.junction_identity.as_ref()
    }

    fn created_at(&self) -> DateTime<FixedOffset> {
        self.created_at
    }

    fn owner_is_live(&self) -> bool {
        self.owner_active
            && matches!(
                self.owner_identity_state(),
                super::session::ProcessIdentityState::Matches
                    | super::session::ProcessIdentityState::Unknown
            )
    }

    fn owner_identity_state(&self) -> super::session::ProcessIdentityState {
        super::session::process_identity_state(self.owner_pid, self.owner_started_at)
    }

    fn corrupt<T>(&self, reason: impl Into<String>) -> Result<T> {
        Err(Error::EnabledTransitionWitnessCorrupt {
            mod_id: self.mod_id.clone(),
            reason: reason.into(),
        })
    }

    pub(super) fn recovery(&self) -> Option<EnabledTransitionRecovery> {
        let owner_uncertain = self.owner_active
            && matches!(
                self.owner_identity_state(),
                super::session::ProcessIdentityState::Unknown
            );
        let reason = match (&self.recovery_error, owner_uncertain) {
            (Some(reason), _) => reason.clone(),
            (None, true) => {
                "GMM cannot establish whether the original producer is still running".to_string()
            }
            (None, false) => return None,
        };
        Some(EnabledTransitionRecovery {
            intended_enabled: self.intended_enabled,
            reason,
            attempted_at: self
                .recovery_attempted_at
                .clone()
                .unwrap_or_else(|| self.created_at.to_rfc3339()),
            attempts: self.recovery_attempts,
            junction_path: self.junction_path.clone(),
            owner_uncertain,
        })
    }
}

pub(super) async fn load_reinstall_swap_witnesses(
    connection: &mut SqliteConnection,
) -> Result<Vec<ReinstallSwapWitness>> {
    let query = format!("SELECT * FROM {REINSTALL_SWAPS_TABLE}");
    let witnesses: Vec<_> = sqlx::query(&query)
        .persistent(false)
        .fetch_all(connection)
        .await?
        .iter()
        .map(ReinstallSwapWitness::from_row)
        .collect::<Result<_>>()?;
    let mut tokens = HashMap::new();
    let mut mod_ids = HashMap::new();
    for witness in &witnesses {
        if let Some(first_mod_id) = tokens.insert(witness.token(), witness.mod_id().to_string()) {
            return Err(Error::ReinstallWitnessCorrupt {
                mod_id: witness.mod_id().to_string(),
                reason: format!(
                    "the swap token {} appears more than once, for Mods {first_mod_id} and {}",
                    witness.token(),
                    witness.mod_id(),
                ),
            });
        }
        if let Some(first_token) = mod_ids.insert(witness.mod_id().to_string(), witness.token()) {
            return Err(Error::ReinstallWitnessCorrupt {
                mod_id: witness.mod_id().to_string(),
                reason: format!(
                    "the Mod ID {} has more than one reinstall witness, with tokens {first_token} and {}",
                    witness.mod_id(),
                    witness.token(),
                ),
            });
        }
    }
    Ok(witnesses)
}

pub(super) async fn load_staged_library_operation_witnesses(
    connection: &mut SqliteConnection,
) -> Result<Vec<StagedLibraryOperationWitness>> {
    let query = format!("SELECT * FROM {STAGED_LIBRARY_OPERATIONS_TABLE}");
    sqlx::query(&query)
        .persistent(false)
        .fetch_all(connection)
        .await?
        .iter()
        .map(|row| UnvalidatedStagedLibraryOperationWitness::from_row(row)?.validate())
        .collect()
}

pub(super) async fn load_enabled_transition_witnesses(
    connection: &mut SqliteConnection,
) -> Result<Vec<EnabledTransitionWitness>> {
    let query = format!("SELECT * FROM {ENABLED_TRANSITIONS_TABLE}");
    sqlx::query(&query)
        .persistent(false)
        .fetch_all(connection)
        .await?
        .iter()
        .map(|row| UnvalidatedEnabledTransitionWitness::from_row(row)?.validate())
        .collect()
}

pub(super) async fn insert_reinstall_swap_witness(
    connection: &mut SqliteConnection,
    witness: NewReinstallSwapWitness<'_>,
) -> Result<()> {
    #[allow(
        clippy::disallowed_methods,
        reason = "Library mutation policy exemption: sqlx Query::execute persists the witness row and does not mutate Library-owned bytes"
    )]
    sqlx::query(
        "INSERT INTO reinstall_swaps (
            token, mod_id, game_code, library_path, staged_path,
            quarantine_path, old_identity, staged_identity, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(witness.token.to_string())
    .bind(witness.mod_id)
    .bind(witness.game.as_str())
    .bind(witness.library_path.to_string_lossy().as_ref())
    .bind(witness.staged_path.to_string_lossy().as_ref())
    .bind(witness.quarantine_path.to_string_lossy().as_ref())
    .bind(witness.old_identity.durable_key())
    .bind(witness.staged_identity.durable_key())
    .bind(Utc::now().to_rfc3339())
    .execute(connection)
    .await?;
    Ok(())
}

pub(super) async fn delete_reinstall_swap_witness(
    connection: &mut SqliteConnection,
    token: Ulid,
) -> Result<()> {
    sqlx::query("DELETE FROM reinstall_swaps WHERE token = ?")
        .bind(token.to_string())
        .execute(connection)
        .await?;
    Ok(())
}

impl StagedLibraryDirectory {
    pub(super) fn path(&self) -> &Path {
        self.directory.path()
    }

    fn identity_key(&self) -> String {
        self.directory.identity().durable_key()
    }
}

impl Core {
    pub(super) async fn reinstall_swap_witnesses(&self) -> Result<Vec<ReinstallSwapWitness>> {
        let mut connection = self.pool.acquire().await?;
        load_reinstall_swap_witnesses(&mut connection).await
    }

    pub(super) async fn begin_library_mutation(
        &self,
        mutation: LibraryMutation,
    ) -> Result<LibraryMutationFence> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if !matches!(
            mutation,
            LibraryMutation::AuditLibrary
                | LibraryMutation::ResolveEnabledTransition
                | LibraryMutation::FinishInterruptedDeletes
                | LibraryMutation::ResolveInterruptedStaging
                | LibraryMutation::WithdrawQuarantinedReinstallJunction
        ) {
            self.prune_stale_session_launch_claims(&mut transaction)
                .await?;
            self.ensure_no_active_session_in_library_mutation(&mut transaction)
                .await?;
        }
        if !matches!(
            mutation,
            LibraryMutation::AuditLibrary | LibraryMutation::ResolveEnabledTransition
        ) {
            if let Some(mod_id) =
                LibraryOwnershipSnapshot::enabled_transition_mod_ids(&mut transaction)
                    .await?
                    .into_iter()
                    .next()
            {
                return Err(Error::EnabledTransitionPending { mod_id });
            }
        }
        Ok(LibraryMutationFence { transaction })
    }

    /// Keep the one Mod record the user selected and discard only the other
    /// records they reviewed in the duplicate audit.
    ///
    /// SQLite cannot lock filesystem identity, so the BEGIN IMMEDIATE fence
    /// covers a fresh identity snapshot, Junction preflight/removal, and the
    /// row deletion. The caller supplies the exact reviewed IDs; any drift
    /// refuses the action instead of silently widening the user's choice.
    pub async fn resolve_duplicate_mods(
        &self,
        keeper_id: &str,
        reviewed_mods: &[ReviewedDuplicateMod],
    ) -> Result<DuplicateResolution> {
        let reviewed_fingerprints: HashMap<_, _> = reviewed_mods
            .iter()
            .map(|record| (record.id.clone(), record.fingerprint.clone()))
            .collect();
        let reviewed: HashSet<_> = reviewed_fingerprints.keys().cloned().collect();
        if reviewed.len() < 2 || reviewed.len() != reviewed_mods.len() {
            return Err(Error::DuplicateModResolutionChanged {
                reason: "the reviewed record list is incomplete or contains the same record twice"
                    .to_string(),
            });
        }
        if !reviewed.contains(keeper_id) {
            return Err(Error::DuplicateModResolutionChanged {
                reason: "the selected keeper is not one of the reviewed records".to_string(),
            });
        }

        let mut fence = self
            .begin_library_mutation(LibraryMutation::ResolveDuplicateMods)
            .await?;
        let ownership = LibraryOwnershipSnapshot::load(&mut fence.transaction).await?;
        let keeper_path: Option<String> =
            sqlx::query_scalar("SELECT library_path FROM mods WHERE id = ?")
                .bind(keeper_id)
                .fetch_optional(&mut *fence.transaction)
                .await?;
        let keeper_path = keeper_path.ok_or_else(|| Error::DuplicateModResolutionChanged {
            reason: format!("the selected keeper {keeper_id} no longer exists"),
        })?;
        let keeper_path = PathBuf::from(keeper_path);
        let keeper_directory = IdentifiedDirectory::open(&keeper_path).map_err(|source| {
            Error::DuplicateModResolutionChanged {
                reason: format!(
                    "the selected keeper's Library directory could not be identified: {source}"
                ),
            }
        })?;
        let current: HashSet<_> = ownership
            .mod_ids_for(keeper_directory.identity())
            .iter()
            .cloned()
            .collect();
        if current != reviewed {
            return Err(Error::DuplicateModResolutionChanged {
                reason: "the set of Mod records naming this directory changed".to_string(),
            });
        }

        for witness in load_reinstall_swap_witnesses(&mut fence.transaction).await? {
            if reviewed.contains(witness.mod_id()) {
                return Err(Error::DuplicateModResolutionBlockedByReinstall {
                    mod_id: witness.mod_id().to_string(),
                });
            }
        }

        let current_records = load_duplicate_mod_records(&mut fence.transaction, &reviewed).await?;
        if current_records.len() != reviewed.len()
            || current_records
                .iter()
                .any(|(id, record)| reviewed_fingerprints.get(id) != Some(&record.fingerprint))
        {
            return Err(Error::DuplicateModResolutionChanged {
                reason: "one or more reviewed records changed after the audit".to_string(),
            });
        }

        let rows = sqlx::query(
            "SELECT m.id, m.game_code, m.library_path, m.junction_dir_name,
                    m.enabled, g.install_path
             FROM mods m JOIN games g ON g.code = m.game_code",
        )
        .fetch_all(&mut *fence.transaction)
        .await?;
        let mut surviving_junctions = Vec::new();
        for row in &rows {
            let mod_id: String = row.try_get("id")?;
            if reviewed.contains(&mod_id) && mod_id != keeper_id {
                continue;
            }
            let Some(install_path) = row.try_get::<Option<String>, _>("install_path")? else {
                continue;
            };
            surviving_junctions.push((
                mod_id,
                PathBuf::from(install_path)
                    .join("Mods")
                    .join(row.try_get::<String, _>("junction_dir_name")?),
            ));
        }
        let mut seen = HashSet::new();
        let mut junctions = Vec::new();
        for row in rows {
            let mod_id: String = row.try_get("id")?;
            if !reviewed.contains(&mod_id) || mod_id == keeper_id {
                continue;
            }
            seen.insert(mod_id.clone());
            let game_code: String = row.try_get("game_code")?;
            let enabled = row.try_get::<i64, _>("enabled")? != 0;
            let library_path = PathBuf::from(row.try_get::<String, _>("library_path")?);
            if enabled {
                let target = self
                    .junction_target_for(&mod_id, &library_path, &mut *fence.transaction)
                    .await?;
                let target_metadata = metadata_if_exists(&target).map_err(|source| Error::Io {
                    path: target.clone(),
                    source,
                })?;
                // Safe: the fallible lookup above preserved I/O uncertainty.
                if !target_metadata.is_some_and(|metadata| metadata.is_dir()) {
                    return Err(Error::DuplicateModResolutionChanged {
                        reason: format!(
                            "duplicate Mod {mod_id}'s selected Library target is no longer a directory"
                        ),
                    });
                }
            }

            let install_path: Option<String> = row.try_get("install_path")?;
            let Some(install_path) = install_path else {
                if enabled {
                    return Err(Error::DuplicateModInstallPathMissing {
                        mod_id,
                        game: game_code,
                    });
                }
                continue;
            };
            let link = PathBuf::from(install_path)
                .join("Mods")
                .join(row.try_get::<String, _>("junction_dir_name")?);
            if !link_exists(&link)? {
                continue;
            }
            for (surviving_mod_id, surviving_link) in &surviving_junctions {
                if same_physical_link_path(&link, surviving_link)? {
                    return Err(Error::DuplicateModJunctionClaimedBySurvivor {
                        mod_id,
                        surviving_mod_id: surviving_mod_id.clone(),
                        path: link,
                    });
                }
            }
            let actual =
                resolve_link(&link).ok_or_else(|| Error::DuplicateModJunctionConflict {
                    mod_id: mod_id.clone(),
                    path: link.clone(),
                })?;
            if !path_within(&actual, &library_path) {
                return Err(Error::DuplicateModJunctionConflict { mod_id, path: link });
            }
            junctions.push((mod_id, link));
        }
        if seen.len() != reviewed.len() - 1 {
            return Err(Error::DuplicateModResolutionChanged {
                reason: "one or more reviewed duplicate records no longer exist".to_string(),
            });
        }

        // Every row, Variant target, witness, and Junction is validated before
        // the first filesystem act. If a later step fails, the rows remain and
        // ordinary reconcile can recreate any already-withdrawn enabled link.
        for (mod_id, link) in &junctions {
            withdraw_reinstall_junction(link)?;
            self.crash_point(crash_points::RESOLVE_DUPLICATES_AFTER_JUNCTION_WITHDRAWAL);
            if link_exists(link)? {
                return Err(Error::DuplicateModJunctionStillPresent {
                    mod_id: mod_id.clone(),
                    path: link.clone(),
                });
            }
        }

        let mut removed_mod_ids: Vec<_> = reviewed
            .into_iter()
            .filter(|mod_id| mod_id != keeper_id)
            .collect();
        removed_mod_ids.sort();
        for mod_id in &removed_mod_ids {
            // Break the mods.active_variant_id -> mod_variants cycle before
            // the chosen row's ON DELETE CASCADE removes its Variant set.
            sqlx::query("UPDATE mods SET active_variant_id = NULL WHERE id = ?")
                .bind(mod_id)
                .execute(&mut *fence.transaction)
                .await?;
            sqlx::query("DELETE FROM mods WHERE id = ?")
                .bind(mod_id)
                .execute(&mut *fence.transaction)
                .await?;
        }
        fence.commit().await?;
        drop(keeper_directory);

        Ok(DuplicateResolution {
            keeper_id: keeper_id.to_string(),
            removed_mod_ids,
        })
    }

    /// Create a staged adopt/import directory and commit its durable owner
    /// before returning it to code that can write source bytes.
    pub(super) async fn create_staged_library_directory(
        &self,
        game: GameCode,
        directory_name: &str,
        mutation: LibraryMutation,
    ) -> Result<(LibraryRootSnapshot, StagedLibraryDirectory)> {
        let operation = match mutation {
            LibraryMutation::AdoptFolder => "adopt",
            LibraryMutation::ImportZip => "import_zip",
            _ => unreachable!("only adopt/import create ordinary Library stages"),
        };
        let mut fence = self.begin_library_mutation(mutation).await?;
        let root_path = self
            .resolved_library_root_for_in_mutation(game, &mut fence)
            .await?;
        fs::create_dir_all(&root_path).map_err(|source| Error::Io {
            path: root_path.clone(),
            source,
        })?;
        let root = IdentifiedDirectory::open(&root_path).map_err(|source| Error::Io {
            path: root_path.clone(),
            source,
        })?;
        let snapshot = LibraryRootSnapshot { game, root };
        let staged_path = snapshot.path().join(directory_name);
        fs::create_dir(&staged_path).map_err(|source| Error::Io {
            path: staged_path.clone(),
            source,
        })?;
        let directory = match IdentifiedDirectory::open(&staged_path) {
            Ok(directory) => directory,
            Err(source) => {
                let _ = fs::remove_dir(&staged_path);
                return Err(Error::Io {
                    path: staged_path,
                    source,
                });
            }
        };
        let staged = StagedLibraryDirectory {
            id: directory_name.to_string(),
            directory,
        };
        let insert = sqlx::query(
            "INSERT INTO staged_library_operations (
                id, game_code, operation, staged_path, staged_identity, created_at
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&staged.id)
        .bind(game.as_str())
        .bind(operation)
        .bind(staged_path.to_string_lossy().as_ref())
        .bind(staged.identity_key())
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *fence.transaction)
        .await;
        if let Err(error) = insert {
            let _ = fence.transaction.rollback().await;
            drop(staged);
            let _ = fs::remove_dir(&staged_path);
            return Err(error.into());
        }
        if let Err(error) = fence.commit().await {
            drop(staged);
            let _ = fs::remove_dir(&staged_path);
            return Err(error);
        }
        self.crash_point(crash_points::STAGING_AFTER_WITNESS_COMMIT);
        Ok((snapshot, staged))
    }

    /// Retire the exact staging witness inside the transaction that makes its
    /// Mod row authoritative. A missing row fails closed rather than allowing
    /// an unwitnessed commit.
    pub(super) async fn retire_staging_witness_for_commit(
        &self,
        staged: &StagedLibraryDirectory,
        fence: &mut LibraryMutationFence,
    ) -> Result<()> {
        let removed = sqlx::query(
            "DELETE FROM staged_library_operations
             WHERE id = ? AND staged_path = ? AND staged_identity = ?",
        )
        .bind(&staged.id)
        .bind(staged.path().to_string_lossy().as_ref())
        .bind(staged.identity_key())
        .execute(&mut *fence.transaction)
        .await?;
        if removed.rows_affected() != 1 {
            return Err(Error::StagingWitnessChanged {
                path: staged.path().to_path_buf(),
            });
        }
        Ok(())
    }

    /// A crashed producer can no longer finish its staged bytes. Release its
    /// durable claim at startup but preserve the directory itself so the
    /// ordinary audit records it and the user can inspect, recover, or delete
    /// it explicitly. If witness deletion fails, record that obstruction on
    /// every surviving witness and stop treating those rows as active owners;
    /// otherwise the durable claim would permanently conceal the only copy of
    /// a partial import from every in-app recovery action.
    pub(super) async fn resolve_interrupted_staging_at_startup(&self) -> Result<usize> {
        let mut fence = self
            .begin_library_mutation(LibraryMutation::ResolveInterruptedStaging)
            .await?;
        let mut validated = load_staged_library_operation_witnesses(&mut fence.transaction).await?;
        validated.sort_by_key(|witness| (witness.created_at(), witness.id));
        let witnesses: Vec<_> = validated
            .into_iter()
            .map(|witness| {
                (
                    witness.id(),
                    witness.staged_path().to_string_lossy().into_owned(),
                )
            })
            .collect();
        let removed = match sqlx::query("DELETE FROM staged_library_operations")
            .execute(&mut *fence.transaction)
            .await
        {
            Ok(removed) => removed,
            Err(error) => {
                let reason = error.to_string();
                fence.transaction.rollback().await?;
                self.record_interrupted_staging_resolution_failure(&witnesses, &reason)
                    .await?;
                return Ok(0);
            }
        };
        if let Err(error) = fence.commit().await {
            self.record_interrupted_staging_resolution_failure(&witnesses, &error.to_string())
                .await?;
            return Ok(0);
        }
        for (_, path) in witnesses {
            tracing::warn!(
                target: "gmm::library",
                path,
                "released an interrupted staging witness; preserved any staged bytes for the Library audit",
            );
        }
        Ok(removed.rows_affected() as usize)
    }

    async fn record_interrupted_staging_resolution_failure(
        &self,
        witnesses: &[(String, String)],
        reason: &str,
    ) -> Result<usize> {
        let attempted_at = Utc::now().to_rfc3339();
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut marked_paths = Vec::new();
        for (id, path) in witnesses {
            let marked = sqlx::query(
                "UPDATE staged_library_operations
                 SET recovery_error = ?, recovery_attempted_at = ?,
                     recovery_attempts = recovery_attempts + 1
                 WHERE id = ?",
            )
            .bind(reason)
            .bind(&attempted_at)
            .bind(id)
            .execute(&mut *transaction)
            .await?;
            if marked.rows_affected() == 1 {
                marked_paths.push(path.clone());
            }
        }
        transaction.commit().await?;
        for path in &marked_paths {
            tracing::error!(
                target: "gmm::library",
                path,
                error = reason,
                "could not release an interrupted staging witness; preserved and exposed any staged bytes through the Library audit",
            );
        }
        Ok(marked_paths.len())
    }

    /// Change both halves of a Mod's enabled deployment state behind one
    /// durable transition witness and the shared Library ownership guard.
    pub(super) async fn set_enabled_in_library_mutation(
        &self,
        id: &str,
        enabled: bool,
        game_mods_dir: &Path,
    ) -> Result<()> {
        let mut fence = self
            .begin_library_mutation(LibraryMutation::SetEnabled)
            .await?;
        self.ensure_mod_reinstall_is_usable(
            id,
            &mut fence.transaction,
            crash_points::SET_ENABLED_AFTER_REINSTALL_GUARD,
        )
        .await?;
        let row = sqlx::query(
            "SELECT game_code, junction_dir_name, library_path, enabled FROM mods WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&mut *fence.transaction)
        .await?;

        let game_code: String = row.try_get("game_code")?;
        let game = GameCode::from_str(&game_code)?;
        let junction_dir_name: String = row.try_get("junction_dir_name")?;
        let library_path = PathBuf::from(row.try_get::<String, _>("library_path")?);
        let current_enabled: i64 = row.try_get("enabled")?;
        let link = game_mods_dir.join(junction_dir_name);

        if (current_enabled != 0) == enabled {
            sqlx::query("UPDATE mods SET enabled = ? WHERE id = ?")
                .bind(if enabled { 1_i64 } else { 0_i64 })
                .bind(id)
                .execute(&mut *fence.transaction)
                .await?;
            self.crash_point(crash_points::SET_ENABLED_AFTER_DB_UPDATE);
            return fence.commit().await;
        }

        let (target, junction_entry) = if enabled {
            (
                self.junction_target_for(id, &library_path, &mut *fence.transaction)
                    .await?,
                None,
            )
        } else {
            let target = resolve_link(&link).ok_or_else(|| Error::Io {
                path: link.clone(),
                source: io::Error::other(
                    "the enabled Mod deployment path is not a Junction GMM can safely disable",
                ),
            })?;
            let entry = IdentifiedDirectory::open(&link).map_err(|source| Error::Io {
                path: link.clone(),
                source,
            })?;
            (target, Some(entry))
        };
        if enabled && !path_within(&target, &library_path) {
            return Err(Error::Io {
                path: link,
                source: io::Error::other(
                    "the Mod deployment Junction does not resolve inside its Library path",
                ),
            });
        }
        if enabled {
            volume::require_ntfs_pair(game_mods_dir, &target)?;
        }
        let junction_parent =
            IdentifiedDirectory::open(game_mods_dir).map_err(|source| Error::Io {
                path: game_mods_dir.to_path_buf(),
                source,
            })?;
        let target_entry = IdentifiedDirectory::open(&target).map_err(|source| Error::Io {
            path: target.clone(),
            source,
        })?;
        let library_entry =
            IdentifiedDirectory::open(&library_path).map_err(|source| Error::Io {
                path: library_path.clone(),
                source,
            })?;

        sqlx::query(
            "INSERT INTO enabled_transitions (
                mod_id, game_code, intended_enabled, junction_path,
                junction_target, junction_parent_identity, junction_identity, owner_pid,
                owner_started_at, owner_active, created_at,
                junction_target_identity, library_identity
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?)",
        )
        .bind(id)
        .bind(game.as_str())
        .bind(if enabled { 1_i64 } else { 0_i64 })
        .bind(link.to_string_lossy().as_ref())
        .bind(target.to_string_lossy().as_ref())
        .bind(junction_parent.identity().durable_key())
        .bind(
            junction_entry
                .as_ref()
                .map(|entry| entry.identity().durable_key()),
        )
        .bind(std::process::id() as i64)
        .bind(super::session::process_started_at(std::process::id()).map(|value| value as i64))
        .bind(Utc::now().to_rfc3339())
        .bind(target_entry.identity().durable_key())
        .bind(library_entry.identity().durable_key())
        .execute(&mut *fence.transaction)
        .await?;
        fence.commit().await?;
        self.crash_point(crash_points::SET_ENABLED_AFTER_WITNESS_COMMIT);

        match self.resolve_enabled_transition(id).await {
            Ok(()) => Ok(()),
            Err(error) => {
                if let Err(record_error) = self
                    .record_enabled_transition_recovery_failure(id, &error.to_string())
                    .await
                {
                    tracing::error!(
                        target: "gmm::library",
                        mod_id = id,
                        error = %record_error,
                        "could not record enable/disable transition recovery failure",
                    );
                }
                Err(error)
            }
        }
    }

    async fn resolve_enabled_transition(&self, mod_id: &str) -> Result<()> {
        let mut fence = self
            .begin_library_mutation(LibraryMutation::ResolveEnabledTransition)
            .await?;
        let Some(witness) = load_enabled_transition_witnesses(&mut fence.transaction)
            .await?
            .into_iter()
            .find(|witness| witness.mod_id() == mod_id)
        else {
            return Ok(());
        };
        let row =
            sqlx::query("SELECT game_code, junction_dir_name, library_path FROM mods WHERE id = ?")
                .bind(mod_id)
                .fetch_one(&mut *fence.transaction)
                .await?;
        let game_code: String = row.try_get("game_code")?;
        let junction_dir_name: String = row.try_get("junction_dir_name")?;
        let library_path = PathBuf::from(row.try_get::<String, _>("library_path")?);
        if game_code != witness.game().as_str()
            || witness
                .junction_path()
                .file_name()
                .and_then(|name| name.to_str())
                != Some(junction_dir_name.as_str())
        {
            return witness.corrupt("the recorded Mod or Junction name no longer matches");
        }
        let target = witness
            .junction_target()
            .expect("validated transition witnesses always carry a target");
        let junction_parent_path = witness.junction_path().parent().ok_or_else(|| {
            Error::EnabledTransitionWitnessCorrupt {
                mod_id: mod_id.to_string(),
                reason: "the recorded Junction path has no parent".to_string(),
            }
        })?;
        let junction_parent =
            IdentifiedDirectory::open(junction_parent_path).map_err(|source| Error::Io {
                path: junction_parent_path.to_path_buf(),
                source,
            })?;
        if junction_parent.identity() != witness.junction_parent_identity() {
            return witness.corrupt("the recorded Junction parent changed filesystem identity");
        }
        if witness.intended_enabled() {
            if !path_within(target, &library_path) {
                return witness
                    .corrupt("the recorded Junction target is outside the Mod Library path");
            }
            let current_library =
                IdentifiedDirectory::open(&library_path).map_err(|source| Error::Io {
                    path: library_path.clone(),
                    source,
                })?;
            if current_library.identity() != witness.library_identity() {
                return witness.corrupt("the recorded Mod Library changed filesystem identity");
            }
            let current_target = IdentifiedDirectory::open(target).map_err(|source| Error::Io {
                path: target.to_path_buf(),
                source,
            })?;
            if current_target.identity() != witness.junction_target_identity() {
                return witness.corrupt("the recorded Junction target changed filesystem identity");
            }
            let selected_target = self
                .junction_target_for(mod_id, &library_path, &mut *fence.transaction)
                .await?;
            if !super::same_path(target, &selected_target) {
                return witness.corrupt("the selected Library target changed during recovery");
            }
            if link_exists(witness.junction_path())? {
                let actual = resolve_link(witness.junction_path()).ok_or_else(|| Error::Io {
                    path: witness.junction_path().to_path_buf(),
                    source: io::Error::other(
                        "the deployment entry is not a Junction GMM can safely recover",
                    ),
                })?;
                if !super::same_path(&actual, target) {
                    return witness.corrupt("the deployment Junction points at another target");
                }
            } else {
                volume::require_ntfs_pair(junction_parent_path, target)?;
                junction::create(witness.junction_path(), target)?;
            }
            self.crash_point(crash_points::SET_ENABLED_AFTER_JUNCTION_CREATE);
        } else {
            if link_exists(witness.junction_path())? {
                let current_entry =
                    IdentifiedDirectory::open(witness.junction_path()).map_err(|source| {
                        Error::Io {
                            path: witness.junction_path().to_path_buf(),
                            source,
                        }
                    })?;
                if Some(current_entry.identity()) != witness.junction_identity() {
                    return witness
                        .corrupt("the recorded deployment entry changed filesystem identity");
                }
                match resolve_link(witness.junction_path()) {
                    Some(actual) if super::same_path(&actual, target) => {
                        junction::remove(witness.junction_path())?;
                    }
                    Some(_) => {
                        return witness.corrupt("the deployment Junction points at another target")
                    }
                    None => remove_empty_partial_junction(witness.junction_path())?,
                }
            }
            self.crash_point(crash_points::SET_ENABLED_AFTER_JUNCTION_REMOVE);
        }

        sqlx::query("UPDATE mods SET enabled = ? WHERE id = ?")
            .bind(if witness.intended_enabled() {
                1_i64
            } else {
                0_i64
            })
            .bind(mod_id)
            .execute(&mut *fence.transaction)
            .await?;
        self.crash_point(crash_points::SET_ENABLED_AFTER_DB_UPDATE);
        let removed = sqlx::query("DELETE FROM enabled_transitions WHERE mod_id = ?")
            .bind(mod_id)
            .execute(&mut *fence.transaction)
            .await?;
        if removed.rows_affected() != 1 {
            return witness.corrupt("the transition witness changed before recovery committed");
        }
        fence.commit().await
    }

    async fn record_enabled_transition_recovery_failure(
        &self,
        mod_id: &str,
        reason: &str,
    ) -> Result<()> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            "UPDATE enabled_transitions
             SET recovery_error = ?, recovery_attempted_at = ?,
                 recovery_attempts = recovery_attempts + 1,
                 owner_active = 0
             WHERE mod_id = ?",
        )
        .bind(reason)
        .bind(Utc::now().to_rfc3339())
        .bind(mod_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub(super) async fn resolve_interrupted_enabled_transitions_at_startup(&self) -> Result<usize> {
        let mut connection = self.pool.acquire().await?;
        let mut witnesses = load_enabled_transition_witnesses(&mut connection).await?;
        witnesses.sort_by_key(EnabledTransitionWitness::created_at);
        drop(connection);
        let mut resolved = 0;
        for witness in witnesses {
            if witness.owner_is_live() {
                continue;
            }
            match self.resolve_enabled_transition(witness.mod_id()).await {
                Ok(()) => resolved += 1,
                Err(error) => {
                    self.record_enabled_transition_recovery_failure(
                        witness.mod_id(),
                        &error.to_string(),
                    )
                    .await?;
                    tracing::error!(
                        target: "gmm::library",
                        mod_id = witness.mod_id(),
                        error = %error,
                        "could not recover an interrupted enable/disable transition",
                    );
                }
            }
        }
        Ok(resolved)
    }

    /// Release an interrupted transition's producer only after explicit user
    /// confirmation. Exact live ownership remains non-retirable; an unknown
    /// identity is deliberately conservative at startup and user-actionable.
    pub(super) async fn retire_interrupted_enabled_transition_in_library_mutation(
        &self,
        mod_id: &str,
    ) -> Result<()> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let Some(witness) = load_enabled_transition_witnesses(&mut transaction)
            .await?
            .into_iter()
            .find(|witness| witness.mod_id() == mod_id)
        else {
            transaction.commit().await?;
            return Ok(());
        };
        if witness.owner_active
            && matches!(
                witness.owner_identity_state(),
                super::session::ProcessIdentityState::Matches
            )
        {
            return Err(Error::EnabledTransitionStillOwned);
        }
        sqlx::query("UPDATE enabled_transitions SET owner_active = 0 WHERE mod_id = ?")
            .bind(mod_id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;

        match self.resolve_enabled_transition(mod_id).await {
            Ok(()) => Ok(()),
            Err(error) => {
                self.record_enabled_transition_recovery_failure(mod_id, &error.to_string())
                    .await?;
                Err(error)
            }
        }
    }

    /// Change the selected Variant and its enabled Junction while holding the
    /// same writer fence used by reinstall recovery and other Library changes.
    pub(super) async fn set_active_variant_in_library_mutation(
        &self,
        mod_id: &str,
        variant_id: &str,
        game_mods_dir: &Path,
    ) -> Result<()> {
        let mut fence = self
            .begin_library_mutation(LibraryMutation::SetActiveVariant)
            .await?;
        self.ensure_mod_reinstall_is_usable(
            mod_id,
            &mut fence.transaction,
            crash_points::SET_ACTIVE_VARIANT_AFTER_REINSTALL_GUARD,
        )
        .await?;

        // Validate the Variant belongs to this Mod before persisting it.
        sqlx::query("SELECT subpath FROM mod_variants WHERE id = ? AND mod_id = ?")
            .bind(variant_id)
            .bind(mod_id)
            .fetch_one(&mut *fence.transaction)
            .await?;

        let mod_row =
            sqlx::query("SELECT junction_dir_name, library_path, enabled FROM mods WHERE id = ?")
                .bind(mod_id)
                .fetch_one(&mut *fence.transaction)
                .await?;
        let junction_dir_name: String = mod_row.try_get("junction_dir_name")?;
        let enabled: i64 = mod_row.try_get("enabled")?;
        let library_path = PathBuf::from(mod_row.try_get::<String, _>("library_path")?);

        sqlx::query("UPDATE mods SET active_variant_id = ? WHERE id = ?")
            .bind(variant_id)
            .bind(mod_id)
            .execute(&mut *fence.transaction)
            .await?;
        self.crash_point(crash_points::SET_ACTIVE_VARIANT_AFTER_DB_UPDATE);

        if enabled != 0 {
            let link = game_mods_dir.join(&junction_dir_name);
            if link_exists(&link)? {
                junction::remove(&link)?;
                self.crash_point(crash_points::SET_ACTIVE_VARIANT_AFTER_JUNCTION_REMOVE);
            }
            let target = self
                .junction_target_for(mod_id, &library_path, &mut *fence.transaction)
                .await?;
            volume::require_ntfs_pair(game_mods_dir, &target)?;
            junction::create(&link, &target)?;
        }

        fence.commit().await
    }

    /// Revalidate one staged reconcile/rebuild deployment decision against the
    /// current durable quarantine state, then keep that decision stable through
    /// the bounded Junction mutation. The caller performs traversal and target
    /// resolution before this short writer-fence window.
    pub(super) async fn create_reconciled_junction_in_library_mutation(
        &self,
        mod_id: &str,
        game_mods_dir: &Path,
        link: &Path,
        target: &Path,
        replace_existing: bool,
    ) -> Result<ReconciledJunctionMutation> {
        let mut fence = match self
            .begin_library_mutation(LibraryMutation::ReconcileJunction)
            .await
        {
            Ok(fence) => fence,
            Err(Error::EnabledTransitionPending { .. }) => {
                return Ok(ReconciledJunctionMutation::Stale)
            }
            Err(error) => return Err(error),
        };
        let enabled: i64 = sqlx::query_scalar("SELECT enabled FROM mods WHERE id = ?")
            .bind(mod_id)
            .fetch_one(&mut *fence.transaction)
            .await?;
        if enabled == 0 {
            fence.commit().await?;
            return Ok(ReconciledJunctionMutation::Stale);
        }
        let quarantined = load_reinstall_swap_witnesses(&mut fence.transaction)
            .await?
            .into_iter()
            .any(|witness| witness.mod_id() == mod_id && witness.is_quarantined());
        if quarantined {
            fence.commit().await?;
            return Ok(ReconciledJunctionMutation::Quarantined);
        }

        if replace_existing {
            junction::remove(link)?;
        }
        volume::require_ntfs_pair(game_mods_dir, target)?;
        junction::create(link, target)?;
        fence.commit().await?;
        Ok(ReconciledJunctionMutation::Applied)
    }

    /// Revalidate a cached disabled-row decision under the short writer fence
    /// before withdrawing its Junction. A transition committed after the
    /// caller's row snapshot either blocks this claim or changes the fresh
    /// enabled flag, so the stale pass leaves the live deployment untouched.
    pub(super) async fn remove_disabled_reconciled_junction_in_library_mutation(
        &self,
        mod_id: &str,
        link: &Path,
        expected_target: Option<&Path>,
    ) -> Result<ReconciledJunctionMutation> {
        let mut fence = match self
            .begin_library_mutation(LibraryMutation::ReconcileJunction)
            .await
        {
            Ok(fence) => fence,
            Err(Error::EnabledTransitionPending { .. }) => {
                return Ok(ReconciledJunctionMutation::Stale)
            }
            Err(error) => return Err(error),
        };
        let enabled: i64 = sqlx::query_scalar("SELECT enabled FROM mods WHERE id = ?")
            .bind(mod_id)
            .fetch_one(&mut *fence.transaction)
            .await?;
        if enabled != 0 || !link_exists(link)? {
            fence.commit().await?;
            return Ok(ReconciledJunctionMutation::Stale);
        }
        if let Some(expected_target) = expected_target {
            let Some(actual_target) = resolve_link(link) else {
                fence.commit().await?;
                return Ok(ReconciledJunctionMutation::Stale);
            };
            if !super::same_path(&actual_target, expected_target) {
                fence.commit().await?;
                return Ok(ReconciledJunctionMutation::Stale);
            }
        }
        junction::remove(link)?;
        fence.commit().await?;
        Ok(ReconciledJunctionMutation::Applied)
    }

    /// Take the first, short quarantine snapshot for a reconcile or rebuild
    /// pass. Traversal starts only after this fence commits; every later
    /// Junction create independently reloads the same durable state.
    pub(super) async fn snapshot_quarantined_reinstalls_in_library_mutation(
        &self,
    ) -> Result<HashMap<String, String>> {
        let mut fence = self
            .begin_library_mutation(LibraryMutation::ReconcileJunction)
            .await?;
        let quarantined = load_reinstall_swap_witnesses(&mut fence.transaction)
            .await?
            .into_iter()
            .filter(|witness| witness.is_quarantined())
            .map(|witness| (witness.mod_id().to_string(), witness.token().to_string()))
            .collect();
        fence.commit().await?;
        Ok(quarantined)
    }

    pub(super) async fn reinstall_swap_witness(
        &self,
        token: Ulid,
        fence: &mut LibraryMutationFence,
    ) -> Result<ReinstallSwapWitness> {
        let witness = load_reinstall_swap_witnesses(&mut fence.transaction)
            .await?
            .into_iter()
            .find(|witness| witness.token() == token)
            .ok_or(sqlx::Error::RowNotFound)?;
        self.rebase_reinstall_swap_witness(witness, fence).await
    }

    /// Attempt every durable reinstall witness independently at startup.
    /// Filesystem/identity failures quarantine only that Mod and remain
    /// retryable through the same witness. Database and schema failures still
    /// abort Core construction because they are not evidence about one Mod's
    /// bytes.
    pub(super) async fn recover_interrupted_reinstalls_at_startup(&self) -> Result<usize> {
        let mut connection = self.pool.acquire().await?;
        let mut witnesses = load_reinstall_swap_witnesses(&mut connection).await?;
        witnesses.sort_by_key(|witness| (witness.created_at(), witness.token()));
        drop(connection);
        let mut rolled_back = 0;
        for witness in witnesses {
            let token = witness.token().to_string();
            let mod_id = witness.mod_id().to_string();
            match self
                .attempt_reinstall_recovery(
                    &token,
                    &mod_id,
                    LibraryMutation::FinishInterruptedDeletes,
                )
                .await?
            {
                ReinstallRecoveryOutcome::Recovered => rolled_back += 1,
                ReinstallRecoveryOutcome::AlreadyRecovered => {}
                ReinstallRecoveryOutcome::Quarantined { recovery } => tracing::error!(
                    target: "gmm::library",
                    token,
                    error = %recovery.reason,
                    "interrupted reinstall remains quarantined; the rest of GMM will start",
                ),
            }
        }
        Ok(rolled_back)
    }

    /// Retry the exact verified rollback that startup attempted for one Mod.
    /// An absent witness is not evidence that the Mod is usable. Preliminary
    /// absence is revalidated under the same Library writer fence as the
    /// enabled/Junction proof; a missing deployment is repaired and reported
    /// as `Recovered`, while `AlreadyRecovered` is reserved for a deployment
    /// that already agrees with the persisted enabled state.
    pub async fn retry_reinstall_recovery(&self, mod_id: &str) -> Result<ReinstallRecoveryOutcome> {
        let mut connection = self.pool.acquire().await?;
        let token = load_reinstall_swap_witnesses(&mut connection)
            .await?
            .into_iter()
            .find(|witness| witness.mod_id() == mod_id)
            .map(|witness| witness.token().to_string());
        drop(connection);
        self.crash_point(super::crash_points::RETRY_REINSTALL_AFTER_WITNESS_LOOKUP);
        let mut fence = self
            .begin_library_mutation(LibraryMutation::RetryReinstallRecovery)
            .await?;
        let token = match token {
            Some(token) => Some(token),
            None => load_reinstall_swap_witnesses(&mut fence.transaction)
                .await?
                .into_iter()
                .find(|witness| witness.mod_id() == mod_id)
                .map(|witness| witness.token().to_string()),
        };
        let Some(token) = token else {
            let outcome = self
                .finish_absent_reinstall_recovery_in_mutation(mod_id, &mut fence)
                .await?;
            fence.commit().await?;
            return Ok(outcome);
        };
        let outcome = self
            .attempt_reinstall_recovery_in_mutation(&token, mod_id, fence)
            .await?;
        if matches!(outcome, ReinstallRecoveryOutcome::Recovered) {
            if let Err(error) = self.finish_interrupted_library_deletes().await {
                tracing::warn!(
                    target: "gmm::library",
                    mod_id,
                    error = %error,
                    "reinstall recovery succeeded but ordinary quarantine cleanup was deferred",
                );
            }
        }
        Ok(outcome)
    }

    async fn attempt_reinstall_recovery(
        &self,
        token_raw: &str,
        mod_id: &str,
        mutation: LibraryMutation,
    ) -> Result<ReinstallRecoveryOutcome> {
        let fence = self.begin_library_mutation(mutation).await?;
        self.attempt_reinstall_recovery_in_mutation(token_raw, mod_id, fence)
            .await
    }

    async fn attempt_reinstall_recovery_in_mutation(
        &self,
        token_raw: &str,
        mod_id: &str,
        mut fence: LibraryMutationFence,
    ) -> Result<ReinstallRecoveryOutcome> {
        let token = Ulid::from_string(token_raw).map_err(|_| Error::ReinstallWitnessCorrupt {
            mod_id: "<unknown>".to_string(),
            reason: format!("the swap token {token_raw:?} is not a ULID"),
        })?;
        let recovery = async {
            let Some(witness) = self
                .reinstall_swap_witness_if_present(token, &mut fence)
                .await?
            else {
                return Ok(None);
            };
            self.rollback_reinstall_swap_in_mutation(&witness, &mut fence)
                .await?;
            Ok(Some(()))
        }
        .await;
        match recovery {
            Ok(Some(())) => {
                fence.commit().await?;
                Ok(ReinstallRecoveryOutcome::Recovered)
            }
            Ok(None) => {
                let outcome = self
                    .finish_absent_reinstall_recovery_in_mutation(mod_id, &mut fence)
                    .await?;
                fence.commit().await?;
                Ok(outcome)
            }
            Err(error) if quarantinable_reinstall_failure(&error) => {
                fence.transaction.rollback().await?;
                let recovery = self
                    .record_reinstall_recovery_failure(token_raw, &error.to_string())
                    .await?;
                match recovery {
                    Some(recovery) => Ok(ReinstallRecoveryOutcome::Quarantined { recovery }),
                    None => self.finish_absent_reinstall_recovery(mod_id).await,
                }
            }
            Err(error) => Err(error),
        }
    }

    async fn finish_absent_reinstall_recovery(
        &self,
        mod_id: &str,
    ) -> Result<ReinstallRecoveryOutcome> {
        let mut fence = self
            .begin_library_mutation(LibraryMutation::RetryReinstallRecovery)
            .await?;
        let outcome = self
            .finish_absent_reinstall_recovery_in_mutation(mod_id, &mut fence)
            .await?;
        fence.commit().await?;
        Ok(outcome)
    }

    async fn finish_absent_reinstall_recovery_in_mutation(
        &self,
        mod_id: &str,
        fence: &mut LibraryMutationFence,
    ) -> Result<ReinstallRecoveryOutcome> {
        if load_reinstall_swap_witnesses(&mut fence.transaction)
            .await?
            .into_iter()
            .any(|witness| witness.mod_id() == mod_id)
        {
            return Err(Error::ReinstallRecoveryDeploymentUnverified {
                mod_id: mod_id.to_string(),
                reason: "new interrupted reinstall state appeared during recovery verification"
                    .to_string(),
            });
        }
        let row = sqlx::query(
            "SELECT m.enabled, m.junction_dir_name, m.library_path, g.install_path
             FROM mods m JOIN games g ON g.code = m.game_code
             WHERE m.id = ?",
        )
        .bind(mod_id)
        .fetch_optional(&mut *fence.transaction)
        .await?
        .ok_or_else(|| Error::ReinstallRecoveryDeploymentUnverified {
            mod_id: mod_id.to_string(),
            reason: "the Mod row no longer exists".to_string(),
        })?;
        let enabled = row.try_get::<i64, _>("enabled")? != 0;
        let Some(install_path) = row
            .try_get::<Option<String>, _>("install_path")?
            .map(PathBuf::from)
        else {
            if enabled {
                return Err(Error::ReinstallRecoveryDeploymentUnverified {
                    mod_id: mod_id.to_string(),
                    reason: "the enabled Mod has no configured game install path".to_string(),
                });
            }
            // A disabled Mod with no configured install path has no persisted
            // deployment namespace to inspect: `install_path` is GMM's only
            // durable route to this Mod's Junction. With no witness and no
            // expected deployment location, the disabled intent leaves no
            // recovery work to prove or perform.
            return Ok(ReinstallRecoveryOutcome::AlreadyRecovered);
        };
        let library_path = PathBuf::from(row.try_get::<String, _>("library_path")?);
        let target = self
            .junction_target_for(mod_id, &library_path, &mut *fence.transaction)
            .await?;
        let link = install_path
            .join("Mods")
            .join(row.try_get::<String, _>("junction_dir_name")?);

        if !enabled {
            if !super::link_exists(&link)? {
                return Ok(ReinstallRecoveryOutcome::AlreadyRecovered);
            }
            let Some(actual) = super::resolve_link(&link) else {
                return Err(Error::ReinstallRecoveryDeploymentUnverified {
                    mod_id: mod_id.to_string(),
                    reason:
                        "the disabled Mod's deployment path is not a Junction GMM can safely remove"
                            .to_string(),
                });
            };
            if !super::same_path(&actual, &target) && !super::path_within(&actual, &library_path) {
                return Err(Error::ReinstallRecoveryDeploymentUnverified {
                    mod_id: mod_id.to_string(),
                    reason: "the disabled Mod's deployment Junction points outside its Library"
                        .to_string(),
                });
            }
            junction::remove(&link)?;
            return Ok(ReinstallRecoveryOutcome::Recovered);
        }

        let target_metadata = fs::metadata(&target).map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                Error::ReinstallRecoveryDeploymentUnverified {
                    mod_id: mod_id.to_string(),
                    reason: "the enabled Mod's Library target is missing".to_string(),
                }
            } else {
                Error::Io {
                    path: target.clone(),
                    source,
                }
            }
        })?;
        // Safe: `metadata()` above propagated I/O uncertainty.
        if !target_metadata.is_dir() {
            return Err(Error::ReinstallRecoveryDeploymentUnverified {
                mod_id: mod_id.to_string(),
                reason: "the enabled Mod's Library target is not a directory".to_string(),
            });
        }

        if !super::link_exists(&link)? {
            let mods_dir = link.parent().expect("a deployment name has a Mods parent");
            fs::create_dir_all(mods_dir).map_err(|source| Error::Io {
                path: mods_dir.to_path_buf(),
                source,
            })?;
            volume::require_ntfs_pair(mods_dir, &target)?;
            junction::create(&link, &target)?;
            return Ok(ReinstallRecoveryOutcome::Recovered);
        }

        let Some(actual) = super::resolve_link(&link) else {
            return Err(Error::ReinstallRecoveryDeploymentUnverified {
                mod_id: mod_id.to_string(),
                reason: "the enabled Mod's deployment path is not a Junction".to_string(),
            });
        };
        if super::same_path(&actual, &target) {
            return Ok(ReinstallRecoveryOutcome::AlreadyRecovered);
        }
        if !super::path_within(&actual, &library_path) {
            return Err(Error::ReinstallRecoveryDeploymentUnverified {
                mod_id: mod_id.to_string(),
                reason: "the enabled Mod's deployment Junction points outside its Library"
                    .to_string(),
            });
        }
        junction::remove(&link)?;
        volume::require_ntfs_pair(
            link.parent().expect("a deployment name has a Mods parent"),
            &target,
        )?;
        junction::create(&link, &target)?;
        Ok(ReinstallRecoveryOutcome::Recovered)
    }

    async fn reinstall_swap_witness_if_present(
        &self,
        token: Ulid,
        fence: &mut LibraryMutationFence,
    ) -> Result<Option<ReinstallSwapWitness>> {
        let Some(witness) = load_reinstall_swap_witnesses(&mut fence.transaction)
            .await?
            .into_iter()
            .find(|witness| witness.token() == token)
        else {
            return Ok(None);
        };
        self.rebase_reinstall_swap_witness(witness, fence)
            .await
            .map(Some)
    }

    async fn record_reinstall_recovery_failure(
        &self,
        token: &str,
        reason: &str,
    ) -> Result<Option<ReinstallRecovery>> {
        let attempted_at = chrono::Utc::now().to_rfc3339();
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let updated = sqlx::query(
            "UPDATE reinstall_swaps
             SET recovery_error = ?, recovery_attempted_at = ?,
                 recovery_attempts = recovery_attempts + 1
             WHERE token = ?",
        )
        .bind(reason)
        .bind(&attempted_at)
        .bind(token)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() == 0 {
            transaction.commit().await?;
            return Ok(None);
        }

        let witness = load_reinstall_swap_witnesses(&mut transaction)
            .await?
            .into_iter()
            .find(|witness| witness.token().to_string() == token)
            .ok_or(sqlx::Error::RowNotFound)?;
        let junction = sqlx::query(
            "SELECT m.junction_dir_name, g.install_path
             FROM mods m
             JOIN games g ON g.code = m.game_code
             WHERE m.id = ? AND m.game_code = ?",
        )
        .bind(witness.mod_id())
        .bind(witness.game.as_str())
        .fetch_one(&mut *transaction)
        .await?;
        let link = junction
            .try_get::<Option<String>, _>("install_path")?
            .map(PathBuf::from)
            .map(|install_path| {
                Ok::<PathBuf, Error>(
                    install_path
                        .join("Mods")
                        .join(junction.try_get::<String, _>("junction_dir_name")?),
                )
            })
            .transpose()?;
        transaction.commit().await?;

        // The quarantine is durable before the fallible Junction operation.
        // A process death here leaves junction_withdrawn = 0, which startup,
        // retry, reconcile, and rebuild all retry without losing the witness.
        self.withdraw_quarantined_reinstall_junction(token, link.as_deref())
            .await?;
        self.reinstall_recovery_for_token(token).await
    }

    /// Public only for the test-only concurrency probe, which must exercise
    /// this fence without passing through caller-level recovery repair.
    #[doc(hidden)]
    pub async fn withdraw_quarantined_reinstall_junction(
        &self,
        token: &str,
        link: Option<&Path>,
    ) -> Result<Option<bool>> {
        let mut fence = self
            .begin_library_mutation(LibraryMutation::WithdrawQuarantinedReinstallJunction)
            .await?;
        let state = load_reinstall_swap_witnesses(&mut fence.transaction)
            .await?
            .into_iter()
            .find(|witness| witness.token().to_string() == token && witness.is_quarantined())
            .map(|witness| witness.junction_withdrawn());
        self.crash_point(crash_points::WITHDRAW_REINSTALL_AFTER_WITNESS_LOOKUP);
        let Some(state) = state else {
            fence.commit().await?;
            return Ok(None);
        };
        if state {
            let absent = match link {
                Some(path) => !super::link_exists(path)?,
                None => true,
            };
            if absent {
                fence.commit().await?;
                return Ok(Some(true));
            }
        }

        let withdrawal = link.map_or(Ok(()), withdraw_reinstall_junction);
        let (withdrawn, withdrawal_error) = match withdrawal {
            Ok(()) => (1_i64, None),
            Err(error) => (0_i64, Some(error.to_string())),
        };
        let updated = sqlx::query(
            "UPDATE reinstall_swaps
             SET junction_withdrawn = ?, junction_withdrawal_error = ?
             WHERE token = ? AND recovery_error IS NOT NULL",
        )
        .bind(withdrawn)
        .bind(&withdrawal_error)
        .bind(token)
        .execute(&mut *fence.transaction)
        .await?;
        if updated.rows_affected() == 0 {
            fence.commit().await?;
            return Ok(None);
        }
        fence.commit().await?;
        if let Some(error) = withdrawal_error {
            tracing::error!(
                target: "gmm::library",
                token,
                error,
                "quarantined reinstall may still be deployed because Junction withdrawal failed",
            );
        }
        Ok(Some(withdrawn != 0))
    }

    async fn reinstall_recovery_for_token(&self, token: &str) -> Result<Option<ReinstallRecovery>> {
        let mut connection = self.pool.acquire().await?;
        Ok(load_reinstall_swap_witnesses(&mut connection)
            .await?
            .into_iter()
            .find(|witness| witness.token().to_string() == token)
            .and_then(|witness| witness.recovery()))
    }

    /// Current relocation refuses to move a subtree with an active witness,
    /// because its cross-volume copy fallback cannot preserve identity. A
    /// recorded root different from the current effective root is therefore
    /// corrupt durable state, not evidence that relocation legitimately carried
    /// the witness elsewhere. Only after proving that root do we rebase the
    /// sibling spellings to the current Mod row.
    pub(super) async fn rebase_reinstall_swap_witness(
        &self,
        mut witness: ReinstallSwapWitness,
        fence: &mut LibraryMutationFence,
    ) -> Result<ReinstallSwapWitness> {
        let current_library_path: String =
            sqlx::query_scalar("SELECT library_path FROM mods WHERE id = ? AND game_code = ?")
                .bind(&witness.mod_id)
                .bind(witness.game.as_str())
                .fetch_one(&mut *fence.transaction)
                .await?;
        let current_library_path = PathBuf::from(current_library_path);
        let current_root = self
            .resolved_library_root_for_in_mutation(witness.game, fence)
            .await?;
        let recorded_root = witness
            .library_path
            .parent()
            .expect("validated witness paths always have a parent");
        if !super::same_path(recorded_root, &current_root) {
            return witness
                .corrupt("the recorded swap root is not the Mod's effective Library root");
        }
        let current_mod_root =
            current_library_path
                .parent()
                .ok_or_else(|| Error::ReinstallWitnessCorrupt {
                    mod_id: witness.mod_id.clone(),
                    reason: "the current Mod row's Library path has no parent".to_string(),
                })?;
        if !super::same_path(current_mod_root, &current_root)
            || current_library_path
                .file_name()
                .and_then(|name| name.to_str())
                != Some(&witness.mod_id)
        {
            return witness.corrupt(
                "the current Mod row is not a direct child of its effective Library root",
            );
        }
        witness.library_path = current_library_path;
        witness.staged_path =
            current_root.join(format!("{REINSTALL_STAGING_PREFIX}{}", witness.token));
        witness.quarantine_path = current_root.join(format!(
            "{}{}",
            super::library_recovery::DELETE_QUARANTINE_PREFIX,
            witness.token
        ));
        witness.validate_paths()?;
        Ok(witness)
    }

    pub(super) async fn rollback_reinstall_swap(&self, token: Ulid) -> Result<()> {
        let mut fence = self
            .begin_library_mutation(LibraryMutation::FinishInterruptedDeletes)
            .await?;
        let witness = self.reinstall_swap_witness(token, &mut fence).await?;
        self.rollback_reinstall_swap_in_mutation(&witness, &mut fence)
            .await?;
        fence.commit().await?;
        // The rollback moved the staged replacement into the ordinary shared
        // delete quarantine. Finish it now; startup repeats the same verified
        // purge if this process stops first.
        if let Err(error) = self.finish_interrupted_library_deletes().await {
            tracing::warn!(
                target: "gmm::library",
                mod_id = %witness.mod_id,
                error = %error,
                "reinstall rollback succeeded but ordinary quarantine cleanup was deferred"
            );
        }
        Ok(())
    }

    async fn rollback_reinstall_swap_in_mutation(
        &self,
        witness: &ReinstallSwapWitness,
        fence: &mut LibraryMutationFence,
    ) -> Result<()> {
        let live = identified_if_exists(&witness.library_path)?;
        let staged = identified_if_exists(&witness.staged_path)?;
        let quarantine = identified_if_exists(&witness.quarantine_path)?;

        reject_unexpected_identity(
            witness,
            "live",
            live.as_ref(),
            &[&witness.old_identity, &witness.staged_identity],
        )?;
        reject_unexpected_identity(
            witness,
            "staged",
            staged.as_ref(),
            &[&witness.staged_identity],
        )?;
        reject_unexpected_identity(
            witness,
            "quarantine",
            quarantine.as_ref(),
            &[&witness.old_identity],
        )?;

        let old_is_live = live
            .as_ref()
            .is_some_and(|directory| directory.identity() == &witness.old_identity);
        let old_is_quarantined = quarantine
            .as_ref()
            .is_some_and(|directory| directory.identity() == &witness.old_identity);
        if old_is_live == old_is_quarantined {
            return witness.uncertain(if old_is_live {
                "the old directory appears at both its live and quarantine names"
            } else {
                "the old directory is at neither its live nor quarantine name"
            });
        }

        // Release directory handles before Windows renames the names they
        // identify. The identities remain recorded in the witness.
        drop(live);
        drop(staged);
        drop(quarantine);

        if old_is_quarantined {
            if let Some(current_live) = identified_if_exists(&witness.library_path)? {
                move_live_replacement_back_to_stage(witness, current_live)?;
            }
            fs::rename(&witness.quarantine_path, &witness.library_path).map_err(|source| {
                Error::Io {
                    path: witness.quarantine_path.clone(),
                    source,
                }
            })?;
        }

        // Once the old object is back at the live name, the swap quarantine's
        // ownership intent is stranded and must not be allowed to classify a
        // later replacement at that reserved name.
        let swap_intent = witness.quarantine_path.with_file_name(format!(
            "{}{}.intent",
            super::library_recovery::DELETE_QUARANTINE_PREFIX,
            witness.token
        ));
        match fs::remove_file(&swap_intent) {
            Ok(()) => {}
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(Error::Io {
                    path: swap_intent,
                    source,
                })
            }
        }

        if let Some(replacement) = identified_if_exists(&witness.staged_path)? {
            if replacement.identity() != &witness.staged_identity {
                return witness.uncertain("the staging name changed identity during rollback");
            }
            let staged_quarantine =
                self.quarantine_library_directory(&witness.staged_path, &replacement, None, None)?;
            drop(replacement);
            drop(staged_quarantine);
        }

        self.restore_reinstall_junction_in_mutation(witness, fence)
            .await?;
        sqlx::query("DELETE FROM reinstall_swaps WHERE token = ?")
            .bind(witness.token.to_string())
            .execute(&mut *fence.transaction)
            .await?;
        Ok(())
    }

    async fn restore_reinstall_junction_in_mutation(
        &self,
        witness: &ReinstallSwapWitness,
        fence: &mut LibraryMutationFence,
    ) -> Result<()> {
        let row = sqlx::query(
            "SELECT m.enabled, m.junction_dir_name, g.install_path
             FROM mods m JOIN games g ON g.code = m.game_code
             WHERE m.id = ? AND m.game_code = ?",
        )
        .bind(&witness.mod_id)
        .bind(witness.game.as_str())
        .fetch_one(&mut *fence.transaction)
        .await?;
        let Some(install) = row
            .try_get::<Option<String>, _>("install_path")?
            .map(PathBuf::from)
        else {
            return Ok(());
        };
        let mods_dir = install.join("Mods");
        let link = mods_dir.join(row.try_get::<String, _>("junction_dir_name")?);
        if super::link_exists(&link)? {
            junction::remove(&link)?;
        }
        if row.try_get::<i64, _>("enabled")? == 0 {
            return Ok(());
        }
        let target = self
            .junction_target_for(
                &witness.mod_id,
                &witness.library_path,
                &mut *fence.transaction,
            )
            .await?;
        fs::create_dir_all(&mods_dir).map_err(|source| Error::Io {
            path: mods_dir.clone(),
            source,
        })?;
        volume::require_ntfs_pair(&mods_dir, &target)?;
        junction::create(&link, &target)
    }

    pub(super) async fn revalidate_library_root_for_mutation(
        &self,
        snapshot: &LibraryRootSnapshot,
        mutation: LibraryMutation,
    ) -> Result<LibraryMutationFence> {
        let mut fence = self.begin_library_mutation(mutation).await?;
        let current = self
            .resolved_library_root_for_in_mutation(snapshot.game, &mut fence)
            .await?;
        let current_directory =
            IdentifiedDirectory::open(&current).map_err(|source| Error::Io {
                path: current.clone(),
                source,
            })?;
        if current != snapshot.root.path()
            || current_directory.identity() != snapshot.root.identity()
        {
            return Err(Error::LibraryRootChangedDuringMutation {
                mutation: mutation.function_name(),
                previous: snapshot.root.path().to_path_buf(),
                current,
            });
        }
        Ok(fence)
    }

    pub(super) async fn cleanup_staged_library_dir(
        &self,
        snapshot: &LibraryRootSnapshot,
        staged: StagedLibraryDirectory,
        mutation: LibraryMutation,
    ) {
        let staged_path = staged.path().to_path_buf();
        let mut fence = match self.begin_library_mutation(mutation).await {
            Ok(fence) => fence,
            Err(error) => {
                tracing::warn!(
                    target: "gmm::library",
                    path = %staged_path.display(),
                    error = %error,
                    "could not fence staged Library cleanup; leaving bytes for orphan audit",
                );
                return;
            }
        };
        let retired = match sqlx::query(
            "DELETE FROM staged_library_operations
             WHERE id = ? AND staged_path = ? AND staged_identity = ?",
        )
        .bind(&staged.id)
        .bind(staged.path().to_string_lossy().as_ref())
        .bind(staged.identity_key())
        .execute(&mut *fence.transaction)
        .await
        {
            Ok(result) if result.rows_affected() == 1 => true,
            Ok(_) => {
                tracing::warn!(
                    target: "gmm::library",
                    path = %staged_path.display(),
                    "staged Library witness changed before cleanup; preserving every candidate",
                );
                false
            }
            Err(error) => {
                tracing::warn!(
                    target: "gmm::library",
                    path = %staged_path.display(),
                    error = %error,
                    "could not retire staged Library witness; preserving every candidate",
                );
                false
            }
        };
        if retired {
            self.crash_point(crash_points::STAGED_CLEANUP_AFTER_WITNESS_RETIRE);
        }
        let current_root = match self
            .resolved_library_root_for_in_mutation(snapshot.game, &mut fence)
            .await
        {
            Ok(root) => root,
            Err(error) => {
                tracing::warn!(
                    target: "gmm::library",
                    path = %staged_path.display(),
                    error = %error,
                    "could not resolve the current Library root during staged cleanup; leaving bytes for orphan audit",
                );
                return;
            }
        };
        let directory_name = staged_path
            .file_name()
            .expect("a staged Library directory has a final component");
        let current_path = current_root.join(directory_name);
        let mut candidates = vec![staged_path.clone()];
        if current_path != staged_path {
            candidates.push(current_path);
        }

        let mut deletion_candidate = None;
        for path in candidates {
            let current = match IdentifiedDirectory::open(&path) {
                Ok(directory) => directory,
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,
                Err(source) => {
                    tracing::warn!(
                        target: "gmm::library",
                        path = %path.display(),
                        error = %source,
                        "could not identify a staged Library cleanup candidate; leaving it for orphan audit",
                    );
                    continue;
                }
            };
            if current.identity() != staged.directory.identity() {
                tracing::warn!(
                    target: "gmm::library",
                    path = %path.display(),
                    "staged Library cleanup candidate changed identity; leaving it for orphan audit",
                );
                continue;
            }
            deletion_candidate = Some((path, current));
            break;
        }

        let mut quarantined = None;
        if retired {
            if let Some((path, current)) = deletion_candidate {
                let ownership = match LibraryOwnershipSnapshot::load(&mut fence.transaction).await {
                    Ok(ownership) => Some(ownership),
                    Err(error) => {
                        tracing::warn!(
                            target: "gmm::library",
                            path = %path.display(),
                            error = %error,
                            "could not prove a staged Library directory is unowned; leaving it for orphan audit",
                        );
                        None
                    }
                };
                match ownership
                    .as_ref()
                    .and_then(|ownership| ownership.owner_of(current.identity()))
                {
                    Some(owner) => {
                        let owner = match owner {
                            LibraryDirectoryOwner::Mod => "a Mod",
                            LibraryDirectoryOwner::ModWithPendingEnabledTransition => {
                                "an interrupted enable/disable transition"
                            }
                            LibraryDirectoryOwner::ActiveReinstall => "interrupted reinstall state",
                            LibraryDirectoryOwner::ActiveStaging => "another staging operation",
                        };
                        tracing::warn!(
                            target: "gmm::library",
                            path = %path.display(),
                            owner,
                            "staged Library cleanup candidate is now owned; leaving it intact",
                        );
                    }
                    None if ownership.is_some() => {
                        self.crash_point(
                            super::crash_points::STAGED_CLEANUP_BEFORE_QUARANTINE_MOVE,
                        );
                        match self.quarantine_library_directory(&path, &current, None, None) {
                            Ok(directory) => {
                                self.crash_point(
                                    super::crash_points::STAGED_CLEANUP_AFTER_QUARANTINE_MOVE,
                                );
                                // Windows keeps a removed directory name visible until
                                // every open handle closes, even when the handles share
                                // DELETE. The intent now records the object moved to
                                // GMM's reserved name, so release the old handles before
                                // the path-based purge; #172 tracks anchoring removal.
                                drop(current);
                                drop(staged);
                                quarantined = Some(directory);
                            }
                            Err(error) => tracing::warn!(
                                target: "gmm::library",
                                path = %path.display(),
                                error = %error,
                                "could not quarantine a staged Library cleanup candidate; leaving bytes for orphan audit",
                            ),
                        }
                    }
                    // Snapshot errors mean ownership is uncertain, so preserve the
                    // bytes for the read-only orphan audit.
                    None => {}
                }
            }
        }
        if let Err(error) = fence.commit().await {
            tracing::warn!(
                target: "gmm::library",
                path = %staged_path.display(),
                error = %error,
                "could not commit the staged Library cleanup fence",
            );
        }
        if let Some(quarantined) = quarantined {
            self.crash_point(super::crash_points::STAGED_CLEANUP_BEFORE_QUARANTINE_PURGE);
            match quarantined.purge(false) {
                Ok(super::library_recovery::QuarantinePurgeOutcome::Reclaimed(_)) => {}
                Ok(super::library_recovery::QuarantinePurgeOutcome::Deferred { path, error }) => {
                    tracing::warn!(
                        target: "gmm::library",
                        path = %staged_path.display(),
                        quarantine = %path.display(),
                        error = %error,
                        "GMM could not reclaim the staged Library cleanup bytes now; later startups will retry while the directory remains at its reserved name",
                    );
                }
                Ok(super::library_recovery::QuarantinePurgeOutcome::OwnershipLost) => {
                    tracing::error!(
                        target: "gmm::library",
                        path = %staged_path.display(),
                        "GMM cannot locate the staged Library cleanup quarantine or determine whether its bytes were reclaimed; a later startup will retry only if GMM can again verify the original directory at its reserved name",
                    );
                }
                Err(error) => tracing::warn!(
                    target: "gmm::library",
                    path = %staged_path.display(),
                    error = %error,
                    "could not inspect or purge a staged Library cleanup quarantine",
                ),
            }
        }
    }

    pub(super) async fn resolved_library_root_in_mutation(
        &self,
        fence: &mut LibraryMutationFence,
    ) -> Result<PathBuf> {
        let override_path = get_setting(&mut *fence.transaction, keys::library_root()).await?;
        Ok(override_path
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_library_root.clone()))
    }

    pub(super) async fn resolved_library_root_for_in_mutation(
        &self,
        game: GameCode,
        fence: &mut LibraryMutationFence,
    ) -> Result<PathBuf> {
        let per_game =
            get_setting(&mut *fence.transaction, &keys::library_root_for_game(game)).await?;
        if let Some(path) = per_game {
            return Ok(PathBuf::from(path));
        }
        Ok(self
            .resolved_library_root_in_mutation(fence)
            .await?
            .join(game.as_str()))
    }

    async fn ensure_no_active_session_in_library_mutation(
        &self,
        transaction: &mut sqlx::Transaction<'_, Sqlite>,
    ) -> Result<()> {
        if let Some(blocker) = self.session_blocker_in_transaction(transaction).await? {
            return Err(blocker.into_error());
        }
        Ok(())
    }

    async fn ensure_mod_reinstall_is_usable(
        &self,
        mod_id: &str,
        connection: &mut SqliteConnection,
        checked_at: &'static str,
    ) -> Result<()> {
        let quarantined = load_reinstall_swap_witnesses(connection)
            .await?
            .into_iter()
            .any(|witness| witness.mod_id() == mod_id && witness.is_quarantined());
        self.crash_point(checked_at);
        if quarantined {
            return Err(Error::ReinstallRecoveryQuarantined {
                mod_id: mod_id.to_string(),
            });
        }
        Ok(())
    }
}

fn quarantinable_reinstall_failure(error: &Error) -> bool {
    matches!(
        error,
        Error::Io { .. } | Error::ReinstallRecoveryUncertain { .. } | Error::NonNtfsVolume { .. }
    )
}

pub(super) fn withdraw_reinstall_junction(link: &Path) -> Result<()> {
    if !super::link_exists(link)? {
        return Ok(());
    }
    if super::resolve_link(link).is_none() {
        return Err(Error::Io {
            path: link.to_path_buf(),
            source: io::Error::other(
                "the Mod deployment path is not a Junction GMM can safely remove",
            ),
        });
    }
    junction::remove(link)
}

fn identified_if_exists(path: &Path) -> Result<Option<IdentifiedDirectory>> {
    match IdentifiedDirectory::open(path) {
        Ok(directory) => Ok(Some(directory)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn move_live_replacement_back_to_stage(
    witness: &ReinstallSwapWitness,
    current_live: IdentifiedDirectory,
) -> Result<()> {
    if current_live.identity() != &witness.staged_identity {
        return witness.uncertain("the live name no longer identifies the staged replacement");
    }
    if entry_exists(&witness.staged_path)? {
        return witness.uncertain("both the live and staging names contain replacement bytes");
    }
    drop(current_live);
    #[allow(
        clippy::disallowed_methods,
        reason = "Library mutation policy exemption: this private rollback helper is called only by rollback_reinstall_swap_in_mutation while its LibraryMutationFence is live"
    )]
    fs::rename(&witness.library_path, &witness.staged_path).map_err(|source| Error::Io {
        path: witness.library_path.clone(),
        source,
    })
}

/// Inspect the named entry itself. Only `NotFound` proves that a rename
/// destination is free; target-following existence checks collapse every
/// other metadata failure into the same false answer.
fn entry_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(Error::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn remove_empty_partial_junction(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.file_type().is_dir()
        || fs::read_dir(path)
            .map_err(|source| Error::Io {
                path: path.to_path_buf(),
                source,
            })?
            .next()
            .is_some()
    {
        return Err(Error::Io {
            path: path.to_path_buf(),
            source: io::Error::other(
                "the interrupted Junction removal left a non-empty or non-directory entry",
            ),
        });
    }
    fs::remove_dir(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn reject_unexpected_identity(
    witness: &ReinstallSwapWitness,
    name: &str,
    directory: Option<&IdentifiedDirectory>,
    expected: &[&DirectoryIdentity],
) -> Result<()> {
    let Some(directory) = directory else {
        return Ok(());
    };
    if expected
        .iter()
        .any(|expected| directory.identity() == *expected)
    {
        return Ok(());
    }
    witness.uncertain(format!(
        "the recorded {name} path identifies an unrelated directory"
    ))
}

/// Compare deployment entry paths without resolving the final Junction.
/// Resolving the whole path would compare targets and falsely conflate two
/// distinct Junction names that happen to deploy the same duplicate bytes.
fn same_physical_link_path(left: &Path, right: &Path) -> Result<bool> {
    #[cfg(windows)]
    {
        // These handles deliberately retain FILE_FLAG_OPEN_REPARSE_POINT via
        // IdentifiedDirectory, so the identity belongs to each Junction entry
        // rather than to the duplicate Library directory both entries target.
        let Some(left) = identified_if_exists(left)? else {
            return Ok(false);
        };
        let Some(right) = identified_if_exists(right)? else {
            return Ok(false);
        };
        Ok(left.identity() == right.identity())
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;

        let left = match fs::symlink_metadata(left) {
            Ok(metadata) => Some(metadata),
            Err(source) if source.kind() == io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(Error::Io {
                    path: left.to_path_buf(),
                    source,
                })
            }
        };
        let Some(left) = left else {
            return Ok(false);
        };
        let right = match fs::symlink_metadata(right) {
            Ok(metadata) => Some(metadata),
            Err(source) if source.kind() == io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(Error::Io {
                    path: right.to_path_buf(),
                    source,
                })
            }
        };
        let Some(right) = right else {
            return Ok(false);
        };
        Ok(left.dev() == right.dev() && left.ino() == right.ino())
    }
    #[cfg(not(any(windows, unix)))]
    {
        let (Some(left_parent), Some(right_parent)) = (left.parent(), right.parent()) else {
            return Ok(false);
        };
        Ok(same_path(left_parent, right_parent) && left.file_name() == right.file_name())
    }
}

pub(super) async fn unique_junction_dir_name(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    game: GameCode,
    base: &str,
) -> Result<String> {
    let rows = sqlx::query("SELECT junction_dir_name FROM mods WHERE game_code = ?")
        .bind(game.as_str())
        .fetch_all(&mut **transaction)
        .await?;
    let existing: std::collections::HashSet<String> = rows
        .iter()
        .filter_map(|row| row.try_get("junction_dir_name").ok())
        .collect();
    if !existing.contains(base) {
        return Ok(base.to_string());
    }
    for n in 2..=u32::MAX {
        let candidate = format!("{base} ({n})");
        if !existing.contains(&candidate) {
            return Ok(candidate);
        }
    }
    unreachable!("u32::MAX collisions on one display name is not a real scenario")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A staging-name inspection error is uncertainty, never evidence that
    /// the name is free for a rename. Mutation oracle: restoring
    /// target-following `Path::exists` reaches the rename, attributes its
    /// failure to the live path, and fires the named pre-rename assertion.
    #[test]
    fn reinstall_rollback_propagates_staging_metadata_error_before_rename() {
        use std::os::unix::fs::PermissionsExt as _;

        let tmp = TempDir::new().expect("tmp");
        let token = Ulid::new();
        let mod_id = Ulid::new().to_string();
        let library_path = tmp.path().join(&mod_id);
        let staged_path = tmp
            .path()
            .join(format!("{REINSTALL_STAGING_PREFIX}{token}"));
        let quarantine_path = tmp.path().join(format!(
            "{}{}",
            super::super::library_recovery::DELETE_QUARANTINE_PREFIX,
            token
        ));
        std::fs::create_dir(&library_path).expect("live replacement");
        std::fs::write(library_path.join("replacement.ini"), b"replacement")
            .expect("live replacement bytes");
        std::fs::create_dir(&quarantine_path).expect("old quarantine");
        let current_live = IdentifiedDirectory::open(&library_path).expect("identify live");
        let old = IdentifiedDirectory::open(&quarantine_path).expect("identify old");
        let witness = ReinstallSwapWitness {
            token,
            mod_id,
            game: GameCode::Gimi,
            library_path: library_path.clone(),
            staged_path: staged_path.clone(),
            quarantine_path,
            old_identity: old.identity().clone(),
            staged_identity: current_live.identity().clone(),
            created_at: Utc::now().fixed_offset(),
            recovery_error: None,
            recovery_attempted_at: None,
            recovery_attempts: 0,
            junction_withdrawn: false,
            junction_withdrawal_error: None,
        };

        let original_permissions = std::fs::metadata(tmp.path())
            .expect("temporary root metadata")
            .permissions();
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o0))
            .expect("make staging name unreadable");
        let result = move_live_replacement_back_to_stage(&witness, current_live);
        std::fs::set_permissions(tmp.path(), original_permissions)
            .expect("restore temporary root permissions");
        let error = result.expect_err("staging metadata errors must stop rollback before rename");

        assert!(
            matches!(
                error,
                Error::Io { ref path, ref source }
                    if path == &staged_path
                        && source.kind() == io::ErrorKind::PermissionDenied
            ),
            "staging metadata errors must stop rollback before rename: {error}",
        );
        assert_eq!(
            std::fs::read(library_path.join("replacement.ini"))
                .expect("live bytes remain before rename"),
            b"replacement",
            "a staging metadata error must not permit the live replacement rename",
        );
        assert!(
            matches!(
                std::fs::symlink_metadata(&staged_path),
                Err(error) if error.kind() == io::ErrorKind::NotFound
            ),
            "a staging metadata error must leave the reserved name untouched",
        );
    }
}
