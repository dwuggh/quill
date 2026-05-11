/// The geometry of brush. The simplest model is one circle.
/// ultimately we want it to be a 3D shape projecting to the 2D plane,
/// parametried by altitude angle and pressure. Thus the circle is a projection
/// of a sphere, representing a ballpoint pen. we can further model other pen.
///
/// NOTE Google's ink library models brush tip by convex hull of at most 4 circles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BrushTip {
    Round(RoundTip),
    // ConvexPolygon(PolygonBrush),
}

#[derive(Clone, Copy, Debug)]
pub struct StrokePaint {
    pub color: [f32; 4],
    pub width: f32,
    pub sharpness: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Brush {
    pub tip: BrushTip,
    pub paint: StrokePaint,
}

/// Ball point pen parameters.
///
/// This backend does not need a brush vertex buffer. The shader reads adjacent
/// path points and draws one analytic capsule per segment.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RoundTip {
    /// Base circle radius in path/world units.
    pub radius: OrderedFloat<f32>,

    /// Pressure response mode.
    ///
    /// `0` keeps a fixed radius. `1` multiplies radius by path pressure.
    pub pressure_mode: u32,

    pub _pad0: [u32; 2],
}

impl RoundTip {
    pub fn new(radius: f32) -> Self {
        Self {
            radius: OrderedFloat(radius),
            pressure_mode: 1,
            _pad0: [0; 2],
        }
    }
}

impl BrushTip {
    pub fn round(radius: f32) -> Self {
        Self::Round(RoundTip::new(radius))
    }
}

use ordered_float::OrderedFloat;
