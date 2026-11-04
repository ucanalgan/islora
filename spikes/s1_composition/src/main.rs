//! S1: saydam, odak çalmayan pencerede yay animasyonlu pill.
//!
//! Kullanım:
//!   s1_composition            pill'e tıklayınca büyür/küçülür
//!   s1_composition --auto     1,5 saniyede bir kendi kendine geçiş yapar
//! Çıkış: Ctrl+Alt+Q

use std::cell::RefCell;
use std::mem::size_of;
use std::time::Duration;

use windows::Foundation::{Size, TimeSpan, TypedEventHandler};
use windows::Graphics::DirectX::{DirectXAlphaMode, DirectXPixelFormat};
use windows::System::DispatcherQueueController;
use windows::UI::Color;
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::UI::Composition::{
    CompositionBatchCompletedEventArgs, CompositionBatchTypes, CompositionDrawingSurface,
    CompositionGraphicsDevice, CompositionRoundedRectangleGeometry, CompositionScopedBatch,
    CompositionStretch, Compositor,
};
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_COLOR_F};
use windows::Win32::Graphics::Direct2D::{
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory, ID2D1DeviceContext, ID2D1Factory1,
};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWriteCreateFactory, IDWriteFactory,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_GPU_PREFERENCE_MINIMUM_POWER, IDXGIAdapter1,
    IDXGIDevice, IDXGIFactory6,
};
use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::WinRT::Composition::{
    ICompositionDrawingSurfaceInterop, ICompositorDesktopInterop, ICompositorInterop,
};
use windows::Win32::System::WinRT::{
    CreateDispatcherQueueController, DQTAT_COM_STA, DQTYPE_THREAD_CURRENT, DispatcherQueueOptions,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{MOD_ALT, MOD_CONTROL, RegisterHotKey};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Interface, Result, h, w};
use windows_numerics::{Matrix3x2, Vector2, Vector3};

const WINDOW_W: f32 = 460.0;
const WINDOW_H: f32 = 220.0;
const SURFACE_W: f32 = 420.0;
const SURFACE_H: f32 = 180.0;

const HOTKEY_QUIT: i32 = 1;
const TIMER_AUTO: usize = 1;
const AUTO_INTERVAL_MS: u32 = 1500;

#[derive(Clone, Copy, PartialEq)]
struct Pill {
    w: f32,
    h: f32,
    radius: f32,
}

const COMPACT: Pill = Pill { w: 180.0, h: 34.0, radius: 17.0 };
const EXPANDED: Pill = Pill { w: 420.0, h: 180.0, radius: 36.0 };

struct App {
    hwnd: HWND,
    compositor: Compositor,
    geometry: CompositionRoundedRectangleGeometry,
    current: Pill,
    // Referans düşerse Completed olayı hiç gelmeyebilir.
    batch: Option<CompositionScopedBatch>,
    _target: DesktopWindowTarget,
    _controller: DispatcherQueueController,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn main() -> Result<()> {
    let auto = std::env::args().any(|a| a == "--auto");
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)?;
    }

    let controller = create_dispatcher_queue()?;
    let hwnd = create_window()?;
    let compositor = Compositor::new()?;
    let (target, geometry) = build_tree(&compositor, hwnd)?;

    set_hit_region(hwnd, COMPACT);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        RegisterHotKey(Some(hwnd), HOTKEY_QUIT, MOD_CONTROL | MOD_ALT, 'Q' as u32)?;
        if auto {
            SetTimer(Some(hwnd), TIMER_AUTO, AUTO_INTERVAL_MS, None);
        }
    }

    println!("hwnd={:#x}", hwnd.0 as usize);

    APP.with_borrow_mut(|slot| {
        *slot = Some(App {
            hwnd,
            compositor,
            geometry,
            current: COMPACT,
            batch: None,
            _target: target,
            _controller: controller,
        })
    });

    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

fn create_dispatcher_queue() -> Result<DispatcherQueueController> {
    let options = DispatcherQueueOptions {
        dwSize: size_of::<DispatcherQueueOptions>() as u32,
        threadType: DQTYPE_THREAD_CURRENT,
        apartmentType: DQTAT_COM_STA,
    };
    unsafe { CreateDispatcherQueueController(options) }
}

