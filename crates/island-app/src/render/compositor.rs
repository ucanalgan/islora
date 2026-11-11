use std::mem::size_of;
use std::time::Duration;

use island_core::layout::{BUBBLE_GAP, NOTCH_SHOULDER, WaveStyle};
use island_core::{Bubble, Frame, IslandStyle, Kind, Material, Millis, Pill, Presentation};
use windows::Foundation::TimeSpan;
use windows::Graphics::{IGeometrySource2D, IGeometrySource2D_Impl};
use windows::System::DispatcherQueueController;
use windows::UI::Color;
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::UI::Composition::{
    AnimationIterationBehavior, CompositionBrush, CompositionClip,
    CompositionColorBrush, CompositionDrawingSurface, CompositionGeometricClip,
    CompositionLinearGradientBrush, CompositionPath, CompositionRoundedRectangleGeometry,
    CompositionSpriteShape, CompositionStretch, CompositionSurfaceBrush, Compositor,
    ContainerVisual, ICompositionSurface, SpriteVisual,
};
use windows::Win32::Foundation::{E_NOTIMPL, HWND};
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{
    D2D1_COMBINE_MODE_EXCLUDE, D2D1_DEFAULT_FLATTENING_TOLERANCE, D2D1_ELLIPSE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1CreateFactory, ID2D1Factory, ID2D1Factory1,
    ID2D1Geometry,
};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_HOSTBACKDROPBRUSH, DwmSetWindowAttribute};
use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
use windows::Win32::System::WinRT::Graphics::Direct2D::{
    IGeometrySource2DInterop, IGeometrySource2DInterop_Impl,
};
use windows::Win32::System::WinRT::{
    CreateDispatcherQueueController, DQTAT_COM_STA, DQTYPE_THREAD_CURRENT, DispatcherQueueOptions,
};
use windows::core::{Interface, Ref, Result, h, implement};
use windows_numerics::{Vector2, Vector3};

use super::glass;
use super::scene::{ClockText, CompactFit, Scene, WAVE_RESERVED, bubble_painting};
use super::spring::{
    CALM, LIVELY, SOFT, Spring, fade, spring_scalar, spring_vector2, spring_vector3, trailing,
};
use super::text::Painter;

const FADE_IN: Duration = Duration::from_millis(240);
const FADE_OUT: Duration = Duration::from_millis(130);
const FADE_IN_DELAY: Duration = Duration::from_millis(50);
/// New content starts this much smaller and grows with the pill, from its top
/// edge, instead of waiting inside it to be revealed.
const CONTENT_START_SCALE: f32 = 0.92;
/// Old content sinks back as it fades, so the two do not sit on top of each other.
const CONTENT_END_SCALE: f32 = 0.97;
/// The island comes up out of nothing from a little smaller than it will be.
const EMERGE_SCALE: f32 = 0.86;
const EMERGE_FADE: Duration = Duration::from_millis(150);
const LEAVE_FADE: Duration = Duration::from_millis(240);
const LEAVE_DELAY: Duration = Duration::from_millis(60);
const LEAVE_SCALE: f32 = 0.92;
/// The dip the island gives under a click, as a pressed thing would.
const PRESS_SCALE: f32 = 0.97;
const PRESS_TIME: Duration = Duration::from_millis(280);
const BOUNCE_SCALE: f32 = 1.04;
const BOUNCE_TIME: Duration = Duration::from_millis(420);
const BUBBLE_START_SCALE: f32 = 0.6;
/// How small content gets as it goes into the bubble or comes out of it, when
/// the pill and the bubble trade what they show.
const TRADE_SCALE: f32 = 0.55;

const WAVE_BARS: usize = 4;
const WAVE_BAR_WIDTH: f32 = 3.0;
const WAVE_BAR_GAP: f32 = 3.0;
const WAVE_BAR_HEIGHT: f32 = 15.0;
/// Where a notch's curve starts to turn solid, which softens its edge.
const SHOULDER_EDGE: f32 = 0.92;

/// Laid over the glass, whose brightness is already held: a light sheen at the
/// top, as light falling from above would leave, and a little more depth
/// towards the bottom, where the controls are.
const GLASS_SHEEN: Color = Color { A: 34, R: 255, G: 255, B: 255 };
const GLASS_MIDDLE: Color = Color { A: 26, R: 0, G: 0, B: 0 };
const GLASS_BOTTOM: Color = Color { A: 86, R: 0, G: 0, B: 0 };
/// Where the sheen has faded into the darker glass.
const SHEEN_END: f32 = 0.42;
/// The top of a notch is off the screen, so its visible part starts lower in
/// the sheen. Its curves take this shade, to run on into it.
const NOTCH_TOP_SHADE: Color = Color { A: 18, R: 255, G: 255, B: 255 };
/// Light caught by the edge of the glass: strongest at the upper corner, a
/// faint answer at the opposite one.
const RIM: [(f32, Color); 3] = [
    (0.0, Color { A: 160, R: 255, G: 255, B: 255 }),
    (0.5, Color { A: 34, R: 255, G: 255, B: 255 }),
    (1.0, Color { A: 80, R: 255, G: 255, B: 255 }),
];
/// A notch has no upper edge of its own, so its rim starts unlit where it
/// meets the curves and only shows along the lower sides.
const RIM_NOTCH: [(f32, Color); 2] = [
    (0.0, Color { A: 0, R: 255, G: 255, B: 255 }),
    (1.0, Color { A: 70, R: 255, G: 255, B: 255 }),
];
const RIM_WIDTH: f32 = 1.2;

/// The compositor needs a dispatcher queue on the thread that creates it.
pub fn create_dispatcher_queue() -> Result<DispatcherQueueController> {
    let options = DispatcherQueueOptions {
        dwSize: size_of::<DispatcherQueueOptions>() as u32,
        threadType: DQTYPE_THREAD_CURRENT,
        apartmentType: DQTAT_COM_STA,
    };
    unsafe { CreateDispatcherQueueController(options) }
}

