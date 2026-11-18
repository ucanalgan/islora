//! The config folder is owned by the current user under %APPDATA%. A process
//! running as that user can rewrite it directly, so a race by that user between
//! the last check and rename is out of scope. We never apply weaker security than
//! the primary had at our last check, write into a file we did not create, or act
//! outside the held directory, and fail closed on any detected change. The window
//! between final revalidation and rename is accepted.

use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{
    ERROR_CALL_NOT_IMPLEMENTED, ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER,
    ERROR_NOT_SUPPORTED, HANDLE,
};
use windows::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
use windows::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FileIdInfo, GETFINALPATHNAMEBYHANDLE_FLAGS, GetFileInformationByHandle,
    GetFileInformationByHandleEx, GetFinalPathNameByHandleW, VOLUME_NAME_DOS, VOLUME_NAME_GUID,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileIdentity {
    volume: u64,
    id: [u8; 16],
}

#[derive(Debug)]
pub struct PrimaryChanged;
impl std::fmt::Display for PrimaryChanged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("configuration primary changed")
    }
}
impl std::error::Error for PrimaryChanged {}

#[derive(Debug)]
pub enum ConfigFsError {
    Locked,
    ReparsePoint,
    NotADirectory,
    IsDirectory,
    ExtraHardLinks,
    OutsideDirectory,
    InvalidName,
    TooLarge,
    Io(io::Error),
}
impl std::fmt::Display for ConfigFsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(
                f,
                "configuration filesystem error ({:?}, code {:?})",
                e.kind(),
                e.raw_os_error()
            ),
            other => write!(f, "unsafe configuration filesystem object: {other:?}"),
        }
    }
}
impl std::error::Error for ConfigFsError {}
impl From<io::Error> for ConfigFsError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<ConfigFsError> for io::Error {
    fn from(e: ConfigFsError) -> Self {
        match e {
            ConfigFsError::Io(e) => e,
            ConfigFsError::Locked => {
                io::Error::new(io::ErrorKind::WouldBlock, ConfigFsError::Locked)
            }
            other => io::Error::new(io::ErrorKind::InvalidData, other),
        }
    }
}
fn handle(file: &File) -> HANDLE {
    HANDLE(file.as_raw_handle())
}
fn information(file: &File) -> Result<BY_HANDLE_FILE_INFORMATION, ConfigFsError> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the file owns a valid handle and info points to writable storage.
    unsafe { GetFileInformationByHandle(handle(file), &mut info) }.map_err(io::Error::from)?;
    Ok(info)
}
fn identity(
    file: &File,
    basic: &BY_HANDLE_FILE_INFORMATION,
) -> Result<FileIdentity, ConfigFsError> {
    let mut info = FILE_ID_INFO::default();
    // SAFETY: the handle is live and the buffer has the documented size and type.
    let result = unsafe {
        GetFileInformationByHandleEx(
            handle(file),
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    let identity = match result {
        Ok(()) => FileIdentity {
            volume: info.VolumeSerialNumber,
            id: info.FileId.Identifier,
        },
        Err(e) => {
            let error = io::Error::from(e);
            if ![
                ERROR_INVALID_FUNCTION,
                ERROR_NOT_SUPPORTED,
                ERROR_INVALID_PARAMETER,
                ERROR_CALL_NOT_IMPLEMENTED,
            ]
            .iter()
            .any(|code| error.raw_os_error() == Some(code.0 as i32))
            {
                return Err(error.into());
            }
            let mut id = [0; 16];
            id[..8].copy_from_slice(
                &(((basic.nFileIndexHigh as u64) << 32) | basic.nFileIndexLow as u64).to_le_bytes(),
            );
            FileIdentity {
                volume: basic.dwVolumeSerialNumber as u64,
                id,
            }
        }
    };
    if identity.id == [0; 16] {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "filesystem returned a zero file ID",
        )
        .into());
    }
    Ok(identity)
}
fn open(path: &Path, directory: bool) -> Result<File, ConfigFsError> {
    Ok(OpenOptions::new()
        .read(true)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
        .custom_flags(
            FILE_FLAG_OPEN_REPARSE_POINT.0
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS.0
                } else {
                    0
                },
        )
        .open(path)?)
}

/// Holds a verified directory handle that denies delete sharing, pinning the
/// directory itself against rename or deletion while the handle is held. This
/// does not prevent renames of files inside the directory.
pub struct ConfigDir {
    _file: File,
    identity: FileIdentity,
}
impl ConfigDir {
    pub fn open(path: &Path) -> Result<Self, ConfigFsError> {
        let file = open(path, true)?;
        let info = information(&file)?;
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Err(ConfigFsError::ReparsePoint);
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0 {
            return Err(ConfigFsError::NotADirectory);
        }
        let identity = identity(&file, &info)?;
        Ok(Self {
            _file: file,
            identity,
        })
    }
    pub fn entries(&self) -> Result<Vec<std::ffi::OsString>, ConfigFsError> {
        use std::os::windows::ffi::OsStringExt;
        use windows::Win32::Foundation::ERROR_NO_MORE_FILES;
        use windows::Win32::Storage::FileSystem::{
            FILE_ID_BOTH_DIR_INFO, FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo,
        };
        let mut buffer = vec![0u64; 8192];
        let mut class = FileIdBothDirectoryRestartInfo;
        let mut names = Vec::new();
        loop {
            buffer.fill(0);
            // SAFETY: the live directory handle and aligned buffer are valid for the call.
            let result = unsafe {
                GetFileInformationByHandleEx(
                    handle(&self._file),
                    class,
                    buffer.as_mut_ptr().cast(),
                    (buffer.len() * 8) as u32,
                )
            };
            if let Err(error) = result {
                let error = io::Error::from(error);
                if error
                    .raw_os_error()
                    .is_some_and(|code| (code as u32 & 0xffff) == ERROR_NO_MORE_FILES.0)
                {
                    break;
                }
                return Err(error.into());
            }
            class = FileIdBothDirectoryInfo;
            let mut offset = 0usize;
            loop {
                let header = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
                if offset
                    .checked_add(header)
                    .is_none_or(|end| end > buffer.len() * 8)
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid directory record",
                    )
                    .into());
                }
                // SAFETY: the checked aligned offset contains the record header.
                let entry = unsafe {
                    &*(buffer
                        .as_ptr()
                        .cast::<u8>()
                        .add(offset)
                        .cast::<FILE_ID_BOTH_DIR_INFO>())
                };
                let start = offset + std::mem::offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
                let len = entry.FileNameLength as usize;
                let next = entry.NextEntryOffset as usize;
                let record_end = if next == 0 {
                    buffer.len() * 8
                } else {
                    offset
                        .checked_add(next)
                        .filter(|&end| {
                            end <= buffer.len() * 8 && next >= header && next.is_multiple_of(8)
                        })
                        .ok_or_else(|| {
                            io::Error::new(io::ErrorKind::InvalidData, "invalid directory offset")
                        })?
                };
                if !len.is_multiple_of(2)
                    || start.checked_add(len).is_none_or(|end| end > record_end)
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid directory entry",
                    )
                    .into());
                }
                // SAFETY: the checked name is an aligned UTF-16 slice within the returned buffer.
                let name = unsafe {
                    std::slice::from_raw_parts(
                        buffer.as_ptr().cast::<u8>().add(start).cast::<u16>(),
                        len / 2,
                    )
                };
                if name != [46] && name != [46, 46] {
                    names.push(std::ffi::OsString::from_wide(name));
                }
                if entry.NextEntryOffset == 0 {
                    break;
                }
                offset = offset
                    .checked_add(entry.NextEntryOffset as usize)
                    .filter(|&v| {
                        v + std::mem::size_of::<FILE_ID_BOTH_DIR_INFO>() <= buffer.len() * 8
                            && v % 8 == 0
                    })
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "invalid directory offset")
                    })?;
            }
        }
        Ok(names)
    }
    pub fn identity(&self) -> FileIdentity {
        self.identity
    }
}

