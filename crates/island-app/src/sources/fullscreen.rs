//! Tells the app when it should get out of the way. All of it is event driven:
//! nothing here wakes the process unless the state actually changes.

use std::mem::size_of;
use std::sync::atomic::{AtomicIsize, Ordering};

use island_core::fullscreen::is_desktop_class;
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::System::Power::{
    HPOWERNOTIFY, POWERBROADCAST_SETTING, RegisterPowerSettingNotification,
    UnregisterPowerSettingNotification,
};
use windows::Win32::System::RemoteDesktop::{
    NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
};
use windows::Win32::System::SystemServices::GUID_CONSOLE_DISPLAY_STATE;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::Shell::{
    ABM_NEW, ABM_REMOVE, APPBARDATA, QUERY_USER_NOTIFICATION_STATE, QUNS_BUSY,
    QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHAppBarMessage,
    SHQueryUserNotificationState,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DEVICE_NOTIFY_WINDOW_HANDLE, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MOVESIZEEND,
    EVENT_SYSTEM_MOVESIZESTART, GetClassNameW, GetForegroundWindow, PBT_POWERSETTINGCHANGE, PostMessageW, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_APP,
};
use windows::core::Result;

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
            Ok(Self { hwnd, drag_hook, front_hook, display })
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
    matches!(state, QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE)
}

/// Whether the desktop or the taskbar is in front, which Windows also reports as
/// full screen. A full screen app need not be in front for Windows to report it,
/// for one on another screen, so nothing else is second-guessed.
pub fn desktop_in_front() -> bool {
    let mut class = [0u16; 32];
    let length = unsafe { GetClassNameW(GetForegroundWindow(), &mut class) }.max(0) as usize;
    is_desktop_class(&String::from_utf16_lossy(&class[..length]))
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
