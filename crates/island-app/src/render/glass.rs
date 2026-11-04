//! What glass shows: Windows' blur of what lies behind the window, with its
//! colours deepened and its brightness held at one level, the way Windows' own
//! acrylic does. Holding the brightness is what keeps white text readable over a
//! white page while colour still comes through.

use std::sync::{Mutex, PoisonError};

use windows::Foundation::{IPropertyValue, PropertyValue};
use windows::Graphics::Effects::{
    IGraphicsEffect, IGraphicsEffect_Impl, IGraphicsEffectSource, IGraphicsEffectSource_Impl,
};
use windows::UI::Composition::{
    CompositionBackdropBrush, CompositionBrush, CompositionEffectSourceParameter, Compositor,
};
use windows::Win32::Foundation::{E_BOUNDS, E_INVALIDARG};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_BLEND_MODE_LUMINOSITY, D2D1_COLORMATRIX_ALPHA_MODE_PREMULTIPLIED,
};
use windows::Win32::Graphics::Direct2D::{CLSID_D2D1Blend, CLSID_D2D1ColorMatrix, CLSID_D2D1Flood};
use windows::Win32::System::WinRT::Graphics::Direct2D::{
    GRAPHICS_EFFECT_PROPERTY_MAPPING, IGraphicsEffectD2D1Interop, IGraphicsEffectD2D1Interop_Impl,
};
use windows::core::{GUID, HSTRING, Interface, PCWSTR, Result, h, implement};

/// How much more colourful the blur is than what it shows.
const SATURATION: f32 = 1.8;
/// The brightness the whole glass is held at, as a grey. Dark enough for white
/// text, light enough for the colours behind to read as colours.
const LEVEL: [f32; 4] = [0.30, 0.30, 0.32, 1.0];

/// One Direct2D effect, described the way the compositor reads effects.
#[implement(IGraphicsEffect, IGraphicsEffectSource, IGraphicsEffectD2D1Interop)]
struct Effect {
    id: GUID,
    properties: Vec<IPropertyValue>,
    sources: Vec<IGraphicsEffectSource>,
    name: Mutex<HSTRING>,
}

impl IGraphicsEffect_Impl for Effect_Impl {
    fn Name(&self) -> Result<HSTRING> {
        Ok(self.name.lock().unwrap_or_else(PoisonError::into_inner).clone())
    }

    fn SetName(&self, name: &HSTRING) -> Result<()> {
        *self.name.lock().unwrap_or_else(PoisonError::into_inner) = name.clone();
        Ok(())
    }
}

impl IGraphicsEffectSource_Impl for Effect_Impl {}

impl IGraphicsEffectD2D1Interop_Impl for Effect_Impl {
    fn GetEffectId(&self) -> Result<GUID> {
        Ok(self.id)
    }

    fn GetNamedPropertyMapping(
        &self,
        _name: &PCWSTR,
        _index: *mut u32,
        _mapping: *mut GRAPHICS_EFFECT_PROPERTY_MAPPING,
    ) -> Result<()> {
        Err(E_INVALIDARG.into())
    }

    fn GetPropertyCount(&self) -> Result<u32> {
        Ok(self.properties.len() as u32)
    }

    fn GetProperty(&self, index: u32) -> Result<IPropertyValue> {
        self.properties.get(index as usize).cloned().ok_or_else(|| E_BOUNDS.into())
    }

    fn GetSource(&self, index: u32) -> Result<IGraphicsEffectSource> {
        self.sources.get(index as usize).cloned().ok_or_else(|| E_BOUNDS.into())
    }

    fn GetSourceCount(&self) -> Result<u32> {
        Ok(self.sources.len() as u32)
    }
}

fn effect(id: GUID, properties: Vec<IPropertyValue>, sources: Vec<IGraphicsEffectSource>) -> Result<IGraphicsEffectSource> {
    let effect: IGraphicsEffect = Effect { id, properties, sources, name: Mutex::new(HSTRING::new()) }.into();
    effect.cast()
}

fn floats(values: &[f32]) -> Result<IPropertyValue> {
    PropertyValue::CreateSingleArray(values)?.cast()
}

fn number(value: u32) -> Result<IPropertyValue> {
    PropertyValue::CreateUInt32(value)?.cast()
}

/// Rows are the red, green, blue and alpha that go in, columns what comes out.
fn saturation_matrix(saturation: f32) -> [f32; 20] {
    let rest = 1.0 - saturation;
    let [red, green, blue] = [0.2126 * rest, 0.7152 * rest, 0.0722 * rest];
    [
        red + saturation, red, red, 0.0,
        green, green + saturation, green, 0.0,
        blue, blue, blue + saturation, 0.0,
        0.0, 0.0, 0.0, 1.0,
        0.0, 0.0, 0.0, 0.0,
    ]
}

pub fn brush(compositor: &Compositor, backdrop: &CompositionBackdropBrush) -> Result<CompositionBrush> {
    let behind: IGraphicsEffectSource = CompositionEffectSourceParameter::Create(h!("backdrop"))?.cast()?;
    let vivid = effect(
        CLSID_D2D1ColorMatrix,
        vec![
            floats(&saturation_matrix(SATURATION))?,
            number(D2D1_COLORMATRIX_ALPHA_MODE_PREMULTIPLIED.0 as u32)?,
            PropertyValue::CreateBoolean(true)?.cast()?,
        ],
        vec![behind],
    )?;
    let level = effect(CLSID_D2D1Flood, vec![floats(&LEVEL)?], Vec::new())?;
    // The colour of what is behind, at the brightness of the level. The blend
    // takes its colour from the first input and its brightness from the second.
    let blend = effect(CLSID_D2D1Blend, vec![number(D2D1_BLEND_MODE_LUMINOSITY.0 as u32)?], vec![level, vivid])?;
    let factory = compositor.CreateEffectFactory(&blend.cast::<IGraphicsEffect>()?)?;
    let brush = factory.CreateBrush()?;
    brush.SetSourceParameter(h!("backdrop"), backdrop)?;
    brush.cast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grey_stays_grey_at_any_saturation() {
        let matrix = saturation_matrix(SATURATION);
        let grey = 0.5;
        for column in 0..3 {
            let out = grey * (matrix[column] + matrix[4 + column] + matrix[8 + column]);
            assert!((out - grey).abs() < 1e-5);
        }
    }

    #[test]
    fn a_saturation_of_one_changes_nothing() {
        let matrix = saturation_matrix(1.0);
        for row in 0..4 {
            for column in 0..4 {
                let expected = if row == column { 1.0 } else { 0.0 };
                assert!((matrix[row * 4 + column] - expected).abs() < 1e-6);
            }
        }
    }
}
