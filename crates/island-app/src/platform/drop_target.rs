//! Files dragged onto the island from Explorer, or any app that drags files.
//! Windows asks the window under the pointer through `IDropTarget`, on the
//! window's own thread, while it runs the message loop.

use std::cell::{Cell, RefCell};

use island_core::files::DroppedFile;
use windows::Win32::Foundation::{HWND, POINT, POINTL};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL,
};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_NONE, IDropTarget,
    IDropTarget_Impl, OleInitialize, RegisterDragDrop, ReleaseStgMedium,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{CLSID_DragDropHelper, DragQueryFileW, HDROP, IDropTargetHelper};
use windows::core::{Ref, Result, implement};

/// What the app does with a drag. Each answers whether a drop at that point
/// would be taken.
pub struct Handlers {
    pub enter: fn(Vec<DroppedFile>, POINT) -> bool,
    pub over: fn(POINT) -> bool,
    pub leave: fn(),
    pub drop: fn(POINT) -> bool,
}

/// Lets `window` take dropped files.
pub fn register(window: HWND, handlers: &'static Handlers) -> Result<()> {
    unsafe {
        // The thread is already a single threaded apartment, which this keeps.
        OleInitialize(None)?;
        let target: IDropTarget = Target { window, handlers, helper: RefCell::new(None), carrying: Cell::new(false) }.into();
        RegisterDragDrop(window, &target)
    }
}

#[implement(IDropTarget)]
struct Target {
    window: HWND,
    handlers: &'static Handlers,
    /// Draws the picture of what is dragged, as Explorer does over its own
    /// windows. Made at the first drag, as it loads much of the shell.
    helper: RefCell<Option<IDropTargetHelper>>,
    /// Whether what is dragged now holds files at all.
    carrying: Cell<bool>,
}

/// Copying is what the island asks for, which leaves the files where they are.
/// It never asks to move them, which would have the source delete them.
fn effect_for(wanted: bool, allowed: DROPEFFECT) -> DROPEFFECT {
    match wanted {
        true if allowed.0 & DROPEFFECT_COPY.0 != 0 => DROPEFFECT_COPY,
        true if allowed.0 & DROPEFFECT_LINK.0 != 0 => DROPEFFECT_LINK,
        _ => DROPEFFECT_NONE,
    }
}

impl Target {
    fn helper(&self) -> Option<IDropTargetHelper> {
        let mut helper = self.helper.borrow_mut();
        if helper.is_none() {
            *helper = unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER) }.ok();
        }
        helper.clone()
    }
}

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let point = POINT { x: point.x, y: point.y };
        let files = data.as_ref().map(read_files).unwrap_or_default();
        self.carrying.set(!files.is_empty());
        let wanted = !files.is_empty() && (self.handlers.enter)(files, point);
        unsafe {
            *effect = effect_for(wanted, *effect);
            if let (Some(helper), Some(data)) = (self.helper(), data.as_ref()) {
                let _ = helper.DragEnter(self.window, data, &point, *effect);
            }
        }
        Ok(())
    }

    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, point: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let point = POINT { x: point.x, y: point.y };
        let wanted = self.carrying.get() && (self.handlers.over)(point);
        unsafe {
            *effect = effect_for(wanted, *effect);
            if let Some(helper) = self.helper.borrow().as_ref() {
                let _ = helper.DragOver(&point, *effect);
            }
        }
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        if let Some(helper) = self.helper.borrow().as_ref() {
            let _ = unsafe { helper.DragLeave() };
        }
        if self.carrying.replace(false) {
            (self.handlers.leave)();
        }
        Ok(())
    }

    fn Drop(&self, data: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, point: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let point = POINT { x: point.x, y: point.y };
        let wanted = self.carrying.replace(false) && (self.handlers.drop)(point);
        unsafe {
            *effect = effect_for(wanted, *effect);
            if let (Some(helper), Some(data)) = (self.helper.borrow().as_ref(), data.as_ref()) {
                let _ = helper.Drop(data, &point, *effect);
            }
        }
        Ok(())
    }
}

/// The paths in a drag, with their sizes. Things that are not files, such as
/// text from a page, have none.
fn read_files(data: &IDataObject) -> Vec<DroppedFile> {
    let format = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let Ok(mut medium) = (unsafe { data.GetData(&format) }) else {
        return Vec::new();
    };
    let drop = HDROP(unsafe { medium.u.hGlobal.0 });
    let count = unsafe { DragQueryFileW(drop, u32::MAX, None) };
    let mut files = Vec::with_capacity(count as usize);
    for index in 0..count {
        let length = unsafe { DragQueryFileW(drop, index, None) } as usize;
        let mut buffer = vec![0_u16; length + 1];
        let copied = unsafe { DragQueryFileW(drop, index, Some(&mut buffer)) } as usize;
        let path = String::from_utf16_lossy(&buffer[..copied]);
        let metadata = std::fs::metadata(&path).ok();
        files.push(DroppedFile {
            size: metadata.as_ref().map_or(0, std::fs::Metadata::len),
            is_folder: metadata.as_ref().is_some_and(std::fs::Metadata::is_dir),
            path,
        });
    }
    unsafe { ReleaseStgMedium(&mut medium) };
    files
}
