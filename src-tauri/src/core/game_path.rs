//! Read-only facts about a saved Game installation and its Model Importer.
//! Filesystem uncertainty is data, not absence; database failures stay errors.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::detect;
use super::error::{Error, Result};
use super::filesystem::metadata_if_exists;
use super::games::GameCode;
use super::importer::{IMPORTER_REQUIRED_DIRS, IMPORTER_REQUIRED_FILE};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GamePathStatus {
    pub game: GameCode,
    /// Configuration only. Retained even when inspection fails or files vanish.
    pub saved_path: Option<PathBuf>,
    pub game_path: GamePathState,
    pub model_importer: ModelImporterState,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum GamePathState {
    Unset,
    /// The existing fallible GameProfile validator verified the Game layout.
    Readable,
    /// Each named path returned NotFound. Executable alternatives are listed
    /// together only when all alternatives are confirmed missing.
    Missing {
        paths: Vec<PathBuf>,
    },
    /// A known wrong type or rejected layout. This is not proven absence.
    Invalid {
        error: String,
    },
    /// The failed observation and its actual error; never a missing install.
    Unreadable {
        path: PathBuf,
        error: String,
    },
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ModelImporterState {
    /// The existing package's required file and directories are readable.
    /// This asserts layout presence, not authenticity, version, or launchability.
    Installed,
    /// Required entries returned NotFound. No version is inferred.
    Absent { paths: Vec<PathBuf> },
    /// Inspection failed, the layout has a wrong type, or no readable Game
    /// installation is available. The Game fact remains independent.
    Unverified { error: String },
}

pub(super) fn inspect(game: GameCode, saved_path: Option<PathBuf>) -> GamePathStatus {
    let game_path = match saved_path.as_deref() {
        None => GamePathState::Unset,
        Some(path) => match inspect_game(game, path) {
            Ok(state) => state,
            Err(Error::Io { path, source }) if source.kind() == io::ErrorKind::NotFound => {
                GamePathState::Missing { paths: vec![path] }
            }
            Err(error) => {
                let path = match &error {
                    Error::Io { path, .. } => path.clone(),
                    _ => path.to_path_buf(),
                };
                GamePathState::Unreadable {
                    path,
                    error: error.to_string(),
                }
            }
        },
    };
    let model_importer = match (&game_path, saved_path.as_deref()) {
        (GamePathState::Readable, Some(path)) => match inspect_importer(path) {
            Ok(state) => state,
            Err(error) => ModelImporterState::Unverified {
                error: error.to_string(),
            },
        },
        _ => ModelImporterState::Unverified {
            error: "Model Importer inspection requires a verified readable Game installation."
                .into(),
        },
    };
    GamePathStatus {
        game,
        saved_path,
        game_path,
        model_importer,
    }
}

fn inspect_game(game: GameCode, path: &Path) -> Result<GamePathState> {
    let profile = game.profile();
    if let Some(validate) = profile.validate {
        if validate(path)? {
            return Ok(GamePathState::Readable);
        }
    }
    // Ok(false) says nothing about why validation failed. Diagnose absence
    // with explicit lookups without changing the setter's existing contract.
    let invalid = || GamePathState::Invalid {
        error: Error::InvalidGameInstallPath {
            path: path.to_path_buf(),
            game: profile.display_name,
            expected: profile.executable_candidates.join(" or "),
        }
        .to_string(),
    };
    let Some(metadata) = metadata_at(path)? else {
        return Ok(GamePathState::Missing {
            paths: vec![path.to_path_buf()],
        });
    };
    if !metadata.is_dir() {
        return Ok(invalid());
    }
    let mut missing_executables = Vec::new();
    let mut executable_present = false;
    for name in profile.executable_candidates {
        let executable = path.join(name);
        match metadata_at(&executable)? {
            None => missing_executables.push(executable),
            Some(metadata) if metadata.is_file() => executable_present = true,
            Some(_) => return Ok(invalid()),
        }
    }
    let data = match game {
        GameCode::Gimi => Some(path.join(detect::genshin::DATA_DIR_NAME)),
        GameCode::Srmi => Some(path.join(detect::star_rail::DATA_DIR_NAME)),
        GameCode::Zzmi => Some(path.join(detect::zenless::DATA_DIR_NAME)),
        GameCode::Himi => Some(path.join(detect::honkai_impact::DATA_DIR_NAME)),
        GameCode::Wwmi | GameCode::Efmi => path
            .parent()
            .and_then(Path::parent)
            .map(|root| root.join("Content")),
    };
    let Some(data) = data else {
        return Ok(invalid());
    };
    let mut missing = if executable_present {
        Vec::new()
    } else {
        missing_executables
    };
    match metadata_at(&data)? {
        None => missing.push(data),
        Some(metadata) if metadata.is_dir() => {}
        Some(_) => return Ok(invalid()),
    }
    // A recheck can race validation. Never upgrade Ok(false) to Readable.
    Ok(if missing.is_empty() {
        invalid()
    } else {
        GamePathState::Missing { paths: missing }
    })
}

fn metadata_at(path: &Path) -> Result<Option<fs::Metadata>> {
    metadata_if_exists(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn directory_entries(path: &Path) -> Result<Vec<PathBuf>> {
    let entries = fs::read_dir(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    consume_entries(path, entries.map(|entry| entry.map(|entry| entry.path())))
}

fn consume_entries(
    path: &Path,
    entries: impl IntoIterator<Item = io::Result<PathBuf>>,
) -> Result<Vec<PathBuf>> {
    entries
        .into_iter()
        .map(|entry| {
            entry.map_err(|source| Error::Io {
                path: path.to_path_buf(),
                source,
            })
        })
        .collect()
}

fn inspect_importer(path: &Path) -> Result<ModelImporterState> {
    // Package entry names are case-insensitive, as in find_d3dx_ini and the
    // archive validator. Enumeration must finish before making any claim.
    let entries = directory_entries(path)?;
    let mut missing = Vec::new();
    for (name, directory) in std::iter::once((IMPORTER_REQUIRED_FILE, false))
        .chain(IMPORTER_REQUIRED_DIRS.iter().map(|name| (*name, true)))
    {
        let entry = entries
            .iter()
            .find(|entry| {
                entry
                    .file_name()
                    .is_some_and(|entry| entry.to_string_lossy().eq_ignore_ascii_case(name))
            })
            .cloned()
            .unwrap_or_else(|| path.join(name));
        match inspect_importer_entry(&entry, directory) {
            Ok(true) => {}
            Ok(false) => missing.push(entry),
            Err(Error::Io { path, source }) if source.kind() == io::ErrorKind::NotFound => {
                missing.push(path);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(if missing.is_empty() {
        ModelImporterState::Installed
    } else {
        ModelImporterState::Absent { paths: missing }
    })
}

fn inspect_importer_entry(path: &Path, directory: bool) -> Result<bool> {
    let Some(metadata) = metadata_at(path)? else {
        return Ok(false);
    };
    if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
        return Err(Error::Importer(format!(
            "Model Importer entry {} must be a {}.",
            path.display(),
            if directory { "directory" } else { "file" },
        )));
    }
    if directory {
        directory_entries(path)?;
    } else {
        fs::File::open(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumeration_failure_after_a_successful_entry_remains_uncertainty() {
        let path = Path::new("Game/Core");
        let entries = [
            Ok(path.join("library.ini")),
            Err(io::Error::from(io::ErrorKind::PermissionDenied)),
        ];
        let error = consume_entries(path, entries).expect_err("late enumeration failure");
        assert!(matches!(error, Error::Io { path: ref failed, ref source }
            if failed == path && source.kind() == io::ErrorKind::PermissionDenied));
    }
}
