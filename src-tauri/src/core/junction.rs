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
        junction::delete(link).map_err(|source| Error::Io {
            path: link.to_path_buf(),
            source,
        })
    }
    #[cfg(unix)]
    {
        std::fs::remove_file(link).map_err(|source| Error::Io {
            path: link.to_path_buf(),
            source,
        })
    }
}
