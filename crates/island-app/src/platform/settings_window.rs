//! The settings window: an ordinary window to move, minimize and close, unlike
//! the island. It exists only while open, so it costs nothing the rest of the time.
//! It paints on the CPU, as a few dozen shapes on demand never need a GPU.

use std::mem::size_of;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, Instant};

use island_core::{IslandStyle, Material, ProgressStyle, TaskLayout};
use island_core::settings_view::{
    ADDRESS_FIELD, Action, Control, Focus, Look, PREVIEW, Page, Rect, SIDEBAR_WIDTH, SettingsState,
    View, WINDOW_HEIGHT, WINDOW_WIDTH, view,
};
use windows::Win32::Foundation::{COLORREF, D2DERR_RECREATE_TARGET, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE,
    D2D1CreateFactory, ID2D1DeviceContext, ID2D1Factory1, ID2D1HwndRenderTarget,
};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, EndPaint, GetMonitorInfoW, HBRUSH,
    HDC, HFONT, InvalidateRect, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
    PAINTSTRUCT, SetBkColor, SetTextColor,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VIRTUAL_KEY, VK_DOWN, VK_ESCAPE, VK_LEFT,
    VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Error, HSTRING, Interface, PCWSTR, Result, w};

use crate::render::{
    Align, BarRun, Bounds, Font, OutlineRun, Painter, Painting, RingRun, ShapeRun, TextRun,
};

const CLASS: PCWSTR = w!("IsloraSettings");
/// Sent to a running island by a second start of the app.
pub const MSG_OPEN_SETTINGS: u32 = WM_APP + 17;
/// Posted to the island, as a closing window cannot touch the app that owns it.
pub const MSG_SETTINGS_CLOSED: u32 = WM_APP + 18;

/// Resource number from app.rc.
const APP_ICON: usize = 1;
const STYLE: WINDOW_STYLE = WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0);
const FIELD_INSET: f32 = 12.0;
const FIELD_TEXT_HEIGHT: f32 = 20.0;
const FIELD_FONT_SIZE: f32 = 14.0;

