use crate::{InputModelConfig, StrokeError};
use kurbo::Vec2;
use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorRgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl ColorRgba {
    pub const BLACK: Self = Self {
        r: 0.02,
        g: 0.02,
        b: 0.018,
        a: 1.0,
    };
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BlendMode {
    SourceOver,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushPaint {
    pub blend_mode: BlendMode,
}

impl Default for BrushPaint {
    fn default() -> Self {
        Self {
            blend_mode: BlendMode::SourceOver,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BrushBehavior {
    pub developer_comment: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BrushTip {
    pub scale: Vec2,
    pub corner_rounding: f64,
    pub slant: f64,
    pub pinch: f64,
    pub rotation: f64,
    pub particle_gap_distance_scale: f64,
    pub particle_gap_duration: f64,
    pub behaviors: Vec<BrushBehavior>,
}

impl Default for BrushTip {
    fn default() -> Self {
        Self {
            scale: Vec2::new(1.0, 1.0),
            corner_rounding: 1.0,
            slant: 0.0,
            pinch: 0.0,
            rotation: 0.0,
            particle_gap_distance_scale: 0.0,
            particle_gap_duration: 0.0,
            behaviors: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BrushCoat {
    pub tip: BrushTip,
    pub paint: BrushPaint,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BrushFamily {
    pub coats: Vec<BrushCoat>,
    pub input_model: InputModelConfig,
}

impl Default for BrushFamily {
    fn default() -> Self {
        Self {
            coats: vec![BrushCoat::default()],
            input_model: InputModelConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Brush {
    pub family: BrushFamily,
    pub color: ColorRgba,
    pub size: f64,
    pub epsilon: f64,
}

impl Brush {
    pub fn new(size: f64, color: ColorRgba) -> Result<Self, StrokeError> {
        let brush = Self {
            family: BrushFamily::default(),
            color,
            size,
            epsilon: (size * 0.03).max(0.1),
        };
        brush.validate()?;
        Ok(brush)
    }

    pub fn validate(&self) -> Result<(), StrokeError> {
        if !self.size.is_finite() || !self.epsilon.is_finite() || self.size <= 0.0 {
            return Err(StrokeError::InvalidBrush(
                "brush size and epsilon must be finite and positive",
            ));
        }
        if self.epsilon <= 0.0 || self.epsilon > self.size {
            return Err(StrokeError::InvalidBrush(
                "brush epsilon must be in (0, size]",
            ));
        }
        if self.family.coats.is_empty() {
            return Err(StrokeError::InvalidBrush(
                "brush family must contain at least one coat",
            ));
        }
        for coat in &self.family.coats {
            let scale = coat.tip.scale;
            if !scale.x.is_finite()
                || !scale.y.is_finite()
                || scale.x < 0.0
                || scale.y < 0.0
                || (scale.x == 0.0 && scale.y == 0.0)
            {
                return Err(StrokeError::InvalidBrush(
                    "brush tip scale must be finite and non-negative",
                ));
            }
            if !(0.0..=1.0).contains(&coat.tip.corner_rounding)
                || !(0.0..=1.0).contains(&coat.tip.pinch)
                || coat.tip.slant.abs() > PI * 0.5
            {
                return Err(StrokeError::InvalidBrush(
                    "brush tip shape parameters are out of range",
                ));
            }
        }
        Ok(())
    }
}
