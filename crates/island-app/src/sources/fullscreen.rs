//! Tells the app when it should get out of the way. All of it is event driven:
//! nothing here wakes the process unless the state actually changes.

use std::mem::size_of;
use std::sync::atomic::{AtomicIsize, Ordering};

use island_core::fullscreen::{ScreenRect, fills_screen, is_desktop_class};
use windows::Win32::Foundation::{FALSE, HANDLE, HWND, LPARAM, RECT, TRUE, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::System::Power::{
    HPOWERNOTIFY, POWERBROADCAST_SETTING, RegisterPowerSettingNotification,
    UnregisterPowerSettingNotification,
};
use windows::Win32::System::RemoteDesktop::{
    NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
};
use windows::Win32::System::SystemServices::GUID_CONSOLE_DISPLAY_STATE;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::Shell::{
    ABM_NEW, ABM_REMOVE, APPBARDATA, QUERY_USER_NOTIFICATION_STATE, QUNS_BUSY,
    QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHAppBarMessage,
    SHQueryUserNotificationState,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DEVICE_NOTIFY_WINDOW_HANDLE, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MOVESIZEEND,
    EVENT_SYSTEM_MOVESIZESTART, EnumWindows, GWL_EXSTYLE, GWL_STYLE, GetClassNameW,
    GetLayeredWindowAttributes, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, IsZoomed, LAYERED_WINDOW_ATTRIBUTES_FLAGS, LWA_ALPHA, PBT_POWERSETTINGCHANGE,
    PostMessageW, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_APP, WS_CAPTION,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TRANSPARENT,
};
use windows::core::{BOOL, Result};

pub const MSG_APPBAR: u32 = WM_APP + 2;
pub const MSG_WINDOW_DRAG: u32 = WM_APP + 3;
pub const MSG_FRONT_CHANGED: u32 = WM_APP + 16;

const WTS_SESSION_LOCK: usize = 7;
const WTS_SESSION_UNLOCK: usize = 8;

static LISTENER: AtomicIsize = AtomicIsize::new(0);

pub struct Presence {
    hwnd: HWND,
    drag_hook: HWINEVENTHOOK,
    front_hook: HWINEVENTHOOK,
    display: HPOWERNOTIFY,
}

impl Presence {
    pub fn start(hwnd: HWND) -> Result<Self> {
        LISTENER.store(hwnd.0 as isize, Ordering::Relaxed);
        unsafe {
            // Registered only for its full-screen notification. Without
            // ABM_SETPOS an app bar reserves no screen space.
            let mut bar = appbar(hwnd);
            SHAppBarMessage(ABM_NEW, &mut bar);

            // Windows Snap Layouts opens at the top centre during a drag.
            let drag_hook = SetWinEventHook(
                EVENT_SYSTEM_MOVESIZESTART,
                EVENT_SYSTEM_MOVESIZEEND,
                None,
                Some(on_move_size),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            );

            // Each new window in front. A window filling the screen may cover
            // the island and the edge strip as it comes forward, and Windows'
            // full screen report has to be checked against what is in front.
            // It wakes the app once per switch between windows, nothing more.
            let front_hook = SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(on_front_changed),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            );

            let display = RegisterPowerSettingNotification(
                HANDLE(hwnd.0),
                &GUID_CONSOLE_DISPLAY_STATE,
                DEVICE_NOTIFY_WINDOW_HANDLE,
            )?;
            WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION)?;
            Ok(Self {
                hwnd,
                drag_hook,
                front_hook,
                display,
            })
        }
    }
}

impl Drop for Presence {
    fn drop(&mut self) {
        unsafe {
            let mut bar = appbar(self.hwnd);
            SHAppBarMessage(ABM_REMOVE, &mut bar);
            let _ = UnhookWinEvent(self.front_hook);
            let _ = UnhookWinEvent(self.drag_hook);
            let _ = UnregisterPowerSettingNotification(self.display);
            let _ = WTSUnRegisterSessionNotification(self.hwnd);
        }
    }
}

fn appbar(hwnd: HWND) -> APPBARDATA {
    APPBARDATA {
        cbSize: size_of::<APPBARDATA>() as u32,
        hWnd: hwnd,
        uCallbackMessage: MSG_APPBAR,
        ..Default::default()
    }
}

unsafe extern "system" fn on_move_size(
    _hook: HWINEVENTHOOK,
    event: u32,
    _hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    let listener = HWND(LISTENER.load(Ordering::Relaxed) as _);
    let started = (event == EVENT_SYSTEM_MOVESIZESTART) as usize;
    unsafe {
        let _ = PostMessageW(Some(listener), MSG_WINDOW_DRAG, WPARAM(started), LPARAM(0));
    }
}