#[cfg(test)]
pub(crate) static TEST_WRITER_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Serializes preparation before the directory exists as well as later writers.
pub struct WriterMutex(HANDLE, std::marker::PhantomData<*const ()>);
impl WriterMutex {
    pub fn acquire() -> Result<Self, ConfigFsError> {
        use windows::Win32::Foundation::{
            CloseHandle, HLOCAL, LocalFree, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
        };
        use windows::Win32::Security::Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetSecurityInfo, SDDL_REVISION_1, SE_KERNEL_OBJECT,
        };
        use windows::Win32::Security::{
            EqualSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
        };
        use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
        let user = CurrentUser::query()?;
        let sid = user.sid();
        let mut sid_string = windows::core::PWSTR::null();
        // SAFETY: sid remains valid in buffer and sid_string receives a LocalAlloc pointer.
        unsafe { ConvertSidToStringSidW(sid, &mut sid_string) }.map_err(io::Error::from)?;
        // SAFETY: the API returned a NUL-terminated string.
        let sid_text = unsafe { sid_string.to_string() }.map_err(io::Error::other);
        // SAFETY: the string was allocated by the system and is freed once.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(sid_string.0.cast())));
        }
        let sid_text = sid_text?;
        let name = windows::core::HSTRING::from(format!("Global\\Islora.ConfigWriter.{sid_text}"));
        let sddl = windows::core::HSTRING::from(format!(
            "O:{sid_text}D:P(A;;GA;;;{sid_text})(A;;GA;;;SY)"
        ));
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: valid SDDL and output pointer; descriptor is freed after creation.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &sddl,
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }
        .map_err(io::Error::from)?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        // SAFETY: attributes and name remain valid during creation.
        let created = unsafe { CreateMutexW(Some(&attributes), false, &name) };
        // SAFETY: descriptor was allocated by the conversion API.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        }
        // A foreign owner can squat the predictable name. Access denial or an
        // unexpected owner must fail closed instead of using an untrusted lock.
        let mutex = created.map_err(|_| ConfigFsError::Locked)?;
        let mut owner = PSID::default();
        let mut security = PSECURITY_DESCRIPTOR::default();
        // SAFETY: output pointers are valid; security owns the returned owner SID.
        let status = unsafe {
            GetSecurityInfo(
                mutex,
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION,
                Some(&mut owner),
                None,
                None,
                None,
                Some(&mut security),
            )
        };
        // SAFETY: both SIDs are valid until security is freed.
        let trusted = status == windows::Win32::Foundation::ERROR_SUCCESS
            && unsafe { EqualSid(owner, sid) }.is_ok();
        // SAFETY: security, when present, is allocated by GetSecurityInfo.
        if !security.0.is_null() {
            unsafe {
                let _ = LocalFree(Some(HLOCAL(security.0)));
            }
        }
        if !trusted {
            // SAFETY: the untrusted mutex handle is owned here.
            unsafe {
                let _ = CloseHandle(mutex);
            }
            return Err(ConfigFsError::Locked);
        }
        for attempt in 0..5 {
            // SAFETY: mutex remains open throughout the wait.
            let result = unsafe { WaitForSingleObject(mutex, 0) };
            // An abandoned mutex is acquired; the previous owner crashed.
            if result == WAIT_OBJECT_0 || result == WAIT_ABANDONED {
                return Ok(Self(mutex, std::marker::PhantomData));
            }
            if result != WAIT_TIMEOUT {
                // SAFETY: mutex is an owned handle and is closed once on failure.
                unsafe {
                    let _ = CloseHandle(mutex);
                }
                return Err(io::Error::last_os_error().into());
            }
            if attempt < 4 {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
        // SAFETY: mutex is an owned handle and is closed once on timeout.
        unsafe {
            let _ = CloseHandle(mutex);
        }
        Err(ConfigFsError::Locked)
    }
}
impl Drop for WriterMutex {
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::ReleaseMutex;
        // SAFETY: this guard owns the acquired mutex and its handle.
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

pub struct WriterLock {
    _file: File,
    _dir: ConfigDir,
    _mutex: WriterMutex,
}
impl WriterLock {
    pub fn directory(&self) -> &ConfigDir {
        &self._dir
    }
    pub fn acquire(path: &Path) -> Result<Self, ConfigFsError> {
        let mutex = WriterMutex::acquire()?;
        Self::with_mutex(path, mutex)
    }
    pub fn with_mutex(path: &Path, mutex: WriterMutex) -> Result<Self, ConfigFsError> {
        use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
        use windows::Wdk::Storage::FileSystem::{
            FILE_DELETE_ON_CLOSE, FILE_NON_DIRECTORY_FILE, FILE_OPEN_IF, FILE_OPEN_REPARSE_POINT,
            FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile,
        };
        use windows::Win32::Foundation::{
            OBJ_CASE_INSENSITIVE, RtlNtStatusToDosError, UNICODE_STRING,
        };
        use windows::Win32::Storage::FileSystem::{
            DELETE, FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_READ,
        };
        use windows::Win32::System::IO::IO_STATUS_BLOCK;
        use windows::core::PWSTR;
        let dir = ConfigDir::open(path)?;
        let name: Vec<u16> = "config.lock".encode_utf16().collect();
        let object_name = UNICODE_STRING {
            Length: (name.len() * 2) as u16,
            MaximumLength: (name.len() * 2) as u16,
            Buffer: PWSTR(name.as_ptr() as *mut _),
        };
        let attributes = OBJECT_ATTRIBUTES {
            Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
            RootDirectory: handle(&dir._file),
            ObjectName: &object_name,
            Attributes: OBJ_CASE_INSENSITIVE,
            ..Default::default()
        };
        let mut result = HANDLE::default();
        let mut status_block = IO_STATUS_BLOCK::default();
        // SAFETY: the directory handle and buffers remain valid for the synchronous call.
        let status = unsafe {
            NtCreateFile(
                &mut result,
                FILE_GENERIC_READ | DELETE,
                &attributes,
                &mut status_block,
                None,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                Default::default(),
                FILE_OPEN_IF,
                FILE_OPEN_REPARSE_POINT
                    | FILE_SYNCHRONOUS_IO_NONALERT
                    | FILE_NON_DIRECTORY_FILE
                    | FILE_DELETE_ON_CLOSE,
                None,
                0,
            )
        };
        if status.0 < 0 {
            // SAFETY: conversion does not dereference memory.
            let error =
                io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32);
            if matches!(error.raw_os_error(), Some(32 | 33 | 80 | 183)) {
                return Err(ConfigFsError::Locked);
            }
            return Err(error.into());
        }
        // SAFETY: a successful call returns an owned handle.
        let file = unsafe { File::from_raw_handle(result.0) };
        let info = information(&file)?;
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Err(ConfigFsError::ReparsePoint);
        }
        if info.nNumberOfLinks != 1 {
            return Err(ConfigFsError::ExtraHardLinks);
        }
        Ok(Self {
            _file: file,
            _dir: dir,
            _mutex: mutex,
        })
    }
}

