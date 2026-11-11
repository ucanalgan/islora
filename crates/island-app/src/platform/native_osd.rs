//! Hides the shell's volume/brightness flyout while Islora shows their levels.
//! The shell creates the host lazily, so watching its show event also handles
//! the first key press after startup and an explorer restart.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_SHOW, FindWindowExW, GetClassNameW, GetParent, GetShellWindow, GetWindowRect,
    GetWindowTextW, GetWindowThreadProcessId, IsWindow, SHOW_WINDOW_CMD, ShowWindowAsync,
    WINEVENT_OUTOFCONTEXT,
};
use windows::core::{Error, Result, w};

static ENABLED: AtomicBool = AtomicBool::new(false);
static HIDDEN: Mutex<Vec<HiddenFlyout>> = Mutex::new(Vec::new());

#[derive(Clone, Copy)]
struct HiddenFlyout {
    host: usize,
    content: usize,
    shell_pid: u32,
}

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
        if !enabled {
            restore_hidden();
        }
    }
}

impl Drop for NativeOsd {
    fn drop(&mut self) {
        ENABLED.store(false, Ordering::Relaxed);
        unsafe {
            let _ = UnhookWinEvent(self.hook);
        }
        restore_hidden();
    }
}

/// A hidden shell window otherwise stays hidden after Islora quits or the user
/// turns either level notice off. Do not reuse handles after Explorer restarts.
fn restore_hidden() {
    let hidden = {
        let mut saved = HIDDEN.lock().unwrap_or_else(|error| error.into_inner());
        std::mem::take(&mut *saved)
    };
    for item in hidden {
        let host = HWND(item.host as *mut _);
        let content = HWND(item.content as *mut _);
        unsafe {
            if !IsWindow(Some(host)).as_bool() {
                continue;
            }
            let mut owner_pid = 0;
            GetWindowThreadProcessId(host, Some(&mut owner_pid));
            if owner_pid != item.shell_pid {
                continue;
            }
            if IsWindow(Some(content)).as_bool() && GetParent(content).ok() == Some(host) {
                let _ = ShowWindowAsync(content, SHOW_WINDOW_CMD(9)); // SW_RESTORE
            }
            let _ = ShowWindowAsync(host, SHOW_WINDOW_CMD(4)); // SW_SHOWNOACTIVATE
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
            || !(100..=650).contains(&(rect.right - rect.left))
            || !(30..=650).contains(&(rect.bottom - rect.top))
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
        let Ok(content) = content else { return };
        {
            let mut hidden = HIDDEN.lock().unwrap_or_else(|error| error.into_inner());
            if !hidden.iter().any(|saved| saved.host == hwnd.0 as usize && saved.shell_pid == shell_pid) {
                hidden.push(HiddenFlyout {
                    host: hwnd.0 as usize,
                    content: content.0 as usize,
                    shell_pid,
                });
            }
        }
        let _ = ShowWindowAsync(content, SHOW_WINDOW_CMD(6)); // SW_MINIMIZE
        let _ = ShowWindowAsync(hwnd, SHOW_WINDOW_CMD(0)); // SW_HIDE
    }
}
