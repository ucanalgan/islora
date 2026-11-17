use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
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
pub enum ConfigFsError {
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
    path: PathBuf,
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
            path: path.to_owned(),
            identity,
        })
    }
    pub fn identity(&self) -> FileIdentity {
        self.identity
    }
}

/// Holds a verified file and its directory. Handles deny FILE_SHARE_DELETE, so an
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
        use std::os::windows::ffi::OsStrExt;
        let name_wide: Vec<u16> = name.encode_wide().collect();
        let stem = name_wide
            .split(|&c| c == b'.' as u16)
            .next()
            .unwrap_or_default();
        let stem_ascii = String::from_utf16_lossy(stem).to_ascii_uppercase();
        let reserved = matches!(
            stem_ascii.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) || (stem_ascii.len() == 4
            && (stem_ascii.starts_with("COM") || stem_ascii.starts_with("LPT"))
            && matches!(stem_ascii.as_bytes()[3], b'1'..=b'9'));
        if name_wide.is_empty()
            || name_wide == [46]
            || name_wide == [46, 46]
            || name_wide.last().is_some_and(|c| matches!(*c, 32 | 46))
            || name_wide
                .iter()
                .any(|c| matches!(*c, 0..=31 | 34 | 42 | 47 | 58 | 60 | 62 | 63 | 92 | 124))
            || reserved
        {
            return Err(ConfigFsError::InvalidName);
        }
        // TODO(2A-2): Open relative to the directory handle using NtCreateFile RootDirectory.
        // Backup semantics permits opening directories so they can be explicitly rejected.
        let file = open(&dir.path.join(name), true)?;
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
        // The opened file and directory deny delete sharing. Resolve the file's final
        // handle path, reopen its parent without following its final reparse point,
        // and compare identities; neither object can be renamed during this check.
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
    fn different_directory_identity_rejected() {
        let temp = Temp::new();
        let a = temp.0.join("a");
        let b = temp.0.join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        std::fs::write(b.join("config"), b"x").unwrap();
        let held = ConfigDir::open(&a).unwrap();
        let dir = ConfigDir {
            _file: held._file,
            identity: held.identity,
            path: b,
        };
        assert!(matches!(
            open_file(&dir, "config"),
            Err(ConfigFsError::OutsideDirectory)
        ));
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
