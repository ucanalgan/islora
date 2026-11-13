//! New screenshots, which Windows saves to a folder of its own. This thread
//! sleeps inside the system call until a file appears there.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use island_core::Indicator;
use island_core::indicator::is_image_file;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ACTION_ADDED, FILE_ACTION_RENAMED_NEW_NAME, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
};
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    IBindCtx,
};
use windows::Win32::UI::Shell::{
    ApplicationActivationManager, FOLDERID_Screenshots, IApplicationActivationManager, IShellItem,
    IShellItemArray, KF_FLAG_CREATE, SHCreateItemFromParsingName,
    SHCreateShellItemArrayFromShellItem, SHGetKnownFolderPath, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow, SW_SHOWNORMAL};
use windows::core::{HSTRING, PCWSTR, Result, w};

use super::IndicatorSink;

/// A file is announced before it is fully written.
const WRITE_WAIT: Duration = Duration::from_millis(400);
const READ_TRIES: u32 = 4;
/// Refuses absurd files rather than holding them in memory.
const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const SNIPPING_TOOL: PCWSTR = w!("Microsoft.ScreenSketch_8wekyb3d8bbwe!App");

pub fn start(sink: IndicatorSink) {
    let spawned = std::thread::Builder::new()
        .name("screenshots".into())
        .spawn(move || {
            watch(&sink);
        });
    if let Err(error) = spawned {
        eprintln!("screenshot thread failed to start: {error}");
    }
}

/// Opens a screenshot in the Snipping Tool, where it can be seen and marked up,
/// or in the usual image app on a machine without one. The island never takes
/// focus, so it has to hand the right to come to the front over.
pub fn open(path: String) {
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = AllowSetForegroundWindow(ASFW_ANY);
        if open_in_snipping_tool(&path).is_err() {
            ShellExecuteW(
                None,
                w!("open"),
                &HSTRING::from(path.as_str()),
                None,
                None,
                SW_SHOWNORMAL,
            );
        }
    });
}

fn open_in_snipping_tool(path: &str) -> Result<()> {
    unsafe {
        let manager: IApplicationActivationManager =
            CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER)?;
        let file: IShellItem =
            SHCreateItemFromParsingName(&HSTRING::from(path), None::<&IBindCtx>)?;
        let files: IShellItemArray = SHCreateShellItemArrayFromShellItem(&file)?;
        manager
            .ActivateForFile(SNIPPING_TOOL, &files, w!("open"))
            .map(|_| ())
    }
}

fn folder() -> Option<String> {
    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_Screenshots, KF_FLAG_CREATE, None).ok()?;
        let text = path.to_string().ok();
        CoTaskMemFree(Some(path.0.cast()));
        text
    }
}

fn watch(sink: &IndicatorSink) {
    let Some(folder) = folder() else { return };
    let sharing = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    let directory = unsafe {
        CreateFileW(
            &HSTRING::from(folder.as_str()),
            FILE_LIST_DIRECTORY.0,
            sharing,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    };
    let Ok(directory) = directory else { return };

    let mut changes = [0_u32; 2048];
    while let Some(written) = wait_for_changes(directory, &mut changes) {
        if let Some(name) = new_image(&changes, written) {
            let path = Path::new(&folder).join(name);
            let image = read_image(&path);
            sink.send(Indicator::Screenshot {
                path: path.display().to_string(),
                image,
            });
        }
    }
}

fn wait_for_changes(directory: HANDLE, changes: &mut [u32]) -> Option<u32> {
    let mut written = 0;
    let waited = unsafe {
        ReadDirectoryChangesW(
            directory,
            changes.as_mut_ptr().cast(),
            size_of_val(changes) as u32,
            false,
            FILE_NOTIFY_CHANGE_FILE_NAME,
            Some(&mut written),
            None,
            None,
        )
    };
    waited.is_ok().then_some(written)
}

/// The last new image among the entries, which Windows packs one after another.
fn new_image(changes: &[u32], written: u32) -> Option<String> {
    if written == 0 {
        return None;
    }
    let mut found = None;
    let mut entry = changes.as_ptr().cast::<FILE_NOTIFY_INFORMATION>();
    loop {
        let current = unsafe { &*entry };
        let units = current.FileNameLength as usize / size_of::<u16>();
        let name = unsafe { std::slice::from_raw_parts(current.FileName.as_ptr(), units) };
        let name = String::from_utf16_lossy(name);
        let is_new =
            current.Action == FILE_ACTION_ADDED || current.Action == FILE_ACTION_RENAMED_NEW_NAME;
        if is_new && is_image_file(&name) {
            found = Some(name);
        }
        if current.NextEntryOffset == 0 {
            return found;
        }
        entry = unsafe { entry.byte_add(current.NextEntryOffset as usize) };
    }
}

/// The notice still goes up without the image, if the file stays out of reach.
fn read_image(path: &Path) -> Option<Arc<[u8]>> {
    for _ in 0..READ_TRIES {
        std::thread::sleep(WRITE_WAIT);
        if let Ok(bytes) = std::fs::read(path)
            && !bytes.is_empty()
        {
            return (bytes.len() <= MAX_IMAGE_BYTES).then(|| bytes.into());
        }
    }
    None
}
