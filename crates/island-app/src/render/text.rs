use std::cell::RefCell;
use std::sync::Arc;

use windows::Foundation::Size;
use windows::Graphics::DirectX::{DirectXAlphaMode, DirectXPixelFormat};
use windows::UI::Composition::{CompositionDrawingSurface, CompositionGraphicsDevice, Compositor};
use windows::Win32::Foundation::{HMODULE, POINT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_F, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_OPEN,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_LARGE, D2D1_ARC_SIZE_SMALL, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_ELLIPSE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_ROUNDED_RECT,
    D2D1_SWEEP_DIRECTION_CLOCKWISE,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory, ID2D1Bitmap, ID2D1DeviceContext,
    ID2D1Factory1,
};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_WARP;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_METRICS, DWRITE_TRIMMING,
    DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory,
    IDWriteFactory, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeHighQualityCubic,
    WICBitmapPaletteTypeMedianCut, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::WinRT::Composition::{
    ICompositionDrawingSurfaceInterop, ICompositorInterop,
};
use windows::core::{Error, Interface, PCWSTR, Result, w};
use windows_numerics::{Matrix3x2, Vector2};

const TEXT_FONT: PCWSTR = w!("Segoe UI Variable Text");
const ICON_FONT: PCWSTR = w!("Segoe Fluent Icons");
const LOCALE: PCWSTR = w!("tr-tr");

#[derive(Clone, Copy)]
pub enum Align {
    Leading,
    Center,
    Trailing,
}

impl Align {
    fn dwrite(self) -> DWRITE_TEXT_ALIGNMENT {
        match self {
            Align::Leading => DWRITE_TEXT_ALIGNMENT_LEADING,
            Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
            Align::Trailing => DWRITE_TEXT_ALIGNMENT_TRAILING,
        }
    }
}

#[derive(Clone, Copy)]
pub enum Font {
    Text,
    Icons,
}

/// All coordinates below are in pixels relative to the surface.
#[derive(Clone, Copy)]
pub struct Bounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Bounds {
    fn rect(self) -> D2D_RECT_F {
        D2D_RECT_F {
            left: self.x,
            top: self.y,
            right: self.x + self.width,
            bottom: self.y + self.height,
        }
    }
}

pub struct TextRun<'a> {
    pub text: &'a str,
    pub bounds: Bounds,
    pub size: f32,
    pub bold: bool,
    pub opacity: f32,
    pub align: Align,
    pub font: Font,
}

/// A white rounded bar, used for the progress track and its fill.
pub struct BarRun {
    pub bounds: Bounds,
    pub opacity: f32,
}

/// A thin circle filled clockwise from the top to `fraction`, from 0 to 1.
pub struct RingRun {
    pub center_x: f32,
    pub center_y: f32,
    pub radius: f32,
    pub width: f32,
    pub fraction: f32,
}

/// A square image, cropped to fill its bounds, as covers usually are shown.
pub struct ImageRun<'a> {
    pub encoded: &'a Arc<[u8]>,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub radius: f32,
}

#[derive(Default)]
pub struct Painting<'a> {
    pub texts: Vec<TextRun<'a>>,
    pub bars: Vec<BarRun>,
    pub rings: Vec<RingRun>,
    pub images: Vec<ImageRun<'a>>,
    /// Centred captions whose text is made while painting.
    pub owned_labels: Vec<(String, Bounds)>,
}

struct DecodedImage {
    encoded: Arc<[u8]>,
    size: u32,
    bitmap: ID2D1Bitmap,
}

/// Draws into composition surfaces.
///
/// Uses the WARP software device on purpose. Content changes rarely, so a GPU
/// device would only buy memory use: loading the display driver costs about
/// 45 MB of private memory, and on hybrid laptops can wake the discrete GPU.
pub struct Painter {
    graphics: CompositionGraphicsDevice,
    dwrite: IDWriteFactory,
    wic: IWICImagingFactory,
    /// The last cover, since the expanded view repaints once a second.
    decoded: RefCell<Option<DecodedImage>>,
}

impl Painter {
    pub fn new(compositor: &Compositor) -> Result<Self> {
        unsafe {
            let mut d3d: Option<ID3D11Device> = None;
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
            let d3d = d3d.ok_or_else(Error::empty)?;
            let dxgi: IDXGIDevice = d3d.cast()?;

            let d2d: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let device = d2d.CreateDevice(&dxgi)?;

            let interop: ICompositorInterop = compositor.cast()?;
            let graphics = interop.CreateGraphicsDevice(&device)?;
            let dwrite = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let wic = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            Ok(Self { graphics, dwrite, wic, decoded: RefCell::new(None) })
        }
    }

