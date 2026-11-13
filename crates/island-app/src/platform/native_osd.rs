//! Hides the shell's volume/brightness flyout while Islora shows their levels.
//! The shell creates the host lazily, so watching its show event also handles
//! the first key press after startup and an explorer restart.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

use windows::Win32::Foundation::{E_FAIL, HWND, LPARAM, WPARAM};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_SHOW, FindWindowExW, GetClassNameW, GetShellWindow, GetWindowRect,
    GetWindowTextW, GetWindowThreadProcessId,
    DispatchMessageW, GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW,
    SHOW_WINDOW_CMD, ShowWindowAsync, TranslateMessage, WINEVENT_OUTOFCONTEXT, WM_QUIT,
};
use windows::core::{Error, HRESULT, Result, w};

static ENABLED: AtomicBool = AtomicBool::new(false);
pub struct NativeOsd {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl NativeOsd {
    pub fn start(enabled: bool) -> Result<Self> {
        ENABLED.store(enabled, Ordering::Relaxed);
        let (ready, result) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new().name("native-osd-watch".into()).spawn(move || unsafe {
            // Out-of-context events are dispatched on the registering thread.
            // A separate message loop prevents rendering or settings work on
            // the island's UI thread from delaying the shell-hide request.
            let thread_id = GetCurrentThreadId();
            let mut message = MSG::default();
            let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
            let hook = SetWinEventHook(EVENT_OBJECT_SHOW, EVENT_OBJECT_SHOW, None, Some(on_show), 0, 0, WINEVENT_OUTOFCONTEXT);
            if hook.is_invalid() {
                let _ = ready.send(Err(Error::from_thread().code().0));
                return;
            }
            if ready.send(Ok(thread_id)).is_ok() {
                loop {
                    let status = GetMessageW(&mut message, None, 0, 0).0;
                    if status < 0 { eprintln!("native OSD message loop failed: {}", Error::from_thread()); break; }
                    if status == 0 { break; }
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            let _ = UnhookWinEvent(hook);
        }).map_err(|error| Error::new(E_FAIL, format!("native OSD worker failed: {error}")))?;
        match result.recv() {
            Ok(Ok(thread_id)) => Ok(Self { thread_id, thread: Some(thread) }),
            Ok(Err(code)) => { ENABLED.store(false, Ordering::Relaxed); let _ = thread.join(); Err(Error::from_hresult(HRESULT(code))) }
            Err(error) => { ENABLED.store(false, Ordering::Relaxed); let _ = thread.join(); Err(Error::new(E_FAIL, format!("native OSD worker stopped: {error}"))) }
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        ENABLED.store(enabled, Ordering::Relaxed);
    }
}

impl Drop for NativeOsd {
    fn drop(&mut self) {
        ENABLED.store(false, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            if let Err(error) = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) } {
                eprintln!("native OSD worker could not be stopped: {error}");
            } else if thread.join().is_err() {
                eprintln!("native OSD worker stopped unexpectedly");
            }
        }
    }
}

unsafe extern "system" fn on_show(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    if !ENABLED.load(Ordering::Relaxed) || hwnd.0.is_null() || object != 0 || child != 0 {
        return;
    }
    unsafe {
        let mut shell_pid = 0;
        GetWindowThreadProcessId(GetShellWindow(), Some(&mut shell_pid));
        let mut owner_pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut owner_pid));
        if shell_pid == 0 || owner_pid != shell_pid {
            return;
        }

        let mut class = [0u16; 64];
        let len = GetClassNameW(hwnd, &mut class) as usize;
        let xaml = &class[..len] == w!("XamlExplorerHostIslandWindow").as_wide();
        let legacy = &class[..len] == w!("NativeHWNDHost").as_wide();
        if !xaml && !legacy {
            return;
        }
        let mut title = [0u16; 2];
        if GetWindowTextW(hwnd, &mut title) != 0 {
            return;
        }
        let mut rect = Default::default();
        if GetWindowRect(hwnd, &mut rect).is_err() { return; }
        // Toasts and other shell flyouts share this host class and bridge.
        // The level indicator is a small flyout at the bottom centre on the
        // supported layout. An unknown placement is safer left alone.
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() || !level_host_geometry(rect, info.rcMonitor, GetDpiForWindow(hwnd)) { return; }
        // Other shell surfaces also use a XAML host. The level flyout has this
        // specific content bridge; on older builds it has a DirectUI child.
        let content = if xaml {
            FindWindowExW(
                Some(hwnd),
                None,
                w!("Windows.UI.Composition.DesktopWindowContentBridge"),
                w!("DesktopWindowXamlSource"),
            )
        } else {
            FindWindowExW(Some(hwnd), None, w!("DirectUIHWND"), w!(""))
        };
        if content.is_err() { return; }
        // Hiding the host is enough. Minimising its content and restoring the
        // host on exit can leave an empty, transparent rectangle on screen.
        if !ShowWindowAsync(hwnd, SHOW_WINDOW_CMD(0)).as_bool() { // SW_HIDE
            eprintln!("Windows level flyout could not be hidden");
        }
    }
}

/// Match relative to the host's own monitor, in device-independent dimensions.
fn level_host_geometry(rect: RECT, monitor: RECT, dpi: u32) -> bool {
    let scale = f64::from(if dpi == 0 { 96 } else { dpi.clamp(96, 384) }) / 96.0;
    let width = f64::from(rect.right - rect.left) / scale;
    let height = f64::from(rect.bottom - rect.top) / scale;
    let center_delta = (i64::from(rect.left) + i64::from(rect.right) - i64::from(monitor.left) - i64::from(monitor.right)).abs() as f64 / 2.0 / scale;
    let bottom_gap = f64::from(monitor.bottom - rect.bottom) / scale;
    (100.0..=300.0).contains(&width) && (30.0..=120.0).contains(&height)
        && center_delta <= 150.0 && (0.0..=180.0).contains(&bottom_gap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_watch_starts_and_stops_cleanly() {
        let watch = NativeOsd::start(false).expect("native OSD worker should start");
        watch.set_enabled(false);
        drop(watch);
        assert!(!ENABLED.load(Ordering::Relaxed));
    }

    #[test]
    fn matches_scaled_level_hosts_on_each_monitor_without_accepting_other_windows() {
        for dpi in [96, 120, 144, 192] {
            let scale = dpi as i32;
            let monitor = RECT { left: 1920, top: 0, right: 3840, bottom: 1440 };
            let width = 234 * scale / 96;
            let height = 62 * scale / 96;
            let bottom = monitor.bottom - 48 * scale / 96;
            let left = (monitor.left + monitor.right - width) / 2;
            let host = RECT { left, top: bottom - height, right: left + width, bottom };
            assert!(level_host_geometry(host, monitor, dpi));
            let off_center = RECT { left: monitor.left, right: monitor.left + width, ..host };
            assert!(!level_host_geometry(off_center, monitor, dpi));
        }
    }
}
