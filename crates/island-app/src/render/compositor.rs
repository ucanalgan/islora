use std::mem::size_of;
use std::time::Duration;

use island_core::layout::{BUBBLE_GAP, NOTCH_SHOULDER};
use island_core::{Bubble, Frame, IslandStyle, Millis, Pill, Presentation};
use windows::Foundation::TimeSpan;
use windows::System::DispatcherQueueController;
use windows::UI::Color;
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::UI::Composition::{
    AnimationIterationBehavior, CompositionDrawingSurface, CompositionRoundedRectangleGeometry,
    CompositionStretch, CompositionSurfaceBrush, Compositor, ContainerVisual, ICompositionSurface,
    SpriteVisual,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
use windows::Win32::System::WinRT::{
    CreateDispatcherQueueController, DQTAT_COM_STA, DQTYPE_THREAD_CURRENT, DispatcherQueueOptions,
};
use windows::core::{Interface, Result, h};
use windows_numerics::{Vector2, Vector3};

use super::scene::{ClockText, CompactFit, Scene, WAVE_RESERVED, bubble_painting};
use super::spring::{CALM, LIVELY, Spring, fade, spring_scalar, spring_vector2};
use super::text::Painter;

const FADE_IN: Duration = Duration::from_millis(180);
const FADE_OUT: Duration = Duration::from_millis(90);
const FADE_IN_DELAY: Duration = Duration::from_millis(70);

const WAVE_BARS: usize = 4;
const WAVE_BAR_WIDTH: f32 = 3.0;
const WAVE_BAR_GAP: f32 = 3.0;
const WAVE_BAR_HEIGHT: f32 = 15.0;
/// Resting height of a bar relative to its full height.
const WAVE_LOW: f32 = 0.38;
/// Where a notch's curve starts to turn solid, which softens its edge.
const SHOULDER_EDGE: f32 = 0.92;

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
#[derive(Clone, Copy, PartialEq)]
pub enum WaveMotion {
    Always,
    Still,
}

/// Equaliser bars that show sound is playing.
struct Wave {
    container: ContainerVisual,
    bars: Vec<SpriteVisual>,
    visible: bool,
    moving: bool,
}

impl Wave {
    fn new(compositor: &Compositor) -> Result<Self> {
        let container = compositor.CreateContainerVisual()?;
        container.SetIsVisible(false)?;
        let white = compositor.CreateColorBrushWithColor(Color { A: 220, R: 255, G: 255, B: 255 })?;
        let mut bars = Vec::with_capacity(WAVE_BARS);
        for index in 0..WAVE_BARS {
            let bar = compositor.CreateSpriteVisual()?;
            bar.SetBrush(&white)?;
            bar.SetScale(rest_scale(index))?;
            container.Children()?.InsertAtTop(&bar)?;
            bars.push(bar);
        }
        Ok(Self { container, bars, visible: false, moving: false })
    }

    fn place(&self, pill: Pill, origin: Vector2, scale: f32) -> Result<()> {
        let total = WAVE_BAR_WIDTH * WAVE_BARS as f32 + WAVE_BAR_GAP * (WAVE_BARS - 1) as f32;
        let left = (WAVE_RESERVED - total) / 2.0 - 6.0;
        let top = (pill.height / scale - WAVE_BAR_HEIGHT) / 2.0;
        self.container.SetOffset(Vector3 {
            X: origin.X + pill.width - (WAVE_RESERVED - left) * scale,
            Y: origin.Y + top * scale,
            Z: 0.0,
        })?;
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

    fn set_visible(&mut self, visible: bool) -> Result<()> {
        if self.visible != visible {
            self.visible = visible;
            self.container.SetIsVisible(visible)?;
        }
        Ok(())
    }

    fn set_moving(&mut self, compositor: &Compositor, moving: bool) -> Result<()> {
        if self.moving == moving {
            return Ok(());
        }
        self.moving = moving;
        for (index, bar) in self.bars.iter().enumerate() {
            if !moving {
                bar.StopAnimation(h!("Scale.Y"))?;
                bar.SetScale(rest_scale(index))?;
                continue;
            }
            let animation = compositor.CreateScalarKeyFrameAnimation()?;
            animation.InsertKeyFrame(0.0, WAVE_LOW)?;
            animation.InsertKeyFrame(0.5, 1.0)?;
            animation.InsertKeyFrame(1.0, WAVE_LOW)?;
            let period = Duration::from_millis(640 + index as u64 * 170);
            animation.SetDuration(TimeSpan::from(period))?;
            animation.SetIterationBehavior(AnimationIterationBehavior::Forever)?;
            bar.StartAnimation(h!("Scale.Y"), &animation)?;
        }
        Ok(())
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

pub struct Renderer {
    compositor: Compositor,
    _target: DesktopWindowTarget,
    geometry: CompositionRoundedRectangleGeometry,
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
}

impl Renderer {
    pub fn new(hwnd: HWND, scale: f32, window_width: f32, style: IslandStyle, wave_motion: WaveMotion) -> Result<Self> {
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

        let shoulders = [shoulder(&compositor, 0.0)?, shoulder(&compositor, 1.0)?];
        for visual in &shoulders {
            root.Children()?.InsertAtTop(visual)?;
        }

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
        let wave = Wave::new(&compositor)?;
        moving.Children()?.InsertAtTop(&wave.container)?;

        let painter = Painter::new(&compositor)?;
        let mut renderer = Self {
            compositor,
            _target: target,
            geometry,
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
        };
        renderer.snap_to_hidden()?;
        Ok(renderer)
    }

    pub fn set_metrics(&mut self, scale: f32, window_width: f32) -> Result<()> {
        self.scale = scale;
        self.window_width = window_width;
        self.scene = None;
        self.snap_to_hidden()
    }

    pub fn set_style(&mut self, style: IslandStyle) -> Result<()> {
        self.style = style;
        self.scene = None;
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
            self.geometry.StopAnimation(property)?;
            self.bubble.geometry.StopAnimation(property)?;
        }
        self.moving.StopAnimation(h!("Offset.X"))?;
        self.moving.SetOffset(Vector3::default())?;
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
            visual.SetIsVisible(self.style == IslandStyle::Notch)?;
        }
        Ok(())
    }

    pub fn pill_for(&self, presentation: Presentation) -> Pill {
        let pill = match presentation {
            Presentation::Compact => Pill { width: self.compact_width, ..Pill::COMPACT },
            other => Pill::for_presentation(other),
        };
        pill.scaled(self.scale)
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
        let mut scene = Scene::new(frame, clock, now, scrub);
        if frame.presentation == Presentation::Compact {
            self.fit_compact(&mut scene);
        }
        let pill = self.pill_for(frame.presentation);
        let resized = self.pill != Some(pill);
        let bubble = frame.secondary.as_ref().map(|activity| activity.bubble());
        let bubble_changed = self.bubble.shown.is_some() != bubble.is_some();

        if resized || bubble_changed {
            let spring = if hidden { CALM } else { LIVELY };
            self.update_bubble(bubble, pill, spring)?;
            self.spring_pill(pill, spring)?;
            self.pill = Some(pill);
        } else {
            self.update_bubble(bubble, pill, LIVELY)?;
        }

        if hidden {
            self.wave.set_moving(&self.compositor, false)?;
            self.wave.set_visible(false)?;
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
            self.cross_fade(&scene, pill)?;
        }

        let origin = self.pill_origin(pill);
        self.wave.place(pill, origin, self.scale)?;
        self.update_wave(&scene)?;
        self.scene = Some(scene);
        Ok(resized)
    }

    fn update_wave(&mut self, scene: &Scene) -> Result<()> {
        let shows = scene.shows_wave();
        self.wave.set_visible(shows)?;
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
        spring_vector2(compositor, geometry, h!("Size"), (shape.width, shape.height), spring)?;
        spring_vector2(compositor, geometry, h!("CornerRadius"), (shape.radius, shape.radius), spring)?;
        spring_vector2(compositor, geometry, h!("Offset"), (offset.X + slide, offset.Y), spring)?;
        if self.style == IslandStyle::Notch {
            let showing = pill != self.pill_for(Presentation::Hidden);
            self.spring_shoulders(offset.X + slide, shape, showing, spring)?;
        }
        spring_scalar(compositor, &self.moving, h!("Offset.X"), slide, spring)
    }

    fn update_bubble(&mut self, wanted: Option<Bubble>, pill: Pill, spring: Spring) -> Result<()> {
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
        let left = self.bubble_left(pill);
        self.bubble.spot = Some((left, top));
        self.bubble.visual.SetSize(Vector2 { X: size.width, Y: size.height })?;
        self.bubble.visual.SetOffset(Vector3 { X: left, Y: top, Z: 0.0 })?;

        if arriving {
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
            fade(&compositor, &self.bubble.visual, 1.0, FADE_IN, FADE_IN_DELAY)?;
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
        let painting = scene.painting(pill, self.scale, self.corner(pill));
        match &self.layers[self.front].surface {
            Some(surface) => self.painter.draw(surface, &painting),
            None => self.cross_fade(scene, pill),
        }
    }

    fn cross_fade(&mut self, scene: &Scene, pill: Pill) -> Result<()> {
        let painting = scene.painting(pill, self.scale, self.corner(pill));
        let surface = self.painter.create_surface(pill.width, pill.height)?;
        self.painter.draw(&surface, &painting)?;

        let origin = self.pill_origin(pill);
        let incoming = &mut self.layers[1 - self.front];
        incoming.brush.SetSurface(&surface)?;
        incoming.surface = Some(surface);
        incoming.visual.SetSize(Vector2 { X: pill.width, Y: pill.height })?;
        incoming.visual.SetOffset(Vector3 { X: origin.X, Y: origin.Y, Z: 0.0 })?;
        fade(&self.compositor, &incoming.visual, 1.0, FADE_IN, FADE_IN_DELAY)?;

        let outgoing = &self.layers[self.front];
        fade(&self.compositor, &outgoing.visual, 0.0, FADE_OUT, Duration::ZERO)?;

        self.front = 1 - self.front;
        Ok(())
    }
}