#[cfg(test)]
thread_local! { pub(crate) static FAIL_SECURITY: std::cell::Cell<u8> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
fn fail_security(stage: u8) -> Result<(), ConfigFsError> {
    if FAIL_SECURITY.with(|value| {
        if value.get() == stage {
            value.set(0);
            true
        } else {
            false
        }
    }) {
        return Err(io::Error::other("injected security failure").into());
    }
    Ok(())
}

/// Owns the LocalAlloc descriptor and the pointers into it until the last use.
pub struct FileSecurity {
    descriptor: windows::Win32::Security::PSECURITY_DESCRIPTOR,
    owner: windows::Win32::Security::PSID,
    group: windows::Win32::Security::PSID,
    dacl: *mut windows::Win32::Security::ACL,
    protected: bool,
    set_owner: bool,
}
impl Drop for FileSecurity {
    fn drop(&mut self) {
        use windows::Win32::Foundation::{HLOCAL, LocalFree};
        // SAFETY: GetSecurityInfo allocated this descriptor once, and its pointers are no longer used.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.descriptor.0)));
        }
    }
}
impl FileSecurity {
    fn capture(file: &File) -> Result<Self, ConfigFsError> {
        #[cfg(test)]
        fail_security(1)?;
        use windows::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
        use windows::Win32::Security::{
            DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, GetSecurityDescriptorControl,
            OWNER_SECURITY_INFORMATION, SE_DACL_PROTECTED,
        };
        let mut result = Self {
            descriptor: Default::default(),
            owner: Default::default(),
            group: Default::default(),
            dacl: std::ptr::null_mut(),
            protected: false,
            set_owner: false,
        };
        // SAFETY: the file is live and all outputs point to writable storage.
        let status = unsafe {
            GetSecurityInfo(
                handle(file),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                Some(&mut result.owner),
                Some(&mut result.group),
                Some(&mut result.dacl),
                None,
                Some(&mut result.descriptor),
            )
        };
        if status.0 != 0 {
            return Err(io::Error::from_raw_os_error(status.0 as i32).into());
        }
        let mut control = 0;
        let mut revision = 0;
        // SAFETY: the descriptor was returned by GetSecurityInfo and outputs are valid.
        unsafe { GetSecurityDescriptorControl(result.descriptor, &mut control, &mut revision) }
            .map_err(io::Error::from)?;
        result.protected = control & SE_DACL_PROTECTED.0 != 0;
        // An unknown owner might be the current user: abort rather than omit required
        // owner preservation. Only a verified foreign owner may use the default owner.
        result.set_owner = CurrentUser::query()?.is(result.owner);
        Ok(result)
    }
    pub fn same_dacl(&self, other: &Self) -> bool {
        use windows::Win32::Security::{
            ACL_SIZE_INFORMATION, AclSizeInformation, GetAclInformation,
        };
        if self.protected != other.protected {
            return false;
        }
        if self.dacl.is_null() || other.dacl.is_null() {
            return self.dacl == other.dacl;
        }
        let mut a = ACL_SIZE_INFORMATION::default();
        let mut b = ACL_SIZE_INFORMATION::default();
        // SAFETY: the ACLs belong to live descriptors and the output structures are writable.
        let valid = unsafe {
            GetAclInformation(
                self.dacl,
                (&mut a as *mut ACL_SIZE_INFORMATION).cast(),
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
            .is_ok()
                && GetAclInformation(
                    other.dacl,
                    (&mut b as *mut ACL_SIZE_INFORMATION).cast(),
                    std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                    AclSizeInformation,
                )
                .is_ok()
        };
        if !valid || a.AclBytesInUse != b.AclBytesInUse {
            return false;
        }
        // SAFETY: GetAclInformation reported these bytes inside both live ACLs.
        unsafe {
            std::slice::from_raw_parts(self.dacl.cast::<u8>(), a.AclBytesInUse as usize)
                == std::slice::from_raw_parts(other.dacl.cast::<u8>(), b.AclBytesInUse as usize)
        }
    }
    #[cfg(test)]
    pub fn restore_dacl(&self, file: &File) {
        use windows::Win32::Security::Authorization::{SE_FILE_OBJECT, SetSecurityInfo};
        use windows::Win32::Security::{
            DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
            UNPROTECTED_DACL_SECURITY_INFORMATION,
        };
        // SAFETY: the handle and captured ACL remain live throughout the call.
        unsafe {
            let _ = SetSecurityInfo(
                handle(file),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION
                    | if self.protected {
                        PROTECTED_DACL_SECURITY_INFORMATION
                    } else {
                        UNPROTECTED_DACL_SECURITY_INFORMATION
                    },
                None,
                None,
                Some(self.dacl),
                None,
            );
        }
    }
    #[cfg(test)]
    pub fn toggle_dacl_protection(&self, file: &File) {
        use windows::Win32::Security::Authorization::{SE_FILE_OBJECT, SetSecurityInfo};
        use windows::Win32::Security::{
            DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
            UNPROTECTED_DACL_SECURITY_INFORMATION,
        };
        // SAFETY: the file handle and descriptor-owned ACL remain live for the call.
        let status = unsafe {
            SetSecurityInfo(
                handle(file),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION
                    | if self.protected {
                        UNPROTECTED_DACL_SECURITY_INFORMATION
                    } else {
                        PROTECTED_DACL_SECURITY_INFORMATION
                    },
                None,
                None,
                Some(self.dacl),
                None,
            )
        };
        assert_eq!(status.0, 0);
    }
    #[cfg(test)]
    pub fn owner_sid_string(&self) -> String {
        use windows::Win32::Foundation::{HLOCAL, LocalFree};
        use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
        let mut value = windows::core::PWSTR::null();
        // SAFETY: owner is a live SID and value receives a system allocation.
        unsafe { ConvertSidToStringSidW(self.owner, &mut value) }.unwrap();
        // SAFETY: the returned string is NUL-terminated and remains live.
        let result = unsafe { value.to_string() }.unwrap();
        // SAFETY: the allocation is released exactly once after copying the string.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(value.0.cast())));
        }
        result
    }
    pub fn same_owner(&self, other: &Self) -> bool {
        // SAFETY: both owner SIDs belong to live security descriptors.
        unsafe { windows::Win32::Security::EqualSid(self.owner, other.owner) }.is_ok()
    }
    fn apply(&self, file: &File) -> Result<(), ConfigFsError> {
        #[cfg(test)]
        fail_security(2)?;
        use windows::Win32::Security::Authorization::{SE_FILE_OBJECT, SetSecurityInfo};
        use windows::Win32::Security::{
            DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
            PROTECTED_DACL_SECURITY_INFORMATION, UNPROTECTED_DACL_SECURITY_INFORMATION,
        };
        let flags = DACL_SECURITY_INFORMATION
            | if self.protected {
                PROTECTED_DACL_SECURITY_INFORMATION
            } else {
                UNPROTECTED_DACL_SECURITY_INFORMATION
            };
        // SAFETY: the verified file handle and descriptor-owned ACL remain valid.
        let status = unsafe {
            SetSecurityInfo(
                handle(file),
                SE_FILE_OBJECT,
                flags,
                None,
                None,
                Some(self.dacl),
                None,
            )
        };
        if status.0 != 0 {
            return Err(io::Error::from_raw_os_error(status.0 as i32).into());
        }
        // Group preservation needs WRITE_OWNER or privilege. Try it independently;
        // if unavailable, retain the DACL and protected bit and do not fail the save.
        // SAFETY: the group SID belongs to the live descriptor and the handle is valid.
        unsafe {
            let _ = SetSecurityInfo(
                handle(file),
                SE_FILE_OBJECT,
                GROUP_SECURITY_INFORMATION,
                None,
                Some(self.group),
                None,
                None,
            );
        }
        // A foreign owner is left at its default; the current user's owner must be preserved.
        // The creation descriptor may already have assigned the required owner.
        // Avoid requiring WRITE_OWNER when the owner is already correct.
        let owner_matches = if self.set_owner {
            let actual = Self::capture(file)?;
            // SAFETY: both SIDs belong to live descriptors.
            unsafe { windows::Win32::Security::EqualSid(self.owner, actual.owner) }.is_ok()
        } else {
            true
        };
        if !owner_matches {
            // SAFETY: the owner SID belongs to the live descriptor and the handle is valid.
            let status = unsafe {
                SetSecurityInfo(
                    handle(file),
                    SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION,
                    Some(self.owner),
                    None,
                    None,
                    None,
                )
            };
            if status.0 != 0 {
                return Err(io::Error::from_raw_os_error(status.0 as i32).into());
            }
        }
        Ok(())
    }
}
struct CurrentUser(Vec<usize>);
impl CurrentUser {
    fn query() -> Result<Self, ConfigFsError> {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TokenUser};
        use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        struct Token(HANDLE);
        impl Drop for Token {
            fn drop(&mut self) {
                // SAFETY: this guard owns the token handle.
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
        let mut handle = HANDLE::default();
        // SAFETY: the process pseudo-handle and output pointer are valid.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) }
            .map_err(io::Error::from)?;
        let token = Token(handle);
        let mut size = 0;
        // SAFETY: the first call queries the required length.
        unsafe {
            let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut size);
        }
        let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        // SAFETY: the aligned buffer has the requested capacity.
        unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                size,
                &mut size,
            )
        }
        .map_err(io::Error::from)?;
        Ok(Self(buffer))
    }
    fn sid(&self) -> windows::Win32::Security::PSID {
        // SAFETY: successful token query initialized TOKEN_USER and its SID in the buffer.
        unsafe {
            (*self
                .0
                .as_ptr()
                .cast::<windows::Win32::Security::TOKEN_USER>())
            .User
            .Sid
        }
    }
    fn is(&self, owner: windows::Win32::Security::PSID) -> bool {
        // SAFETY: both SIDs remain live for the comparison.
        unsafe { windows::Win32::Security::EqualSid(owner, self.sid()) }.is_ok()
    }
}