const BACKGROUND: [f32; 4] = [0.078, 0.078, 0.094, 1.0];
const SIDEBAR: [f32; 4] = [0.055, 0.055, 0.071, 1.0];
const SURFACE: [f32; 4] = [0.122, 0.122, 0.149, 1.0];
const HAIRLINE: [f32; 4] = [1.0, 1.0, 1.0, 0.08];
const ACCENT: [f32; 4] = [1.0, 0.624, 0.039, 1.0];
const SWITCH_OFF: [f32; 4] = [0.267, 0.267, 0.306, 1.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const FIELD_COLOR: COLORREF = COLORREF(0x0026_1F1F);
const FIELD_TEXT: COLORREF = COLORREF(0x00FF_FFFF);

const ICON_MUSIC: &str = "\u{E8D6}";

static OWNER: AtomicIsize = AtomicIsize::new(0);
/// The address box, whose keys the message loop has to look at first.
static FIELD: AtomicIsize = AtomicIsize::new(0);

pub struct SettingsWindow {
    hwnd: HWND,
    target: ID2D1HwndRenderTarget,
    context: ID2D1DeviceContext,
    address: HWND,
    address_font: HFONT,
    field_brush: HBRUSH,
    scale: f32,
    state: SettingsState,
    view: View,
    hovered: Option<Action>,
    tracking_pointer: bool,
    focus: Option<Focus>,
    /// The focus is outlined only once the keyboard is in use, as in Windows.
    ring: bool,
    /// The preview's shapes for the choices in effect.
    shapes: Vec<Shape>,
    /// The shapes it is moving from, and since when, after a change.
    transition: Option<(Vec<Shape>, Instant)>,
}

impl SettingsWindow {
    /// `hidden_away` keeps it off every screen and out of focus, for tests.
    pub fn open(
        owner: HWND,
        wnd_proc: WNDPROC,
        state: SettingsState,
        address: &str,
        hidden_away: bool,
    ) -> Result<Self> {
        OWNER.store(owner.0 as isize, Ordering::Relaxed);
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = WNDCLASSW {
                lpfnWndProc: wnd_proc,
                hInstance: instance.into(),
                lpszClassName: CLASS,
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hIcon: LoadIconW(Some(instance.into()), PCWSTR(std::ptr::without_provenance(APP_ICON)))?,
                ..Default::default()
            };
            // Fails only when the window was opened before, which leaves the class.
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                CLASS,
                w!("Islora Ayarları"),
                STYLE,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(instance.into()),
                None,
            )?;
            let dark: i32 = 1;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                (&raw const dark).cast(),
                size_of::<i32>() as u32,
            );
            let dpi = GetDpiForWindow(hwnd);
            let scale = dpi as f32 / 96.0;
            let (width, height) = client_size(scale);
            let mut frame = RECT { left: 0, top: 0, right: width as i32, bottom: height as i32 };
            AdjustWindowRectExForDpi(&mut frame, STYLE, false, WINDOW_EX_STYLE::default(), dpi)?;
            let (frame_width, frame_height) = (frame.right - frame.left, frame.bottom - frame.top);
            let (x, y) = if hidden_away { (-8000, 100) } else { centered(frame_width, frame_height)? };
            SetWindowPos(hwnd, None, x, y, frame_width, frame_height, SWP_NOZORDER | SWP_NOACTIVATE)?;

            let target = create_target(hwnd, width, height)?;
            let context = target.cast()?;
            let address = create_address_field(hwnd, instance.into(), address)?;
            FIELD.store(address.0 as isize, Ordering::Relaxed);
            let field_brush = CreateSolidBrush(FIELD_COLOR);
            let mut window = Self {
                hwnd,
                target,
                context,
                address,
                address_font: HFONT::default(),
                field_brush,
                scale,
                view: view(&state),
                shapes: preview_shapes(&state),
                state,
                hovered: None,
                tracking_pointer: false,
                focus: None,
                ring: false,
                transition: None,
            };
            window.fit_address_field();
            window.show_address_on_its_page();
            let show = if hidden_away { SW_SHOWNOACTIVATE } else { SW_SHOW };
            let _ = ShowWindow(hwnd, show);
            if !hidden_away {
                // STARTUPINFO may override the first ShowWindow call (for
                // example when a background launcher starts us hidden).
                // Ensure the settings window is actually visible.
                if !IsWindowVisible(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_SHOW);
                }
                let _ = SetForegroundWindow(hwnd);
            }
            Ok(window)
        }
    }

    pub fn bring_to_front(&self) {
        unsafe {
            if IsIconic(self.hwnd).as_bool() {
                let _ = ShowWindow(self.hwnd, SW_RESTORE);
            } else if !IsWindowVisible(self.hwnd).as_bool() {
                let _ = ShowWindow(self.hwnd, SW_SHOW);
            }
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    pub fn state(&self) -> &SettingsState {
        &self.state
    }

    /// One step of the preview's move to a new choice. The timer stops with it.
    pub fn on_timer(&mut self) {
        let done = self.transition.as_ref().is_none_or(|(_, started)| started.elapsed() >= TRANSITION);
        if done {
            self.transition = None;
            unsafe {
                let _ = KillTimer(Some(self.hwnd), PREVIEW_TIMER);
            }
        }
        self.repaint();
    }

    pub fn update(&mut self, state: SettingsState) {
        self.view = view(&state);
        let shapes = preview_shapes(&state);
        // A new choice on the same page moves the preview there. Another page
        // simply shows its own.
        if state.page == self.state.page && shapes != self.shapes && !self.shapes.is_empty() {
            let from = std::mem::replace(&mut self.shapes, shapes);
            self.transition = Some((from, Instant::now()));
            unsafe { SetTimer(Some(self.hwnd), PREVIEW_TIMER, TRANSITION_TICK_MS, None) };
        } else {
            self.shapes = shapes;
            self.transition = None;
        }
        self.state = state;
        // A page change can take away what the keyboard was on.
        if self.focus.is_some_and(|focus| !self.view.focus_order(self.state.page).contains(&focus)) {
            self.focus = None;
        }
        self.show_address_on_its_page();
        self.repaint();
    }

    /// Tab and the arrows move the focus, and Enter or Space press what it is on.
    /// Returns the press for the app to carry out.
    pub fn on_key(&mut self, key: VIRTUAL_KEY, shift: bool) -> Option<Action> {
        match key {
            VK_TAB => self.move_focus(!shift),
            VK_DOWN | VK_RIGHT => self.move_focus(true),
            VK_UP | VK_LEFT => self.move_focus(false),
            VK_RETURN | VK_SPACE => {
                return match self.focus {
                    Some(Focus::Control(action)) => Some(action),
                    Some(Focus::Address) => Some(Action::SaveCalendar),
                    None => None,
                };
            }
            _ => {}
        }
        None
    }

    fn move_focus(&mut self, forward: bool) {
        self.focus = self.view.step_focus(self.state.page, self.focus, forward);
        self.ring = true;
        let target = if self.focus == Some(Focus::Address) { self.address } else { self.hwnd };
        unsafe {
            let _ = SetFocus(Some(target));
        }
        self.repaint();
    }

    /// A click moves the focus too, without outlining it.
    pub fn focus_on(&mut self, action: Action) {
        self.focus = Some(Focus::Control(action));
        self.ring = false;
    }

    pub fn pointer_on_control(&self) -> bool {
        let mut pointer = POINT::default();
        unsafe {
            if GetCursorPos(&mut pointer).is_err() || !ScreenToClient(self.hwnd, &mut pointer).as_bool() {
                return false;
            }
        }
        self.action_at(pointer.x, pointer.y).is_some()
    }

    fn show_address_on_its_page(&self) {
        let shown = if self.state.page == Page::Calendar { SW_SHOWNA } else { SW_HIDE };
        unsafe {
            let _ = ShowWindow(self.address, shown);
        }
    }

    pub fn address(&self) -> String {
        unsafe {
            let length = GetWindowTextLengthW(self.address).max(0) as usize;
            let mut text = vec![0_u16; length + 1];
            let copied = GetWindowTextW(self.address, &mut text).max(0) as usize;
            String::from_utf16_lossy(&text[..copied]).trim().to_owned()
        }
    }

    pub fn action_at(&self, x: i32, y: i32) -> Option<Action> {
        self.view.action_at(x as f32 / self.scale, y as f32 / self.scale)
    }

    pub fn on_pointer(&mut self, x: i32, y: i32) {
        if !self.tracking_pointer {
            let mut leave = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            self.tracking_pointer = unsafe { TrackMouseEvent(&mut leave) }.is_ok();
        }
        self.set_hovered(self.action_at(x, y));
    }

    pub fn on_pointer_left(&mut self) {
        self.tracking_pointer = false;
        self.set_hovered(None);
    }

    fn set_hovered(&mut self, action: Option<Action>) {
        if action != self.hovered {
            self.hovered = action;
            self.repaint();
        }
    }

    /// Takes the keyboard from the address box, so that a click outside it counts.
    pub fn take_focus(&self) {
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(self.hwnd));
        }
    }

    /// The colours of the address box, which Windows asks for on every paint of it.
    pub fn color_field(&self, dc: HDC) -> LRESULT {
        unsafe {
            SetTextColor(dc, FIELD_TEXT);
            SetBkColor(dc, FIELD_COLOR);
        }
        LRESULT(self.field_brush.0 as isize)
    }

    pub fn on_dpi_changed(&mut self, dpi: u32, suggested: &RECT) {
        self.scale = dpi as f32 / 96.0;
        let (width, height) = client_size(self.scale);
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                None,
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            let _ = self.target.Resize(&D2D_SIZE_U { width, height });
        }
        self.fit_address_field();
        self.repaint();
    }

    /// Sizes the address box to the window's scale, with a font to match.
    fn fit_address_field(&mut self) {
        let scale = self.scale;
        unsafe {
            let font = CreateFontW(
                -((FIELD_FONT_SIZE * scale).round() as i32),
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                0,
                w!("Segoe UI Variable Text"),
            );
            SendMessageW(self.address, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
            if !self.address_font.is_invalid() {
                let _ = DeleteObject(self.address_font.into());
            }
            self.address_font = font;
            let field = ADDRESS_FIELD;
            let _ = SetWindowPos(
                self.address,
                None,
                ((field.x + FIELD_INSET) * scale) as i32,
                ((field.y + (field.height - FIELD_TEXT_HEIGHT) / 2.0) * scale) as i32,
                ((field.width - FIELD_INSET * 2.0) * scale) as i32,
                (FIELD_TEXT_HEIGHT * scale) as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    fn repaint(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    pub fn paint(&mut self, painter: &Painter) {
        let mut paint = PAINTSTRUCT::default();
        unsafe {
            BeginPaint(self.hwnd, &mut paint);
            let [r, g, b, a] = BACKGROUND;
            self.target.BeginDraw();
            self.target.Clear(Some(&D2D1_COLOR_F { r, g, b, a }));
            if let Err(error) = painter.draw_on(&self.context, &self.painting()) {
                eprintln!("settings paint failed: {error}");
            }
            let ended = self.target.EndDraw(None, None);
            let _ = EndPaint(self.hwnd, &paint);
            if ended.as_ref().err().map(Error::code) == Some(D2DERR_RECREATE_TARGET) {
                self.recreate_target();
            }
        }
    }

    fn recreate_target(&mut self) {
        let (width, height) = client_size(self.scale);
        let Ok(target) = create_target(self.hwnd, width, height) else { return };
        let Ok(context) = target.cast() else { return };
        self.target = target;
        self.context = context;
        self.repaint();
    }

    fn painting(&self) -> Painting<'_> {
        let mut painting = Painting::default();
        painting.shapes.push(ShapeRun {
            bounds: self.bounds(Rect { x: 0.0, y: 0.0, width: SIDEBAR_WIDTH, height: WINDOW_HEIGHT }),
            radius: 0.0,
            fill: SIDEBAR,
            border: None,
        });
        match self.state.page {
            page if page.has_preview() => self.preview(&mut painting),
            Page::Calendar => painting.shapes.push(ShapeRun {
                bounds: self.bounds(ADDRESS_FIELD),
                radius: 10.0 * self.scale,
                fill: SURFACE,
                border: Some((HAIRLINE, self.scale)),
            }),
            _ => {}
        }
        for control in &self.view.controls {
            self.control(&mut painting, control);
        }
        let focused = match self.focus {
            Some(Focus::Control(action)) => self.view.bounds_of(action),
            Some(Focus::Address) => Some(ADDRESS_FIELD),
            None => None,
        };
        if let Some(rect) = focused.filter(|_| self.ring) {
            const GAP: f32 = 3.0;
            let around = Rect { x: rect.x - GAP, y: rect.y - GAP, width: rect.width + GAP * 2.0, height: rect.height + GAP * 2.0 };
            painting.shapes.push(ShapeRun {
                bounds: self.bounds(around),
                radius: 12.0 * self.scale,
                fill: [0.0; 4],
                border: Some((WHITE, 2.0 * self.scale)),
            });
        }
        for label in &self.view.labels {
            painting.texts.push(TextRun {
                text: &label.text,
                bounds: self.bounds(label.bounds),
                size: label.size * self.scale,
                bold: label.bold,
                opacity: label.opacity,
                align: if label.centered { Align::Center } else { Align::Leading },
                font: Font::Text,
                even_digits: false,
            });
        }
        painting
    }

    fn bounds(&self, rect: Rect) -> Bounds {
        let scale = self.scale;
        Bounds { x: rect.x * scale, y: rect.y * scale, width: rect.width * scale, height: rect.height * scale }
    }

    fn control<'a>(&self, painting: &mut Painting<'a>, control: &'a Control) {
        let bounds = self.bounds(control.bounds);
        let hovered = self.hovered == Some(control.action);
        let radius = 10.0 * self.scale;
        let text = |bounds, size, bold, opacity, align| TextRun {
            text: &control.label,
            bounds,
            size: size * self.scale,
            bold,
            opacity,
            align,
            font: Font::Text,
            even_digits: false,
        };
        match control.look {
            Look::Tab { selected } => {
                let wash = if selected { 0.11 } else if hovered { 0.05 } else { 0.0 };
                painting.shapes.push(ShapeRun { bounds, radius, fill: [1.0, 1.0, 1.0, wash], border: None });
                let inset = Bounds { x: bounds.x + 12.0 * self.scale, width: bounds.width - 12.0 * self.scale, ..bounds };
                painting.texts.push(text(inset, 14.0, selected, if selected { 1.0 } else { 0.75 }, Align::Leading));
            }
            Look::Choice { selected } => {
                let border = if selected { (ACCENT, 2.0 * self.scale) } else { (HAIRLINE, self.scale) };
                painting.shapes.push(ShapeRun { bounds, radius, fill: self.surface(hovered), border: Some(border) });
                painting.texts.push(text(bounds, 14.0, true, 1.0, Align::Center));
            }
            Look::Switch { on } => {
                painting.shapes.push(ShapeRun { bounds, radius, fill: self.surface(hovered), border: None });
                let inset = 14.0 * self.scale;
                let label = Bounds { x: bounds.x + inset, width: bounds.width - inset * 2.0 - 40.0 * self.scale, ..bounds };
                painting.texts.push(text(label, 14.0, false, 1.0, Align::Leading));
                self.switch(painting, bounds, on);
            }
            Look::Button => {
                let wash = if hovered { 0.17 } else { 0.11 };
                painting.shapes.push(ShapeRun { bounds, radius, fill: [1.0, 1.0, 1.0, wash], border: None });
                painting.texts.push(text(bounds, 14.0, false, 1.0, Align::Center));
            }
            Look::Step => {
                let wash = if hovered { 0.17 } else { 0.11 };
                painting.shapes.push(ShapeRun { bounds, radius: 8.0 * self.scale, fill: [1.0, 1.0, 1.0, wash], border: None });
                painting.texts.push(text(bounds, 17.0, false, 1.0, Align::Center));
            }
        }
    }

    fn surface(&self, hovered: bool) -> [f32; 4] {
        if hovered { [0.157, 0.157, 0.192, 1.0] } else { SURFACE }
    }

    fn switch(&self, painting: &mut Painting<'_>, row: Bounds, on: bool) {
        let scale = self.scale;
        let (width, height, knob) = (38.0 * scale, 22.0 * scale, 16.0 * scale);
        let track = Bounds {
            x: row.x + row.width - width - 14.0 * scale,
            y: row.y + (row.height - height) / 2.0,
            width,
            height,
        };
        painting.shapes.push(ShapeRun {
            bounds: track,
            radius: height / 2.0,
            fill: if on { ACCENT } else { SWITCH_OFF },
            border: None,
        });
        let gap = (height - knob) / 2.0;
        let x = if on { track.x + width - knob - gap } else { track.x + gap };
        painting.shapes.push(ShapeRun {
            bounds: Bounds { x, y: track.y + gap, width: knob, height: knob },
            radius: knob / 2.0,
            fill: WHITE,
            border: None,
        });
    }

    /// The small scene with the island on it, in the look chosen on the page.
    /// After a change its shapes move from where they were for a moment, and
    /// what they hold fades across.
    fn preview(&self, painting: &mut Painting<'_>) {
        let scene = PREVIEW;
        let scale = self.scale;
        let place = |x: f32, y: f32, width: f32, height: f32| self.bounds(Rect { x: scene.x + x, y: scene.y + y, width, height });
        let plain = |bounds: Bounds, radius: f32, fill: [f32; 4]| ShapeRun { bounds, radius: radius * scale, fill, border: None };
        painting.shapes.push(plain(place(0.0, 0.0, scene.width, scene.height), 12.0, [0.204, 0.314, 0.486, 1.0]));
        painting.shapes.push(plain(place(40.0, 26.0, 60.0, 60.0), 30.0, [0.91, 0.725, 0.227, 1.0]));
        painting.shapes.push(plain(place(scene.width - 120.0, 10.0, 76.0, 76.0), 38.0, [0.184, 0.659, 0.604, 1.0]));

        let progress = self.transition.as_ref().map(|(from, started)| (from, ease(started.elapsed().as_secs_f32() / TRANSITION.as_secs_f32())));
        for part in Part::ALL {
            let to = self.shapes.iter().find(|shape| shape.part == part);
            match progress {
                Some((from, t)) => {
                    let from = from.iter().find(|shape| shape.part == part);
                    match (from, to) {
                        (Some(a), Some(b)) => {
                            let outline = a.outline().blend(&b.outline(), t);
                            if a.marks == b.marks {
                                self.draw_part(painting, &outline, &b.marks, 1.0, 1.0);
                            } else {
                                self.draw_part(painting, &outline, &a.marks, 1.0, (1.0 - t * 1.6).max(0.0));
                                self.draw_part(painting, &outline, &b.marks, 0.0, ((t - 0.3) / 0.7).clamp(0.0, 1.0));
                            }
                        }
                        (Some(a), None) => self.draw_part(painting, &a.outline().scaled(1.0 - 0.4 * t), &a.marks, 1.0 - t, 1.0 - t),
                        (None, Some(b)) => self.draw_part(painting, &b.outline().scaled(0.6 + 0.4 * t), &b.marks, t, t),
                        (None, None) => {}
                    }
                }
                None => {
                    if let Some(shape) = to {
                        self.draw_part(painting, &shape.outline(), &shape.marks, 1.0, 1.0);
                    }
                }
            }
        }
        // A notch starts above the scene, which is then painted over, so that
        // its top corners stay square against the edge.
        painting.shapes.push(plain(place(0.0, -NOTCH_REACH - 2.0, scene.width, NOTCH_REACH + 2.0), 0.0, BACKGROUND));
    }

    /// One shape at `body` opacity, and what it holds at `marks` opacity. Only
    /// the first drawing of a shape paints its body.
    fn draw_part(&self, painting: &mut Painting<'_>, outline: &Outline, marks: &[Mark], body: f32, opacity: f32) {
        let scale = self.scale;
        let scene = PREVIEW;
        let at = outline.at;
        let place = |x: f32, y: f32, width: f32, height: f32| self.bounds(Rect { x: scene.x + at.x + x, y: scene.y + at.y + y, width, height });
        let shape_bounds = place(0.0, -outline.above, at.width, at.height + outline.above);
        let radius = outline.radius * scale;
        if body > 0.0 {
            let fade = |color: [f32; 4]| [color[0], color[1], color[2], color[3] * body];
            let border = (outline.border[3] > 0.0).then(|| (fade(outline.border), scale));
            painting.shapes.push(ShapeRun { bounds: shape_bounds, radius, fill: fade(outline.fill), border });
        }
        if opacity <= 0.0 {
            return;
        }
        let green = |alpha: f32| [GREEN[0], GREEN[1], GREEN[2], alpha * opacity];
        for mark in marks {
            match mark {
                Mark::Text { text, at: spot, size, bold, opacity: own, centered, icon, tinted } => {
                    let run = TextRun {
                        text,
                        bounds: place(spot.x, spot.y, spot.width, spot.height),
                        size: size * scale,
                        bold: *bold,
                        opacity: own * opacity,
                        align: if *centered { Align::Center } else { Align::Leading },
                        font: if *icon { Font::Icons } else { Font::Text },
                        even_digits: false,
                    };
                    if *tinted {
                        painting.colored.push((run, GREEN));
                    } else {
                        painting.texts.push(run);
                    }
                }
                Mark::Bar { at: spot, opacity: own } => {
                    painting.bars.push(BarRun { bounds: place(spot.x, spot.y, spot.width, spot.height), opacity: own * opacity });
                }
                Mark::Ring { x, y, radius: ring, width, fraction } => {
                    let point = place(*x, *y, 0.0, 0.0);
                    painting.rings.push(RingRun {
                        center_x: point.x,
                        center_y: point.y,
                        radius: ring * scale,
                        width: width * scale,
                        fraction: *fraction,
                        opacity,
                    });
                }
                Mark::Edge { fraction } => {
                    let whole = place(0.0, 0.0, at.width, at.height);
                    let open_top = outline.above > 0.0;
                    painting.outlines.push(OutlineRun { bounds: whole, radius, width: 5.0 * scale, color: green(0.25), fraction: *fraction, dash: None, open_top });
                    painting.outlines.push(OutlineRun { bounds: whole, radius, width: 2.0 * scale, color: green(1.0), fraction: *fraction, dash: None, open_top });
                }
                Mark::Fill { fraction } => {
                    let reach = at.width * fraction;
                    let filled = ShapeRun { bounds: place(0.0, 0.0, reach, at.height), radius: 0.0, fill: green(0.26), border: None };
                    let front = ShapeRun { bounds: place(reach - 2.0, 0.0, 2.0, at.height), radius: 0.0, fill: green(0.8), border: None };
                    painting.clipped.push((filled, shape_bounds, radius));
                    painting.clipped.push((front, shape_bounds, radius));
                }
            }
        }
    }
}

impl Drop for SettingsWindow {
    fn drop(&mut self) {
        unsafe {
            if IsWindow(Some(self.hwnd)).as_bool() {
                let _ = DestroyWindow(self.hwnd);
            }
            let _ = DeleteObject(self.address_font.into());
            let _ = DeleteObject(self.field_brush.into());
        }
    }
}

/// The keys that the address box would otherwise swallow or beep at.
pub fn is_dialog_key(message: &MSG) -> bool {
    let field = FIELD.load(Ordering::Relaxed);
    field != 0
        && message.hwnd.0 as isize == field
        && message.message == WM_KEYDOWN
        && [VK_TAB, VK_RETURN, VK_ESCAPE].iter().any(|key| key.0 as usize == message.wParam.0)
}

/// Tells the app the window is gone. Called while it is being destroyed.
pub fn notify_closed() {
    FIELD.store(0, Ordering::Relaxed);
    let owner = HWND(OWNER.load(Ordering::Relaxed) as _);
    unsafe {
        let _ = PostMessageW(Some(owner), MSG_SETTINGS_CLOSED, WPARAM(0), LPARAM(0));
    }
}

fn client_size(scale: f32) -> (u32, u32) {
    ((WINDOW_WIDTH * scale).round() as u32, (WINDOW_HEIGHT * scale).round() as u32)
}

fn centered(width: i32, height: i32) -> Result<(i32, i32)> {
    let mut monitor = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe {
        let primary = MonitorFromPoint(Default::default(), MONITOR_DEFAULTTOPRIMARY);
        GetMonitorInfoW(primary, &mut monitor).ok()?;
    }
    let area = monitor.rcWork;
    Ok((area.left + (area.right - area.left - width) / 2, area.top + (area.bottom - area.top - height) / 2))
}

fn create_target(hwnd: HWND, width: u32, height: u32) -> Result<ID2D1HwndRenderTarget> {
    let properties = D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_IGNORE,
        },
        // Everything is sized in real pixels already.
        dpiX: 96.0,
        dpiY: 96.0,
        ..Default::default()
    };
    let window = D2D1_HWND_RENDER_TARGET_PROPERTIES {
        hwnd,
        pixelSize: D2D_SIZE_U { width, height },
        presentOptions: D2D1_PRESENT_OPTIONS_NONE,
    };
    unsafe {
        let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        factory.CreateHwndRenderTarget(&properties, &window)
    }
}

