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

    /// Reading upstream release metadata failed. Distinct from
    /// [`Error::Importer`] because the release check also serves the
    /// Loader, which installs nothing — reporting a Loader check
    /// failure as an "importer install error" named the wrong
    /// subsystem to the user (#78).
    #[error("could not read the upstream release: {0}")]
    ReleaseMetadata(String),

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