/// The curve joining a notch to the top of the screen: black, with a quarter
/// circle taken out around the lower corner at `center_x`, 0 for left or 1 for
/// right. It is a gradient that changes all at once, so nothing has to be painted.
fn shoulder(compositor: &Compositor, center_x: f32) -> Result<SpriteVisual> {
    let brush = compositor.CreateRadialGradientBrush()?;
    brush.SetEllipseCenter(Vector2 { X: center_x, Y: 1.0 })?;
    brush.SetEllipseRadius(Vector2 { X: 1.0, Y: 1.0 })?;
    let clear = Color { A: 0, R: 0, G: 0, B: 0 };
    let black = Color { A: 255, R: 0, G: 0, B: 0 };
    let stops = brush.ColorStops()?;
    stops.Append(&compositor.CreateColorGradientStopWithOffsetAndColor(SHOULDER_EDGE, clear)?)?;
    stops.Append(&compositor.CreateColorGradientStopWithOffsetAndColor(1.0, black)?)?;
    let visual = compositor.CreateSpriteVisual()?;
    visual.SetBrush(&brush)?;
    visual.SetOpacity(0.0)?;
    Ok(visual)
}

/// Colours from top to bottom, each at its offset from 0 to 1.
fn vertical_gradient(compositor: &Compositor, stops: &[(f32, Color)]) -> Result<CompositionLinearGradientBrush> {
    let brush = compositor.CreateLinearGradientBrush()?;
    brush.SetStartPoint(Vector2 { X: 0.0, Y: 0.0 })?;
    brush.SetEndPoint(Vector2 { X: 0.0, Y: 1.0 })?;
    let list = brush.ColorStops()?;
    for &(offset, color) in stops {
        list.Append(&compositor.CreateColorGradientStopWithOffsetAndColor(offset, color)?)?;
    }
    Ok(brush)
}

/// Colours from the upper left corner to the lower right one.
fn diagonal_gradient(compositor: &Compositor, stops: &[(f32, Color)]) -> Result<CompositionLinearGradientBrush> {
    let brush = vertical_gradient(compositor, stops)?;
    brush.SetEndPoint(Vector2 { X: 1.0, Y: 1.0 })?;
    Ok(brush)
}

/// Hands a Direct2D shape to the compositor, which accepts shapes only this way.
#[implement(IGeometrySource2D, IGeometrySource2DInterop)]
struct Outline(ID2D1Geometry);

impl IGeometrySource2D_Impl for Outline_Impl {}

impl IGeometrySource2DInterop_Impl for Outline_Impl {
    fn GetGeometry(&self) -> Result<ID2D1Geometry> {
        Ok(self.0.clone())
    }

    fn TryGetGeometryUsingFactory(&self, _factory: Ref<'_, ID2D1Factory>) -> Result<ID2D1Geometry> {
        Err(E_NOTIMPL.into())
    }
}

/// The curve of a notch as a real shape, for glass to be cut to: its square,
/// less the quarter circle at the lower corner on the side `center_x` names.
fn shoulder_clip(compositor: &Compositor, size: f32, center_x: f32) -> Result<CompositionGeometricClip> {
    unsafe {
        let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let square = factory.CreateRectangleGeometry(&D2D_RECT_F { left: 0.0, top: 0.0, right: size, bottom: size })?;
        let circle = factory.CreateEllipseGeometry(&D2D1_ELLIPSE {
            point: Vector2 { X: center_x * size, Y: size },
            radiusX: size,
            radiusY: size,
        })?;
        let path = factory.CreatePathGeometry()?;
        let sink = path.Open()?;
        square.CombineWithGeometry(&circle, D2D1_COMBINE_MODE_EXCLUDE, None, D2D1_DEFAULT_FLATTENING_TOLERANCE, &sink)?;
        sink.Close()?;
        let source: IGeometrySource2D = Outline(path.cast()?).into();
        let geometry = compositor.CreatePathGeometryWithPath(&CompositionPath::Create(&source)?)?;
        compositor.CreateGeometricClipWithGeometry(&geometry)
    }
}

/// The blurred view of what is behind the window, cut to the shape it backs.
/// Shape visuals cannot be filled with this brush, so it has to be a sprite.
fn frost(
    compositor: &Compositor,
    brush: &CompositionBrush,
    shape: &CompositionRoundedRectangleGeometry,
) -> Result<SpriteVisual> {
    let visual = compositor.CreateSpriteVisual()?;
    visual.SetBrush(brush)?;
    visual.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
    visual.SetClip(&compositor.CreateGeometricClipWithGeometry(shape)?)?;
    visual.SetIsVisible(false)?;
    Ok(visual)
}

/// A sprite showing painted content. Two exist so a change can cross-fade.
struct Layer {
    visual: SpriteVisual,
    brush: CompositionSurfaceBrush,
    surface: Option<CompositionDrawingSurface>,
}

impl Layer {
    fn new(compositor: &Compositor) -> Result<Self> {
        let brush = compositor.CreateSurfaceBrush()?;
        brush.SetStretch(CompositionStretch::None)?;
        brush.SetHorizontalAlignmentRatio(0.0)?;
        brush.SetVerticalAlignmentRatio(0.0)?;
        let visual = compositor.CreateSpriteVisual()?;
        visual.SetBrush(&brush)?;
        visual.SetOpacity(0.0)?;
        Ok(Self { visual, brush, surface: None })
    }
}

/// `Always` moves for as long as the bars are on screen, which is the chosen
/// look. It costs nothing in this process, but dwm.exe has to run on every
/// display frame for it: at 144 Hz, about 40 to 70% of a core during playback,
/// against almost nothing for `Still`. It stops by itself whenever the bars
/// leave the screen, so a hidden or expanded island pays none of this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveMotion {
    Always,
    Still,
    Hidden,
}

/// Equaliser bars that show sound is playing.
struct Wave {
    container: ContainerVisual,
    bars: Vec<SpriteVisual>,
    visible: bool,
    moving: bool,
    style: WaveStyle,
}

