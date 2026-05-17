use ordered_float::OrderedFloat;
use peniko::Color;

/// The geometry of brush. The simplest model is one circle.
/// ultimately we want it to be a 3D shape projecting to the 2D plane,
/// parametried by altitude angle and pressure. Thus the circle is a projection
/// of a sphere, representing a ballpoint pen. we can further model other pen.
///
/// NOTE Google's ink library models brush tip by convex hull of at most 4 circles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BrushTip {
    Round(RoundTip),
    FixedRound(RoundTip),
    // ConvexPolygon(PolygonBrush),
}

#[derive(Clone, Copy, Debug)]
pub struct StrokePaint {
    pub color: Color,
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
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RoundTip {
    /// Base circle radius in path/world units.
    pub radius: OrderedFloat<f32>,
}

impl RoundTip {
    pub fn new(radius: f32) -> Self {
        Self {
            radius: OrderedFloat(radius),
        }
    }
}

impl BrushTip {
    pub fn round(radius: f32) -> Self {
        Self::Round(RoundTip::new(radius))
    }
}
