//! Per-game install-path detection.
//!
//! Each supported Game lives in its own submodule with a single
//! responsibility: produce an `Option<PathBuf>` pointing at the
//! directory that contains the game's executable. Detection is a
//! best-effort heuristic — when it fails the UI falls back to a
//! manual picker, exactly as it did before this slice landed.
//!
//! GMM never copies XXMI Launcher's per-game detection code (ADR 0002),
//! but the heuristics (registry uninstall keys, exe + Data-folder
//! validation) are public.

pub mod endfield;
pub mod genshin;
pub mod honkai_impact;
pub mod star_rail;
pub mod wuthering;
pub mod zenless;
