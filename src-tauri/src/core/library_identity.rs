//! Filesystem identity for Library directories.
//!
//! Paths are user-facing names, not ownership evidence. In particular,
//! Windows can name one NTFS directory with alternate casing or an 8.3 alias.
//! On Windows this module opens the directory itself without following a
//! reparse point. On non-Windows, `File::open` follows symlinks; callers that
//! require link refusal check `symlink_metadata` before opening the handle.
//! The handle stays alive alongside the volume/file identity learned from it.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct DirectoryIdentity {
    volume: u64,
    file: u64,
}

#[derive(Debug)]
pub(super) struct IdentifiedDirectory {
    path: PathBuf,
    identity: DirectoryIdentity,
    // Keeping the handle is intentional: validation must return the evidence
    // it established rather than discarding it before the filesystem act. It
    // is not an exclusion lock: the Windows handle shares READ/WRITE/DELETE.
    _handle: File,
}

impl IdentifiedDirectory {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        let handle = open_directory(path)?;
        let identity = identity_from_handle(&handle)?;
        Ok(Self {
            path: path.to_path_buf(),
            identity,
            _handle: handle,
        })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn identity(&self) -> &DirectoryIdentity {
        &self.identity
    }
}

impl DirectoryIdentity {
    pub(super) fn durable_key(&self) -> String {
        format!("{:016x}:{:016x}", self.volume, self.file)
    }

    /// Parse the one canonical representation persisted as durable ownership
    /// evidence. Re-serializing after the numeric parse rejects shortened,
    /// upper-case, over-wide, or otherwise non-canonical strings rather than
    /// letting multiple database spellings describe one identity.
    pub(super) fn from_durable_key(value: &str) -> Option<Self> {
        let (volume, file) = value.split_once(':')?;
        let identity = Self {
            volume: u64::from_str_radix(volume, 16).ok()?,
            file: u64::from_str_radix(file, 16).ok()?,
        };
        (identity.durable_key() == value).then_some(identity)
    }
}

#[cfg(windows)]
fn open_directory(path: &Path) -> io::Result<File> {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(windows))]
fn open_directory(path: &Path) -> io::Result<File> {
    File::open(path)
}

#[cfg(windows)]
fn identity_from_handle(handle: &File) -> io::Result<DirectoryIdentity> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    let ok = unsafe { GetFileInformationByHandle(handle.as_raw_handle(), info.as_mut_ptr()) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    Ok(DirectoryIdentity {
        volume: u64::from(info.dwVolumeSerialNumber),
        file: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    })
}

#[cfg(unix)]
fn identity_from_handle(handle: &File) -> io::Result<DirectoryIdentity> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = handle.metadata()?;
    Ok(DirectoryIdentity {
        volume: metadata.dev(),
        file: metadata.ino(),
    })
}

#[cfg(not(any(unix, windows)))]
fn identity_from_handle(_handle: &File) -> io::Result<DirectoryIdentity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "directory identity is unsupported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::DirectoryIdentity;

    #[test]
    fn durable_key_parser_accepts_only_the_canonical_fixed_width_format() {
        let canonical = "000000000000000a:00000000000000ff";
        let identity = DirectoryIdentity::from_durable_key(canonical)
            .expect("the canonical lower-case fixed-width key must be accepted");
        assert_eq!(identity.durable_key(), canonical);

        for malformed in [
            "000000000000000A:00000000000000FF",
            "a:ff",
            "0000000000000000a:000000000000000ff",
        ] {
            assert!(
                DirectoryIdentity::from_durable_key(malformed).is_none(),
                "non-canonical durable identity {malformed:?} must be rejected",
            );
        }
    }
}