impl Wave {
    fn new(compositor: &Compositor, style: WaveStyle) -> Result<Self> {
        let container = compositor.CreateContainerVisual()?;
        container.SetOpacity(0.0)?;
        let white = compositor.CreateColorBrushWithColor(Color { A: 220, R: 255, G: 255, B: 255 })?;
        let mut bars = Vec::with_capacity(WAVE_BARS);
        for index in 0..WAVE_BARS {
            let bar = compositor.CreateSpriteVisual()?;
            bar.SetBrush(&white)?;
            bar.SetScale(rest_scale(index))?;
            container.Children()?.InsertAtTop(&bar)?;
            bars.push(bar);
        }
        Ok(Self { container, bars, visible: false, moving: false, style })
    }

    /// The right edge of a pill moves with its spring, so bars already on show
    /// follow it rather than jump to where it will end up.
    fn place(&self, compositor: &Compositor, pill: Pill, origin: Vector2, scale: f32) -> Result<()> {
        let total = WAVE_BAR_WIDTH * WAVE_BARS as f32 + WAVE_BAR_GAP * (WAVE_BARS - 1) as f32;
        let left = (WAVE_RESERVED - total) / 2.0 - 6.0;
        let top = (pill.height / scale - WAVE_BAR_HEIGHT) / 2.0;
        let x = origin.X + pill.width - (WAVE_RESERVED - left) * scale;
        let y = origin.Y + top * scale;
        if self.visible {
            // Both ways: the pill's height and its distance from the top edge
            // change too, not only its width.
            spring_scalar(compositor, &self.container, h!("Offset.X"), x, LIVELY)?;
            spring_scalar(compositor, &self.container, h!("Offset.Y"), y, LIVELY)?;
        } else {
            self.container.StopAnimation(h!("Offset.X"))?;
            self.container.StopAnimation(h!("Offset.Y"))?;
            self.container.SetOffset(Vector3 { X: x, Y: y, Z: 0.0 })?;
        }
        for (index, bar) in self.bars.iter().enumerate() {
            let height = WAVE_BAR_HEIGHT * scale;
            bar.SetSize(Vector2 { X: WAVE_BAR_WIDTH * scale, Y: height })?;
            bar.SetOffset(Vector3 {
                X: index as f32 * (WAVE_BAR_WIDTH + WAVE_BAR_GAP) * scale,
                Y: 0.0,
                Z: 0.0,
            })?;
            bar.SetCenterPoint(Vector3 { X: 0.0, Y: height / 2.0, Z: 0.0 })?;
        }
        Ok(())
    }

    /// Out of sight at once. The next placement then puts the bars where they
    /// belong and fades them in there, instead of sliding them over from a
    /// layout that is gone, such as the other style.
    fn hide_now(&mut self) -> Result<()> {
        for property in [h!("Opacity"), h!("Offset.X"), h!("Offset.Y")] {
            self.container.StopAnimation(property)?;
        }
        // A layout or DPI change can hide the pill without going through
        // update_wave. Stop the looping bars and any settling springs too.
        for (index, bar) in self.bars.iter().enumerate() {
            bar.StopAnimation(h!("Scale.Y"))?;
            bar.SetScale(rest_scale(index))?;
        }
        self.container.SetOpacity(0.0)?;
        self.visible = false;
        self.moving = false;
        Ok(())
    }

    fn set_visible(&mut self, compositor: &Compositor, visible: bool) -> Result<()> {
        if self.visible != visible {
            self.visible = visible;
            let (opacity, duration, delay) = match visible {
                true => (1.0, FADE_IN, FADE_IN_DELAY),
                false => (0.0, FADE_OUT, Duration::ZERO),
            };
            fade(compositor, &self.container, opacity, duration, delay)?;
        }
        Ok(())
    }

    fn set_moving(&mut self, compositor: &Compositor, moving: bool) -> Result<()> {
        if self.moving == moving {
            return Ok(());
        }
        for (index, bar) in self.bars.iter().enumerate() {
            if !moving {
                // Stopping keeps the bar where it was, so it can sink from there.
                bar.StopAnimation(h!("Scale.Y"))?;
                spring_scalar(compositor, bar, h!("Scale.Y"), rest_scale(index).Y, SOFT)?;
                continue;
            }
            let animation = compositor.CreateScalarKeyFrameAnimation()?;
            animation.InsertKeyFrame(0.0, self.style.low())?;
            animation.InsertKeyFrame(0.5, self.style.high())?;
            animation.InsertKeyFrame(1.0, self.style.low())?;
            let period = Duration::from_millis(self.style.period_ms(index));
            animation.SetDuration(TimeSpan::from(period))?;
            animation.SetIterationBehavior(AnimationIterationBehavior::Forever)?;
            bar.StartAnimation(h!("Scale.Y"), &animation)?;
        }
        self.moving = moving;
        Ok(())
    }

    fn set_style(&mut self, compositor: &Compositor, style: WaveStyle) -> Result<()> {
        if self.style == style {
            return Ok(());
        }
        let moving = self.moving;
        self.set_moving(compositor, false)?;
        self.style = style;
        self.set_moving(compositor, moving)
    }
}

fn rest_scale(index: usize) -> Vector3 {
    let height = if index.is_multiple_of(2) { 0.55 } else { 0.9 };
    Vector3 { X: 1.0, Y: height, Z: 1.0 }
}

/// The bubble beside the pill. It starts at no size behind the pill's edge, so
/// black on black it looks as if it splits off rather than appears.
struct BubbleShape {
    geometry: CompositionRoundedRectangleGeometry,
    visual: SpriteVisual,
    brush: CompositionSurfaceBrush,
    shown: Option<Bubble>,
    /// Its left and top edge in window pixels.
    spot: Option<(f32, f32)>,
}

/// Whether the pill and the bubble trade what they show.
#[derive(Clone, Copy, Default)]
struct Trade {
    /// What the pill showed moves to the bubble.
    into_bubble: bool,
    /// What the bubble showed moves to the pill.
    out_of_bubble: bool,
}

