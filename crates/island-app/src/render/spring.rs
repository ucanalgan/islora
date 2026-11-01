use std::time::Duration;

use windows::Foundation::TimeSpan;
use windows::UI::Composition::{CompositionObject, Compositor};
use windows::core::{HSTRING, Interface, Result, h};

#[derive(Clone, Copy)]
pub struct Spring {
    pub damping: f32,
    pub period: Duration,
}

/// Slight overshoot, used for everything that appears or grows.
pub const LIVELY: Spring = Spring { damping: 0.74, period: Duration::from_millis(70) };
/// No overshoot, used on the way out.
pub const CALM: Spring = Spring { damping: 1.0, period: Duration::from_millis(55) };

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

pub fn fade(
    compositor: &Compositor,
    target: &impl Interface,
    opacity: f32,
    duration: Duration,
    delay: Duration,
) -> Result<()> {
    let animation = compositor.CreateScalarKeyFrameAnimation()?;
    animation.InsertKeyFrame(1.0, opacity)?;
    animation.SetDuration(TimeSpan::from(duration))?;
    animation.SetDelayTime(TimeSpan::from(delay))?;
    target.cast::<CompositionObject>()?.StartAnimation(h!("Opacity"), &animation)
}
