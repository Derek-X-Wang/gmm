use std::path::PathBuf;

use thiserror::Error;

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
    #[error(
        "no Model Importer origin is in effect for {game}. \
         Choose one in Settings to install it.{reason}"
    )]
    NoImporterOriginInEffect { game: String, reason: String },

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

    #[error("network error: {0}")]
    Network(String),

    #[error("GameBanana error: {0}")]
    GameBanana(String),

    #[error(
        "{game} is running (game session active since {since}); close the game before changing mods."
    )]
    SessionActive { game: String, since: String },
}

pub type Result<T> = std::result::Result<T, Error>;
