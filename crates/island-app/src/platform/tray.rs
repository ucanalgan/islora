use std::mem::size_of;

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, POINT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    ShellExecuteW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, Result, w};

pub const MSG_TRAY: u32 = WM_APP + 1;

#[derive(Clone, Copy, PartialEq)]
pub enum Command {
    OpenSettings,
    ReleaseNotes,
    Quit,
}

const ID_SETTINGS: usize = 1;
const ID_RELEASE_NOTES: usize = 2;
const ID_QUIT: usize = 3;

const RELEASE_NOTES: &str = concat!("https://github.com/ucanalgan/islora/releases/tag/v", env!("CARGO_PKG_VERSION"));

// Resource numbers from app.rc.
const ICON_WHITE: usize = 2;
const ICON_BLACK: usize = 3;

pub struct Tray {
    data: NOTIFYICONDATAW,
    white: HICON,
    black: HICON,
}

impl Tray {
    pub fn add(hwnd: HWND) -> Result<Self> {
        let (white, black) = (load_icon(hwnd, ICON_WHITE)?, load_icon(hwnd, ICON_BLACK)?);
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: MSG_TRAY,
            hIcon: if taskbar_is_light() { black } else { white },
            ..Default::default()
        };
        let tip: Vec<u16> = "Islora".encode_utf16().collect();
        data.szTip[..tip.len()].copy_from_slice(&tip);

        let tray = Self { data, white, black };
        tray.restore();
        Ok(tray)
    }

    /// A white icon would vanish on a light taskbar, and a black one on a dark.
    pub fn follow_theme(&mut self) {
        self.data.hIcon = if taskbar_is_light() { self.black } else { self.white };
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &self.data);
        }
    }

    /// Adds the icon. Also needed after Explorer restarts, which forgets it.
    pub fn restore(&self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &self.data);
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.data);
        }
    }
}

fn load_icon(hwnd: HWND, resource: usize) -> Result<HICON> {
    unsafe {
        let size = GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForWindow(hwnd));
        let handle: HANDLE = LoadImageW(
            Some(GetModuleHandleW(None)?.into()),
            PCWSTR(resource as *const u16),
            IMAGE_ICON,
            size,
            size,
            LR_DEFAULTCOLOR,
        )?;
        Ok(HICON(handle.0))
    }
}

fn taskbar_is_light() -> bool {
    theme_setting(w!("SystemUsesLightTheme")).is_some_and(|value| value != 0)
}

/// Whether Windows draws its own surfaces transparent. A missing value means yes.
pub fn transparency_is_on() -> bool {
    theme_setting(w!("EnableTransparency")).is_none_or(|value| value != 0)
}

fn theme_setting(name: PCWSTR) -> Option<u32> {
    let mut value = 0_u32;
    let mut size = size_of::<u32>() as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            name,
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&mut size),
        )
    };
    result.is_ok().then_some(value)
}

/// Decodes `WM_SETTINGCHANGE`, which Windows sends for many other settings too.
pub fn is_theme_change(lparam: LPARAM) -> bool {
    lparam.0 != 0
        && unsafe { PCWSTR(lparam.0 as *const u16).to_string() }
            .is_ok_and(|setting| setting == "ImmersiveColorSet")
}

/// Blocks in a modal loop until the menu closes, so it must not be called
/// while the app state is borrowed.
/// Everything else lives in the settings window, which the first item opens.
pub fn show_menu(hwnd: HWND) -> Option<Command> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        AppendMenuW(menu, MF_STRING, ID_SETTINGS, w!("Ayarlar")).ok()?;
        AppendMenuW(menu, MF_STRING, ID_RELEASE_NOTES, &HSTRING::from("Sürüm notları")).ok()?;
        AppendMenuW(menu, MF_SEPARATOR, 0, None).ok()?;
        AppendMenuW(menu, MF_STRING, ID_QUIT, &HSTRING::from("Islora'dan çık")).ok()?;

        let mut cursor = POINT::default();
        GetCursorPos(&mut cursor).ok()?;
        // Without this the menu stays open when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            cursor.x,
            cursor.y,
            None,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        match chosen.0 as usize {
            ID_SETTINGS => Some(Command::OpenSettings),
            ID_RELEASE_NOTES => Some(Command::ReleaseNotes),
            ID_QUIT => Some(Command::Quit),
            _ => None,
        }
    }
}

/// Opens this version's notes in the browser. The app itself fetches nothing.
pub fn open_release_notes() {
    // Handing a link to the browser can take a while, which the UI must not wait for.
    std::thread::spawn(|| unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(RELEASE_NOTES), None, None, SW_SHOWNORMAL);
    });
}
