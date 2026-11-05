//! Pictures made away from the island's thread: small previews of files on the
//! shelf, and pictures saved in another format beside their original.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use island_core::convert::{self, JPG_QUALITY, SMALLER_QUALITY, Target};
use island_core::files::extension;
use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HWND, LPARAM, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{DeleteObject, HPALETTE};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatJpeg, GUID_ContainerFormatPng,
    GUID_WICPixelFormat24bppBGR, GUID_WICPixelFormat32bppBGRA, GUID_WICPixelFormat32bppPBGRA,
    IWICBitmapFrameDecode, IWICBitmapSource, IWICImagingFactory, IWICPixelFormatInfo2,
    WICBitmapDitherTypeNone, WICBitmapEncoderNoCache, WICBitmapInterpolationModeHighQualityCubic,
    WICBitmapPaletteTypeMedianCut, WICBitmapTransformFlipHorizontal, WICBitmapTransformFlipVertical,
    WICBitmapTransformOptions, WICBitmapTransformRotate0, WICBitmapTransformRotate90,
    WICBitmapTransformRotate180, WICBitmapTransformRotate270, WICBitmapUsePremultipliedAlpha,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::StructuredStorage::{PROPBAG2, PROPVARIANT};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET,
};
use windows::Win32::System::Variant::{VARIANT, VT_R4, VT_UI2};
use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SHCreateMemStream, SIIGBF_RESIZETOFIT};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{Error, HSTRING, Interface, PWSTR, Result, w};

pub const MSG_IMAGING: u32 = WM_APP + 20;

/// What the codec for a format is called in the Microsoft Store.
const HEIF_MISSING: &str = "HEIC için Windows'un HEIF uzantısı gerekli";
const AV1_MISSING: &str = "AVIF için Windows'un AV1 uzantısı gerekli";

pub enum Made {
    Thumbnail { path: String, picture: Arc<[u8]> },
    Converted(Converted),
}

pub struct Converted {
    pub outputs: Vec<String>,
    /// Why the first picture that failed did so.
    pub failure: Option<String>,
    /// A small picture of the first result, for the notice.
    pub preview: Option<Arc<[u8]>>,
}

pub struct Imaging {
    window: isize,
    sender: Sender<Made>,
    receiver: Receiver<Made>,
}

impl Imaging {
    pub fn new(window: HWND) -> Self {
        let (sender, receiver) = channel();
        Self { window: window.0 as isize, sender, receiver }
    }

    pub fn results(&self) -> Vec<Made> {
        self.receiver.try_iter().collect()
    }

    /// Asks Windows for the pictures Explorer shows of these files, `size`
    /// pixels at most on each side.
    pub fn thumbnails(&self, paths: Vec<String>, size: i32) {
        let (sender, window) = (self.sender.clone(), self.window);
        std::thread::spawn(move || {
            with_com(|wic| {
                for path in paths {
                    match thumbnail(wic, &path, size) {
                        Ok(picture) => send(&sender, window, Made::Thumbnail { path, picture: picture.into() }),
                        Err(error) => eprintln!("thumbnail failed: {error}"),
                    }
                }
            });
        });
    }

    pub fn convert(&self, paths: Vec<String>, target: Target) {
        let (sender, window) = (self.sender.clone(), self.window);
        std::thread::spawn(move || {
            let mut done = Converted { outputs: Vec::new(), failure: Some("Dönüştürülemedi".into()), preview: None };
            with_com(|wic| done = convert_all(wic, &paths, target));
            send(&sender, window, Made::Converted(done));
        });
    }
}

fn send(sender: &Sender<Made>, window: isize, made: Made) {
    if sender.send(made).is_ok() {
        let _ = unsafe { PostMessageW(Some(HWND(window as _)), MSG_IMAGING, WPARAM(0), LPARAM(0)) };
    }
}