fn create_window() -> Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = w!("IsloraSpike");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        RegisterClassW(&wc);

        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let x = (screen_w - WINDOW_W as i32) / 2;

        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP,
            class,
            w!("Islora Spike"),
            WS_POPUP,
            x,
            0,
            WINDOW_W as i32,
            WINDOW_H as i32,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }
}

fn build_tree(
    compositor: &Compositor,
    hwnd: HWND,
) -> Result<(DesktopWindowTarget, CompositionRoundedRectangleGeometry)> {
    let interop: ICompositorDesktopInterop = compositor.cast()?;
    let target = unsafe { interop.CreateDesktopWindowTarget(hwnd, false)? };

    let root = compositor.CreateContainerVisual()?;
    root.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
    target.SetRoot(&root)?;

    let shape_visual = compositor.CreateShapeVisual()?;
    shape_visual.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
    root.Children()?.InsertAtTop(&shape_visual)?;

    let geometry = compositor.CreateRoundedRectangleGeometry()?;
    geometry.SetSize(Vector2 { X: COMPACT.w, Y: COMPACT.h })?;
    geometry.SetCornerRadius(Vector2 { X: COMPACT.radius, Y: COMPACT.radius })?;
    geometry.SetOffset(Vector2 { X: centered_x(COMPACT), Y: 0.0 })?;

    let sprite = compositor.CreateSpriteShapeWithGeometry(&geometry)?;
    let fill = compositor.CreateColorBrushWithColor(Color { A: 255, R: 0, G: 0, B: 0 })?;
    sprite.SetFillBrush(&fill)?;
    shape_visual.Shapes()?.Append(&sprite)?;

    // İçerik, pill ile aynı geometriye kırpılır; böylece pill büyüdükçe açığa çıkar.
    let content = compositor.CreateContainerVisual()?;
    content.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
    content.SetClip(&compositor.CreateGeometricClipWithGeometry(&geometry)?)?;
    root.Children()?.InsertAtTop(&content)?;

    let surface = draw_text_surface(compositor)?;
    let brush = compositor.CreateSurfaceBrushWithSurface(&surface)?;
    brush.SetStretch(CompositionStretch::None)?;
    let text = compositor.CreateSpriteVisual()?;
    text.SetBrush(&brush)?;
    text.SetSize(Vector2 { X: SURFACE_W, Y: SURFACE_H })?;
    text.SetOffset(Vector3 { X: (WINDOW_W - SURFACE_W) / 2.0, Y: 0.0, Z: 0.0 })?;
    content.Children()?.InsertAtTop(&text)?;

    Ok((target, geometry))
}

/// En az güç harcayan GPU'da aygıt açar, metni yüzeye bir kez çizer.
fn draw_text_surface(compositor: &Compositor) -> Result<CompositionDrawingSurface> {
    unsafe {
        let dxgi: IDXGIFactory6 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
        let adapter: IDXGIAdapter1 =
            dxgi.EnumAdapterByGpuPreference(0, DXGI_GPU_PREFERENCE_MINIMUM_POWER)?;
        let desc = adapter.GetDesc1()?;
        let name_len = desc.Description.iter().position(|&c| c == 0).unwrap_or(0);
        println!(
            "adapter={} luid={:#x}_{:#x}",
            String::from_utf16_lossy(&desc.Description[..name_len]),
            desc.AdapterLuid.HighPart,
            desc.AdapterLuid.LowPart,
        );

        let mut d3d: Option<ID3D11Device> = None;
        if std::env::args().any(|a| a == "--warp") {
            println!("cizici=WARP");
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_WARP,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut d3d),
                None,
                None,
            )?;
        } else {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut d3d),
                None,
                None,
            )?;
        }
        let d3d = d3d.ok_or_else(windows::core::Error::empty)?;
        let dxgi_device: IDXGIDevice = d3d.cast()?;

        let d2d: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let d2d_device = d2d.CreateDevice(&dxgi_device)?;

        let interop: ICompositorInterop = compositor.cast()?;
        let graphics: CompositionGraphicsDevice = interop.CreateGraphicsDevice(&d2d_device)?;
        let surface = graphics.CreateDrawingSurface(
            Size { Width: SURFACE_W, Height: SURFACE_H },
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            DirectXAlphaMode::Premultiplied,
        )?;

        let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let format = dwrite.CreateTextFormat(
            w!("Segoe UI Variable Text"),
            None,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            14.0,
            w!("tr-tr"),
        )?;
        format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;

        let surface_interop: ICompositionDrawingSurfaceInterop = surface.cast()?;
        let mut offset = POINT::default();
        let dc: ID2D1DeviceContext = surface_interop.BeginDraw(None, &mut offset)?;
        dc.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        dc.SetTransform(&Matrix3x2::translation(offset.x as f32, offset.y as f32));
        // Saydam yüzeyde ClearType çalışmaz.
        dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);

        let white = dc.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }, None)?;
        let lines = [
            ("Şu an çalıyor", 7.0_f32),
            ("Genişleyince görünen içerik: ğüşıöç", 84.0),
        ];
        for (line, y) in lines {
            let utf16: Vec<u16> = line.encode_utf16().collect();
            let rect = D2D_RECT_F { left: 0.0, top: y, right: SURFACE_W, bottom: y + 24.0 };
            dc.DrawText(
                &utf16,
                &format,
                &rect,
                &white,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        surface_interop.EndDraw()?;
        Ok(surface)
    }
}

