//! Junction creation and removal.
//!
//! Windows uses real NTFS directory junctions via the `junction` crate, which
//! does not require admin rights or Developer Mode (see ADR 0003). On unix
//! we use a directory symlink purely so integration tests run on macOS dev
//! hosts; production never sees this path.

use std::path::Path;

use super::error::{Error, Result};

pub fn create(link: &Path, target: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        junction::create(target, link).map_err(|source| Error::Io {
            path: link.to_path_buf(),
            source,
        })
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).map_err(|source| Error::Io {
            path: link.to_path_buf(),
            source,
        })
    }
}

pub fn remove(link: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        // junction::delete clears the reparse point and is supposed to also
        // remove the underlying directory entry, but in practice (observed
        // on windows-latest GitHub runners) the directory sometimes lingers.
        // Belt-and-suspenders: clear the reparse point, then fs::remove_dir
        // if anything is left.
        let primary = junction::delete(link);
        if link.exists() {
            std::fs::remove_dir(link).map_err(|source| Error::Io {
                path: link.to_path_buf(),
                source,
            })?;
        }
        // A dangling directory symlink has an entry but `Path::exists` says
        // false because it follows the missing target. Inspect the entry
        // itself both before and after the fallback so success means the
        // deployment name is truly absent.
        let entry_survives = link.exists();
        if let Err(source) = primary {
            if entry_survives {
                return Err(Error::Io {
                    path: link.to_path_buf(),
                    source,
                });
            }
        }
        if entry_survives {
            return Err(Error::Io {
                path: link.to_path_buf(),
                source: std::io::Error::other(
                    "the Junction removal returned while the deployment entry still exists",
                ),
            });
        }
        Ok(())
    }
    #[cfg(unix)]
    {
        std::fs::remove_file(link).map_err(|source| Error::Io {
            path: link.to_path_buf(),
            source,
        })
    }
}

#[cfg(windows)]
#[allow(dead_code)]
fn link_entry_exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(Error::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}