/// Runs `work` with its own apartment, which the shell and WIC need.
fn with_com(work: impl FnOnce(&IWICImagingFactory)) {
    unsafe {
        let started = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        match CoCreateInstance::<_, IWICImagingFactory>(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) {
            Ok(wic) => work(&wic),
            Err(error) => eprintln!("imaging unavailable: {error}"),
        }
        if started {
            CoUninitialize();
        }
    }
}

fn thumbnail(wic: &IWICImagingFactory, path: &str, size: i32) -> Result<Vec<u8>> {
    unsafe {
        let item: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(path), None)?;
        let bitmap = item.GetImage(SIZE { cx: size, cy: size }, SIIGBF_RESIZETOFIT)?;
        let source = wic.CreateBitmapFromHBITMAP(bitmap, HPALETTE::default(), WICBitmapUsePremultipliedAlpha);
        let _ = DeleteObject(bitmap.into());
        let source = source?;

        let (mut width, mut height) = (0, 0);
        source.GetSize(&mut width, &mut height)?;
        let stride = width * 4;
        let mut pixels = vec![0_u8; (stride * height) as usize];
        source.CopyPixels(std::ptr::null(), stride, &mut pixels)?;
        // Photos often come back with no alpha at all, which would draw as nothing.
        if pixels.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 0) {
            pixels.as_chunks_mut::<4>().0.iter_mut().for_each(|pixel| pixel[3] = 255);
        }
        let fixed = wic.CreateBitmapFromMemory(width, height, &GUID_WICPixelFormat32bppPBGRA, stride, &pixels)?;
        encode_png(wic, &fixed.cast()?)
    }
}

/// A PNG of `source` in memory, which the painter decodes like any cover.
fn encode_png(wic: &IWICImagingFactory, source: &IWICBitmapSource) -> Result<Vec<u8>> {
    unsafe {
        let stream = SHCreateMemStream(None).ok_or_else(Error::empty)?;
        let encoder = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        encoder.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.ok_or_else(Error::empty)?;
        frame.Initialize(None)?;
        let (mut width, mut height) = (0, 0);
        source.GetSize(&mut width, &mut height)?;
        frame.SetSize(width, height)?;
        let mut format = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut format)?;
        let ready = converted_to(wic, source, &format)?;
        frame.WriteSource(&ready, std::ptr::null())?;
        frame.Commit()?;
        encoder.Commit()?;

        let mut stat = STATSTG::default();
        stream.Stat(&mut stat, STATFLAG_NONAME)?;
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        let mut bytes = vec![0_u8; stat.cbSize as usize];
        let mut read = 0;
        stream.Read(bytes.as_mut_ptr().cast(), bytes.len() as u32, Some(&mut read)).ok()?;
        bytes.truncate(read as usize);
        Ok(bytes)
    }
}

fn converted_to(wic: &IWICImagingFactory, source: &IWICBitmapSource, format: &windows::core::GUID) -> Result<IWICBitmapSource> {
    unsafe {
        let converter = wic.CreateFormatConverter()?;
        converter.Initialize(source, format, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)?;
        converter.cast()
    }
}

fn convert_all(wic: &IWICImagingFactory, paths: &[String], target: Target) -> Converted {
    let mut done = Converted { outputs: Vec::new(), failure: None, preview: None };
    for path in paths {
        match convert_one(wic, path, target) {
            Ok((output, result)) => {
                if done.preview.is_none() {
                    done.preview = preview(wic, &result).ok().map(Arc::from);
                }
                done.outputs.push(output);
            }
            Err(reason) => {
                done.failure.get_or_insert(reason);
            }
        }
    }
    done
}