    pub fn create_surface(&self, width: f32, height: f32) -> Result<CompositionDrawingSurface> {
        self.graphics.CreateDrawingSurface(
            Size { Width: width, Height: height },
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            DirectXAlphaMode::Premultiplied,
        )
    }

    pub fn draw(&self, surface: &CompositionDrawingSurface, painting: &Painting) -> Result<()> {
        unsafe {
            let interop: ICompositionDrawingSurfaceInterop = surface.cast()?;
            let mut offset = POINT::default();
            let dc: ID2D1DeviceContext = interop.BeginDraw(None, &mut offset)?;

            dc.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            dc.SetTransform(&Matrix3x2::translation(offset.x as f32, offset.y as f32));
            // ClearType needs an opaque background, which this surface lacks.
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);

            let result = self.draw_all(&dc, painting);
            interop.EndDraw()?;
            result
        }
    }

    pub fn forget_images(&self) {
        self.decoded.borrow_mut().take();
    }

    /// How wide a line of text is at `size`, in the same units.
    pub fn measure(&self, text: &str, size: f32, bold: bool) -> Result<f32> {
        const ROOMY: f32 = 4096.0;
        let bounds = Bounds { x: 0.0, y: 0.0, width: ROOMY, height: ROOMY };
        let run = TextRun { text, bounds, size, bold, opacity: 1.0, align: Align::Leading, font: Font::Text };
        let format = self.format(&run)?;
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let mut metrics = DWRITE_TEXT_METRICS::default();
        unsafe {
            let layout = self.dwrite.CreateTextLayout(&utf16, &format, ROOMY, ROOMY)?;
            layout.GetMetrics(&mut metrics)?;
        }
        Ok(metrics.widthIncludingTrailingWhitespace)
    }

    unsafe fn draw_all(&self, dc: &ID2D1DeviceContext, painting: &Painting) -> Result<()> {
        for bar in &painting.bars {
            let color = D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: bar.opacity };
            let radius = bar.bounds.height / 2.0;
            let rounded = D2D1_ROUNDED_RECT { rect: bar.bounds.rect(), radiusX: radius, radiusY: radius };
            unsafe {
                let brush = dc.CreateSolidColorBrush(&color, None)?;
                dc.FillRoundedRectangle(&rounded, &brush);
            }
        }
        for ring in &painting.rings {
            unsafe { self.draw_ring(dc, ring)? };
        }
        for image in &painting.images {
            // A cover that fails to decode is not worth losing the text for.
            if let Err(error) = unsafe { self.draw_image(dc, image) } {
                eprintln!("artwork failed: {error}");
            }
        }
        for run in &painting.texts {
            let format = self.format(run)?;
            let color = D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: run.opacity };
            let utf16: Vec<u16> = run.text.encode_utf16().collect();
            unsafe {
                let brush = dc.CreateSolidColorBrush(&color, None)?;
                dc.DrawText(
                    &utf16,
                    &format,
                    &run.bounds.rect(),
                    &brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
        }
        for (text, bounds) in &painting.owned_labels {
            let run = TextRun {
                text,
                bounds: *bounds,
                size: bounds.height * 0.36,
                bold: true,
                opacity: 1.0,
                align: Align::Center,
                font: Font::Text,
            };
            let format = self.format(&run)?;
            let color = D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: run.opacity };
            let utf16: Vec<u16> = run.text.encode_utf16().collect();
            unsafe {
                let brush = dc.CreateSolidColorBrush(&color, None)?;
                dc.DrawText(
                    &utf16,
                    &format,
                    &run.bounds.rect(),
                    &brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
        }
        Ok(())
    }

    unsafe fn draw_ring(&self, dc: &ID2D1DeviceContext, ring: &RingRun) -> Result<()> {
        let center = Vector2 { X: ring.center_x, Y: ring.center_y };
        let full = D2D1_ELLIPSE { point: center, radiusX: ring.radius, radiusY: ring.radius };
        unsafe {
            let track = dc.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.22 }, None)?;
            dc.DrawEllipse(&full, &track, ring.width, None);

            let white = dc.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }, None)?;
            if ring.fraction >= 0.999 {
                dc.DrawEllipse(&full, &white, ring.width, None);
                return Ok(());
            }
            if ring.fraction <= 0.0 {
                return Ok(());
            }
            let angle = ring.fraction * std::f32::consts::TAU;
            let top = Vector2 { X: center.X, Y: center.Y - ring.radius };
            let end = Vector2 {
                X: center.X + ring.radius * angle.sin(),
                Y: center.Y - ring.radius * angle.cos(),
            };
            let path = dc.GetFactory()?.CreatePathGeometry()?;
            let sink = path.Open()?;
            sink.BeginFigure(top, D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: end,
                size: D2D_SIZE_F { width: ring.radius, height: ring.radius },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: if ring.fraction > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
            });
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            sink.Close()?;
            dc.DrawGeometry(&path, &white, ring.width, None);
        }
        Ok(())
    }

    unsafe fn draw_image(&self, dc: &ID2D1DeviceContext, image: &ImageRun) -> Result<()> {
        let bitmap = self.bitmap_for(dc, image)?;
        unsafe {
            let size = bitmap.GetSize();
            let brush = dc.CreateBitmapBrush(&bitmap, None, None)?;
            brush.SetTransform(&Matrix3x2::translation(
                image.x - (size.width - image.size) / 2.0,
                image.y - (size.height - image.size) / 2.0,
            ));
            let rect = Bounds { x: image.x, y: image.y, width: image.size, height: image.size }.rect();
            let rounded = D2D1_ROUNDED_RECT { rect, radiusX: image.radius, radiusY: image.radius };
            dc.FillRoundedRectangle(&rounded, &brush);
        }
        Ok(())
    }

    fn bitmap_for(&self, dc: &ID2D1DeviceContext, image: &ImageRun) -> Result<ID2D1Bitmap> {
        let size = image.size.ceil() as u32;
        let mut decoded = self.decoded.borrow_mut();
        if let Some(cached) = decoded.as_ref()
            && cached.size == size
            && Arc::ptr_eq(&cached.encoded, image.encoded)
        {
            return Ok(cached.bitmap.clone());
        }
        let bitmap = self.decode(dc, image.encoded, size)?;
        *decoded = Some(DecodedImage { encoded: image.encoded.clone(), size, bitmap: bitmap.clone() });
        Ok(bitmap)
    }

    /// Scales down while decoding: a 640 px cover shown at 72 px would both
    /// waste memory and shimmer if the GPU-less device had to minify it.
    fn decode(&self, dc: &ID2D1DeviceContext, encoded: &[u8], size: u32) -> Result<ID2D1Bitmap> {
        unsafe {
            let stream = self.wic.CreateStream()?;
            stream.InitializeFromMemory(encoded)?;
            let decoder = self.wic.CreateDecoderFromStream(
                &stream,
                std::ptr::null(),
                WICDecodeMetadataCacheOnDemand,
            )?;
            let frame = decoder.GetFrame(0)?;

            let (mut width, mut height) = (0, 0);
            frame.GetSize(&mut width, &mut height)?;
            let scale = size as f64 / width.min(height).max(1) as f64;
            let scaled_width = (width as f64 * scale).round().max(1.0) as u32;
            let scaled_height = (height as f64 * scale).round().max(1.0) as u32;

            let scaler = self.wic.CreateBitmapScaler()?;
            scaler.Initialize(&frame, scaled_width, scaled_height, WICBitmapInterpolationModeHighQualityCubic)?;

            let converter = self.wic.CreateFormatConverter()?;
            converter.Initialize(
                &scaler,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeMedianCut,
            )?;
            dc.CreateBitmapFromWicBitmap(&converter, None)?.cast()
        }
    }

    fn format(&self, run: &TextRun) -> Result<IDWriteTextFormat> {
        let weight = if run.bold { DWRITE_FONT_WEIGHT_SEMI_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL };
        let family = match run.font {
            Font::Text => TEXT_FONT,
            Font::Icons => ICON_FONT,
        };
        unsafe {
            let format = self.dwrite.CreateTextFormat(
                family,
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                run.size,
                LOCALE,
            )?;
            format.SetTextAlignment(run.align.dwrite())?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            let ellipsis = self.dwrite.CreateEllipsisTrimmingSign(&format)?;
            let trimming = DWRITE_TRIMMING {
                granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                ..Default::default()
            };
            format.SetTrimming(&trimming, &ellipsis)?;
            Ok(format)
        }
    }
}
