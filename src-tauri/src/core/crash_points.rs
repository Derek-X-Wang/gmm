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

/// `set_enabled(true)`: the Junction now exists, the row still says
/// disabled. Recoverable — see the Junction, believe the row, remove it.
pub const SET_ENABLED_AFTER_JUNCTION_CREATE: &str = "set_enabled.after_junction_create";

/// `set_enabled(false)`: the Junction is gone, the row still says
/// enabled. Recoverable — reconcile recreates it.
pub const SET_ENABLED_AFTER_JUNCTION_REMOVE: &str = "set_enabled.after_junction_remove";

/// `set_active_variant`: the row names the new Variant, the Junction
/// still points at the old one. Recoverable — the row is the source of
/// truth for which Variant is active.
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

/// `adopt_folder`: the row exists, Variants have not been detected yet.
pub const ADOPT_AFTER_ROW_INSERT: &str = "adopt.after_row_insert";

/// `import_zip`: the archive is extracted into the Library, no row
/// references it. Same orphan shape as [`ADOPT_AFTER_LIBRARY_COPY`].
pub const IMPORT_ZIP_AFTER_EXTRACT: &str = "import_zip.after_extract";

/// `import_zip`: the row exists, Variants have not been detected yet.
pub const IMPORT_ZIP_AFTER_ROW_INSERT: &str = "import_zip.after_row_insert";

/// `recover_unreferenced_library_dir`: a Library-root directory whose
/// name was not a usable ULID has been renamed to the fresh one, and no
/// row references it yet. Leaves the same orphan shape as
/// [`ADOPT_AFTER_LIBRARY_COPY`], under the new name — recoverable a
/// second time by the same feature, which is why the rename goes first.
pub const RECOVER_AFTER_LIBRARY_MOVE: &str = "recover.after_library_move";

/// `delete_unreferenced_library_dir`: the durable ownership intent exists,
/// while the proven orphan is still at its original path. A restart may
/// remove the stranded intent, but must leave the directory intact.
pub const DELETE_AFTER_INTENT_WRITE: &str = "delete.after_intent_write";

/// `delete_unreferenced_library_dir`: the proven orphan has atomically moved
/// into GMM's reserved quarantine and can no longer be recovered as a Mod.
/// A restart finishes purging that quarantine.
pub const DELETE_AFTER_QUARANTINE_MOVE: &str = "delete.after_quarantine_move";

/// Every point, so a test can enumerate them and so adding one without
/// covering it is visible in review.
pub const ALL: &[&str] = &[
    SET_ENABLED_AFTER_JUNCTION_CREATE,
    SET_ENABLED_AFTER_JUNCTION_REMOVE,
    SET_ACTIVE_VARIANT_AFTER_DB_UPDATE,
    SET_ACTIVE_VARIANT_AFTER_JUNCTION_REMOVE,
    ADOPT_AFTER_LIBRARY_COPY,
    ADOPT_AFTER_ROW_INSERT,
    IMPORT_ZIP_AFTER_EXTRACT,
    IMPORT_ZIP_AFTER_ROW_INSERT,
    RECOVER_AFTER_LIBRARY_MOVE,
    DELETE_AFTER_INTENT_WRITE,
    DELETE_AFTER_QUARANTINE_MOVE,
];
