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
        // If the initial delete erroed but the path is now gone we treat
        // that as success; if it errored AND the path is still gone above's
        // remove_dir would have already errored, so this only forwards
        // genuine reparse-point removal failures.
        if let Err(source) = primary {
            if link.exists() {
                return Err(Error::Io {
                    path: link.to_path_buf(),
                    source,
                });
            }
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