/// What it takes to switch the pill, the bubble and a notch's curves between
/// materials.
struct Surfaces {
    shapes: [CompositionSpriteShape; 2],
    frost: [SpriteVisual; 2],
    glass: CompositionBrush,
    black: CompositionColorBrush,
    tint: CompositionLinearGradientBrush,
    rim: CompositionLinearGradientBrush,
    rim_notch: CompositionLinearGradientBrush,
    /// The painted black of each curve, and the tint laid over its glass.
    shoulder_black: [CompositionBrush; 2],
    shoulder_tint: [SpriteVisual; 2],
}

pub struct Renderer {
    compositor: Compositor,
    _target: DesktopWindowTarget,
    hwnd: HWND,
    /// Holds everything, so the island can come and go as one.
    root: ContainerVisual,
    geometry: CompositionRoundedRectangleGeometry,
    surfaces: Surfaces,
    material: Material,
    /// Left and right curve of a notch.
    shoulders: [SpriteVisual; 2],
    /// Carries everything drawn on the pill, so it can slide with it.
    moving: ContainerVisual,
    bubble: BubbleShape,
    layers: [Layer; 2],
    front: usize,
    wave: Wave,
    wave_motion: WaveMotion,
    painter: Painter,
    scene: Option<Scene>,
    pill: Option<Pill>,
    scale: f32,
    window_width: f32,
    style: IslandStyle,
    /// Width of the compact pill for what it shows now, in DIPs.
    compact_width: f32,
    /// The last fit worked out, with what it was worked out for.
    fitted: Option<(String, CompactFit)>,
    /// The open view has music along its bottom, which makes it taller.
    strip: bool,
    /// What the pill and the bubble showed last, to see when they trade.
    owner: Option<Kind>,
    aside: Option<Kind>,
}

impl Renderer {
    pub fn new(hwnd: HWND, scale: f32, window_width: f32, style: IslandStyle, wave_motion: WaveMotion, wave_style: WaveStyle) -> Result<Self> {
        let compositor = Compositor::new()?;
        let interop: ICompositorDesktopInterop = compositor.cast()?;
        let target = unsafe { interop.CreateDesktopWindowTarget(hwnd, false)? };

        let root = compositor.CreateContainerVisual()?;
        root.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
        target.SetRoot(&root)?;

        let shape = compositor.CreateShapeVisual()?;
        shape.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
        root.Children()?.InsertAtTop(&shape)?;

        let black = compositor.CreateColorBrushWithColor(Color { A: 255, R: 0, G: 0, B: 0 })?;

        // Added first so it sits under the pill and slides out from behind it.
        let bubble_geometry = compositor.CreateRoundedRectangleGeometry()?;
        let bubble_sprite = compositor.CreateSpriteShapeWithGeometry(&bubble_geometry)?;
        bubble_sprite.SetFillBrush(&black)?;
        shape.Shapes()?.Append(&bubble_sprite)?;

        let geometry = compositor.CreateRoundedRectangleGeometry()?;
        let sprite = compositor.CreateSpriteShapeWithGeometry(&geometry)?;
        sprite.SetFillBrush(&black)?;
        shape.Shapes()?.Append(&sprite)?;

        let backdrop = compositor.CreateHostBackdropBrush()?;
        // A plain blur is still glass, if Windows turns the effects down.
        let glass = glass::brush(&compositor, &backdrop).or_else(|error| {
            eprintln!("glass effects failed: {error}");
            backdrop.cast()
        })?;
        let frosted = [frost(&compositor, &glass, &geometry)?, frost(&compositor, &glass, &bubble_geometry)?];
        for visual in &frosted {
            root.Children()?.InsertAtBottom(visual)?;
        }

        let shoulders = [shoulder(&compositor, 0.0)?, shoulder(&compositor, 1.0)?];
        let sheen = compositor.CreateColorBrushWithColor(NOTCH_TOP_SHADE)?;
        let mut shoulder_tint = Vec::with_capacity(2);
        for visual in &shoulders {
            let tint = compositor.CreateSpriteVisual()?;
            tint.SetBrush(&sheen)?;
            tint.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
            tint.SetIsVisible(false)?;
            visual.Children()?.InsertAtTop(&tint)?;
            root.Children()?.InsertAtTop(visual)?;
            shoulder_tint.push(tint);
        }
        let [left_tint, right_tint]: [SpriteVisual; 2] = shoulder_tint.try_into().map_err(|_| windows::core::Error::empty())?;
        let surfaces = Surfaces {
            shapes: [sprite, bubble_sprite],
            frost: frosted,
            tint: vertical_gradient(&compositor, &[(0.0, GLASS_SHEEN), (SHEEN_END, GLASS_MIDDLE), (1.0, GLASS_BOTTOM)])?,
            rim: diagonal_gradient(&compositor, &RIM)?,
            rim_notch: vertical_gradient(&compositor, &RIM_NOTCH)?,
            shoulder_black: [shoulders[0].Brush()?, shoulders[1].Brush()?],
            shoulder_tint: [left_tint, right_tint],
            glass,
            black,
        };

        let bubble_content = compositor.CreateContainerVisual()?;
        bubble_content.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
        bubble_content.SetClip(&compositor.CreateGeometricClipWithGeometry(&bubble_geometry)?)?;
        root.Children()?.InsertAtTop(&bubble_content)?;
        let bubble_layer = Layer::new(&compositor)?;
        bubble_content.Children()?.InsertAtTop(&bubble_layer.visual)?;
        let bubble = BubbleShape {
            geometry: bubble_geometry,
            visual: bubble_layer.visual,
            brush: bubble_layer.brush,
            shown: None,
            spot: None,
        };

        // Content shares the pill's geometry as a clip, so it is revealed as
        // the pill grows and needs no animation of its own.
        let content = compositor.CreateContainerVisual()?;
        content.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
        content.SetClip(&compositor.CreateGeometricClipWithGeometry(&geometry)?)?;
        root.Children()?.InsertAtTop(&content)?;

        let moving = compositor.CreateContainerVisual()?;
        moving.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
        content.Children()?.InsertAtTop(&moving)?;

        let layers = [Layer::new(&compositor)?, Layer::new(&compositor)?];
        for layer in &layers {
            moving.Children()?.InsertAtTop(&layer.visual)?;
        }
        let wave = Wave::new(&compositor, wave_style)?;
        moving.Children()?.InsertAtTop(&wave.container)?;

        let painter = Painter::new(&compositor)?;
        let mut renderer = Self {
            compositor,
            _target: target,
            hwnd,
            root,
            geometry,
            surfaces,
            material: Material::Black,
            shoulders,
            moving,
            bubble,
            layers,
            front: 0,
            wave,
            wave_motion,
            painter,
            scene: None,
            pill: None,
            scale,
            window_width,
            style,
            compact_width: Pill::COMPACT.width,
            fitted: None,
            strip: false,
            owner: None,
            aside: None,
        };
        renderer.snap_to_hidden()?;
        Ok(renderer)
    }