fn convert_one(wic: &IWICImagingFactory, path: &str, target: Target) -> std::result::Result<(String, IWICBitmapSource), String> {
    let unreadable = |error: Error| {
        eprintln!("cannot read {path}: {error}");
        match extension(path).as_str() {
            "heic" | "heif" => HEIF_MISSING.to_owned(),
            "avif" => AV1_MISSING.to_owned(),
            _ => "Görsel okunamadı".to_owned(),
        }
    };
    let source = unsafe {
        let decoder = wic
            .CreateDecoderFromFilename(&HSTRING::from(path), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)
            .map_err(unreadable)?;
        let frame = decoder.GetFrame(0).map_err(unreadable)?;
        prepare(wic, &frame, path, target).map_err(unreadable)?
    };
    let output = convert::output_path(path, target, |candidate| Path::new(candidate).exists());
    if let Err(error) = write(wic, &source, &output, target) {
        eprintln!("cannot write {output}: {error}");
        let _ = std::fs::remove_file(&output);
        return Err("Klasöre yazılamadı".to_owned());
    }
    Ok((output, source))
}

/// The picture upright, at its new size, in the pixels its format takes.
unsafe fn prepare(wic: &IWICImagingFactory, frame: &IWICBitmapFrameDecode, path: &str, target: Target) -> Result<IWICBitmapSource> {
    unsafe {
        let mut source: IWICBitmapSource = frame.cast()?;
        if target == Target::Smaller {
            let (mut width, mut height) = (0, 0);
            source.GetSize(&mut width, &mut height)?;
            let (new_width, new_height) = convert::smaller_size(width, height);
            if (new_width, new_height) != (width, height) {
                let scaler = wic.CreateBitmapScaler()?;
                scaler.Initialize(&source, new_width, new_height, WICBitmapInterpolationModeHighQualityCubic)?;
                source = scaler.cast()?;
            }
        }
        // JPEG and TIFF keep pictures sideways with a note on how to turn them,
        // which a new file would lose. HEIF decoders turn the picture themselves.
        let turn = if matches!(extension(path).as_str(), "jpg" | "jpeg" | "jpe" | "jfif" | "tif" | "tiff") {
            turning(orientation(frame))
        } else {
            WICBitmapTransformRotate0
        };
        if turn != WICBitmapTransformRotate0 {
            let rotator = wic.CreateBitmapFlipRotator()?;
            rotator.Initialize(&source, turn)?;
            source = rotator.cast()?;
        }
        if target == Target::Png {
            return converted_to(wic, &source, &GUID_WICPixelFormat32bppBGRA);
        }
        if has_alpha(wic, &source) { on_white(wic, &source) } else { converted_to(wic, &source, &GUID_WICPixelFormat24bppBGR) }
    }
}

fn orientation(frame: &IWICBitmapFrameDecode) -> u16 {
    unsafe {
        let Ok(reader) = frame.GetMetadataQueryReader() else { return 1 };
        let mut value = PROPVARIANT::default();
        if reader.GetMetadataByName(w!("System.Photo.Orientation"), &mut value).is_err() {
            return 1;
        }
        let inner = &value.Anonymous.Anonymous;
        if inner.vt == VT_UI2 { inner.Anonymous.uiVal } else { 1 }
    }
}

/// What undoes an EXIF orientation. WIC turns before it flips.
fn turning(orientation: u16) -> WICBitmapTransformOptions {
    let combine = |a: WICBitmapTransformOptions, b: WICBitmapTransformOptions| WICBitmapTransformOptions(a.0 | b.0);
    match orientation {
        2 => WICBitmapTransformFlipHorizontal,
        3 => WICBitmapTransformRotate180,
        4 => WICBitmapTransformFlipVertical,
        5 => combine(WICBitmapTransformRotate90, WICBitmapTransformFlipHorizontal),
        6 => WICBitmapTransformRotate90,
        7 => combine(WICBitmapTransformRotate270, WICBitmapTransformFlipHorizontal),
        8 => WICBitmapTransformRotate270,
        _ => WICBitmapTransformRotate0,
    }
}

