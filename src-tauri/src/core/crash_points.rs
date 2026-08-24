//! Named points inside GMM's durable mutations where a test can kill the
//! process (issue #59).
//!
//! # Why this exists
//!
//! Every mutation that changes both the filesystem and the database does
//! so in two or more steps, and the process can die between any of them:
//! a crash, a `SIGKILL`, a power cut, or Windows killing the app during
//! shutdown. The invariant that matters — *never a DB row saying enabled
//! with a missing or wrong Junction* — was previously maintained only by
//! reading the code and believing it.
//!
//! `tests/session.rs` covers crash recovery for a Game Session because a
//! Game Session has an obvious external witness: a PID that is or is not
//! alive. Ordinary mutations have no such witness, so testing them needs
//! a way to stop the process at a chosen instant.
//!
//! # The shape of the seam
//!
//! [`Core::with_crash_hook`] takes a callback invoked at each of the
//! points named below. Nothing sets it except `crates/probe`, which
//! installs a hook that calls `std::process::abort()` when the point
//! matches its `--crash-at` argument.
//!
//! Injection rather than a `cfg` flag or an environment variable, for
//! three reasons: a `cfg(test)` gate would not compile into the separate
//! probe binary that has to do the dying; a cargo feature would leak
//! across the workspace through feature unification and end up enabled in
//! the binary `cargo build --workspace` produces; and an environment
//! variable is a live footgun in a shipped app, where anything that sets
//! `GMM_CRASH_AT` would crash a user's install. An `Option` field on
//! `Core` cannot be switched on from outside the process, costs one
//! null check per durable step, and is dead weight nobody can trip over.
//!
//! # Reading the names
//!
//! `<mutation>.<the step that just completed>`. So
//! [`SET_ENABLED_AFTER_JUNCTION_CREATE`] fires once the Junction exists
//! on disk and before the row is updated — the window in which the
//! Library and the DB disagree.