    pub fn set_metrics(&mut self, scale: f32, window_width: f32) -> Result<()> {
        self.scale = scale;
        self.window_width = window_width;
        self.scene = None;
        self.dress_surfaces()?;
        self.snap_to_hidden()
    }

    /// Windows only blurs behind a window that asks, and it costs the compositor
    /// work for as long as it does, so black leaves the request off.
    pub fn set_material(&mut self, material: Material) -> Result<()> {
        if material == self.material {
            return Ok(());
        }
        self.material = material;
        let glass = i32::from(material == Material::Glass);
        unsafe {
            DwmSetWindowAttribute(
                self.hwnd,
                DWMWA_USE_HOSTBACKDROPBRUSH,
                (&raw const glass).cast(),
                size_of::<i32>() as u32,
            )?;
        }
        self.dress_surfaces()
    }

    fn dress_surfaces(&self) -> Result<()> {
        let glass = self.material == Material::Glass;
        let surfaces = &self.surfaces;
        for visual in &surfaces.frost {
            visual.SetIsVisible(glass)?;
        }
        let rim = match self.style {
            IslandStyle::Island => &surfaces.rim,
            IslandStyle::Notch => &surfaces.rim_notch,
        };
        for shape in &surfaces.shapes {
            if glass {
                shape.SetFillBrush(&surfaces.tint)?;
            } else {
                shape.SetFillBrush(&surfaces.black)?;
            }
            shape.SetStrokeBrush(rim)?;
            shape.SetStrokeThickness(if glass { RIM_WIDTH * self.scale } else { 0.0 })?;
        }
        // Black curves are a painted gradient. Glass ones are the blur itself, cut
        // to the curve, so that they match the pill they lead into.
        let size = NOTCH_SHOULDER * self.scale;
        for (index, (visual, center_x)) in self.shoulders.iter().zip([0.0, 1.0]).enumerate() {
            surfaces.shoulder_tint[index].SetIsVisible(glass)?;
            if glass {
                visual.SetBrush(&surfaces.glass)?;
                visual.SetClip(&shoulder_clip(&self.compositor, size, center_x)?)?;
            } else {
                visual.SetBrush(&surfaces.shoulder_black[index])?;
                visual.SetClip(None::<&CompositionClip>)?;
            }
        }
        Ok(())
    }

    fn shows_shoulders(&self) -> bool {
        self.style == IslandStyle::Notch
    }

    pub fn painter(&self) -> &Painter {
        &self.painter
    }

    pub fn set_wave_motion(&mut self, motion: WaveMotion) -> Result<()> {
        self.wave_motion = motion;
        match self.scene.take() {
            Some(scene) => {
                let updated = self.update_wave(&scene);
                self.scene = Some(scene);
                updated
            }
            None => Ok(()),
        }
    }

    pub fn set_wave_style(&mut self, style: WaveStyle) -> Result<()> {
        self.wave.set_style(&self.compositor, style)
    }

    pub fn set_style(&mut self, style: IslandStyle) -> Result<()> {
        self.style = style;
        self.scene = None;
        self.dress_surfaces()?;
        self.snap_to_hidden()
    }

    /// Where the black shape of a pill goes, and what it is.
    fn outline(&self, pill: Pill) -> (Vector2, Pill) {
        let origin = self.pill_origin(pill);
        if self.style == IslandStyle::Island {
            return (origin, pill);
        }
        // A hidden notch has gone back into the top of the screen.
        let hidden = pill == self.pill_for(Presentation::Hidden);
        let (top, notch) = pill.as_notch(if hidden { 0.0 } else { pill.height });
        (Vector2 { X: origin.X, Y: top }, notch)
    }

    /// The curves of a notch follow its sides with the same spring as its shape.
    fn spring_shoulders(&self, left: f32, shape: Pill, showing: bool, spring: Spring) -> Result<()> {
        let size = NOTCH_SHOULDER * self.scale;
        let [start, end] = &self.shoulders;
        spring_scalar(&self.compositor, start, h!("Offset.X"), left - size, spring)?;
        spring_scalar(&self.compositor, end, h!("Offset.X"), left + shape.width, spring)?;
        for visual in &self.shoulders {
            fade(&self.compositor, visual, if showing { 1.0 } else { 0.0 }, FADE_OUT, Duration::ZERO)?;
        }
        Ok(())
    }

