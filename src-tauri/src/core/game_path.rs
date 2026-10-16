//! Read-only facts about a saved Game installation and its Model Importer.
//! Filesystem uncertainty is data, not absence; database failures stay errors.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::detect;
use super::error::{Error, Result};
use super::filesystem::symlink_metadata_if_exists;
use super::games::{GameCode, ValidateFn};
use super::importer::{IMPORTER_REQUIRED_DIRS, IMPORTER_REQUIRED_FILE};
use super::updates::ImporterInstallRecord;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GamePathStatus {
    pub game: GameCode,
    /// Configuration only. Retained even when inspection fails or files vanish.
    pub saved_path: Option<PathBuf>,
    pub game_path: GamePathState,
    pub model_importer: ModelImporterState,
    /// User intent, not install evidence. A pin is never a receipt.
    pub pinned_version: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum GamePathState {
    NoSavedPath,
    /// The existing fallible GameProfile validator verified the Game layout.
    Readable,
    /// The saved Game directory itself returned NotFound.
    SavedPathNotFound {
        paths: Vec<PathBuf>,
    },
    /// The saved directory exists, but required Game entries returned NotFound.
    /// Executable alternatives are listed only when all are confirmed missing.
    RequiredEntriesNotFound {
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
    /// GMM recorded this install and the required entries are readable.
    /// Presence is not proof of authenticity or launchability.
    Installed { record: ImporterInstallRecord },
    /// A recorded install has missing or unreadable required entries.
    Damaged {
        record: ImporterInstallRecord,
        paths: Vec<PathBuf>,
        error: Option<String>,
    },
    /// No receipt and no required entries found. This cannot establish that
    /// an importer was never installed (or was not damaged/removed by hand).
    NoImporterEntriesFound { paths: Vec<PathBuf> },
    /// At least one required entry exists, but GMM has no install receipt.
    /// Missing entries remain explicit; no origin or version is inferred.
    UnrecordedImporterPresent { paths: Vec<PathBuf> },
    /// Inspection was not possible or a required entry has the wrong type.
    Unverified {
        path: Option<PathBuf>,
        error: String,
        record: Option<ImporterInstallRecord>,
    },
}

pub(super) fn inspect(
    game: GameCode,
    saved_path: Option<PathBuf>,
    record: Option<ImporterInstallRecord>,
    pinned_version: Option<String>,
) -> GamePathStatus {
    let game_path = match saved_path.as_deref() {
        None => GamePathState::NoSavedPath,
        Some(path) => match inspect_game(game, path, game.profile().validate) {
            Ok(state) => state,
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
        (GamePathState::Readable, Some(path)) => match inspect_importer(path, record.clone()) {
            Ok(state) => state,
            Err(error) => importer_inspection_failure(error, record),
        },
        _ => ModelImporterState::Unverified {
            path: None,
            error: "Model Importer inspection requires a verified readable Game installation."
                .into(),
            record,
        },
    };
    GamePathStatus {
        game,
        saved_path,
        game_path,
        model_importer,
        pinned_version,
    }
}

fn importer_inspection_failure(
    error: Error,
    record: Option<ImporterInstallRecord>,
) -> ModelImporterState {
    let path = match &error {
        Error::Io { path, .. } => Some(path.clone()),
        _ => None,
    };
    // Wrong types keep the existing concrete layout error and unverified state.
    if matches!(&error, Error::Io { .. }) {
        if let Some(record) = record {
            return ModelImporterState::Damaged {
                record,
                paths: path.into_iter().collect(),
                error: Some(error.to_string()),
            };
        }
    }
    ModelImporterState::Unverified {
        path,
        error: error.to_string(),
        record,
    }
}

fn inspect_game(
    game: GameCode,
    path: &Path,
    validate: Option<ValidateFn>,
) -> Result<GamePathState> {
    inspect_game_with_metadata(game, path, validate, metadata_at)
}

fn inspect_game_with_metadata(
    game: GameCode,
    path: &Path,
    validate: Option<ValidateFn>,
    mut metadata_at: impl FnMut(&Path) -> Result<Option<fs::Metadata>>,
) -> Result<GamePathState> {
    let profile = game.profile();
    let mut validation_failure = None;
    if let Some(validate) = validate {
        match validate(path) {
            Ok(true) => return Ok(GamePathState::Readable),
            Ok(false) => {}
            Err(Error::Io { path, source }) if source.kind() == io::ErrorKind::NotFound => {
                // NotFound for one executable alternative does not prove the
                // Game is missing. Diagnose every alternative and required
                // directory before making an absence claim.
                validation_failure = Some(Error::Io { path, source });
            }
            Err(error) => return Err(error),
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
        return Ok(GamePathState::SavedPathNotFound {
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
    if missing.is_empty() {
        if let Some(error) = validation_failure {
            return Err(error);
        }
    }
    Ok(if missing.is_empty() {
        invalid()
    } else {
        // Child NotFound results can follow the directory disappearing. Make
        // the specific missing-entry claim only after rechecking its scope.
        let Some(metadata) = metadata_at(path)? else {
            return Ok(GamePathState::SavedPathNotFound {
                paths: vec![path.to_path_buf()],
            });
        };
        if !metadata.is_dir() {
            return Ok(invalid());
        }
        GamePathState::RequiredEntriesNotFound { paths: missing }
    })
}

fn metadata_at(path: &Path) -> Result<Option<fs::Metadata>> {
    let entry = symlink_metadata_if_exists(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if entry.is_none() {
        return Ok(None);
    }
    // The entry exists. A missing or unavailable link target must remain an
    // error, rather than becoming permission to replace the existing entry.
    fs::metadata(path).map(Some).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn missing_after_failure(error: Error) -> Result<PathBuf> {
    if let Error::Io { path, source } = &error {
        // A validator or directory iterator can report NotFound for a child
        // while naming its parent as the failed inspection scope. Only a
        // direct lookup proves that the named required path is itself absent.
        if source.kind() == io::ErrorKind::NotFound && metadata_at(path)?.is_none() {
            return Ok(path.clone());
        }
    }
    Err(error)
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

fn inspect_importer(
    path: &Path,
    record: Option<ImporterInstallRecord>,
) -> Result<ModelImporterState> {
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
            Err(error) => match missing_after_failure(error) {
                Ok(path) => missing.push(path),
                Err(error) => {
                    let mut state = importer_inspection_failure(error, record);
                    match &mut state {
                        ModelImporterState::Damaged { paths, .. } => paths.extend(missing),
                        ModelImporterState::Unverified { path, .. } => *path = Some(entry),
                        _ => unreachable!("inspection failure is damaged or unverified"),
                    }
                    return Ok(state);
                }
            },
        }
    }
    Ok(match record {
        Some(record) if missing.is_empty() => ModelImporterState::Installed { record },
        Some(record) => ModelImporterState::Damaged {
            record,
            paths: missing,
            error: None,
        },
        None if missing.len() == 1 + IMPORTER_REQUIRED_DIRS.len() => {
            ModelImporterState::NoImporterEntriesFound { paths: missing }
        }
        None => ModelImporterState::UnrecordedImporterPresent { paths: missing },
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
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path();
        for kind in [io::ErrorKind::PermissionDenied, io::ErrorKind::NotFound] {
            let entries = [Ok(path.join("library.ini")), Err(io::Error::from(kind))];
            let error = consume_entries(path, entries).expect_err("late enumeration failure");
            let error = missing_after_failure(error)
                .expect_err("an enumeration error cannot prove the inspected parent is missing");
            assert!(matches!(error, Error::Io { path: ref failed, ref source }
                if failed == path && source.kind() == kind));
        }
    }

    #[test]
    fn missing_entries_recheck_saved_directory_after_child_lookups() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("Game");
        fs::create_dir(&path).unwrap();
        let mut reads = Vec::new();
        let state = inspect_game_with_metadata(GameCode::Gimi, &path, None, |entry| {
            // The initial directory lookup succeeds; it vanishes before any
            // child lookup, as it can when a drive disconnects.
            if reads.len() == 1 {
                fs::remove_dir(&path).unwrap();
            }
            reads.push(entry.to_path_buf());
            metadata_at(entry)
        })
        .unwrap();
        assert_eq!(
            state,
            GamePathState::SavedPathNotFound {
                paths: vec![path.clone()]
            }
        );
        assert_eq!(
            reads,
            vec![
                path.clone(),
                path.join("GenshinImpact.exe"),
                path.join("YuanShen.exe"),
                path.join(detect::genshin::DATA_DIR_NAME),
                path,
            ]
        );
    }

    #[test]
    fn missing_entries_recheck_preserves_uncertainty_and_wrong_directory_type() {
        for unavailable in [true, false] {
            let tmp = tempfile::TempDir::new().unwrap();
            let path = tmp.path().join("Game");
            fs::create_dir(&path).unwrap();
            let mut root_reads = 0;
            let result = inspect_game_with_metadata(GameCode::Gimi, &path, None, |entry| {
                if entry == path {
                    root_reads += 1;
                    if root_reads == 2 {
                        if unavailable {
                            return Err(Error::Io {
                                path: path.clone(),
                                source: io::Error::from(io::ErrorKind::PermissionDenied),
                            });
                        }
                        fs::remove_dir(&path).unwrap();
                        fs::write(&path, b"replacement file").unwrap();
                    }
                }
                metadata_at(entry)
            });
            if unavailable {
                assert!(matches!(result, Err(Error::Io { path: failed, source })
                    if failed == path && source.kind() == io::ErrorKind::PermissionDenied));
            } else {
                assert!(matches!(result, Ok(GamePathState::Invalid { .. })));
            }
            assert_eq!(root_reads, 2);
        }
    }

    #[test]
    fn validator_not_found_diagnoses_every_executable_alternative() {
        fn vanished_alternative(path: &Path) -> Result<bool> {
            Err(Error::Io {
                path: path.join("YuanShen.exe"),
                source: io::Error::from(io::ErrorKind::NotFound),
            })
        }
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path();
        fs::write(path.join("GenshinImpact.exe"), b"valid alternative").unwrap();
        fs::create_dir(path.join(detect::genshin::DATA_DIR_NAME)).unwrap();
        let error = inspect_game(GameCode::Gimi, path, Some(vanished_alternative))
            .expect_err("a vanished optional alternative cannot prove a missing Game");
        assert!(matches!(error, Error::Io { path: ref failed, ref source }
            if failed == &path.join("YuanShen.exe") && source.kind() == io::ErrorKind::NotFound));
        fs::remove_file(path.join("GenshinImpact.exe")).unwrap();
        let state = inspect_game(GameCode::Gimi, path, Some(vanished_alternative)).unwrap();
        assert_eq!(
            state,
            GamePathState::RequiredEntriesNotFound {
                paths: vec![path.join("GenshinImpact.exe"), path.join("YuanShen.exe")],
            },
            "all alternatives must be directly confirmed missing, not just the failed one"
        );
    }
}
