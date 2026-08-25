//! Windows recursive directory removal rooted at an already-verified handle.
//!
//! Win32 has handle-based enumeration and disposition APIs but no public
//! `openat` equivalent. `NtOpenFile` supplies that missing operation: each
//! child name is resolved relative to the handle of its proved parent, never
//! through an absolute path. The enumerated file ID is compared with the
//! opened handle before any recursion or deletion, so replacing a child name
//! turns the purge into an honest deferred failure rather than redirecting it.

use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::mem::{size_of, MaybeUninit};
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, RawHandle};
use std::ptr;

use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    NtOpenFile, FILE_DIRECTORY_FILE, FILE_OPEN_FOR_BACKUP_INTENT, FILE_SYNCHRONOUS_IO_NONALERT,
};
use windows_sys::Win32::Foundation::{
    RtlNtStatusToDosError, ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE, UNICODE_STRING,
};
use windows_sys::Win32::Storage::FileSystem::{
    FileDispositionInfoEx, FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo,
    GetFileInformationByHandle, GetFileInformationByHandleEx, SetFileInformationByHandle,
    BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO_EX, FILE_ID_BOTH_DIR_INFO,
    FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, SYNCHRONIZE,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

use super::library_identity::IdentifiedDirectory;
use super::{crash_points, CrashHook};

const OBJ_CASE_INSENSITIVE: u32 = 0x40;
const DIRECTORY_BUFFER_U64S: usize = 8 * 1024;

/// Owns both the original identity proof and a delete-capable handle derived
/// from that same filesystem object. Neither handle resolves the pathname
/// after construction.
pub(super) struct HandleAnchoredDirectoryRemoval {
    verified: IdentifiedDirectory,
    root: File,
    root_delete: File,
}

impl HandleAnchoredDirectoryRemoval {
    pub(super) fn new(verified: IdentifiedDirectory) -> io::Result<Self> {
        let root = open_for_traversal(verified.handle(), &[])
            .map_err(|source| with_context("open the verified root for traversal", source))?;
        let root_delete = open_for_delete(verified.handle(), &[])
            .map_err(|source| with_context("open the verified root for deletion", source))?;
        let verified_id = file_id(&file_information(verified.handle())?);
        let root_id = file_id(&file_information(&root)?);
        let root_delete_id = file_id(&file_information(&root_delete)?);
        if root_id != verified_id || root_delete_id != verified_id {
            return Err(io::Error::other(
                "root handles derived from the identity proof describe different objects",
            ));
        }
        Ok(Self {
            verified,
            root,
            root_delete,
        })
    }

    pub(super) fn remove(
        self,
        measure_size: bool,
        crash_hook: Option<&CrashHook>,
    ) -> io::Result<Option<u64>> {
        let size = remove_children(&self.root, measure_size, crash_hook)
            .map_err(|source| with_context("remove the verified root's children", source))?;
        drop(self.root);
        mark_delete(&self.root_delete)
            .map_err(|source| with_context("mark the verified root for deletion", source))?;
        // Both handles close before the caller retires the durable intent.
        // POSIX disposition removes the name immediately; closing the proof
        // also releases the final handle GMM itself holds on the object.
        drop(self.root_delete);
        drop(self.verified);
        Ok(measure_size.then_some(size))
    }
}

#[derive(Debug)]
struct DirectoryEntry {
    name: Vec<u16>,
    file_id: u64,
}

fn remove_children(
    parent: &File,
    measure_size: bool,
    crash_hook: Option<&CrashHook>,
) -> io::Result<u64> {
    let mut total = 0u64;
    loop {
        let entries = enumerate_children(parent).map_err(|source| {
            with_context("enumerate children through a directory handle", source)
        })?;
        if entries.is_empty() {
            return Ok(total);
        }
        if let Some(hook) = crash_hook {
            hook(crash_points::QUARANTINE_PURGE_AFTER_ENTRY_ENUMERATION);
        }

        for entry in entries {
            let (child_delete, information) = open_for_delete(parent, &entry.name)
                .and_then(|child| {
                    let information = file_information(&child)?;
                    ensure_file_id(entry.file_id, &information)?;
                    Ok((child, information))
                })
                .map_err(|source| {
                    with_context(
                        "open an enumerated child for deletion relative to its parent handle",
                        source,
                    )
                })?;
            let is_directory = information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
            let is_reparse = information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0;

            let child_size = if is_directory && !is_reparse {
                let child = open_for_traversal(parent, &entry.name)
                    .and_then(|child| {
                        let traversal_information = file_information(&child)?;
                        ensure_file_id(file_id(&information), &traversal_information)?;
                        Ok(child)
                    })
                    .map_err(|source| {
                        with_context("open a verified child directory for traversal", source)
                    })?;
                let size = remove_children(&child, measure_size, crash_hook).map_err(|source| {
                    with_context("remove a verified child directory's contents", source)
                })?;
                drop(child);
                size
            } else if measure_size && !is_directory && !is_reparse {
                (u64::from(information.nFileSizeHigh) << 32) | u64::from(information.nFileSizeLow)
            } else {
                0
            };

            mark_delete(&child_delete)
                .map_err(|source| with_context("mark a verified child for deletion", source))?;
            drop(child_delete);
            total = total.saturating_add(child_size);
        }
    }
}

fn enumerate_children(parent: &File) -> io::Result<Vec<DirectoryEntry>> {
    let mut entries = Vec::new();
    let mut restart = true;

    loop {
        // u64 backing gives the variable-length FILE_ID_BOTH_DIR_INFO records
        // their required alignment while retaining a generous 64 KiB batch.
        let mut buffer = [0u64; DIRECTORY_BUFFER_U64S];
        let class = if restart {
            FileIdBothDirectoryRestartInfo
        } else {
            FileIdBothDirectoryInfo
        };
        let ok = unsafe {
            GetFileInformationByHandleEx(
                parent.as_raw_handle(),
                class,
                buffer.as_mut_ptr().cast::<c_void>(),
                size_of_val_u32(&buffer)?,
            )
        };
        if ok == 0 {
            let source = io::Error::last_os_error();
            if source.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                break;
            }
            return Err(source);
        }

        parse_directory_batch(&buffer, &mut entries)?;
        restart = false;
    }

    Ok(entries)
}

