use std::mem::size_of;

use island_core::layout::{EDGE_TRIGGER_WIDTH, NOTCH_SHOULDER, WINDOW_HEIGHT, WINDOW_WIDTH};
use island_core::{IslandStyle, Pill};
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    CombineRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject, GetMonitorInfoW, HRGN,
    MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint, RGN_OR, SetWindowRgn,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, Result, w};

const ISLAND_CLASS: PCWSTR = w!("DynamicIslandWindow");
const EDGE_CLASS: PCWSTR = w!("DynamicIslandEdge");

/// Geometry of the island window on the primary monitor.
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
            w!("Dynamic Island"),
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

/// Centers both windows on the primary monitor's top edge.
pub fn place(island: HWND, edge: HWND, style: IslandStyle) -> Result<Placement> {
    let monitor = primary_monitor()?;
    let monitor_width = monitor.right - monitor.left;
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
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
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
