#[path = "input/will.rs"]
mod will;

pub use will::{WillInputModeler, WillInputModelerConfig};

pub enum PenType {
    Stylus,
}

#[derive(Debug, Clone, Copy)]
pub struct Input {
    x: f32,
    y: f32,
    timestamp: f64,
    /// normally scale in 1/8192 or 1/16384
    pressure: f32,
    tilt_x: f32,
    tilt_y: f32,
}

impl Input {
    pub fn new(x: f32, y: f32, timestamp: f64) -> Self {
        Self {
            x,
            y,
            timestamp,
            pressure: 1.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
        }
    }

    pub fn with_pressure(mut self, pressure: f32) -> Self {
        self.pressure = pressure;
        self
    }

    pub fn with_tilt(mut self, tilt_x: f32, tilt_y: f32) -> Self {
        self.tilt_x = tilt_x;
        self.tilt_y = tilt_y;
        self
    }

    /// Return this sample with a different position and the same timing and
    /// tablet metadata.
    pub fn with_position(mut self, x: f32, y: f32) -> Self {
        self.x = x;
        self.y = y;
        self
    }

    pub fn x(self) -> f32 {
        self.x
    }

    pub fn y(self) -> f32 {
        self.y
    }
}

const PATH_POINT_VISIBLE: u32 = 1;
const PATH_POINT_HIDDEN: u32 = 0;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PathPoint {
    x: f32,
    y: f32,
    pressure: f32,
    altitude: f32,
    azimuth: f32,
    t: f32,
    visibility: u32,
}

impl PathPoint {
    /// Return the x coordinate in stroke-local canvas space.
    pub fn x(self) -> f32 {
        self.x
    }

    /// Return the y coordinate in stroke-local canvas space.
    pub fn y(self) -> f32 {
        self.y
    }

    /// Return normalized pressure for pressure-sensitive brushes.
    pub fn pressure(self) -> f32 {
        self.pressure
    }

    /// Return the altitude-like tablet value currently stored for this point.
    pub fn altitude(self) -> f32 {
        self.altitude
    }

    /// Return the azimuth-like tablet value currently stored for this point.
    pub fn azimuth(self) -> f32 {
        self.azimuth
    }

    /// Return the path parameter used by interpolation.
    pub fn t(self) -> f32 {
        self.t
    }

    /// Return whether this point should contribute to visible stroke rendering.
    pub fn is_visible(self) -> bool {
        self.visibility != PATH_POINT_HIDDEN
    }

    /// Set whether this point contributes to visible stroke rendering.
    pub fn set_visible(&mut self, visible: bool) {
        self.visibility = if visible {
            PATH_POINT_VISIBLE
        } else {
            PATH_POINT_HIDDEN
        };
    }

    /// Return a copy of this point with the requested visibility state.
    pub fn with_visibility(mut self, visible: bool) -> Self {
        self.set_visible(visible);
        self
    }

    /// Linearly interpolate this point toward `other`.
    ///
    /// This keeps tablet metadata continuous when geometry code needs to cut a
    /// modeled stroke segment, for example when an eraser creates fragments.
    pub fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            x: lerp_value(self.x, other.x, t),
            y: lerp_value(self.y, other.y, t),
            pressure: lerp_value(self.pressure, other.pressure, t).clamp(0.0, 1.0),
            altitude: lerp_sensor(self.altitude, other.altitude, t),
            azimuth: lerp_sensor(self.azimuth, other.azimuth, t),
            t: lerp_value(self.t, other.t, t),
            visibility: if self.is_visible() && other.is_visible() {
                PATH_POINT_VISIBLE
            } else {
                PATH_POINT_HIDDEN
            },
        }
    }
}

fn lerp_value(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_sensor(a: f32, b: f32, t: f32) -> f32 {
    match (a.is_finite(), b.is_finite()) {
        (true, true) => lerp_value(a, b, t),
        (true, false) => a,
        (false, true) | (false, false) => b,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputSource {
    Real,
    Predicted,
}

#[derive(Debug, thiserror::Error)]
pub enum InputModelError {
    #[error("invalid input modeler config field `{field}` with value {value}")]
    InvalidConfig { field: &'static str, value: f64 },

    #[error("non-finite {input_source:?} input at index {index}, field `{field}`, value {value}")]
    NonFiniteInput {
        input_source: InputSource,
        index: usize,
        field: &'static str,
        value: f64,
    },

    #[error(
        "non-monotonic {input_source:?} input timestamp at index {index}: previous {previous}, current {current}"
    )]
    NonMonotonicTimestamp {
        input_source: InputSource,
        index: usize,
        previous: f64,
        current: f64,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct ModelUpdateMeta {
    /// Old modeled path indices that should be considered invalid.
    ///
    /// Downstream geometry caches should delete/rebuild from this index.
    pub invalidated_from: usize,

    /// Prefix [0, stable_len) will not change anymore.
    pub stable_len: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct ModelUpdate<'a> {
    pub metadata: ModelUpdateMeta,

    pub updated: &'a [PathPoint],
}

/// process input datas to points on a path. This combines smoothing,
/// spline production and interpolation
pub trait InputModeler {
    fn reset(&mut self);
    /// add input data. should erase any previously predicted point.
    fn enqueue<'a>(
        &'a mut self,
        points: &[Input],
        predicted: &[Input],
    ) -> Result<ModelUpdate<'a>, InputModelError>;
    // fn path(&self) -> &[PathPoint];
    fn stable_len(&self) -> usize;
}
