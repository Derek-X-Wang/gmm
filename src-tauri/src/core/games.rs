use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::detect;
use super::error::Error;

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

    /// Look up the static per-game wiring profile. Always returns a
    /// row (one per `GameCode`); rows for games that have not been
    /// ported yet leave their `importer_repo` / `detect` / exe
    /// candidates empty so callers can fall back gracefully.
    pub fn profile(&self) -> &'static GameProfile {
        for p in GAME_PROFILES {
            if p.code as u8 == *self as u8 {
                return p;
            }
        }
        unreachable!("GAME_PROFILES must cover every GameCode variant")
    }

    /// Iterate every game whose backend wiring is complete (detect +
    /// importer repo + at least one exe candidate). Drives the
    /// per-game tab strip in the React UI.
    pub fn ported() -> impl Iterator<Item = &'static GameProfile> {
        GAME_PROFILES.iter().filter(|p| p.is_ported())
    }
}

impl FromStr for GameCode {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
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

/// Function signature shared by every per-game detector.
pub type DetectFn = fn() -> Option<PathBuf>;

/// How `launch_game` gets the Model Importer DLL into the running
/// game process. Defaults to `Hook` for every game where XXMI uses
/// the CBT-hook + window-created path; switches to `Inject` for
/// titles upstream marks `custom_launch_inject_mode = 'Inject'` (EFMI
/// today; see slice 10 / #20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InjectMode {
    /// Install a CBT hook before spawning; wait for the game window
    /// creation to trigger LoadLibraryW inside the game process. The
    /// default for the Hoyoverse trio + Kuro's Wuthering Waves.
    Hook,
    /// Spawn first, then call `Loader::inject(pid, dll)` directly
    /// against the running process. Used by EFMI per XXMI upstream.
    Inject,
}

/// Static per-game wiring. The registry lets us add a new game (slices
/// #16–#20) by appending a row instead of touching match arms across
/// `commands.rs` / `detect/` / the UI.
///
/// An unported game ships with `importer_repo = None`, `detect = None`,
/// and `executable_candidates = &[]`. Callers see `is_ported() == false`
/// and surface a "wired up soon" message instead of pretending the game
/// works.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameProfile {
    pub code: GameCode,
    /// Human-readable name for tabs and copy.
    pub display_name: &'static str,
    /// `(repo, asset_filter)` tuple matching the upstream Model
    /// Importer release on GitHub. `None` until the per-game port
    /// lands.
    pub importer_repo: Option<(&'static str, &'static str)>,
    /// Game executable file names tried in order under the install
    /// directory. Empty `&[]` until the per-game port lands.
    pub executable_candidates: &'static [&'static str],
    /// Best-effort install-path auto-detector. `None` until the
    /// per-game port lands.
    #[serde(skip_serializing)]
    pub detect: Option<DetectFn>,
    /// How `launch_game` injects the Model Importer DLL. See
    /// [`InjectMode`]; defaults to `Hook` everywhere except EFMI.
    pub inject_mode: InjectMode,
}

impl GameProfile {
    /// `true` iff the per-game port has wired importer + detect + exe.
    /// Used by `GameCode::ported` to surface the tabs in the UI.
    pub fn is_ported(&self) -> bool {
        self.importer_repo.is_some()
            && self.detect.is_some()
            && !self.executable_candidates.is_empty()
    }
}

/// The single registry of per-game wiring. Order is preserved by
/// `GameCode::ported()` so the React tab strip renders games in the
/// same sequence (Genshin / Star Rail / ZZZ / Wuthering Waves /
/// Honkai Impact 3rd / Endfield).
pub const GAME_PROFILES: &[GameProfile] = &[
    GameProfile {
        code: GameCode::Gimi,
        display_name: "Genshin Impact",
        importer_repo: Some(("SpectrumQT/GIMI-Package", "GIMI")),
        executable_candidates: &["GenshinImpact.exe", "YuanShen.exe"],
        detect: Some(detect::genshin::detect),
        inject_mode: InjectMode::Hook,
    },
    GameProfile {
        code: GameCode::Srmi,
        display_name: "Honkai: Star Rail",
        importer_repo: Some(("SpectrumQT/SRMI-Package", "SRMI")),
        executable_candidates: &["StarRail.exe"],
        detect: Some(detect::star_rail::detect),
        inject_mode: InjectMode::Hook,
    },
    GameProfile {
        code: GameCode::Zzmi,
        display_name: "Zenless Zone Zero",
        importer_repo: Some(("SpectrumQT/ZZMI-Package", "ZZMI")),
        executable_candidates: &["ZenlessZoneZero.exe"],
        detect: Some(detect::zenless::detect),
        inject_mode: InjectMode::Hook,
    },
    GameProfile {
        code: GameCode::Wwmi,
        display_name: "Wuthering Waves",
        importer_repo: Some(("SpectrumQT/WWMI-Package", "WWMI")),
        executable_candidates: &["Client-Win64-Shipping.exe"],
        detect: Some(detect::wuthering::detect),
        inject_mode: InjectMode::Hook,
    },
    GameProfile {
        code: GameCode::Himi,
        display_name: "Honkai Impact 3rd",
        importer_repo: Some(("SpectrumQT/HIMI-Package", "HIMI")),
        executable_candidates: &["BH3.exe", "Bh3.exe"],
        detect: Some(detect::honkai_impact::detect),
        inject_mode: InjectMode::Hook,
    },
    GameProfile {
        code: GameCode::Efmi,
        display_name: "Endfield",
        importer_repo: Some(("SpectrumQT/EFMI-Package", "EFMI")),
        executable_candidates: &["Endfield-Win64-Shipping.exe", "Endfield.exe"],
        detect: Some(detect::endfield::detect),
        inject_mode: InjectMode::Inject,
    },
];
