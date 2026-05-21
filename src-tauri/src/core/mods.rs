use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::error::Error;
use super::games::GameCode;

/// How a Mod entered the Library. See `CONTEXT.md` § Source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// User pointed GMM at an already-extracted folder (slice 1a).
    Manual,
    /// User dropped or picked a local ZIP (slice 1b).
    Local,
    /// Ingested from GameBanana by URL or submission ID (slice 11).
    Gamebanana,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Manual => "manual",
            Source::Local => "local",
            Source::Gamebanana => "gamebanana",
        }
    }
}

impl FromStr for Source {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "manual" => Ok(Source::Manual),
            "local" => Ok(Source::Local),
            "gamebanana" => Ok(Source::Gamebanana),
            other => Err(Error::InvalidSource(other.to_string())),
        }
    }
}

/// A Mod — the unit of enable/disable. One Junction per enabled Mod.
/// See `CONTEXT.md` § Mod.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mod {
    pub id: String,
    pub game: GameCode,
    pub name: String,
    pub source: Source,
    pub library_path: PathBuf,
    pub enabled: bool,
    /// GameBanana submission ID when `source == "gamebanana"`. None for
    /// manual/local mods.
    #[serde(default)]
    pub gamebanana_id: Option<u64>,
    /// Public URL on the source platform (e.g. the GameBanana
    /// submission page). Used by the UI's "View on GameBanana" link.
    #[serde(default)]
    pub source_url: Option<String>,
    /// Author name as the upstream platform records it.
    #[serde(default)]
    pub author: Option<String>,
    /// Version string captured at ingest time. Slice 13c surfaces a
    /// badge when the upstream version moves ahead.
    #[serde(default)]
    pub version: Option<String>,
    /// Screenshot URL (preview thumbnail).
    #[serde(default)]
    pub screenshot_url: Option<String>,
}
