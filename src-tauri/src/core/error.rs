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

    #[error("diagnostics error: {0}")]
    Diagnostics(String),

    #[error(
        "the path {path:?} is on a {format} volume, but GMM junctions require NTFS. \
         Move the Library or the game install to an NTFS drive, or convert the volume."
    )]
    NonNtfsVolume { path: PathBuf, format: String },

    #[error("importer install error: {0}")]
    Importer(String),

    #[error("network error: {0}")]
    Network(String),
}

pub type Result<T> = std::result::Result<T, Error>;
