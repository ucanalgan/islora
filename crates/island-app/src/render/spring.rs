use std::time::Duration;

use windows::Foundation::TimeSpan;
use windows::UI::Composition::{CompositionObject, Compositor};
use windows::core::{HSTRING, Interface, Result, h};
use windows_numerics::Vector2;

#[derive(Clone, Copy)]
pub struct Spring {
    pub damping: f32,
    pub period: Duration,
}

/// Slight overshoot, used for everything that appears or grows.
pub const LIVELY: Spring = Spring { damping: 0.8, period: Duration::from_millis(80) };
/// Barely any overshoot. A pill getting smaller should settle, not bounce.
pub const SOFT: Spring = Spring { damping: 0.92, period: Duration::from_millis(72) };
/// No overshoot, used on the way out.
pub const CALM: Spring = Spring { damping: 1.0, period: Duration::from_millis(52) };

/// Height lags width a little, which makes a change of shape read as one soft
/// motion rather than a rectangle being resized.
pub fn trailing(spring: Spring) -> Spring {
    Spring { period: spring.period.mul_f32(1.18), ..spring }
}

/// Hands a Vector2 property to the system compositor. Nothing runs in this
/// process until the next change, so an animation costs no per-frame work here.
pub fn spring_vector2(
    compositor: &Compositor,
    target: &impl Interface,
    property: &HSTRING,
    (x, y): (f32, f32),
    spring: Spring,
) -> Result<()> {
    let animation = compositor.CreateSpringVector2Animation()?;
    animation.SetDampingRatio(spring.damping)?;
    animation.SetPeriod(TimeSpan::from(spring.period))?;
    animation
        .InitialValueExpressions()?
        .Insert(h!("FinalValue"), &format!("Vector2({x}, {y})").into())?;
    target.cast::<CompositionObject>()?.StartAnimation(property, &animation)
}

pub fn spring_vector3(
    compositor: &Compositor,
    target: &impl Interface,
    property: &HSTRING,
    (x, y, z): (f32, f32, f32),
    spring: Spring,
) -> Result<()> {
    let animation = compositor.CreateSpringVector3Animation()?;
    animation.SetDampingRatio(spring.damping)?;
    animation.SetPeriod(TimeSpan::from(spring.period))?;
    animation
        .InitialValueExpressions()?
        .Insert(h!("FinalValue"), &format!("Vector3({x}, {y}, {z})").into())?;
    target.cast::<CompositionObject>()?.StartAnimation(property, &animation)
}

pub fn spring_scalar(
    compositor: &Compositor,
    target: &impl Interface,
    property: &HSTRING,
    value: f32,
    spring: Spring,
) -> Result<()> {
    let animation = compositor.CreateSpringScalarAnimation()?;
    animation.SetDampingRatio(spring.damping)?;
    animation.SetPeriod(TimeSpan::from(spring.period))?;
    animation.InitialValueExpressions()?.Insert(h!("FinalValue"), &format!("{value}").into())?;
    target.cast::<CompositionObject>()?.StartAnimation(property, &animation)
}

/// Starts quickly and eases out, so a change reads at once and lands softly.
pub fn fade(
    compositor: &Compositor,
    target: &impl Interface,
    opacity: f32,
    duration: Duration,
    delay: Duration,
) -> Result<()> {
    let easing = compositor
        .CreateCubicBezierEasingFunction(Vector2 { X: 0.2, Y: 0.0 }, Vector2 { X: 0.0, Y: 1.0 })?;
    let animation = compositor.CreateScalarKeyFrameAnimation()?;
    animation.InsertKeyFrameWithEasingFunction(1.0, opacity, &easing)?;
    animation.SetDuration(TimeSpan::from(duration))?;
    animation.SetDelayTime(TimeSpan::from(delay))?;
    target.cast::<CompositionObject>()?.StartAnimation(h!("Opacity"), &animation)
}
