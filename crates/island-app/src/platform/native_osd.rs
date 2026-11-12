//! Hides the shell's volume/brightness flyout while Islora shows their levels.
//! The shell creates the host lazily, so watching its show event also handles
//! the first key press after startup and an explorer restart.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_SHOW, FindWindowExW, GetClassNameW, GetShellWindow, GetWindowRect,
    GetWindowTextW, GetWindowThreadProcessId, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
    SHOW_WINDOW_CMD, ShowWindowAsync, WINEVENT_OUTOFCONTEXT,
};
use windows::core::{Error, Result, w};

static ENABLED: AtomicBool = AtomicBool::new(false);
pub struct NativeOsd {
    hook: HWINEVENTHOOK,
}

impl NativeOsd {
    pub fn start(enabled: bool) -> Result<Self> {
        ENABLED.store(enabled, Ordering::Relaxed);
        let hook = unsafe {
            SetWinEventHook(
                EVENT_OBJECT_SHOW,
                EVENT_OBJECT_SHOW,
                None,
                Some(on_show),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        if hook.is_invalid() {
            return Err(Error::from_thread());
        }
        Ok(Self { hook })
    }

    pub fn set_enabled(&self, enabled: bool) {
        ENABLED.store(enabled, Ordering::Relaxed);
    }
}

impl Drop for NativeOsd {
    fn drop(&mut self) {
        ENABLED.store(false, Ordering::Relaxed);
        unsafe {
            let _ = UnhookWinEvent(self.hook);
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
        if GetWindowRect(hwnd, &mut rect).is_err()
            || !(100..=300).contains(&(rect.right - rect.left))
            || !(30..=120).contains(&(rect.bottom - rect.top))
        {
            return;
        }
        // Toasts and other shell flyouts share this host class and bridge.
        // The level indicator is a small flyout at the bottom centre on the
        // supported layout. An unknown placement is safer left alone.
        let screen_width = GetSystemMetrics(SM_CXSCREEN);
        let screen_height = GetSystemMetrics(SM_CYSCREEN);
        if screen_width <= 0 || screen_height <= 0
            || ((rect.left + rect.right) / 2 - screen_width / 2).abs() > 150
            || rect.bottom < screen_height - 180
            || rect.bottom > screen_height
        {
            return;
        }
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
        let _ = ShowWindowAsync(hwnd, SHOW_WINDOW_CMD(0)); // SW_HIDE
    }
}
