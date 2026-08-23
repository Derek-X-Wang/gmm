use std::path::PathBuf;

use thiserror::Error;

/// The one sentence that points a stuck user at the Importer Origin
/// control, written down once.
///
/// #127 removed "choose one in Settings" from these messages because no
/// such control existed — a message that sends a user somewhere they
/// cannot go is worse than one that admits GMM cannot proceed. #109
/// built the control, so the copy can name it again; keeping it in a
/// single constant is what stops the two halves drifting apart a second
/// time, and gives the test that guards this something concrete to
/// assert against.
pub const SET_AN_ORIGIN_HINT: &str =
    "Set one under Model Importer → Importer Origin, or switch the game to a \
     package you choose yourself.";

#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("io error at {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid game code: {0}")]
    InvalidGameCode(String),

    #[error("invalid mod source: {0}")]
    InvalidSource(String),

    #[error("zip error at {path:?}: {message}")]
    Zip { path: PathBuf, message: String },

    #[error(
        "zip entry escapes the import target (zip-slip): {0}. Aborted before any files were written."
    )]
    ZipSlip(String),

    #[error(
        "archive declares {actual} bytes uncompressed, but the import limit is {cap} bytes. \
         Raise the limit in settings if you trust this archive."
    )]
    ZipSizeCap { cap: u64, actual: u64 },

    #[error(
        "archive contains {actual} entries, but the import limit is {cap}. \
         Raise the limit in settings if you trust this archive."
    )]
    ZipEntryCap { cap: u32, actual: usize },

    #[error("zip entry {name:?} is unsafe: {reason}. Aborted before any files were written.")]
    ZipUnsafeEntry { name: String, reason: &'static str },

    #[error("diagnostics error: {0}")]
    Diagnostics(String),

    #[error(
        "the path {path:?} is on a {format} volume, but GMM junctions require NTFS. \
         Move the Library or the game install to an NTFS drive, or convert the volume."
    )]
    NonNtfsVolume { path: PathBuf, format: String },

    #[error("importer install error: {0}")]
    Importer(String),

    /// The archive does not have the shape of a Model Importer package.
    ///
    /// Its own variant rather than an IO or zip error, because "you
    /// picked the wrong file" and "the download is corrupt" need
    /// different actions from the user — hence `missing` naming exactly
    /// what was not found.
    #[error(
        "this archive does not look like a Model Importer package: {missing}. \
         A Model Importer ships {expected}. Nothing in the game directory was \
         changed."
    )]
    NotAModelImporter {
        missing: String,
        expected: &'static str,
    },

    /// The archive carries a compiled binary. A Model Importer is
    /// configuration and shaders; the DLLs it drives ship with the Loader
    /// package (ADR 0001), so an executable image here means the archive
    /// is something else — and one that would be dropped straight beside
    /// the game executable.
    #[error(
        "this archive contains compiled binaries ({entries}), which a Model \
         Importer never ships — the DLLs come with the Loader. Nothing in the \
         game directory was changed."
    )]
    ImporterArchiveHasBinaries { entries: String },

    /// Reading upstream release metadata failed. Distinct from
    /// [`Error::Importer`] because the release check also serves the
    /// Loader, which installs nothing — reporting a Loader check
    /// failure as an "importer install error" named the wrong
    /// subsystem to the user (#78).
    #[error("could not read the upstream release: {0}")]
    ReleaseMetadata(String),

    /// The origin's release-asset pattern is not a valid regular
    /// expression. Compiled-in patterns are covered by a test, but
    /// patterns also arrive from the recommended-importers manifest and
    /// from a user's own origin (ADR 0005), so this is reachable at
    /// runtime.
    #[error(
        "the release-asset pattern {pattern:?} is not a valid regular expression: {message}. \
         Fix the pattern for this Importer Origin."
    )]
    InvalidAssetPattern { pattern: String, message: String },

    /// No asset in the release matched the origin's pattern.
    ///
    /// Deliberately an error and never an empty result: `"Libs"` matched
    /// nothing in every `XXMI-Libs-Package` release ever published, and
    /// `.ok().flatten()` turned that into "up to date" for the entire
    /// life of the Loader update check (#78).
    #[error(
        "no asset in release {release} matched the pattern {pattern:?}. \
         That release publishes: {candidates}. Either upstream renamed its assets \
         or this Importer Origin's pattern is wrong."
    )]
    ReleaseAssetNoMatch {
        release: String,
        pattern: String,
        candidates: String,
    },

    /// More than one asset matched, so there is no single right answer.
    ///
    /// Never "first match wins": picking by release order is exactly how
    /// `SRMI-TEST-PACKAGE-v2.4.2.zip` would have been chosen over a real
    /// package had both existed (#79).
    #[error(
        "{count} assets in release {release} matched the pattern {pattern:?} ({matches}), \
         but exactly one must match. Narrow the pattern for this Importer Origin."
    )]
    ReleaseAssetAmbiguous {
        release: String,
        pattern: String,
        matches: String,
        count: usize,
    },

    /// No Importer Origin is in effect for the game, so there is
    /// nothing to install or check (ADR 0005 / #97). Either the
    /// recommended manifest retracted the compiled-in default, or the
    /// game never had one.
    ///
    /// The message now names a control, which #127 forbade *because the
    /// control did not exist*: no origin command was registered and
    /// nothing in the frontend exposed one, so the copy sent users
    /// somewhere they could not go. #109 built it. The rule #127 was
    /// really about — never point at a control that does not exist —
    /// holds either way, and `SET_AN_ORIGIN_HINT` is the single place
    /// that names it so the copy cannot drift out of step again.
    #[error(
        "GMM has no Model Importer origin for {game}, so there is nothing to \
         install from.{reason} {}",
        SET_AN_ORIGIN_HINT
    )]
    NoImporterOriginInEffect { game: String, reason: String },

    /// GMM recorded a Model Importer install and can no longer read the
    /// Importer Origin it came from (#124).
    ///
    /// Surfaced rather than worked around. Installing from whatever
    /// resolves would be exactly the silent origin switch #109 forbids,
    /// performed on the one install GMM understands least — and it is
    /// the project's recurring defect in its purest form: a read failure
    /// rendered as a perfectly ordinary install. The user's route out is
    /// the origin control, which also clears the unreadable record.
    #[error(
        "GMM recorded a Model Importer install for {game} but can no longer read \
         which Importer Origin it came from ({message}), so it will not install \
         over it from a different one. {}",
        SET_AN_ORIGIN_HINT
    )]
    InstalledImporterOriginUnreadable { game: String, message: String },

    /// The Model Importer files were installed, but GMM could not
    /// record what it installed.
    ///
    /// Never collapsed into success. The install command used to
    /// discard this failure and return the report anyway (#122), so the
    /// UI said "Installed" while GMM still held the previous version,
    /// the previous origin, or a mixture — and pin clearing, the update
    /// badge and the recommendation logic all went on reading it.
    #[error(
        "{game}'s Model Importer {version} was installed, but GMM could not record it: \
         {message}. The files are in place; GMM's record of the version and the \
         Importer Origin is not. Re-run the install once the problem is resolved."
    )]
    ImporterInstallNotRecorded {
        game: String,
        version: String,
        message: String,
    },

    /// The previous package was restored into the game directory, but
    /// GMM could not update its record of what is installed.
    ///
    /// Same rule as [`Self::ImporterInstallNotRecorded`] (#122), on the
    /// other path (#126): the files moved and the bookkeeping did not,
    /// which the caller has to be told rather than shown a success.
    #[error(
        "{game}'s Model Importer was rolled back to the backup at {backup}, but GMM \
         could not update its record of what is installed: {message}. The previous \
         package is in place; GMM's record still describes the install that was \
         undone."
    )]
    RollbackNotRecorded {
        game: String,
        backup: String,
        message: String,
    },

    #[error("network error: {0}")]
    Network(String),

    #[error("GameBanana error: {0}")]
    GameBanana(String),

    /// A reveal, recover or delete was asked for on a Library directory
    /// that does not (or no longer) qualifies.
    ///
    /// Always checked at the instant of the action rather than trusted
    /// from the report the user clicked: a Mod row can be created between
    /// the two, and delete is the one place GMM destroys Library bytes.
    #[error("{path:?} is not an unreferenced Library folder GMM can act on: {reason}.")]
    NotAnUnreferencedLibraryDir { path: PathBuf, reason: String },

    #[error(
        "Library delete quarantine {path:?} changed identity before purge; refusing to remove it. \
         Its intent and bytes remain for startup to retry."
    )]
    DeleteQuarantineIdentityChanged { path: PathBuf },

    #[error(
        "{mutation} stopped because the Library root changed from {previous:?} to {current:?} \
         while files were being prepared. No Mod row was committed."
    )]
    LibraryRootChangedDuringMutation {
        mutation: &'static str,
        previous: PathBuf,
        current: PathBuf,
    },

    #[error(
        "{game} is running (game session active since {since}); close the game before changing mods."
    )]
    SessionActive { game: String, since: String },
}

pub type Result<T> = std::result::Result<T, Error>;