/// The address is a secret, so the box shows dots, which also stops it being copied.
fn create_address_field(parent: HWND, instance: windows::Win32::Foundation::HINSTANCE, address: &str) -> Result<HWND> {
    let style = WS_CHILD.0 | ES_AUTOHSCROLL as u32 | ES_PASSWORD as u32;
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("EDIT"),
            &HSTRING::from(address),
            WINDOW_STYLE(style),
            0,
            0,
            1,
            1,
            Some(parent),
            None,
            Some(instance),
            None,
        )
    }
}

/// How long the preview takes to move to a new choice.
const TRANSITION: Duration = Duration::from_millis(260);
/// How often it repaints meanwhile, which is all the time it costs.
const TRANSITION_TICK_MS: u32 = 15;
const PREVIEW_TIMER: usize = 1;
/// How far a notch reaches up past the top of the scene.
const NOTCH_REACH: f32 = 16.0;
const GREEN: [f32; 3] = [0.25, 0.82, 0.48];
/// How far the send in the picture has got.
const SENT: f32 = 0.64;
const PILL_WIDTH: f32 = 176.0;
const PILL_HEIGHT: f32 = 36.0;
const BUBBLE_SIZE: f32 = 36.0;
const BUBBLE_GAP: f32 = 8.0;
const CARD_WIDTH: f32 = 184.0;
const CARD_HEIGHT: f32 = 64.0;
const GROUP_GAP: f32 = 22.0;

