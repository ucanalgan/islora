use std::mem::size_of;

use island_core::layout::{EDGE_TRIGGER_WIDTH, NOTCH_SHOULDER, WINDOW_HEIGHT, WINDOW_WIDTH};
use island_core::{IslandStyle, Pill};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    CombineRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, HRGN,
    MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MONITORINFOEXW, MonitorFromPoint, RGN_OR, SetWindowRgn,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PCWSTR, Result, w};

const ISLAND_CLASS: PCWSTR = w!("IsloraWindow");
const EDGE_CLASS: PCWSTR = w!("IsloraEdge");

/// Geometry of the island window on its current monitor.
#[derive(Clone, Copy)]
pub struct Placement {
    pub scale: f32,
    pub width: f32,
    pub edge: RECT,
    pub style: IslandStyle,
}

/// Borderless, always on top, never takes focus and stays out of the taskbar
/// and Alt+Tab. It has no redirection surface, so everything outside the
/// composition visuals is truly transparent.
pub fn create_island(wnd_proc: WNDPROC) -> Result<HWND> {
    register_class(ISLAND_CLASS, wnd_proc)?;
    let style = WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP;
    create(ISLAND_CLASS, style)
}

/// The island of a copy that is already running.
pub fn find_running() -> Option<HWND> {
    unsafe { FindWindowW(ISLAND_CLASS, None) }.ok()
}

/// Leaves the island out of screenshots, recordings and shared screens, where
/// it would only be in the way. What is behind it shows there instead.
pub fn exclude_from_capture(hwnd: HWND) {
    if let Err(error) = unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) } {
        eprintln!("capture exclusion failed: {error}");
    }
}

/// A one pixel strip along the top edge. Nearly transparent rather than fully,
/// because a fully transparent layered window receives no mouse input.
pub fn create_edge(wnd_proc: WNDPROC) -> Result<HWND> {
    register_class(EDGE_CLASS, wnd_proc)?;
    let style = WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_LAYERED;
    let hwnd = create(EDGE_CLASS, style)?;
    unsafe { SetLayeredWindowAttributes(hwnd, COLORREF(0), 1, LWA_ALPHA)? };
    Ok(hwnd)
}

fn register_class(name: PCWSTR, wnd_proc: WNDPROC) -> Result<()> {
    unsafe {
        let class = WNDCLASSW {
            lpfnWndProc: wnd_proc,
            hInstance: GetModuleHandleW(None)?.into(),
            lpszClassName: name,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        RegisterClassW(&class);
    }
    Ok(())
}

fn create(class: PCWSTR, style: WINDOW_EX_STYLE) -> Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        CreateWindowExW(
            style,
            class,
            w!("Islora"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }
}

#[derive(Clone, Debug)]
pub struct Monitor {
    pub device: String,
    pub rect: RECT,
    pub primary: bool,
}

pub fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn collect(handle: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let monitors = unsafe { &mut *(data.0 as *mut Vec<Monitor>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(handle, &mut info.monitorInfo) }.as_bool() {
            let end = info.szDevice.iter().position(|c| *c == 0).unwrap_or(info.szDevice.len());
            monitors.push(Monitor {
                device: String::from_utf16_lossy(&info.szDevice[..end]),
                rect: info.monitorInfo.rcMonitor,
                primary: info.monitorInfo.dwFlags & 1 != 0,
            });
        }
        BOOL(1)
    }
    let mut found = Vec::new();
    unsafe { let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut found as *mut _ as isize)); }
    found.sort_by(|a: &Monitor, b| b.primary.cmp(&a.primary).then(a.rect.left.cmp(&b.rect.left)).then(a.rect.top.cmp(&b.rect.top)));
    found
}

pub fn selected_monitor<'a>(available: &'a [Monitor], selected: Option<&str>) -> Option<&'a Monitor> {
    available.iter().find(|monitor| Some(monitor.device.as_str()) == selected)
        .or_else(|| available.iter().find(|monitor| monitor.primary))
        .or_else(|| available.first())
}

pub fn step_monitor<'a>(available: &'a [Monitor], selected: Option<&str>, next: bool) -> Option<&'a Monitor> {
    let current = selected_monitor(available, selected)?;
    let index = available.iter().position(|monitor| monitor.device == current.device)?;
    let index = if next { (index + 1) % available.len() } else { (index + available.len() - 1) % available.len() };
    available.get(index)
}

/// Centers both windows on the chosen monitor, falling back to the primary.
pub fn place(island: HWND, edge: HWND, style: IslandStyle, selected: Option<&str>) -> Result<Placement> {
    let available = monitors();
    let monitor = match selected_monitor(&available, selected) {
        Some(monitor) => monitor.rect,
        None => primary_monitor()?,
    };
    let monitor_width = monitor.right - monitor.left;
    // Move first so GetDpiForWindow reports the target monitor's DPI.
    unsafe { SetWindowPos(island, None, monitor.left, monitor.top, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER)?; }
    let scale = unsafe { GetDpiForWindow(island) } as f32 / 96.0;

    let width = (WINDOW_WIDTH * scale).round() as i32;
    let height = (WINDOW_HEIGHT * scale).round() as i32;
    let x = monitor.left + (monitor_width - width) / 2;

    let edge_width = (EDGE_TRIGGER_WIDTH * scale).round() as i32;
    let edge_x = monitor.left + (monitor_width - edge_width) / 2;
    let edge_rect = RECT {
        left: edge_x,
        top: monitor.top,
        right: edge_x + edge_width,
        bottom: monitor.top + 1,
    };

    unsafe {
        SetWindowPos(island, None, x, monitor.top, width, height, SWP_NOACTIVATE | SWP_NOZORDER)?;
        SetWindowPos(edge, None, edge_x, monitor.top, edge_width, 1, SWP_NOACTIVATE | SWP_NOZORDER)?;
    }
    Ok(Placement { scale, width: width as f32, edge: edge_rect, style })
}