#[allow(clippy::too_many_arguments)]
fn open_relative(
    dir: &ConfigDir,
    name: &[u16],
    write: bool,
    create: bool,
    security: Option<&FileSecurity>,
    owner_access: bool,
    assign_identity: bool,
    share_delete: bool,
    write_through: bool,
) -> Result<File, ConfigFsError> {
    use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows::Wdk::Storage::FileSystem::{
        FILE_CREATE, FILE_OPEN, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
        FILE_WRITE_THROUGH, NtCreateFile,
    };
    use windows::Win32::Foundation::{OBJ_CASE_INSENSITIVE, RtlNtStatusToDosError, UNICODE_STRING};
    use windows::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_READ, FILE_GENERIC_WRITE, WRITE_DAC,
        WRITE_OWNER,
    };
    use windows::Win32::System::IO::IO_STATUS_BLOCK;
    use windows::core::PWSTR;
    let length = u16::try_from(
        name.len()
            .checked_mul(2)
            .ok_or(ConfigFsError::InvalidName)?,
    )
    .map_err(|_| ConfigFsError::InvalidName)?;
    let object_name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: PWSTR(name.as_ptr() as *mut _),
    };
    // An absolute descriptor serves as SECURITY_ATTRIBUTES for the relative NtCreateFile.
    use windows::Win32::Security::{
        InitializeSecurityDescriptor, PSECURITY_DESCRIPTOR, SE_DACL_PROTECTED, SECURITY_DESCRIPTOR,
        SetSecurityDescriptorControl, SetSecurityDescriptorDacl, SetSecurityDescriptorGroup,
        SetSecurityDescriptorOwner,
    };
    let mut descriptor = SECURITY_DESCRIPTOR::default();
    let descriptor_ptr = PSECURITY_DESCRIPTOR((&mut descriptor as *mut SECURITY_DESCRIPTOR).cast());
    if let Some(security) = security {
        // SAFETY: descriptor is writable and its SID and ACL pointers outlive NtCreateFile.
        unsafe {
            InitializeSecurityDescriptor(descriptor_ptr, 1).map_err(io::Error::from)?;
            SetSecurityDescriptorDacl(descriptor_ptr, true, Some(security.dacl), false)
                .map_err(io::Error::from)?;
            if assign_identity {
                SetSecurityDescriptorGroup(descriptor_ptr, Some(security.group), false)
                    .map_err(io::Error::from)?;
            }
            if security.set_owner {
                SetSecurityDescriptorOwner(descriptor_ptr, Some(security.owner), false)
                    .map_err(io::Error::from)?;
            }
            SetSecurityDescriptorControl(
                descriptor_ptr,
                SE_DACL_PROTECTED,
                if security.protected {
                    SE_DACL_PROTECTED
                } else {
                    Default::default()
                },
            )
            .map_err(io::Error::from)?;
        }
    }
    let attributes = OBJECT_ATTRIBUTES {
        SecurityDescriptor: if security.is_some() {
            &descriptor
        } else {
            std::ptr::null()
        },
        Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: handle(&dir._file),
        ObjectName: &object_name,
        Attributes: OBJ_CASE_INSENSITIVE,
        ..Default::default()
    };
    let mut result = HANDLE::default();
    let mut status_block = IO_STATUS_BLOCK::default();
    // SAFETY: the directory handle and all input and output buffers remain valid for this synchronous call.
    let status = unsafe {
        NtCreateFile(
            &mut result,
            if write {
                FILE_GENERIC_READ
                    | FILE_GENERIC_WRITE
                    | DELETE
                    | if security.is_some() {
                        WRITE_DAC
                            | if owner_access {
                                WRITE_OWNER
                            } else {
                                Default::default()
                            }
                    } else {
                        Default::default()
                    }
            } else {
                FILE_GENERIC_READ
            },
            &attributes,
            &mut status_block,
            None,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            FILE_SHARE_READ
                | FILE_SHARE_WRITE
                | if share_delete {
                    windows::Win32::Storage::FileSystem::FILE_SHARE_DELETE
                } else {
                    Default::default()
                },
            if create { FILE_CREATE } else { FILE_OPEN },
            FILE_OPEN_REPARSE_POINT
                | FILE_SYNCHRONOUS_IO_NONALERT
                | if write_through {
                    FILE_WRITE_THROUGH
                } else {
                    Default::default()
                },
            None,
            0,
        )
    };
    // Retry only permission and owner assignment failures, never collisions or I/O errors.
    let retryable = matches!(status.0 as u32, 0xC0000022 | 0xC0000061 | 0xC000005A);
    if retryable && security.is_some() && owner_access {
        // WRITE_OWNER may be unavailable; keep the DACL and retry without it.
        return open_relative(
            dir,
            name,
            write,
            create,
            security,
            false,
            assign_identity,
            share_delete,
            write_through,
        );
    }
    if retryable && security.is_some() && create && assign_identity {
        // The source group may require unavailable privileges at creation.
        // Keep the DACL and any required owner while Windows chooses the group.
        return open_relative(
            dir,
            name,
            write,
            create,
            security,
            false,
            false,
            share_delete,
            write_through,
        );
    }
    if status.0 < 0 {
        // SAFETY: converting a returned NTSTATUS does not dereference memory.
        return Err(
            io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32).into(),
        );
    }
    // SAFETY: successful NtCreateFile returns a new owned file handle.
    Ok(unsafe { File::from_raw_handle(result.0) })
}

