use std::mem::size_of;

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, POINT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, Result, w};

pub const MSG_TRAY: u32 = WM_APP + 1;

#[derive(Clone, Copy, PartialEq)]
pub enum Command {
    ToggleDemo,
    Reload,
    OpenSettings,
    ToggleAutostart,
    ToggleNotch,
    Quit,
}

/// What the menu shows as switched on.
pub struct MenuState {
    pub demo_running: bool,
    pub autostart: bool,
    pub notch: bool,
}

const ID_DEMO: usize = 1;
const ID_QUIT: usize = 2;
const ID_RELOAD: usize = 3;
const ID_SETTINGS: usize = 4;
const ID_AUTOSTART: usize = 5;
const ID_NOTCH: usize = 6;

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
        let tip: Vec<u16> = "Dynamic Island".encode_utf16().collect();
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
    let mut value = 0_u32;
    let mut size = size_of::<u32>() as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&mut size),
        )
    };
    result.is_ok() && value != 0
}

/// Decodes `WM_SETTINGCHANGE`, which Windows sends for many other settings too.
pub fn is_theme_change(lparam: LPARAM) -> bool {
    lparam.0 != 0
        && unsafe { PCWSTR(lparam.0 as *const u16).to_string() }
            .is_ok_and(|setting| setting == "ImmersiveColorSet")
}

/// Blocks in a modal loop until the menu closes, so it must not be called
/// while the app state is borrowed.
pub fn show_menu(hwnd: HWND, state: &MenuState) -> Option<Command> {
    let switch = |on: bool| if on { MF_STRING | MF_CHECKED } else { MF_STRING };
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        let label = if state.demo_running { w!("Demoyu durdur") } else { w!("Demoyu başlat") };
        AppendMenuW(menu, MF_STRING, ID_DEMO, label).ok()?;
        AppendMenuW(menu, MF_STRING, ID_SETTINGS, w!("Ayarları aç")).ok()?;
        AppendMenuW(menu, MF_STRING, ID_RELOAD, w!("Takvimi ve ayarları yenile")).ok()?;
        AppendMenuW(menu, MF_SEPARATOR, 0, None).ok()?;
        AppendMenuW(menu, switch(state.autostart), ID_AUTOSTART, w!("Windows ile başlat")).ok()?;
        AppendMenuW(menu, switch(state.notch), ID_NOTCH, w!("Notch görünümü")).ok()?;
        AppendMenuW(menu, MF_SEPARATOR, 0, None).ok()?;
        AppendMenuW(menu, MF_STRING, ID_QUIT, &HSTRING::from("Çıkış")).ok()?;

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
            ID_DEMO => Some(Command::ToggleDemo),
            ID_RELOAD => Some(Command::Reload),
            ID_SETTINGS => Some(Command::OpenSettings),
            ID_AUTOSTART => Some(Command::ToggleAutostart),
            ID_NOTCH => Some(Command::ToggleNotch),
            ID_QUIT => Some(Command::Quit),
            _ => None,
        }
    }
}