fn centered_x(pill: Pill) -> f32 {
    (WINDOW_W - pill.w) / 2.0
}

fn set_hit_region(hwnd: HWND, pill: Pill) {
    let left = centered_x(pill) as i32;
    let right = left + pill.w as i32 + 1;
    let bottom = pill.h as i32 + 1;
    let diameter = (pill.radius * 2.0) as i32;
    unsafe {
        let region = CreateRoundRectRgn(left, 0, right, bottom, diameter, diameter);
        SetWindowRgn(hwnd, Some(region), true);
    }
}

fn spring_to(compositor: &Compositor, geometry: &CompositionRoundedRectangleGeometry, property: &windows::core::HSTRING, x: f32, y: f32) -> Result<()> {
    let spring = compositor.CreateSpringVector2Animation()?;
    spring.SetDampingRatio(0.72)?;
    spring.SetPeriod(TimeSpan::from(Duration::from_millis(68)))?;
    spring
        .InitialValueExpressions()?
        .Insert(h!("FinalValue"), &format!("Vector2({x}, {y})").into())?;
    geometry.StartAnimation(property, &spring)
}

fn toggle(app: &mut App) -> Result<()> {
    let next = if app.current == COMPACT { EXPANDED } else { COMPACT };

    // Animasyon boyunca hit bölgesi iki durumun büyük olanı olmalı, yoksa pill kırpılır.
    set_hit_region(app.hwnd, EXPANDED);

    let batch = app.compositor.CreateScopedBatch(CompositionBatchTypes::Animation)?;
    spring_to(&app.compositor, &app.geometry, h!("Size"), next.w, next.h)?;
    spring_to(&app.compositor, &app.geometry, h!("CornerRadius"), next.radius, next.radius)?;
    spring_to(&app.compositor, &app.geometry, h!("Offset"), centered_x(next), 0.0)?;
    batch.End()?;

    let hwnd_value = app.hwnd.0 as isize;
    let started = std::time::Instant::now();
    batch.Completed(&TypedEventHandler::<_, CompositionBatchCompletedEventArgs>::new(
        move |_, _| {
            println!("bitti: {}x{} {} ms", next.w, next.h, started.elapsed().as_millis());
            set_hit_region(HWND(hwnd_value as _), next);
            Ok(())
        },
    ))?;

    app.batch = Some(batch);
    app.current = next;
    Ok(())
}

fn toggle_app() {
    APP.with_borrow_mut(|slot| {
        if let Some(app) = slot {
            if let Err(error) = toggle(app) {
                eprintln!("gecis hatasi: {error}");
            }
        }
    });
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_LBUTTONDOWN => {
            toggle_app();
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_AUTO => {
            toggle_app();
            LRESULT(0)
        }
        WM_HOTKEY if wparam.0 as i32 == HOTKEY_QUIT => {
            unsafe { let _ = DestroyWindow(hwnd); }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
