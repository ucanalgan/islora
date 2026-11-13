//! Handing files to Explorer: dragging them out of the island, opening them,
//! and showing them in their folder.

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, IDataObject};
use windows::Win32::System::Ole::{DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    ACTIVATEOPTIONS, ApplicationActivationManager, IApplicationActivationManager,
    BHID_DataObject, ILCreateFromPathW, ILFree, SHCreateShellItemArrayFromIDLists, SHDoDragDrop,
    SHOpenFolderAndSelectItems, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, Result, w};

/// Shell item lists, freed when dropped.
struct ItemList(Vec<*mut ITEMIDLIST>);

impl ItemList {
    fn new(paths: &[String]) -> Self {
        let items = paths.iter().map(|path| unsafe { ILCreateFromPathW(&HSTRING::from(path.as_str())) });
        Self(items.filter(|item| !item.is_null()).collect())
    }

    fn as_const(&self) -> Vec<*const ITEMIDLIST> {
        self.0.iter().map(|item| item.cast_const()).collect()
    }
}

impl Drop for ItemList {
    fn drop(&mut self) {
        for item in &self.0 {
            unsafe { ILFree(Some(item.cast_const())) };
        }
    }
}

/// Lets the user drag files somewhere else, with the picture Explorer shows
/// for them. Returns once they are dropped or the drag is called off.
pub fn drag_out(window: HWND, paths: &[String]) -> Result<DROPEFFECT> {
    let items = ItemList::new(paths);
    if items.0.is_empty() {
        return Ok(DROPEFFECT::default());
    }
    unsafe {
        let array = SHCreateShellItemArrayFromIDLists(&items.as_const())?;
        let data: IDataObject = array.BindToHandler(None, &BHID_DataObject)?;
        // Explorer decides whether a drop copies or moves, as it does for its own windows.
        SHDoDragDrop(Some(window), &data, None, DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK)
    }
}

/// Opens a file with the app Windows uses for it.
pub fn open(path: &str) {
    let path = HSTRING::from(path);
    let result = unsafe { ShellExecuteW(None, w!("open"), &path, None, None, SW_SHOWNORMAL) };
    if result.0 as isize <= 32 {
        // Avoid logging a potentially sensitive file path or URI.
        eprintln!("Windows could not open the requested item (shell code {}).", result.0 as isize);
    }
}

/// Opens the app that sent a Windows notification. The listener does not offer
/// activation of the original notification action or conversation.
pub fn launch_app(app_id: String, title: String) {
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if super::browser::is_whatsapp_notice(&app_id, &title) && super::browser::open_whatsapp_tab() {
            return;
        }
        let manager: Result<IApplicationActivationManager> =
            CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER);
        let launched = manager.and_then(|manager| {
            manager.ActivateApplication(&HSTRING::from(app_id.as_str()), w!(""), ACTIVATEOPTIONS::default())
        }).is_ok();
        if !launched {
            let path = HSTRING::from(format!("shell:AppsFolder\\{app_id}"));
            ShellExecuteW(None, w!("open"), &path, None, None, SW_SHOWNORMAL);
        }
    });
}

/// Opens the folder of the first file with all of the files in it selected.
/// Explorer can take a moment, so it happens away from the island's thread.
pub fn reveal(paths: Vec<String>) {
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Some(first) = paths.first() else { return };
        let folder_path = std::path::Path::new(first).parent().map(|folder| folder.to_string_lossy().into_owned());
        let Some(folder_path) = folder_path else { return };
        let folder = ItemList::new(std::slice::from_ref(&folder_path));
        let same_folder: Vec<String> = paths
            .iter()
            .filter(|path| std::path::Path::new(path).parent().map(|p| p.to_string_lossy().into_owned()) == Some(folder_path.clone()))
            .cloned()
            .collect();
        let files = ItemList::new(&same_folder);
        if let Some(folder) = folder.0.first() {
            let _ = SHOpenFolderAndSelectItems(folder.cast_const(), Some(&files.as_const()), 0);
        }
    });
}