fn validate_name(name: &[u16]) -> Result<(), ConfigFsError> {
    let stem = name.split(|&c| c == b'.' as u16).next().unwrap_or_default();
    let stem_ascii = String::from_utf16_lossy(stem).to_ascii_uppercase();
    let reserved = matches!(
        stem_ascii.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || (stem_ascii.len() == 4
        && (stem_ascii.starts_with("COM") || stem_ascii.starts_with("LPT"))
        && matches!(stem_ascii.as_bytes()[3], b'1'..=b'9'));
    if name.is_empty()
        || name == [46]
        || name == [46, 46]
        || name.last().is_some_and(|c| matches!(*c, 32 | 46))
        || name
            .iter()
            .any(|c| matches!(*c, 0..=31 | 34 | 42 | 47 | 58 | 60 | 62 | 63 | 92 | 124))
        || reserved
    {
        return Err(ConfigFsError::InvalidName);
    }
    Ok(())
}

#[cfg(test)]
thread_local! { pub(crate) static FORCE_RENAME_FALLBACK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

/// Retries rename variants when a variant is unsupported.
fn rename_fallback(mut attempt: impl FnMut(usize) -> (i32, u32)) -> Result<(), u32> {
    for stage in 0..3 {
        let (status, code) = attempt(stage);
        if status >= 0 {
            return Ok(());
        }
        if stage == 2 || !matches!(code, 87 | 50) {
            return Err(code);
        }
    }
    unreachable!()
}

/// Holds a verified file and its directory. Ordinary handles deny FILE_SHARE_DELETE, so an
/// editor's rename-over may fail during a short read, and opening fails if DELETE
/// access is already held. Guarantees last only while handles are held; a hard
/// link can still be added after verification.
pub struct VerifiedFile {
    file: File,
    _directory: File,
    _identity: FileIdentity,
}
impl VerifiedFile {
    pub fn open(dir: &ConfigDir, name: &std::ffi::OsStr) -> Result<Self, ConfigFsError> {
        Self::open_mode(dir, name, false, false, None, false)
    }
    pub fn open_source(dir: &ConfigDir, name: &std::ffi::OsStr) -> Result<Self, ConfigFsError> {
        Self::open_mode(dir, name, false, false, None, true)
    }
    fn open_mode(
        dir: &ConfigDir,
        name: &std::ffi::OsStr,
        write: bool,
        create: bool,
        security: Option<&FileSecurity>,
        share_delete: bool,
    ) -> Result<Self, ConfigFsError> {
        use std::os::windows::ffi::OsStrExt;
        let name_wide: Vec<u16> = name.encode_wide().collect();
        validate_name(&name_wide)?;
        let file = open_relative(
            dir,
            &name_wide,
            write,
            create,
            security,
            true,
            true,
            share_delete,
            false,
        )?;
        let info = information(&file)?;
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Err(ConfigFsError::ReparsePoint);
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0 {
            return Err(ConfigFsError::IsDirectory);
        }
        if info.nNumberOfLinks != 1 {
            return Err(ConfigFsError::ExtraHardLinks);
        }
        // Resolve the file's final
        // handle path, reopen its parent without following its final reparse point,
        // and compare identities. The directory cannot be renamed during this check.
        fn final_path(file: &File, flags: GETFINALPATHNAMEBYHANDLE_FLAGS) -> io::Result<PathBuf> {
            use std::os::windows::ffi::OsStringExt;
            let mut buffer = vec![0u16; 512];
            loop {
                // SAFETY: buffer is writable and the file handle remains live.
                let len =
                    unsafe { GetFinalPathNameByHandleW(handle(file), &mut buffer, flags) } as usize;
                if len == 0 {
                    return Err(io::Error::last_os_error());
                }
                if len < buffer.len() {
                    buffer.truncate(len);
                    return Ok(PathBuf::from(std::ffi::OsString::from_wide(&buffer)));
                }
                buffer.resize(len + 1, 0);
            }
        }
        let final_path =
            final_path(&file, VOLUME_NAME_DOS).or_else(|_| final_path(&file, VOLUME_NAME_GUID))?;
        let final_name: Vec<u16> = final_path
            .file_name()
            .ok_or(ConfigFsError::InvalidName)?
            .encode_wide()
            .collect();
        // SAFETY: both slices are valid UTF-16 buffers for the duration of the call.
        if unsafe { CompareStringOrdinal(&name_wide, &final_name, true) } != CSTR_EQUAL {
            return Err(ConfigFsError::InvalidName);
        }
        let parent = final_path.parent().ok_or(ConfigFsError::OutsideDirectory)?;
        let actual = ConfigDir::open(parent)?;
        if actual.identity() != dir.identity() {
            return Err(ConfigFsError::OutsideDirectory);
        }
        let identity = identity(&file, &info)?;
        Ok(Self {
            file,
            _directory: dir._file.try_clone()?,
            _identity: identity,
        })
    }

    pub fn try_clone(&self) -> Result<Self, ConfigFsError> {
        Ok(Self {
            file: self.file.try_clone()?,
            _directory: self._directory.try_clone()?,
            _identity: self._identity,
        })
    }
    pub fn delete_verified(dir: &ConfigDir, name: &std::ffi::OsStr) -> Result<(), ConfigFsError> {
        use windows::Win32::Storage::FileSystem::{
            FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
        };
        let file = Self::open_mode(dir, name, true, false, None, false)?;
        let info = FILE_DISPOSITION_INFO { DeleteFile: true };
        // SAFETY: the verified file handle and disposition structure remain live.
        unsafe {
            SetFileInformationByHandle(
                handle(&file.file),
                FileDispositionInfo,
                (&info as *const FILE_DISPOSITION_INFO).cast(),
                std::mem::size_of_val(&info) as u32,
            )
        }
        .map_err(io::Error::from)?;
        Ok(())
    }
    pub fn security(&self) -> Result<FileSecurity, ConfigFsError> {
        FileSecurity::capture(&self.file)
    }
    #[cfg(test)]
    pub fn rename(
        dir: &ConfigDir,
        source: &std::ffi::OsStr,
        target: &std::ffi::OsStr,
        security: Option<&FileSecurity>,
    ) -> Result<(), ConfigFsError> {
        let file = Self::open_mode(dir, source, true, false, security, false)?;
        rename_handle(dir, &file.file, target, true)
    }
    pub fn revalidate(
        &self,
        dir: &ConfigDir,
        name: &std::ffi::OsStr,
        expected: &FileSecurity,
    ) -> Result<(), ConfigFsError> {
        let current = Self::open_source(dir, name)?;
        let held_identity = identity(&self.file, &information(&self.file)?)?;
        let current_security = self.security()?;
        if held_identity != self._identity
            || current._identity != self._identity
            || !expected.same_owner(&current_security)
            || !expected.same_dacl(&current_security)
        {
            return Err(io::Error::other("configuration source changed").into());
        }
        Ok(())
    }
    #[cfg(test)]
    pub fn identity(&self) -> FileIdentity {
        self._identity
    }
    pub fn read_bounded(&mut self, limit: usize) -> Result<Vec<u8>, ConfigFsError> {
        let mut bytes = Vec::new();
        (&mut self.file)
            .take((limit as u64).saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() > limit {
            return Err(ConfigFsError::TooLarge);
        }
        Ok(bytes)
    }
    pub fn rewind(&mut self) -> Result<(), ConfigFsError> {
        use std::io::Seek;
        self.file.rewind()?;
        Ok(())
    }
}

/// A draft remains owned by its creation handle until it is renamed or deleted.
pub struct Draft {
    file: File,
    renamed: bool,
}
impl Drop for Draft {
    fn drop(&mut self) {
        if !self.renamed {
            use windows::Win32::Storage::FileSystem::{
                FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
            };
            let info = FILE_DISPOSITION_INFO { DeleteFile: true };
            // SAFETY: the handle is owned and live; info is valid for the duration of the call.
            unsafe {
                let _ = SetFileInformationByHandle(
                    handle(&self.file),
                    FileDispositionInfo,
                    (&info as *const FILE_DISPOSITION_INFO).cast(),
                    std::mem::size_of_val(&info) as u32,
                );
            }
        }
    }
}
impl Draft {
    pub fn create(
        dir: &ConfigDir,
        target: &std::ffi::OsStr,
        security: Option<&FileSecurity>,
    ) -> Result<Self, ConfigFsError> {
        use std::os::windows::ffi::OsStrExt;
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let target = target.to_string_lossy();
        for attempt in 0..16 {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let name = format!(
                "{target}.{}-{}-{nanos}.tmp",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let wide: Vec<u16> = std::ffi::OsStr::new(&name).encode_wide().collect();
            validate_name(&wide)?;
            match open_relative(dir, &wide, true, true, security, true, true, false, true) {
                Ok(file) => {
                    let draft = Self {
                        file,
                        renamed: false,
                    };
                    if let Some(expected) = security
                        && !expected.same_dacl(&FileSecurity::capture(&draft.file)?)
                    {
                        return Err(io::Error::other("draft DACL differs at creation").into());
                    }
                    return Ok(draft);
                }
                Err(ConfigFsError::Io(e))
                    if e.kind() == io::ErrorKind::AlreadyExists && attempt < 15 =>
                {
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), ConfigFsError> {
        use std::io::Write;
        self.file.write_all(bytes)?;
        self.file.sync_all()?;
        Ok(())
    }
    pub fn read_back(&mut self) -> Result<Vec<u8>, ConfigFsError> {
        use std::io::{Seek, SeekFrom};
        self.file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        self.file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }
    pub fn rename(
        mut self,
        dir: &ConfigDir,
        target: &std::ffi::OsStr,
        security: Option<&FileSecurity>,
        source: Option<(VerifiedFile, &std::ffi::OsStr)>,
        replace: bool,
        expected: Option<&[u8]>,
    ) -> Result<(), ConfigFsError> {
        if let Some(expected) = security {
            expected.apply(&self.file)?;
            if !expected.same_dacl(&FileSecurity::capture(&self.file)?) {
                return Err(io::Error::other("replacement DACL changed during application").into());
            }
        }
        if let Some((mut file, name)) = source {
            match file.revalidate(dir, name, security.expect("source security")) {
                Err(ConfigFsError::Io(error))
                    if expected.is_some() && error.kind() == io::ErrorKind::NotFound =>
                {
                    return Err(io::Error::other(PrimaryChanged).into());
                }
                result => result?,
            }
            if let Some(expected) = expected {
                file.rewind()?;
                match file.read_bounded(island_core::migration::MAX_CONFIG_BYTES) {
                    Ok(actual) if actual == expected => {}
                    Ok(_) | Err(ConfigFsError::TooLarge) => {
                        return Err(io::Error::other(PrimaryChanged).into());
                    }
                    Err(error) => return Err(error),
                }
            }
        } else if expected.is_some() {
            return Err(io::Error::other(PrimaryChanged).into());
        }
        rename_handle(dir, &self.file, target, replace)?;
        self.renamed = true;
        Ok(())
    }
}
fn rename_handle(
    dir: &ConfigDir,
    file: &File,
    target: &std::ffi::OsStr,
    replace: bool,
) -> Result<(), ConfigFsError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Wdk::Storage::FileSystem::{
        FileRenameInformation, FileRenameInformationEx, NtSetInformationFile,
    };
    use windows::Win32::Foundation::RtlNtStatusToDosError;
    use windows::Win32::Storage::FileSystem::FILE_RENAME_INFO;
    use windows::Win32::System::IO::IO_STATUS_BLOCK;
    let wide: Vec<u16> = target.encode_wide().collect();
    validate_name(&wide)?;
    match VerifiedFile::open(dir, target) {
        Ok(target_file) => drop(target_file),
        Err(ConfigFsError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let size = std::mem::size_of::<FILE_RENAME_INFO>()
        .checked_add(wide.len() * 2)
        .ok_or(ConfigFsError::InvalidName)?;
    let mut buffer = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    const REPLACE_IF_EXISTS: u32 = 1;
    const POSIX_SEMANTICS: u32 = 2;
    // SAFETY: the aligned allocation covers the header and name at its field offset; both handles remain live.
    unsafe {
        (*info).Anonymous.Flags = REPLACE_IF_EXISTS | POSIX_SEMANTICS;
        (*info).RootDirectory = handle(&dir._file);
        (*info).FileNameLength =
            u32::try_from(wide.len() * 2).map_err(|_| ConfigFsError::InvalidName)?;
        let name_ptr = buffer
            .as_mut_ptr()
            .cast::<u8>()
            .add(std::mem::offset_of!(FILE_RENAME_INFO, FileName))
            .cast::<u16>();
        std::ptr::copy_nonoverlapping(wide.as_ptr(), name_ptr, wide.len());
        rename_fallback(|stage| {
            #[cfg(test)]
            if stage == 0 && FORCE_RENAME_FALLBACK.with(|v| v.replace(false)) {
                return (-1, 50);
            }
            (*info).Anonymous.Flags = if !replace {
                0
            } else if stage == 0 {
                REPLACE_IF_EXISTS | POSIX_SEMANTICS
            } else {
                REPLACE_IF_EXISTS
            };
            let mut status_block = IO_STATUS_BLOCK::default();
            let class = if stage < 2 {
                FileRenameInformationEx
            } else {
                FileRenameInformation
            };
            let status = NtSetInformationFile(
                handle(file),
                &mut status_block,
                info.cast(),
                size as u32,
                class,
            );
            (status.0, RtlNtStatusToDosError(status))
        })
        .map_err(|code| ConfigFsError::from(io::Error::from_raw_os_error(code as i32)))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::{symlink_dir, symlink_file};
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "islora-config-fs-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn open_file(dir: &ConfigDir, name: &str) -> Result<VerifiedFile, ConfigFsError> {
        VerifiedFile::open(dir, name.as_ref())
    }
    #[test]
    fn draft_handle_has_write_through() {
        use windows::Wdk::Storage::FileSystem::{
            FILE_MODE_INFORMATION, FILE_WRITE_THROUGH, FileModeInformation, NtQueryInformationFile,
        };
        use windows::Win32::System::IO::IO_STATUS_BLOCK;

        let temp = Temp::new();
        let dir = ConfigDir::open(&temp.0).unwrap();
        let draft = Draft::create(&dir, "config.toml".as_ref(), None).unwrap();
        let mut info = FILE_MODE_INFORMATION::default();
        let mut status_block = IO_STATUS_BLOCK::default();
        // SAFETY: the draft handle is live and both output buffers are valid for the call.
        let status = unsafe {
            NtQueryInformationFile(
                handle(&draft.file),
                &mut status_block,
                (&mut info as *mut FILE_MODE_INFORMATION).cast(),
                std::mem::size_of::<FILE_MODE_INFORMATION>() as u32,
                FileModeInformation,
            )
        };
        assert!(status.0 >= 0, "NtQueryInformationFile failed: {status:?}");
        assert_ne!(info.Mode & FILE_WRITE_THROUGH.0, 0);
    }
    fn symlink_result(result: io::Result<()>) -> bool {
        match result {
            Ok(()) => true,
            Err(e) if e.raw_os_error() == Some(1314) => {
                eprintln!("Skipping symlink test: Windows privilege 1314");
                false
            }
            Err(e) => panic!("symlink creation failed: {e}"),
        }
    }
    #[test]
    fn writer_lock_is_bounded_exclusive_and_deleted() {
        let _serial = TEST_WRITER_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let temp = Temp::new();
        let lock = WriterLock::acquire(&temp.0).unwrap();
        assert!(std::fs::File::open(temp.0.join("config.lock")).is_err());
        let path = temp.0.clone();
        let elapsed = std::thread::spawn(move || {
            let start = std::time::Instant::now();
            assert!(matches!(
                WriterLock::acquire(&path),
                Err(ConfigFsError::Locked)
            ));
            start.elapsed()
        })
        .join()
        .unwrap();
        assert!(elapsed < std::time::Duration::from_secs(1));
        drop(lock);
        assert!(!temp.0.join("config.lock").exists());
        drop(WriterLock::acquire(&temp.0).unwrap());
        assert!(!temp.0.join("config.lock").exists());
    }
    #[test]
    fn rename_fallback_tries_only_unsupported_variants() {
        let mut stages = Vec::new();
        assert_eq!(
            rename_fallback(|stage| {
                stages.push(stage);
                if stage < 2 { (-1, 87) } else { (0, 0) }
            }),
            Ok(())
        );
        assert_eq!(stages, [0, 1, 2]);
        stages.clear();
        assert_eq!(
            rename_fallback(|stage| {
                stages.push(stage);
                (-1, if stage == 0 { 50 } else { 5 })
            }),
            Err(5)
        );
        assert_eq!(stages, [0, 1]);
    }
    #[test]
    fn rename_rejects_invalid_targets_without_touching_source() {
        let temp = Temp::new();
        std::fs::write(temp.0.join("source"), b"original").unwrap();
        let dir = ConfigDir::open(&temp.0).unwrap();
        for target in [r"a\b", "a/b", "..", "x:y", "CON", ""] {
            assert!(
                matches!(
                    VerifiedFile::rename(&dir, "source".as_ref(), target.as_ref(), None),
                    Err(ConfigFsError::InvalidName)
                ),
                "{target:?}"
            );
            assert_eq!(std::fs::read(temp.0.join("source")).unwrap(), b"original");
        }
    }
    #[test]
    fn regular_file_identity_and_read() {
        let temp = Temp::new();
        std::fs::write(temp.0.join("config"), b"hello").unwrap();
        let dir = ConfigDir::open(&temp.0).unwrap();
        let mut a = open_file(&dir, "config").unwrap();
        let b = open_file(&dir, "config").unwrap();
        assert_eq!(a.identity(), b.identity());
        assert_eq!(a.read_bounded(5).unwrap(), b"hello");
    }
    #[test]
    fn filename_case_difference_accepted() {
        let temp = Temp::new();
        std::fs::write(temp.0.join("config"), b"x").unwrap();
        let dir = ConfigDir::open(&temp.0).unwrap();
        assert!(open_file(&dir, "CONFIG").is_ok());
    }
    #[test]
    fn short_filename_rejected() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use windows::Win32::Storage::FileSystem::GetShortPathNameW;
        use windows::core::PCWSTR;

        let temp = Temp::new();
        let long_name = "configuration-long-name.toml";
        let path = temp.0.join(long_name);
        std::fs::write(&path, b"x").unwrap();
        let path_wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut buffer = vec![0u16; 32768];
        // SAFETY: path_wide is NUL-terminated and buffer is writable for the call.
        let len =
            unsafe { GetShortPathNameW(PCWSTR(path_wide.as_ptr()), Some(&mut buffer)) } as usize;
        if len == 0 || len >= buffer.len() {
            eprintln!(
                "Skipping short-name test: GetShortPathNameW failed or the path is too long (8.3 names may be disabled)"
            );
            return;
        }
        let short_path = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..len]));
        let short_name = short_path.file_name().unwrap();
        if short_name.to_string_lossy().eq_ignore_ascii_case(long_name) {
            eprintln!("Skipping short-name test: no 8.3 alias is available on this volume");
            return;
        }
        let dir = ConfigDir::open(&temp.0).unwrap();
        assert!(matches!(
            VerifiedFile::open(&dir, short_name),
            Err(ConfigFsError::InvalidName)
        ));
    }
    #[test]
    fn bounded_read_too_large() {
        let temp = Temp::new();
        std::fs::write(temp.0.join("config"), b"hello").unwrap();
        let dir = ConfigDir::open(&temp.0).unwrap();
        assert!(matches!(
            open_file(&dir, "config").unwrap().read_bounded(4),
            Err(ConfigFsError::TooLarge)
        ));
    }
    #[test]
    fn file_symlink_rejected() {
        let temp = Temp::new();
        std::fs::write(temp.0.join("target"), b"x").unwrap();
        if !symlink_result(symlink_file(temp.0.join("target"), temp.0.join("link"))) {
            return;
        }
        let dir = ConfigDir::open(&temp.0).unwrap();
        assert!(matches!(
            open_file(&dir, "link"),
            Err(ConfigFsError::ReparsePoint)
        ));
    }
    #[test]
    fn directory_symlink_rejected() {
        let temp = Temp::new();
        std::fs::create_dir(temp.0.join("target")).unwrap();
        if !symlink_result(symlink_dir(temp.0.join("target"), temp.0.join("link"))) {
            return;
        }
        assert!(matches!(
            ConfigDir::open(&temp.0.join("link")),
            Err(ConfigFsError::ReparsePoint)
        ));
    }
    #[test]
    fn junction_rejected() {
        let temp = Temp::new();
        std::fs::create_dir(temp.0.join("target")).unwrap();
        let status = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(temp.0.join("link"))
            .arg(temp.0.join("target"))
            .status()
            .unwrap();
        assert!(status.success());
        assert!(matches!(
            ConfigDir::open(&temp.0.join("link")),
            Err(ConfigFsError::ReparsePoint)
        ));
        std::fs::remove_dir(temp.0.join("link")).unwrap();
    }
    #[test]
    fn extra_hard_link_rejected() {
        let temp = Temp::new();
        std::fs::write(temp.0.join("config"), b"x").unwrap();
        std::fs::hard_link(temp.0.join("config"), temp.0.join("link")).unwrap();
        let dir = ConfigDir::open(&temp.0).unwrap();
        assert!(matches!(
            open_file(&dir, "config"),
            Err(ConfigFsError::ExtraHardLinks)
        ));
        std::fs::remove_file(temp.0.join("link")).unwrap();
        assert!(open_file(&dir, "config").is_ok());
    }
    #[test]
    fn directory_as_file_rejected() {
        let temp = Temp::new();
        std::fs::create_dir(temp.0.join("child")).unwrap();
        let dir = ConfigDir::open(&temp.0).unwrap();
        assert!(matches!(
            open_file(&dir, "child"),
            Err(ConfigFsError::IsDirectory)
        ));
    }
    #[test]
    fn invalid_names_rejected() {
        let temp = Temp::new();
        let dir = ConfigDir::open(&temp.0).unwrap();
        for name in [
            "..\\x",
            "a/b",
            "C:x",
            "x:stream",
            "",
            ".",
            "..",
            "a<",
            "a>",
            "a\"",
            "a|",
            "a?",
            "a*",
            "a\u{1}",
            "a.",
            "a ",
            "con",
            "PrN.txt",
            "aux",
            "nul.log",
            "COM1",
            "com9.txt",
            "LPT1",
            "lpt9.ext",
            "conin$",
            "CONOUT$.txt",
        ] {
            assert!(matches!(
                open_file(&dir, name),
                Err(ConfigFsError::InvalidName)
            ));
        }
    }
    #[test]
    fn stale_lock_file_can_be_reused() {
        let _serial = TEST_WRITER_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let temp = Temp::new();
        std::fs::write(temp.0.join("config.lock"), b"stale").unwrap();
        let lock = WriterLock::acquire(&temp.0).unwrap();
        drop(lock);
        assert!(!temp.0.join("config.lock").exists());
    }
    #[test]
    fn held_directory_denies_rename() {
        let _serial = TEST_WRITER_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let temp = Temp::new();
        let old = temp.0.join("old");
        let moved = temp.0.join("moved");
        std::fs::create_dir(&old).unwrap();
        std::fs::write(old.join("config"), b"held").unwrap();
        let lock = WriterLock::acquire(&old).unwrap();
        // The held handle denies delete sharing, preventing replacement of the path.
        assert!(std::fs::rename(&old, &moved).is_err());
        assert!(!moved.exists());
        let mut file = open_file(lock.directory(), "config").unwrap();
        assert_eq!(file.read_bounded(32).unwrap(), b"held");
    }
    #[test]
    fn ancestor_junction_accepted() {
        let temp = Temp::new();
        let target = temp.0.join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::create_dir(target.join("config-dir")).unwrap();
        std::fs::write(target.join("config-dir").join("config"), b"x").unwrap();
        let link = temp.0.join("link");
        let status = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .status()
            .unwrap();
        assert!(status.success());
        {
            let dir = ConfigDir::open(&link.join("config-dir")).unwrap();
            assert!(open_file(&dir, "config").is_ok());
        }
        std::fs::remove_dir(link).unwrap();
    }
    #[test]
    fn file_as_directory_rejected() {
        let temp = Temp::new();
        std::fs::write(temp.0.join("file"), b"x").unwrap();
        assert!(matches!(
            ConfigDir::open(&temp.0.join("file")),
            Err(ConfigFsError::NotADirectory)
        ));
    }
}