unsafe extern "system" fn on_front_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    let listener = HWND(LISTENER.load(Ordering::Relaxed) as _);
    unsafe {
        let _ = PostMessageW(Some(listener), MSG_FRONT_CHANGED, WPARAM(0), LPARAM(0));
    }
}

/// Whether Windows calls something full screen, for the state at startup.
pub fn fullscreen_now() -> bool {
    let state: QUERY_USER_NOTIFICATION_STATE =
        unsafe { SHQueryUserNotificationState() }.unwrap_or_default();
    matches!(
        state,
        QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE
    )
}

/// Whether a window fills the island's screen in front of the island. Windows
/// reports full screen for the whole session, so an app on another screen, an
/// invisible overlay spanning one, or the bare desktop would hide the island
/// on a screen where nothing covers it. The window in front need not have the
/// focus: a game stays in front on its screen while another screen is used.
pub fn covers_island(island: HWND) -> bool {
    let mut spot = RECT::default();
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        if GetWindowRect(island, &mut spot).is_err() {
            return false;
        }
        let monitor = MonitorFromWindow(island, MONITOR_DEFAULTTONEAREST);
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return false;
        }
    }
    let mut search = Search {
        spot: screen_rect(spot),
        screen: screen_rect(info.rcMonitor),
        own: unsafe { GetCurrentProcessId() },
        covered: false,
    };
    unsafe {
        // Stopping early reports an error, which means nothing here.
        let _ = EnumWindows(
            Some(front_of_island),
            LPARAM(&mut search as *mut Search as isize),
        );
    }
    search.covered
}

struct Search {
    spot: ScreenRect,
    screen: ScreenRect,
    own: u32,
    covered: bool,
}

/// Visits windows front to back and stops at the first one over the island.
unsafe extern "system" fn front_of_island(hwnd: HWND, data: LPARAM) -> BOOL {
    let search = unsafe { &mut *(data.0 as *mut Search) };
    if !could_be_in_front(hwnd, search.own) {
        return TRUE;
    }
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return TRUE;
    }
    let rect = screen_rect(rect);
    if !rect.intersects(search.spot) {
        return TRUE;
    }
    let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
    let maximized_with_caption =
        unsafe { IsZoomed(hwnd) }.as_bool() && style & WS_CAPTION.0 == WS_CAPTION.0;
    search.covered = fills_screen(rect, search.screen, maximized_with_caption);
    FALSE
}

/// Whether a window can be what is seen in front of the island. The island's
/// own windows, the desktop and taskbars, and windows that are hidden, cloaked,
/// click-through, never activated or fully transparent, as overlays are, are
/// looked past.
fn could_be_in_front(hwnd: HWND, own: u32) -> bool {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return false;
        }
        let mut process = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut process));
        if process == own {
            return false;
        }
        let mut cloaked = 0u32;
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut _,
            size_of::<u32>() as u32,
        )
        .is_ok()
            && cloaked != 0
        {
            return false;
        }
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex_style & (WS_EX_TRANSPARENT.0 | WS_EX_NOACTIVATE.0) != 0 {
            return false;
        }
        if ex_style & WS_EX_LAYERED.0 != 0 {
            let mut alpha = 255u8;
            let mut flags = LAYERED_WINDOW_ATTRIBUTES_FLAGS(0);
            if GetLayeredWindowAttributes(hwnd, None, Some(&mut alpha), Some(&mut flags)).is_ok()
                && flags.0 & LWA_ALPHA.0 != 0
                && alpha == 0
            {
                return false;
            }
        }
        let mut class = [0u16; 32];
        let length = GetClassNameW(hwnd, &mut class).max(0) as usize;
        !is_desktop_class(&String::from_utf16_lossy(&class[..length]))
    }
}

fn screen_rect(rect: RECT) -> ScreenRect {
    ScreenRect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

/// Decodes `WM_POWERBROADCAST`. Returns whether the display is now on.
pub fn display_state(wparam: WPARAM, lparam: LPARAM) -> Option<bool> {
    if wparam.0 as u32 != PBT_POWERSETTINGCHANGE {
        return None;
    }
    let setting = unsafe { &*(lparam.0 as *const POWERBROADCAST_SETTING) };
    (setting.PowerSetting == GUID_CONSOLE_DISPLAY_STATE).then(|| setting.Data[0] != 0)
}

/// Decodes `WM_WTSSESSION_CHANGE`. Returns whether the session is now locked.
pub fn session_locked(wparam: WPARAM) -> Option<bool> {
    match wparam.0 {
        WTS_SESSION_LOCK => Some(true),
        WTS_SESSION_UNLOCK => Some(false),
        _ => None,
    }
}