    /// Jumps to the hidden sliver, ready to spring open.
    pub fn snap_to_hidden(&mut self) -> Result<()> {
        for property in [h!("Size"), h!("CornerRadius"), h!("Offset")] {
            self.bubble.geometry.StopAnimation(property)?;
        }
        for property in [h!("Size.X"), h!("Size.Y"), h!("CornerRadius"), h!("Offset.X"), h!("Offset.Y")] {
            self.geometry.StopAnimation(property)?;
        }
        self.root.StopAnimation(h!("Opacity"))?;
        self.root.StopAnimation(h!("Scale"))?;
        self.root.SetOpacity(0.0)?;
        self.root.SetCenterPoint(Vector3 { X: self.window_width / 2.0, Y: 0.0, Z: 0.0 })?;
        self.root.SetScale(Vector3 { X: EMERGE_SCALE, Y: EMERGE_SCALE, Z: 1.0 })?;
        self.moving.StopAnimation(h!("Offset.X"))?;
        self.moving.SetOffset(Vector3::default())?;
        self.wave.hide_now()?;
        self.bubble.geometry.SetSize(Vector2::default())?;
        self.bubble.shown = None;
        self.bubble.spot = None;

        let pill = Pill::HIDDEN.scaled(self.scale);
        let (offset, shape) = self.outline(pill);
        self.geometry.SetSize(Vector2 { X: shape.width, Y: shape.height })?;
        self.geometry.SetCornerRadius(Vector2 { X: shape.radius, Y: shape.radius })?;
        self.geometry.SetOffset(offset)?;
        self.pill = Some(pill);

        let size = NOTCH_SHOULDER * self.scale;
        for (visual, x) in self.shoulders.iter().zip([offset.X - size, offset.X + shape.width]) {
            visual.StopAnimation(h!("Offset.X"))?;
            visual.StopAnimation(h!("Opacity"))?;
            visual.SetOpacity(0.0)?;
            visual.SetSize(Vector2 { X: size, Y: size })?;
            visual.SetOffset(Vector3 { X: x, Y: 0.0, Z: 0.0 })?;
            visual.SetIsVisible(self.shows_shoulders())?;
        }
        Ok(())
    }

    pub fn pill_for(&self, presentation: Presentation) -> Pill {
        let pill = match presentation {
            Presentation::Compact => Pill { width: self.compact_width, ..Pill::COMPACT },
            Presentation::Expanded if self.strip => Pill::EXPANDED_WITH_STRIP,
            other => Pill::for_presentation(other),
        };
        pill.scaled(self.scale)
    }

    /// Left edge of the pill in window pixels, which a bubble pushes left.
    pub fn pill_left(&self, pill: Pill) -> f32 {
        let slide = if self.has_bubble() { self.bubble_shift() } else { 0.0 };
        (self.window_width - pill.width) / 2.0 - slide
    }

    /// Gives way a little under a click, which makes it feel like a real object.
    pub fn press(&self) -> Result<()> {
        let rest = Vector3 { X: 1.0, Y: 1.0, Z: 1.0 };
        let animation = self.compositor.CreateVector3KeyFrameAnimation()?;
        animation.InsertKeyFrame(0.0, rest)?;
        animation.InsertKeyFrame(0.3, Vector3 { X: PRESS_SCALE, Y: PRESS_SCALE, Z: 1.0 })?;
        animation.InsertKeyFrame(1.0, rest)?;
        animation.SetDuration(TimeSpan::from(PRESS_TIME))?;
        self.root.StartAnimation(h!("Scale"), &animation)
    }

    /// A small hop when something finished, such as files reaching a phone.
    pub fn bounce(&self) -> Result<()> {
        let rest = Vector3 { X: 1.0, Y: 1.0, Z: 1.0 };
        let animation = self.compositor.CreateVector3KeyFrameAnimation()?;
        animation.InsertKeyFrame(0.0, rest)?;
        animation.InsertKeyFrame(0.35, Vector3 { X: BOUNCE_SCALE, Y: BOUNCE_SCALE, Z: 1.0 })?;
        animation.InsertKeyFrame(0.7, Vector3 { X: 0.99, Y: 0.99, Z: 1.0 })?;
        animation.InsertKeyFrame(1.0, rest)?;
        animation.SetDuration(TimeSpan::from(BOUNCE_TIME))?;
        self.root.StartAnimation(h!("Scale"), &animation)
    }

    /// Fits the compact pill to the scene. Only measures when its text changed.
    fn fit_compact(&mut self, scene: &mut Scene) {
        let key = scene.fit_key();
        let known = self.fitted.as_ref().filter(|(fitted, _)| *fitted == key).map(|(_, fit)| *fit);
        let fit = known.unwrap_or_else(|| {
            scene.fit(|text, size, bold| self.painter.measure(text, size, bold).unwrap_or(0.0))
        });
        scene.fit = fit;
        self.compact_width = fit.width;
        self.fitted = Some((key, fit));
    }

    /// How round the corners look, which what sits inside them follows.
    fn corner(&self, pill: Pill) -> f32 {
        self.outline(pill).1.radius / self.scale
    }

    /// How far the pill sits left of centre to make room for a bubble.
    fn bubble_shift(&self) -> f32 {
        let bubble = Pill::BUBBLE.scaled(self.scale);
        (bubble.width + BUBBLE_GAP * self.scale) / 2.0
    }

    pub fn has_bubble(&self) -> bool {
        self.bubble.shown.is_some()
    }

    /// Left edge of the bubble in window pixels, given the pill beside it.
    pub fn bubble_left(&self, pill: Pill) -> f32 {
        (self.window_width + pill.width) / 2.0 - self.bubble_shift() + BUBBLE_GAP * self.scale
    }

    pub fn on_bubble(&self, x: i32, y: i32, pill: Pill) -> bool {
        let size = Pill::BUBBLE.scaled(self.scale);
        let left = self.bubble_left(pill);
        let top = self.style.bubble_top(self.scale, pill, size);
        let (x, y) = (x as f32, y as f32);
        self.has_bubble() && x >= left && x <= left + size.width && y >= top && y <= top + size.height
    }

