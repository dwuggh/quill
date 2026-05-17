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

    pub fn x(self) -> f32 {
        self.x
    }

    pub fn y(self) -> f32 {
        self.y
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PathPoint {
    x: f32,
    y: f32,
    pressure: f32,
    altitude: f32,
    azimuth: f32,
    t: f32,
}

impl PathPoint {
    pub fn x(self) -> f32 {
        self.x
    }

    pub fn y(self) -> f32 {
        self.y
    }

    pub fn pressure(self) -> f32 {
        self.pressure
    }

    pub fn altitude(self) -> f32 {
        self.altitude
    }

    pub fn azimuth(self) -> f32 {
        self.azimuth
    }

    pub fn t(self) -> f32 {
        self.t
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
