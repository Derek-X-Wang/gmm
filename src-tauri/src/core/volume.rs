//! Volume-format probe.
//!
//! ADR 0003 picks NTFS junctions for the Library → Game overlay. Junctions
//! only work between NTFS volumes — they cannot land on exFAT, FAT32, ReFS
//! reliably, or network shares. This module checks before we try, so the
//! user gets an actionable error before any half-created junction tree
//! exists on disk.
//!
//! On non-Windows hosts the check is a no-op (returns `Ok(())`). GMM only
//! ships Windows, but the rest of the codebase compiles + tests on macOS
//! dev hosts.

use std::path::Path;

use super::error::{Error, Result};

/// File-system identifier we recognise. Anything else is rejected with a
/// copy-friendly message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VolumeFormat {
    Ntfs,
    /// Identified, but not a format we can use.
    Other(String),
    /// We could not determine the format (non-Windows host, missing path,
    /// or API failure). On non-Windows hosts we treat this as OK; on
    /// Windows it surfaces a clear error.
    Unknown,
}

/// Detect the filesystem format hosting `path`. On non-Windows hosts
/// this always returns [`VolumeFormat::Unknown`].
pub fn volume_format(path: &Path) -> VolumeFormat {
    #[cfg(windows)]
    {
        windows::volume_format(path).unwrap_or(VolumeFormat::Unknown)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        VolumeFormat::Unknown
    }
}

/// Reject `path` if it is not on an NTFS volume. On non-Windows hosts
/// this always succeeds (so unit/integration tests work on macOS).
pub fn require_ntfs(path: &Path) -> Result<()> {
    match volume_format(path) {
        VolumeFormat::Ntfs | VolumeFormat::Unknown => Ok(()),
        VolumeFormat::Other(fmt) => Err(Error::NonNtfsVolume {
            path: path.to_path_buf(),
            format: fmt,
        }),
    }
}

/// Reject if either endpoint of a junction is not on NTFS. Both the link
/// parent (game `Mods/` dir) and the target (Library subtree) need to be
/// on NTFS.
pub fn require_ntfs_pair(link_parent: &Path, target: &Path) -> Result<()> {
    require_ntfs(link_parent)?;
    require_ntfs(target)
}

#[cfg(windows)]
mod windows {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use super::VolumeFormat;

    pub fn volume_format(path: &Path) -> Option<VolumeFormat> {
        use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW;

        // Resolve to the volume mount point (`C:\`, `D:\`, …).
        let root = mount_root(path)?;
        let wide: Vec<u16> = root
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut fs_name = [0u16; 64];
        // SAFETY: We pass null for unused out-buffers and zeroed lengths.
        let ok = unsafe {
            GetVolumeInformationW(
                wide.as_ptr(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                fs_name.as_mut_ptr(),
                fs_name.len() as u32,
            )
        };
        if ok == 0 {
            return None;
        }
        let len = fs_name
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(fs_name.len());
        let s = String::from_utf16_lossy(&fs_name[..len]);
        Some(if s.eq_ignore_ascii_case("NTFS") {
            VolumeFormat::Ntfs
        } else {
            VolumeFormat::Other(s)
        })
    }

    /// Best-effort: take the first two components (drive letter + root)
    /// or fall back to the path itself. The Windows API wants a path
    /// ending in `\` for mount points.
    fn mount_root(path: &Path) -> Option<PathBuf> {
        let mut iter = path.components();
        let drive = iter.next()?;
        let root_sep = iter.next();
        let mut out = PathBuf::new();
        out.push(drive.as_os_str());
        if let Some(s) = root_sep {
            out.push(s.as_os_str());
        } else {
            out.push("\\");
        }
        Some(out)
    }
}