fn primary_monitor() -> Result<RECT> {
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe {
        let monitor = MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY);
        GetMonitorInfoW(monitor, &mut info).ok()?;
    }
    Ok(info.rcMonitor)
}

pub fn show(hwnd: HWND) {
    to_top(hwnd, SWP_SHOWWINDOW);
}

/// Puts a shown window back above the other always-on-top windows. Recent
/// Windows 11 builds let a window that fills the screen cover them when it
/// comes to the front, as with a maximized window over a taskbar that hides.
pub fn raise(hwnd: HWND) {
    to_top(hwnd, SET_WINDOW_POS_FLAGS(0));
}

fn to_top(hwnd: HWND, extra: SET_WINDOW_POS_FLAGS) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | extra,
        );
    }
}

pub fn hide(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
}

/// Lets the whole window take input while the pill springs, so its overshoot
/// is not cut off by the region.
pub fn open_region(hwnd: HWND) {
    unsafe {
        SetWindowRgn(hwnd, None, false);
    }
}

/// Restricts input and drawing to the settled pill. Everything outside it
/// reaches the windows underneath.
pub fn set_pill_region(hwnd: HWND, pill: Pill, placement: &Placement) {
    let left = (placement.width - pill.width) / 2.0;
    unsafe {
        SetWindowRgn(hwnd, Some(pill_region(left, pill, placement)), false);
    }
}

/// The same, for a pill shifted left with a bubble beside it.
pub fn set_group_region(hwnd: HWND, pill: Pill, pill_left: f32, bubble: Pill, bubble_left: f32, placement: &Placement) {
    let bubble_top = placement.style.bubble_top(placement.scale, pill, bubble);
    unsafe {
        let group = pill_region(pill_left, pill, placement);
        let beside = match placement.style {
            IslandStyle::Island => rounded_region(bubble_left, bubble_top, bubble),
            IslandStyle::Notch => {
                let (top, tab) = bubble.as_notch(bubble.height);
                rounded_region(bubble_left, top, tab)
            }
        };
        CombineRgn(Some(group), Some(group), Some(beside), RGN_OR);
        let _ = DeleteObject(beside.into());
        SetWindowRgn(hwnd, Some(group), false);
    }
}

/// As a notch this covers the top row of the screen too, where a pointer thrown
/// upwards ends up. It also lies above the edge strip there, as the island is
/// always the last of the two to be shown. The region clips drawing as well, so
/// it has to take in the curves on both sides of the notch.
fn pill_region(left: f32, pill: Pill, placement: &Placement) -> HRGN {
    match placement.style {
        IslandStyle::Island => rounded_region(left, placement.style.margin() * placement.scale, pill),
        IslandStyle::Notch => unsafe {
            let (top, notch) = pill.as_notch(pill.height);
            let region = rounded_region(left, top, notch);
            let shoulder = (NOTCH_SHOULDER * placement.scale).ceil() as i32;
            let (from, to) = (left.floor() as i32 - shoulder, (left + pill.width).ceil() as i32 + shoulder);
            let curves = CreateRectRgn(from, 0, to, shoulder);
            CombineRgn(Some(region), Some(region), Some(curves), RGN_OR);
            let _ = DeleteObject(curves.into());
            region
        },
    }
}

fn rounded_region(left: f32, top: f32, pill: Pill) -> HRGN {
    let (x, y) = (left.floor() as i32, top.floor() as i32);
    let right = x + pill.width.ceil() as i32 + 1;
    let bottom = y + pill.height.ceil() as i32 + 1;
    let diameter = (pill.radius * 2.0).ceil() as i32;
    unsafe { CreateRoundRectRgn(x, y, right, bottom, diameter, diameter) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(device: &str, primary: bool, left: i32) -> Monitor {
        Monitor { device: device.into(), primary, rect: RECT { left, top: 0, right: left + 1920, bottom: 1080 } }
    }

    #[test]
    fn selection_survives_reordering_and_falls_back_while_disconnected() {
        let both = vec![monitor("primary", true, 0), monitor("external", false, -1920)];
        assert_eq!(selected_monitor(&both, Some("external")).unwrap().rect.left, -1920);
        let disconnected = vec![both[0].clone()];
        assert_eq!(selected_monitor(&disconnected, Some("external")).unwrap().device, "primary");
        let reconnected = vec![both[1].clone(), both[0].clone()];
        assert_eq!(selected_monitor(&reconnected, Some("external")).unwrap().device, "external");
        assert_eq!(step_monitor(&both, Some("primary"), true).unwrap().device, "external");
        assert_eq!(step_monitor(&both, Some("primary"), false).unwrap().device, "external");
        assert!(step_monitor(&[], None, true).is_none());
    }
}