/// Starts quickly and settles, as the island's own springs do.
fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_color(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t), lerp(a[3], b[3], t)]
}

/// The shapes of the preview, which keep who they are across a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Pill,
    Bubble,
    Card,
}

impl Part {
    const ALL: [Part; 3] = [Part::Pill, Part::Bubble, Part::Card];
}

/// Something drawn inside a shape, placed from its top left, in DIPs.
#[derive(Clone, Debug, PartialEq)]
enum Mark {
    Text { text: &'static str, at: Rect, size: f32, bold: bool, opacity: f32, centered: bool, icon: bool, tinted: bool },
    Bar { at: Rect, opacity: f32 },
    Ring { x: f32, y: f32, radius: f32, width: f32, fraction: f32 },
    /// Light along the edge of the shape, as far as the send got.
    Edge { fraction: f32 },
    /// The shape filled from the left, as far as the send got.
    Fill { fraction: f32 },
}

impl Mark {
    fn text(text: &'static str, at: Rect, size: f32, bold: bool) -> Mark {
        Mark::Text { text, at, size, bold, opacity: 1.0, centered: false, icon: false, tinted: false }
    }

    fn icon(glyph: &'static str, at: Rect, size: f32) -> Mark {
        Mark::Text { text: glyph, at, size, bold: false, opacity: 1.0, centered: true, icon: true, tinted: false }
    }
}

/// Where a shape is and how it looks, which a change blends.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Outline {
    /// Within the scene: the part on screen, without what a notch hides above.
    at: Rect,
    above: f32,
    radius: f32,
    fill: [f32; 4],
    border: [f32; 4],
}

