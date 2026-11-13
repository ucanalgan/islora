//! Selects an already open WhatsApp Web tab for its Chrome notification.

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::System::Variant::{VARIANT, VT_I4};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationSelectionItemPattern, TreeScope_Descendants,
    UIA_ControlTypePropertyId, UIA_SelectionItemPatternId, UIA_TabItemControlTypeId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, SW_RESTORE, SetForegroundWindow, ShowWindow,
};
use windows::core::{BOOL, PWSTR, Result};

pub fn is_whatsapp_notice(app_id: &str, title: &str) -> bool {
    app_id.eq_ignore_ascii_case("chrome") && title.trim().eq_ignore_ascii_case("whatsapp")
}

fn is_whatsapp_tab(title: &str) -> bool {
    let mut title = title.trim();
    if let Some(rest) = title.strip_prefix('(')
        && let Some((count, rest)) = rest.split_once(')')
        && !count.is_empty()
        && count.chars().all(|c| c.is_ascii_digit())
    {
        title = rest.trim_start();
    }
    title.eq_ignore_ascii_case("whatsapp")
        || title
            .get(..9)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("whatsapp "))
}

/// Called on the shell worker's COM thread. Returns false if no tab can be selected.
pub fn open_whatsapp_tab() -> bool {
    unsafe { find_and_select().unwrap_or(false) }
}

unsafe fn find_and_select() -> Result<bool> {
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)? };
    let mut value = VARIANT::default();
    unsafe {
        (*value.Anonymous.Anonymous).vt = VT_I4;
        (*value.Anonymous.Anonymous).Anonymous.lVal = UIA_TabItemControlTypeId.0;
    }
    let condition =
        unsafe { automation.CreatePropertyCondition(UIA_ControlTypePropertyId, &value)? };
    let mut windows: Vec<HWND> = Vec::new();
    unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
        let mut class = [0u16; 64];
        let len = unsafe { GetClassNameW(hwnd, &mut class) }.max(0) as usize;
        if String::from_utf16_lossy(&class[..len]) == "Chrome_WidgetWin_1"
            && unsafe { IsWindowVisible(hwnd) }.as_bool()
            && unsafe { is_chrome(hwnd) }
        {
            unsafe { &mut *(data.0 as *mut Vec<HWND>) }.push(hwnd);
        }
        BOOL(1)
    }
    unsafe {
        EnumWindows(Some(collect), LPARAM(&mut windows as *mut _ as isize))?;
    }
    // Prefer the current Chrome window when it has a matching tab.
    let foreground = unsafe { GetForegroundWindow() };
    windows.sort_by_key(|window| *window != foreground);
    for hwnd in windows {
        let Ok(root) = (unsafe { automation.ElementFromHandle(hwnd) }) else {
            continue;
        };
        let Ok(tabs) = (unsafe { root.FindAll(TreeScope_Descendants, &condition) }) else {
            continue;
        };
        for index in 0..unsafe { tabs.Length()? } {
            let Ok(tab) = (unsafe { tabs.GetElement(index) }) else {
                continue;
            };
            let Ok(name) = (unsafe { tab.CurrentName() }) else {
                continue;
            };
            if !is_whatsapp_tab(&name.to_string()) {
                continue;
            }
            let Ok(selection) = (unsafe {
                tab.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                    UIA_SelectionItemPatternId,
                )
            }) else {
                continue;
            };
            if unsafe { IsIconic(hwnd) }.as_bool() {
                let _ = unsafe { ShowWindow(hwnd, SW_RESTORE) };
            }
            if unsafe { selection.Select() }.is_ok() {
                let _ = unsafe { SetForegroundWindow(hwnd) };
                return Ok(true);
            }
        }
    }
    Ok(false)
}

unsafe fn is_chrome(hwnd: HWND) -> bool {
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
    else {
        return false;
    };
    let mut path = [0u16; 1024];
    let mut len = path.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_FORMAT(0),
            PWSTR(path.as_mut_ptr()),
            &mut len,
        )
    };
    let _ = unsafe { CloseHandle(process) };
    result.is_ok()
        && String::from_utf16_lossy(&path[..len as usize])
            .rsplit('\\')
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("chrome.exe"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_whatsapp_chrome_notices_use_tab_selection() {
        assert!(is_whatsapp_notice("Chrome", "WhatsApp"));
        assert!(!is_whatsapp_notice("Chrome", "Other app"));
        assert!(!is_whatsapp_notice("Other", "WhatsApp"));
        assert!(is_whatsapp_tab("(2) WhatsApp - Google Chrome"));
        assert!(is_whatsapp_tab("WhatsApp"));
        assert!(!is_whatsapp_tab("Search for WhatsApp"));
    }
}