/// Define every named seam and derive the registry from those same declarations.
///
/// Keeping the declarations inside one macro invocation makes registration a
/// consequence of defining a point: there is no second list for an author to
/// remember to update.
macro_rules! define_crash_points {
    ($(
        $(#[$attribute:meta])*
        pub const $name:ident: &str = $value:literal;
    )*) => {
        $(
            $(#[$attribute])*
            pub const $name: &str = $value;
        )*

        /// Every declared crash point. Adding a declaration above adds it here
        /// automatically, so the execution-coverage test cannot overlook it.
        pub const ALL: &[&str] = &[$($name),*];
    };
}

define_crash_points! {

/// `set_enabled`: the reinstall-quarantine guard passed while the Library
/// writer fence is already held, so recovery cannot quarantine this Mod before
/// the deployment-state transition commits.
pub const SET_ENABLED_AFTER_REINSTALL_GUARD: &str = "set_enabled.after_reinstall_guard";

/// `set_enabled(true)`: the Junction now exists, the row still says
/// disabled. Recoverable — see the Junction, believe the row, remove it.
pub const SET_ENABLED_AFTER_JUNCTION_CREATE: &str = "set_enabled.after_junction_create";

/// `set_enabled(false)`: the Junction is gone, the row still says
/// enabled. Recoverable — reconcile recreates it.
pub const SET_ENABLED_AFTER_JUNCTION_REMOVE: &str = "set_enabled.after_junction_remove";

/// `set_enabled`: the row now matches the requested Junction state, while the
/// writer fence is still held until both halves commit together.
pub const SET_ENABLED_AFTER_DB_UPDATE: &str = "set_enabled.after_db_update";

/// `set_active_variant`: the reinstall-quarantine guard passed while the
/// Library writer fence is already held, so recovery cannot quarantine this
/// Mod before the Variant transition commits.
pub const SET_ACTIVE_VARIANT_AFTER_REINSTALL_GUARD: &str =
    "set_active_variant.after_reinstall_guard";

/// `set_active_variant`: the transaction names the new Variant, while the
/// Junction still points at the old one. A process death rolls the transaction
/// back, leaving the old persisted selection and old Junction in agreement.
pub const SET_ACTIVE_VARIANT_AFTER_DB_UPDATE: &str = "set_active_variant.after_db_update";

/// `set_active_variant`: the old Junction is gone and the new one is not
/// there yet. Recoverable — reconcile recreates it against the row.
pub const SET_ACTIVE_VARIANT_AFTER_JUNCTION_REMOVE: &str =
    "set_active_variant.after_junction_remove";

/// `adopt_folder`: the Library copy exists, no row references it. Leaves
/// an orphaned Library directory — see the module docs in
/// `tests/crash_recovery.rs` for why this is reported rather than
/// deleted.
pub const ADOPT_AFTER_LIBRARY_COPY: &str = "adopt.after_library_copy";

/// `adopt_folder`: the Mod row has been inserted into the still-open
/// transaction, while the already-detected Variants and active selection have
/// not been persisted. A process death rolls the whole transaction back.
pub const ADOPT_AFTER_ROW_INSERT: &str = "adopt.after_row_insert";

/// `import_zip`: the archive is extracted into the Library, no row
/// references it. Same orphan shape as [`ADOPT_AFTER_LIBRARY_COPY`].
pub const IMPORT_ZIP_AFTER_EXTRACT: &str = "import_zip.after_extract";

/// `import_zip`: the Mod row has been inserted into the still-open
/// transaction, while the already-detected Variants and active selection have
/// not been persisted. A process death rolls the whole transaction back.
pub const IMPORT_ZIP_AFTER_ROW_INSERT: &str = "import_zip.after_row_insert";

/// `reinstall_gamebanana_mod`: the empty same-root stage and its durable
/// witness committed, and extraction has not started. Tests pause here to
/// interleave a Library relocation with the in-flight reinstall.
pub const REINSTALL_AFTER_WITNESS_COMMIT: &str = "reinstall.after_witness_commit";

/// `reinstall_gamebanana_mod`: the complete old tree has moved into its
/// intent-backed quarantine, while the complete staged tree has not yet taken
/// the live Mod name. The durable swap witness requires startup to restore old.
pub const REINSTALL_AFTER_OLD_QUARANTINE_MOVE: &str = "reinstall.after_old_quarantine_move";

/// `reinstall_gamebanana_mod`: the complete replacement occupies the live Mod
/// name, but metadata/Variants and witness deletion have not committed. The
/// still-present witness requires startup to put the old tree back.
pub const REINSTALL_AFTER_REPLACEMENT_MOVE: &str = "reinstall.after_replacement_move";

/// `reinstall_gamebanana_mod`: replacement metadata/Variants and witness
/// deletion committed atomically. The live replacement wins; startup may
/// finish purging the old intent-backed quarantine.
pub const REINSTALL_AFTER_METADATA_COMMIT: &str = "reinstall.after_metadata_commit";

/// `retry_reinstall_recovery`: the caller observed a durable witness but has
/// not yet entered the serialized recovery mutation. Tests pause two real
/// processes here to prove the later retry treats a concurrently retired row
/// as already recovered.
pub const RETRY_REINSTALL_AFTER_WITNESS_LOOKUP: &str =
    "retry_reinstall.after_witness_lookup";

/// Failed adopt/ZIP cleanup: identity and database ownership have been
/// re-proved under the writer fence, immediately before the staged directory
/// is renamed into the durable delete quarantine.
pub const STAGED_CLEANUP_BEFORE_QUARANTINE_MOVE: &str = "staged_cleanup.before_quarantine_move";

/// Failed adopt/ZIP cleanup: the proven staged directory has moved into the
/// durable delete quarantine and still carries the writer fence. A restart
/// can finish the purge only while the reserved path still names that object.
pub const STAGED_CLEANUP_AFTER_QUARANTINE_MOVE: &str = "staged_cleanup.after_quarantine_move";

/// Failed adopt/ZIP cleanup: the durable quarantine is committed and the
/// identity handles used to prove it have been released, immediately before
/// the shared purge re-opens the reserved path.
pub const STAGED_CLEANUP_BEFORE_QUARANTINE_PURGE: &str = "staged_cleanup.before_quarantine_purge";

/// `set_library_path_for_game` / `set_library_root`: the Mod rows whose
/// paths need rewriting have been snapshotted, but no Library bytes have
/// moved yet. A concurrent row committer must not enter after this snapshot.
pub const RELOCATE_AFTER_MOD_SNAPSHOT: &str = "relocate.after_mod_snapshot";

/// `set_library_path_for_game` / `set_library_root`: one previously-enabled
/// Mod's Junction has been restored while the relocation writer fence is
/// still held. Tests use the repeated point to inject a later restore failure
/// without a timing rendezvous.
pub const RELOCATE_AFTER_JUNCTION_RESTORE: &str = "relocate.after_junction_restore";

/// `set_library_path_for_game` / `set_library_root`: the Library move,
/// rewritten rows, and junction restoration have committed. A Game Session
/// may claim the database after this point without observing a half-restored
/// relocation.
pub const RELOCATE_AFTER_FENCE_COMMIT: &str = "relocate.after_fence_commit";

/// `recover_unreferenced_library_dir`: a Library-root directory whose
/// name was not a usable ULID has been renamed to the fresh one, and no
/// row references it yet. Leaves the same orphan shape as
/// [`ADOPT_AFTER_LIBRARY_COPY`], under the new name — recoverable a
/// second time by the same feature, which is why the rename goes first.
pub const RECOVER_AFTER_LIBRARY_MOVE: &str = "recover.after_library_move";

/// `recover_unreferenced_library_dir`: the Mod row has been inserted into the
/// still-open transaction, and the Variants detected outside the writer fence
/// have not been persisted. A process death rolls the transaction back,
/// leaving the directory visible as an orphan that recovery can complete on a
/// later attempt.
pub const RECOVER_AFTER_ROW_INSERT: &str = "recover.after_row_insert";

/// `delete_unreferenced_library_dir`: the durable ownership intent exists,
/// while the proven orphan is still at its original path. A restart may
/// remove the stranded intent, but must leave the directory intact.
pub const DELETE_AFTER_INTENT_WRITE: &str = "delete.after_intent_write";

/// `delete_unreferenced_library_dir`: the proven orphan has atomically moved
/// into GMM's reserved quarantine and can no longer be recovered as a Mod.
/// A restart can finish the purge only while the reserved path still names
/// that same filesystem object.
pub const DELETE_AFTER_QUARANTINE_MOVE: &str = "delete.after_quarantine_move";

/// `delete_unreferenced_library_dir`: the durable quarantine is committed and
/// the identity handle that proved it has been released, immediately before
/// the shared purge re-opens the reserved path.
pub const DELETE_BEFORE_QUARANTINE_PURGE: &str = "delete.before_quarantine_purge";
}