    /// Returns whether the pill changes size, which the window has to allow for.
    pub fn present(&mut self, frame: &Frame, clock: &ClockText, now: Millis, scrub: Option<f32>) -> Result<bool> {
        let hidden = frame.presentation == Presentation::Hidden;
        let mut scene = Scene::new(frame, clock, now, scrub, self.wave_motion != WaveMotion::Hidden);
        if frame.presentation == Presentation::Compact {
            self.fit_compact(&mut scene);
        }
        self.strip = scene.has_strip();
        let pill = self.pill_for(frame.presentation);
        let resized = self.pill != Some(pill);
        let bubble = frame.secondary.as_ref().map(|activity| activity.bubble());
        let bubble_changed = self.bubble.shown.is_some() != bubble.is_some();

        // When the pill hands what it showed to the bubble, or takes it from
        // there, the content shrinks into the bubble or grows out of it.
        let owner = frame.activity.as_ref().map(|activity| activity.kind);
        let aside = frame.secondary.as_ref().map(|activity| activity.kind);
        let trade = Trade {
            into_bubble: self.owner.is_some() && self.owner != owner && self.owner == aside,
            out_of_bubble: owner.is_some() && owner != self.owner && owner == self.aside,
        };
        let new_aside = aside.is_some() && self.aside.is_some() && aside != self.aside;
        (self.owner, self.aside) = if hidden { (None, None) } else { (owner, aside) };

        let was_gone = self.pill == Some(self.pill_for(Presentation::Hidden));
        if !hidden && was_gone {
            fade(&self.compositor, &self.root, 1.0, EMERGE_FADE, Duration::ZERO)?;
            spring_vector3(&self.compositor, &self.root, h!("Scale"), (1.0, 1.0, 1.0), LIVELY)?;
        } else if hidden && !was_gone {
            fade(&self.compositor, &self.root, 0.0, LEAVE_FADE, LEAVE_DELAY)?;
            spring_vector3(&self.compositor, &self.root, h!("Scale"), (LEAVE_SCALE, LEAVE_SCALE, 1.0), CALM)?;
        }

        if resized || bubble_changed {
            let shrinking = self.pill.is_some_and(|shown| pill.width * pill.height < shown.width * shown.height);
            let spring = match (hidden, shrinking) {
                (true, _) => CALM,
                (false, true) => SOFT,
                (false, false) => LIVELY,
            };
            self.update_bubble(bubble, pill, spring, new_aside)?;
            self.spring_pill(pill, spring)?;
            self.pill = Some(pill);
        } else {
            self.update_bubble(bubble, pill, LIVELY, new_aside)?;
        }

        if hidden {
            self.wave.set_moving(&self.compositor, false)?;
            self.wave.set_visible(&self.compositor, false)?;
            fade(&self.compositor, &self.layers[self.front].visual, 0.0, FADE_OUT, Duration::ZERO)?;
            self.scene = None;
            return Ok(resized);
        }

        if self.scene.as_ref() == Some(&scene) {
            return Ok(resized);
        }
        let in_place = self.scene.as_ref().is_some_and(|shown| shown.differs_only_in_value(&scene));
        if in_place {
            self.repaint_front(&scene, pill)?;
        } else {
            self.cross_fade(&scene, pill, resized, trade)?;
        }

        let origin = self.pill_origin(pill);
        self.wave.place(&self.compositor, pill, origin, self.scale)?;
        self.update_wave(&scene)?;
        self.scene = Some(scene);
        Ok(resized)
    }

    fn update_wave(&mut self, scene: &Scene) -> Result<()> {
        let shows = scene.shows_wave() && self.wave_motion != WaveMotion::Hidden;
        self.wave.set_visible(&self.compositor, shows)?;
        self.wave.set_moving(&self.compositor, shows && self.wave_motion == WaveMotion::Always)
    }

    /// Frees painted surfaces while nothing is on screen.
    pub fn release_content(&mut self) -> Result<()> {
        for layer in &mut self.layers {
            layer.brush.SetSurface(None::<&ICompositionSurface>)?;
            layer.visual.SetOpacity(0.0)?;
            layer.surface = None;
        }
        self.painter.forget_images();
        self.scene = None;
        Ok(())
    }

    fn spring_pill(&self, pill: Pill, spring: Spring) -> Result<()> {
        let (offset, shape) = self.outline(pill);
        let compositor = &self.compositor;
        let geometry = &self.geometry;
        let slide = if self.has_bubble() { -self.bubble_shift() } else { 0.0 };
        let tall = trailing(spring);
        spring_scalar(compositor, geometry, h!("Size.X"), shape.width, spring)?;
        spring_scalar(compositor, geometry, h!("Offset.X"), offset.X + slide, spring)?;
        spring_scalar(compositor, geometry, h!("Size.Y"), shape.height, tall)?;
        spring_scalar(compositor, geometry, h!("Offset.Y"), offset.Y, tall)?;
        spring_vector2(compositor, geometry, h!("CornerRadius"), (shape.radius, shape.radius), tall)?;
        if self.style == IslandStyle::Notch {
            let showing = pill != self.pill_for(Presentation::Hidden);
            self.spring_shoulders(offset.X + slide, shape, showing, spring)?;
        }
        spring_scalar(compositor, &self.moving, h!("Offset.X"), slide, spring)
    }

