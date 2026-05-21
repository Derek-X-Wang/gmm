use serde::{Deserialize, Serialize};

use super::error::{Error, Result};

/// The six XXMI-family games GMM supports in v1.
///
/// Stored on disk as a lowercase slug (`gimi`, `srmi`, ...). See `CONTEXT.md`
/// for the canonical naming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GameCode {
    Gimi,
    Srmi,
    Zzmi,
    Wwmi,
    Himi,
    Efmi,
}

impl GameCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            GameCode::Gimi => "gimi",
            GameCode::Srmi => "srmi",
            GameCode::Zzmi => "zzmi",
            GameCode::Wwmi => "wwmi",
            GameCode::Himi => "himi",
            GameCode::Efmi => "efmi",
        }
    }

    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "gimi" => Ok(GameCode::Gimi),
            "srmi" => Ok(GameCode::Srmi),
            "zzmi" => Ok(GameCode::Zzmi),
            "wwmi" => Ok(GameCode::Wwmi),
            "himi" => Ok(GameCode::Himi),
            "efmi" => Ok(GameCode::Efmi),
            other => Err(Error::InvalidGameCode(other.to_string())),
        }
    }
}