fn has_alpha(wic: &IWICImagingFactory, source: &IWICBitmapSource) -> bool {
    unsafe {
        let check = || -> Result<bool> {
            let format = source.GetPixelFormat()?;
            let info: IWICPixelFormatInfo2 = wic.CreateComponentInfo(&format)?.cast()?;
            Ok(info.SupportsTransparency()?.as_bool())
        };
        check().unwrap_or(false)
    }
}

/// JPEG has no transparency. Transparent parts turn white, as they look on a
/// page, instead of whatever colour happens to be stored under them.
fn on_white(wic: &IWICImagingFactory, source: &IWICBitmapSource) -> Result<IWICBitmapSource> {
    unsafe {
        let straight = converted_to(wic, source, &GUID_WICPixelFormat32bppBGRA)?;
        let (mut width, mut height) = (0, 0);
        straight.GetSize(&mut width, &mut height)?;
        let mut pixels = vec![0_u8; (width * 4 * height) as usize];
        straight.CopyPixels(std::ptr::null(), width * 4, &mut pixels)?;
        let stride = (width * 3).div_ceil(4) * 4;
        let mut flat = vec![0_u8; (stride * height) as usize];
        for y in 0..height as usize {
            for x in 0..width as usize {
                let from = &pixels[(y * width as usize + x) * 4..][..4];
                let alpha = u32::from(from[3]);
                for channel in 0..3 {
                    let value = (u32::from(from[channel]) * alpha + 255 * (255 - alpha) + 127) / 255;
                    flat[y * stride as usize + x * 3 + channel] = value as u8;
                }
            }
        }
        wic.CreateBitmapFromMemory(width, height, &GUID_WICPixelFormat24bppBGR, stride, &flat)?.cast()
    }
}

fn write(wic: &IWICImagingFactory, source: &IWICBitmapSource, output: &str, target: Target) -> Result<()> {
    unsafe {
        let stream = wic.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(output), GENERIC_WRITE.0)?;
        let container = if target == Target::Png { GUID_ContainerFormatPng } else { GUID_ContainerFormatJpeg };
        let encoder = wic.CreateEncoder(&container, std::ptr::null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let (mut frame, mut options) = (None, None);
        encoder.CreateNewFrame(&mut frame, &mut options)?;
        let frame = frame.ok_or_else(Error::empty)?;
        if target != Target::Png
            && let Some(options) = &options
        {
            let quality = if target == Target::Smaller { SMALLER_QUALITY } else { JPG_QUALITY };
            let name = PROPBAG2 { pstrName: PWSTR(w!("ImageQuality").as_ptr().cast_mut()), ..Default::default() };
            let mut value = VARIANT::default();
            (*value.Anonymous.Anonymous).vt = VT_R4;
            (*value.Anonymous.Anonymous).Anonymous.fltVal = quality;
            options.Write(1, &name, &value)?;
        }
        frame.Initialize(options.as_ref())?;
        let (mut width, mut height) = (0, 0);
        source.GetSize(&mut width, &mut height)?;
        frame.SetSize(width, height)?;
        let mut format = source.GetPixelFormat()?;
        frame.SetPixelFormat(&mut format)?;
        let ready = converted_to(wic, source, &format)?;
        frame.WriteSource(&ready, std::ptr::null())?;
        frame.Commit()?;
        encoder.Commit()
    }
}

/// A small PNG of a result, for the notice that says it is done.
fn preview(wic: &IWICImagingFactory, source: &IWICBitmapSource) -> Result<Vec<u8>> {
    const SIDE: u32 = 96;
    unsafe {
        let (mut width, mut height) = (0, 0);
        source.GetSize(&mut width, &mut height)?;
        let scale = SIDE as f64 / width.min(height).max(1) as f64;
        let small = |side: u32| ((side as f64 * scale).round() as u32).max(1);
        let scaler = wic.CreateBitmapScaler()?;
        scaler.Initialize(source, small(width), small(height), WICBitmapInterpolationModeHighQualityCubic)?;
        encode_png(wic, &scaler.cast()?)
    }
}
