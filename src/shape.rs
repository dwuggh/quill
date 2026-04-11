use crate::{Brush, BrushTip, ModeledInput};
use kurbo::{Point, Rect};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorShift {
    pub hue_offset_turns: f32,
    pub saturation_multiplier: f32,
    pub luminosity_offset: f32,
    pub opacity_multiplier: f32,
}

impl Default for ColorShift {
    fn default() -> Self {
        Self {
            hue_offset_turns: 0.0,
            saturation_multiplier: 1.0,
            luminosity_offset: 0.0,
            opacity_multiplier: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TipState {
    pub position: Point,
    pub width: f64,
    pub height: f64,
    pub corner_rounding: f64,
    pub rotation: f64,
    pub slant: f64,
    pub pinch: f64,
    pub color_shift: ColorShift,
    pub texture_animation_progress_offset: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Circle {
    pub center: Point,
    pub radius: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TipShape {
    pub center: Point,
    pub perimeter_circles: Vec<Circle>,
    pub bounds: Rect,
}

impl TipShape {
    pub fn from_tip_state(state: &TipState) -> Self {
        let radius = 0.5 * state.width.max(state.height);
        let circle = Circle {
            center: state.position,
            radius,
        };
        Self {
            center: state.position,
            perimeter_circles: vec![circle],
            bounds: Rect::new(
                state.position.x - radius,
                state.position.y - radius,
                state.position.x + radius,
                state.position.y + radius,
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TangentQuality {
    FirstContainsSecond,
    SecondContainsFirst,
    BadJoinedCoverage,
    Good,
}

pub(crate) fn tip_state_for_input(input: &ModeledInput, brush: &Brush, tip: &BrushTip) -> TipState {
    let pressure_width = input.pressure.unwrap_or(1.0).clamp(0.0, 1.0);
    let pressure_width = 0.45 + 0.75 * pressure_width;
    TipState {
        position: input.position,
        width: brush.size * tip.scale.x * pressure_width,
        height: brush.size * tip.scale.y,
        corner_rounding: tip.corner_rounding,
        rotation: tip.rotation,
        slant: tip.slant,
        pinch: tip.pinch,
        color_shift: ColorShift::default(),
        texture_animation_progress_offset: 0.0,
    }
}
