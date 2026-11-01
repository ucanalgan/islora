//! Confirms a copy. Only the kind of content is asked for, never the content.

use island_core::Indicator;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, IsClipboardFormatAvailable, RegisterClipboardFormatW,
};
use windows::core::{Result, w};

const CF_BITMAP: u32 = 2;
const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;

/// Windows then sends `WM_CLIPBOARDUPDATE` for each change.
pub fn start(hwnd: HWND) -> Result<()> {
    unsafe { AddClipboardFormatListener(hwnd) }
}

fn holds(format: u32) -> bool {
    unsafe { IsClipboardFormatAvailable(format) }.is_ok()
}

/// What to say about the latest change, if anything. Password managers mark
/// their copies with a format of their own, asking to be left alone.
pub fn copied() -> Option<Indicator> {
    let private = unsafe { RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")) };
    if private != 0 && holds(private) {
        return None;
    }
    let what = if holds(CF_HDROP) {
        "Dosya"
    } else if holds(CF_DIB) || holds(CF_BITMAP) {
        "Görsel"
    } else if holds(CF_UNICODETEXT) {
        "Metin"
    } else {
        return None;
    };
    Some(Indicator::Copied { what })
}
