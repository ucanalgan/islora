//! Caps Lock, Num Lock and the keyboard language. Windows has no event for these
//! outside the app in use, so this listens to raw keyboard input. Unlike a
//! low-level hook it adds no delay to other apps' typing: Windows sends a copy
//! here instead of waiting for us.

use std::mem::size_of;

use island_core::Indicator;
use island_core::indicator::language_code;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::Globalization::LCIDToLocaleName;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, GetKeyboardLayout, VK_CAPITAL, VK_CONTROL, VK_LWIN, VK_MENU, VK_NUMLOCK, VK_RWIN,
    VK_SHIFT, VK_SPACE,
};
use windows::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT, RIDEV_INPUTSINK,
    RIM_TYPEKEYBOARD, RegisterRawInputDevices,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, RI_KEY_BREAK};
use windows::core::Result;

const USAGE_PAGE_GENERIC: u16 = 0x01;
const USAGE_KEYBOARD: u16 = 0x06;
const LANGUAGE_MASK: usize = 0xFFFF;
const LONGEST_LOCALE_NAME: usize = 85;

pub enum KeyEvent {
    Lock(Indicator),
    /// A key from the shortcuts that switch language, so it may have changed.
    LanguageKey,
}

pub fn start(hwnd: HWND) -> Result<()> {
    let keyboard = RAWINPUTDEVICE {
        usUsagePage: USAGE_PAGE_GENERIC,
        usUsage: USAGE_KEYBOARD,
        dwFlags: RIDEV_INPUTSINK,
        hwndTarget: hwnd,
    };
    unsafe { RegisterRawInputDevices(&[keyboard], size_of::<RAWINPUTDEVICE>() as u32) }
}

/// Decodes `WM_INPUT`. Acts when the key is released, as holding it down repeats.
pub fn key_event(lparam: LPARAM) -> Option<KeyEvent> {
    let mut input = RAWINPUT::default();
    let mut size = size_of::<RAWINPUT>() as u32;
    let written = unsafe {
        GetRawInputData(
            HRAWINPUT(lparam.0 as _),
            RID_INPUT,
            Some((&raw mut input).cast()),
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        )
    };
    if written == u32::MAX || input.header.dwType != RIM_TYPEKEYBOARD.0 {
        return None;
    }

    let keyboard = unsafe { input.data.keyboard };
    if u32::from(keyboard.Flags) & RI_KEY_BREAK == 0 {
        return None;
    }
    let key = keyboard.VKey;
    if key == VK_CAPITAL.0 {
        Some(KeyEvent::Lock(Indicator::CapsLock { on: is_toggled(VK_CAPITAL.0) }))
    } else if key == VK_NUMLOCK.0 {
        Some(KeyEvent::Lock(Indicator::NumLock { on: is_toggled(VK_NUMLOCK.0) }))
    } else if [VK_SPACE, VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN].iter().any(|shortcut| shortcut.0 == key) {
        Some(KeyEvent::LanguageKey)
    } else {
        None
    }
}

fn is_toggled(key: u16) -> bool {
    unsafe { GetKeyState(key as i32) & 1 != 0 }
}

/// The language of the app in use, such as `TR`.
pub fn language() -> Option<String> {
    let mut name = [0_u16; LONGEST_LOCALE_NAME];
    let written = unsafe {
        let thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let layout = GetKeyboardLayout(thread);
        LCIDToLocaleName((layout.0 as usize & LANGUAGE_MASK) as u32, Some(&mut name), 0)
    };
    // The count includes the terminator.
    let length = usize::try_from(written).ok()?.checked_sub(1)?;
    language_code(&String::from_utf16_lossy(name.get(..length)?))
}