    /// `pop` says the bubble now stands for something else, which it shows by
    /// growing again rather than just changing its picture.
    fn update_bubble(&mut self, wanted: Option<Bubble>, pill: Pill, spring: Spring, pop: bool) -> Result<()> {
        let size = Pill::BUBBLE.scaled(self.scale);
        let compositor = self.compositor.clone();
        let top = self.style.bubble_top(self.scale, pill, size);
        let geometry = &self.bubble.geometry;

        let Some(content) = wanted else {
            if self.bubble.shown.take().is_some() {
                let edge = (self.window_width + pill.width) / 2.0 - size.width / 2.0;
                spring_vector2(&compositor, geometry, h!("Size"), (0.0, 0.0), CALM)?;
                spring_vector2(&compositor, geometry, h!("Offset"), (edge, top + size.height / 2.0), CALM)?;
                fade(&compositor, &self.bubble.visual, 0.0, FADE_OUT, Duration::ZERO)?;
            }
            return Ok(());
        };
        if self.bubble.shown.as_ref() == Some(&content) {
            return self.move_bubble(pill, spring);
        }

        let arriving = self.bubble.shown.is_none();
        let painting = bubble_painting(&content, size.width, self.scale);
        let surface = self.painter.create_surface(size.width, size.height)?;
        self.painter.draw(&surface, &painting)?;
        self.bubble.brush.SetSurface(&surface)?;

        // The left edge is only known once the bubble counts as shown.
        self.bubble.shown = Some(content);
        self.bubble.visual.SetSize(Vector2 { X: size.width, Y: size.height })?;
        if !arriving {
            // A new picture in a bubble that is already there follows the pill,
            // shape and all, as the same picture would.
            if pop {
                let visual = &self.bubble.visual;
                visual.SetCenterPoint(Vector3 { X: size.width / 2.0, Y: size.height / 2.0, Z: 0.0 })?;
                visual.SetScale(Vector3 { X: BUBBLE_START_SCALE, Y: BUBBLE_START_SCALE, Z: 1.0 })?;
                spring_vector3(&compositor, visual, h!("Scale"), (1.0, 1.0, 1.0), LIVELY)?;
            }
            return self.move_bubble(pill, spring);
        }
        let left = self.bubble_left(pill);
        self.bubble.spot = Some((left, top));
        self.bubble.visual.SetOffset(Vector3 { X: left, Y: top, Z: 0.0 })?;

        {
            let geometry = &self.bubble.geometry;
            let edge = left - BUBBLE_GAP * self.scale - size.width / 2.0;
            let (shape_top, shape) = match self.style {
                IslandStyle::Island => (top, size),
                IslandStyle::Notch => size.as_notch(size.height),
            };
            geometry.SetSize(Vector2::default())?;
            geometry.SetOffset(Vector2 { X: edge, Y: top + size.height / 2.0 })?;
            spring_vector2(&compositor, geometry, h!("Size"), (shape.width, shape.height), spring)?;
            spring_vector2(&compositor, geometry, h!("CornerRadius"), (shape.radius, shape.radius), spring)?;
            spring_vector2(&compositor, geometry, h!("Offset"), (left, shape_top), spring)?;
            let visual = &self.bubble.visual;
            visual.SetCenterPoint(Vector3 { X: size.width / 2.0, Y: size.height / 2.0, Z: 0.0 })?;
            visual.SetScale(Vector3 { X: BUBBLE_START_SCALE, Y: BUBBLE_START_SCALE, Z: 1.0 })?;
            spring_vector3(&compositor, visual, h!("Scale"), (1.0, 1.0, 1.0), spring)?;
            fade(&compositor, visual, 1.0, FADE_IN, FADE_IN_DELAY)?;
        }
        Ok(())
    }

    /// Keeps a bubble beside a pill that changed size.
    fn move_bubble(&mut self, pill: Pill, spring: Spring) -> Result<()> {
        let size = Pill::BUBBLE.scaled(self.scale);
        let spot = (self.bubble_left(pill), self.style.bubble_top(self.scale, pill, size));
        if self.bubble.spot == Some(spot) {
            return Ok(());
        }
        self.bubble.spot = Some(spot);
        let (left, top) = spot;
        let shape_top = match self.style {
            IslandStyle::Island => top,
            IslandStyle::Notch => size.as_notch(size.height).0,
        };
        spring_vector2(&self.compositor, &self.bubble.geometry, h!("Offset"), (left, shape_top), spring)?;
        spring_scalar(&self.compositor, &self.bubble.visual, h!("Offset.X"), left, spring)?;
        spring_scalar(&self.compositor, &self.bubble.visual, h!("Offset.Y"), top, spring)
    }

    fn pill_origin(&self, pill: Pill) -> Vector2 {
        Vector2 { X: (self.window_width - pill.width) / 2.0, Y: self.style.margin() * self.scale }
    }

    /// Updates running time without any fade, which would flicker every second.
    fn repaint_front(&mut self, scene: &Scene, pill: Pill) -> Result<()> {
        let painting = scene.painting(pill, self.scale, self.corner(pill), self.style == IslandStyle::Notch);
        match &self.layers[self.front].surface {
            Some(surface) => self.painter.draw(surface, &painting),
            None => self.cross_fade(scene, pill, false, Trade::default()),
        }
    }

    fn cross_fade(&mut self, scene: &Scene, pill: Pill, resized: bool, trade: Trade) -> Result<()> {
        let painting = scene.painting(pill, self.scale, self.corner(pill), self.style == IslandStyle::Notch);
        let surface = self.painter.create_surface(pill.width, pill.height)?;
        self.painter.draw(&surface, &painting)?;

        let origin = self.pill_origin(pill);
        let incoming = &mut self.layers[1 - self.front];
        incoming.brush.SetSurface(&surface)?;
        incoming.surface = Some(surface);
        incoming.visual.SetSize(Vector2 { X: pill.width, Y: pill.height })?;
        incoming.visual.SetOffset(Vector3 { X: origin.X, Y: origin.Y, Z: 0.0 })?;
        incoming.visual.StopAnimation(h!("Scale"))?;
        // The bubble sits on the right, so content from it grows out of there.
        let (center, start) = match (trade.out_of_bubble, resized) {
            (true, _) => (Vector3 { X: pill.width, Y: pill.height / 2.0, Z: 0.0 }, TRADE_SCALE),
            (false, true) => (Vector3 { X: pill.width / 2.0, Y: 0.0, Z: 0.0 }, CONTENT_START_SCALE),
            (false, false) => (Vector3 { X: pill.width / 2.0, Y: 0.0, Z: 0.0 }, 1.0),
        };
        incoming.visual.SetCenterPoint(center)?;
        incoming.visual.SetScale(Vector3 { X: start, Y: start, Z: 1.0 })?;
        if start < 1.0 {
            spring_vector3(&self.compositor, &incoming.visual, h!("Scale"), (1.0, 1.0, 1.0), LIVELY)?;
        }
        fade(&self.compositor, &incoming.visual, 1.0, FADE_IN, FADE_IN_DELAY)?;

        let outgoing = &self.layers[self.front];
        fade(&self.compositor, &outgoing.visual, 0.0, FADE_OUT, Duration::ZERO)?;
        if trade.into_bubble {
            let size = outgoing.visual.Size()?;
            outgoing.visual.SetCenterPoint(Vector3 { X: size.X, Y: size.Y / 2.0, Z: 0.0 })?;
            spring_vector3(&self.compositor, &outgoing.visual, h!("Scale"), (TRADE_SCALE, TRADE_SCALE, 1.0), SOFT)?;
        } else if resized {
            let end = (CONTENT_END_SCALE, CONTENT_END_SCALE, 1.0);
            spring_vector3(&self.compositor, &outgoing.visual, h!("Scale"), end, SOFT)?;
        }

        self.front = 1 - self.front;
        Ok(())
    }
}