impl Outline {
    fn blend(&self, other: &Outline, t: f32) -> Outline {
        Outline {
            at: Rect {
                x: lerp(self.at.x, other.at.x, t),
                y: lerp(self.at.y, other.at.y, t),
                width: lerp(self.at.width, other.at.width, t),
                height: lerp(self.at.height, other.at.height, t),
            },
            above: lerp(self.above, other.above, t),
            radius: lerp(self.radius, other.radius, t),
            fill: lerp_color(self.fill, other.fill, t),
            border: lerp_color(self.border, other.border, t),
        }
    }

    /// Grown or shrunk about its middle, for a shape that comes or goes.
    fn scaled(&self, factor: f32) -> Outline {
        let width = self.at.width * factor;
        let height = self.at.height * factor;
        Outline {
            at: Rect {
                x: self.at.x + (self.at.width - width) / 2.0,
                y: self.at.y + (self.at.height - height) / 2.0,
                width,
                height,
            },
            radius: self.radius * factor,
            ..*self
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Shape {
    part: Part,
    outline: Outline,
    marks: Vec<Mark>,
}

impl Shape {
    fn outline(&self) -> Outline {
        self.outline
    }
}

/// What the chosen look makes of every shape.
struct Dress {
    top: f32,
    above: f32,
    notch: bool,
    fill: [f32; 4],
    border: [f32; 4],
}

impl Dress {
    fn new(state: &SettingsState) -> Self {
        let notch = state.style == IslandStyle::Notch;
        let glass = state.material == Material::Glass;
        Self {
            top: if notch { 0.0 } else { 12.0 },
            above: if notch { NOTCH_REACH } else { 0.0 },
            notch,
            fill: if glass { [0.05, 0.05, 0.09, 0.62] } else { [0.0, 0.0, 0.0, 1.0] },
            border: if glass { [1.0, 1.0, 1.0, 0.3] } else { [1.0, 1.0, 1.0, 0.0] },
        }
    }

    /// `round` is its corner as a floating island, then as a notch.
    fn shape(&self, part: Part, x: f32, (width, height): (f32, f32), round: (f32, f32), marks: Vec<Mark>) -> Shape {
        let (round, notch_round) = round;
        let radius = if self.notch { notch_round } else { round };
        let outline = Outline {
            at: Rect { x, y: self.top, width, height },
            above: self.above,
            radius,
            fill: self.fill,
            border: self.border,
        };
        Shape { part, outline, marks }
    }
}

/// What the music pill shows, with the task on its right when they share it.
fn music_marks(state: &SettingsState, task_beside: bool) -> Vec<Mark> {
    let middle = PILL_HEIGHT / 2.0;
    let mut marks = vec![
        Mark::icon(ICON_MUSIC, Rect { x: 16.0, y: middle - 12.0, width: 22.0, height: 24.0 }, 15.0),
        Mark::text("Kısa Şarkı", Rect { x: 46.0, y: middle - 12.0, width: 84.0, height: 24.0 }, 13.0, true),
    ];
    if task_beside {
        marks.push(Mark::Ring { x: PILL_WIDTH - 24.0, y: middle, radius: 8.0, width: 2.2, fraction: SENT });
        return marks;
    }
    if !state.sound_wave || state.battery_saver {
        return marks;
    }
    let heights = match state.wave_style {
        island_core::layout::WaveStyle::Calm => [6.0, 9.0, 11.0, 7.0],
        island_core::layout::WaveStyle::Normal => [6.0, 12.0, 15.0, 9.0],
        island_core::layout::WaveStyle::Lively => [4.0, 15.0, 17.0, 6.0],
    };
    for (index, bar) in heights.into_iter().enumerate() {
        let at = Rect { x: 136.0 + index as f32 * 6.0, y: middle - bar / 2.0, width: 3.0, height: bar };
        marks.push(Mark::Bar { at, opacity: 1.0 });
    }
    marks
}

/// The send itself in the pill, as when it comes to the front.
fn send_marks() -> Vec<Mark> {
    let middle = PILL_HEIGHT / 2.0;
    vec![
        Mark::Ring { x: 24.0, y: middle, radius: 7.0, width: 2.2, fraction: SENT },
        Mark::text("Kapadokya.jpg", Rect { x: 42.0, y: middle - 12.0, width: 88.0, height: 24.0 }, 12.0, true),
        Mark::Text {
            text: "%64",
            at: Rect { x: PILL_WIDTH - 44.0, y: middle - 12.0, width: 32.0, height: 24.0 },
            size: 12.0,
            bold: false,
            opacity: 0.75,
            centered: false,
            icon: false,
            tinted: false,
        },
    ]
}

/// The open view of a send, as the chosen style shows it.
fn card_marks(state: &SettingsState) -> Vec<Mark> {
    let center = CARD_HEIGHT / 2.0;
    let percent = |large: bool| Mark::Text {
        text: "%64",
        at: if large {
            Rect { x: 4.0, y: center - 13.0, width: 60.0, height: 26.0 }
        } else {
            Rect { x: 10.0, y: center - 10.0, width: 48.0, height: 20.0 }
        },
        size: if large { 15.0 } else { 10.5 },
        bold: true,
        opacity: 1.0,
        centered: true,
        icon: false,
        tinted: false,
    };
    let mut marks = match state.progress {
        ProgressStyle::Ring => vec![Mark::Ring { x: 34.0, y: center, radius: 19.0, width: 3.5, fraction: SENT }, percent(false)],
        ProgressStyle::Edge => vec![Mark::Edge { fraction: SENT }, percent(true)],
        ProgressStyle::Fill => vec![Mark::Fill { fraction: SENT }, percent(true)],
    };
    let words = CARD_WIDTH - 76.0;
    marks.push(Mark::text("Gönderiliyor", Rect { x: 66.0, y: center - 17.0, width: words, height: 17.0 }, 11.5, true));
    marks.push(Mark::Text {
        text: "Kapadokya.jpg",
        at: Rect { x: 66.0, y: center + 1.0, width: words, height: 16.0 },
        size: 10.5,
        bold: false,
        opacity: 0.6,
        centered: false,
        icon: false,
        tinted: false,
    });
    marks
}

/// The shapes of the preview on a page, for the choices in `state`.
fn preview_shapes(state: &SettingsState) -> Vec<Shape> {
    let dress = Dress::new(state);
    let pill = |x: f32, marks| dress.shape(Part::Pill, x, (PILL_WIDTH, PILL_HEIGHT), (PILL_HEIGHT / 2.0, 14.0), marks);
    match state.page {
        // The look on its own: music playing in the pill.
        Page::Appearance => vec![pill((PREVIEW.width - PILL_WIDTH) / 2.0, music_marks(state, false))],
        // Music and a send: the closed island in the chosen layout, and beside it
        // the open view in the chosen progress style.
        Page::Layout => {
            let group = PILL_WIDTH + BUBBLE_GAP + BUBBLE_SIZE;
            let left = (PREVIEW.width - group - GROUP_GAP - CARD_WIDTH) / 2.0;
            let bubble = |marks| dress.shape(Part::Bubble, left + PILL_WIDTH + BUBBLE_GAP, (BUBBLE_SIZE, BUBBLE_SIZE), (BUBBLE_SIZE / 2.0, 12.0), marks);
            let ring = vec![
                Mark::Ring { x: BUBBLE_SIZE / 2.0, y: BUBBLE_SIZE / 2.0, radius: 15.0, width: 2.0, fraction: SENT },
                Mark::Text {
                    text: "64",
                    at: Rect { x: 0.0, y: 0.0, width: BUBBLE_SIZE, height: BUBBLE_SIZE },
                    size: 11.0,
                    bold: true,
                    opacity: 1.0,
                    centered: true,
                    icon: false,
                    tinted: false,
                },
            ];
            let note = vec![Mark::icon(ICON_MUSIC, Rect { x: 0.0, y: 0.0, width: BUBBLE_SIZE, height: BUBBLE_SIZE }, 14.0)];
            let card = dress.shape(Part::Card, left + group + GROUP_GAP, (CARD_WIDTH, CARD_HEIGHT), (22.0, 16.0), card_marks(state));
            match state.layout {
                TaskLayout::Bubble => vec![pill(left, music_marks(state, false)), bubble(ring), card],
                // Without a bubble the pill sits in the middle of where the two were.
                TaskLayout::Shared => vec![pill(left + (group - PILL_WIDTH) / 2.0, music_marks(state, true)), card],
                TaskLayout::Front => vec![pill(left, send_marks()), bubble(note), card],
            }
        }
        _ => Vec::new(),
    }
}