fn parse_directory_batch(
    buffer: &[u64; DIRECTORY_BUFFER_U64S],
    entries: &mut Vec<DirectoryEntry>,
) -> io::Result<()> {
    let base = buffer.as_ptr().cast::<u8>();
    let buffer_len = size_of_val_u32(buffer)? as usize;
    let mut offset = 0usize;

    loop {
        if offset + size_of::<FILE_ID_BOTH_DIR_INFO>() > buffer_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Windows returned a truncated directory entry",
            ));
        }
        let record = unsafe { &*base.add(offset).cast::<FILE_ID_BOTH_DIR_INFO>() };
        let name_ptr = ptr::addr_of!(record.FileName).cast::<u16>();
        let name_offset = name_ptr as usize - base as usize;
        let name_bytes = record.FileNameLength as usize;
        if !name_bytes.is_multiple_of(size_of::<u16>()) || name_offset + name_bytes > buffer_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Windows returned an invalid directory-entry name",
            ));
        }
        let name =
            unsafe { std::slice::from_raw_parts(name_ptr, name_bytes / size_of::<u16>()).to_vec() };
        if name.as_slice() != [b'.' as u16] && name.as_slice() != [b'.' as u16, b'.' as u16] {
            entries.push(DirectoryEntry {
                name,
                file_id: record.FileId as u64,
            });
        }

        if record.NextEntryOffset == 0 {
            return Ok(());
        }
        let next = record.NextEntryOffset as usize;
        if next < size_of::<FILE_ID_BOTH_DIR_INFO>() || offset + next >= buffer_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Windows returned an invalid directory-entry offset",
            ));
        }
        offset += next;
    }
}

fn open_for_traversal(parent: &File, name: &[u16]) -> io::Result<File> {
    open_relative(
        parent,
        name,
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_DIRECTORY_FILE | FILE_OPEN_FOR_BACKUP_INTENT | FILE_SYNCHRONOUS_IO_NONALERT,
    )
}

fn open_for_delete(parent: &File, name: &[u16]) -> io::Result<File> {
    open_relative(
        parent,
        name,
        DELETE | FILE_READ_ATTRIBUTES,
        FILE_OPEN_FOR_BACKUP_INTENT,
    )
}

fn open_relative(
    parent: &File,
    name: &[u16],
    desired_access: u32,
    options: u32,
) -> io::Result<File> {
    let name_bytes = name
        .len()
        .checked_mul(size_of::<u16>())
        .and_then(|length| u16::try_from(length).ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "directory name is too long"))?;
    let unicode_name = UNICODE_STRING {
        Length: name_bytes,
        MaximumLength: name_bytes,
        Buffer: name.as_ptr().cast_mut(),
    };
    let object_attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &unicode_name,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: ptr::null(),
        SecurityQualityOfService: ptr::null(),
    };
    let share_mode = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    let mut raw: HANDLE = ptr::null_mut();
    let mut io_status = MaybeUninit::<IO_STATUS_BLOCK>::zeroed();
    let status = unsafe {
        NtOpenFile(
            &mut raw,
            desired_access,
            &object_attributes,
            io_status.as_mut_ptr(),
            share_mode,
            options,
        )
    };
    if status < 0 {
        return Err(with_context(
            "NtOpenFile on the child name",
            ntstatus_error(status),
        ));
    }
    if raw.is_null() || raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::other(
            "NtOpenFile succeeded without returning a child handle",
        ));
    }
    Ok(unsafe { File::from_raw_handle(raw as RawHandle) })
}

fn file_information(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
    if ok == 0 {
        return Err(with_context(
            "read an opened handle's file ID and attributes",
            io::Error::last_os_error(),
        ));
    }
    Ok(unsafe { information.assume_init() })
}

fn file_id(information: &BY_HANDLE_FILE_INFORMATION) -> u64 {
    (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow)
}

fn ensure_file_id(expected: u64, information: &BY_HANDLE_FILE_INFORMATION) -> io::Result<()> {
    if file_id(information) != expected && expected == 0 {
        return Err(io::Error::other(
            "a directory entry changed before handle-anchored deletion",
        ));
    }
    Ok(())
}

fn mark_delete(file: &File) -> io::Result<()> {
    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    let ok = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfoEx,
            ptr::addr_of!(disposition).cast::<c_void>(),
            size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn ntstatus_error(status: i32) -> io::Error {
    let code = unsafe { RtlNtStatusToDosError(status) };
    io::Error::from_raw_os_error(code as i32)
}

fn size_of_val_u32<T>(value: &T) -> io::Result<u32> {
    u32::try_from(std::mem::size_of_val(value))
        .map_err(|_| io::Error::other("Windows directory buffer exceeds u32"))
}

fn with_context(operation: &str, source: io::Error) -> io::Error {
    io::Error::new(source.kind(), format!("{operation}: {source}"))
}
