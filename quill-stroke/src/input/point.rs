//! Raw input samples and modeled points, independent of the frontend.

use super::model::{InputModelError, InputSource};

/// Pen categories recognized by the stroke input layer.
pub enum PenType {
    /// A pressure-sensitive drawing stylus.
    Stylus,
}

/// A raw pointer or stylus measurement in the caller's coordinate space.
///
/// The canvas accepts document coordinates and transforms samples into
/// stroke-local coordinates before modeling them.
#[derive(Debug, Clone, Copy)]
pub struct InputSample {
    pub(super) x: f32,
    pub(super) y: f32,
    /// Seconds on a consistent monotonic clock. Used to validate sample order,
    /// not to scale smoothing by elapsed time.
    pub(super) timestamp: f64,
    /// Normalized device pressure; conversion to a path point clamps to 0..=1.
    pub(super) pressure: f32,
    /// Legacy field name for the altitude value supplied by the tablet adapter.
    pub(super) tilt_x: f32,
    /// Legacy field name for the azimuth value supplied by the tablet adapter.
    pub(super) tilt_y: f32,
}

impl InputSample {
    /// Create a sample with full pressure and zero orientation values.
    ///
    /// `timestamp` is measured in seconds on the caller's monotonic clock.
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

    /// Set raw pressure, which is clamped when converted into a path point.
    pub fn with_pressure(mut self, pressure: f32) -> Self {
        self.pressure = pressure;
        self
    }

    /// Set tablet orientation using the legacy tilt field names.
    ///
    /// `tilt_x` carries altitude and `tilt_y` carries azimuth, both in radians.
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

    /// Return the x coordinate in the sample's current coordinate space.
    pub fn x(self) -> f32 {
        self.x
    }

    /// Return the y coordinate in the sample's current coordinate space.
    pub fn y(self) -> f32 {
        self.y
    }

    /// Reject non-finite fields before they can enter model state.
    pub(super) fn validate(
        &self,
        input_source: InputSource,
        index: usize,
    ) -> Result<(), InputModelError> {
        self.validate_finite(input_source, index, "position.x", self.x as f64)?;
        self.validate_finite(input_source, index, "position.y", self.y as f64)?;
        self.validate_finite(input_source, index, "timestamp", self.timestamp)?;
        self.validate_finite(input_source, index, "pressure", self.pressure as f64)?;
        self.validate_finite(input_source, index, "tilt.x", self.tilt_x as f64)?;
        self.validate_finite(input_source, index, "tilt.y", self.tilt_y as f64)?;
        Ok(())
    }

    fn validate_finite(
        &self,
        input_source: InputSource,
        index: usize,
        field: &'static str,
        value: f64,
    ) -> Result<(), InputModelError> {
        if value.is_finite() {
            return Ok(());
        }

        Err(InputModelError::NonFiniteInput {
            input_source,
            index,
            field,
            value,
        })
    }
}

const PATH_POINT_VISIBLE: u32 = 1;
const PATH_POINT_HIDDEN: u32 = 0;

/// A modeled stroke-local point with tablet metadata and a stable GPU layout.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PathPoint {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) pressure: f32,
    pub(super) altitude: f32,
    pub(super) azimuth: f32,
    /// Accumulated curve parameter, independent of input timestamps.
    pub(super) t: f32,
    /// Integer flag rather than a bool to keep the representation GPU-compatible.
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

    /// Initialize the position and pressure trend for a new smoothed path.
    pub(super) fn zero_delta() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            pressure: 0.0,
            altitude: 0.0,
            azimuth: 0.0,
            t: 0.0,
            visibility: PATH_POINT_VISIBLE,
        }
    }

    /// Predict position and pressure while retaining this point's other metadata.
    fn offset_by(self, delta: Self) -> Self {
        Self {
            x: self.x + delta.x,
            y: self.y + delta.y,
            pressure: self.pressure + delta.pressure,
            altitude: self.altitude,
            azimuth: self.azimuth,
            t: self.t,
            visibility: self.visibility,
        }
    }

    /// Measure the position and pressure change used to update a smoothing trend.
    fn delta_to(self, other: Self) -> Self {
        Self {
            x: other.x - self.x,
            y: other.y - self.y,
            pressure: other.pressure - self.pressure,
            altitude: 0.0,
            azimuth: 0.0,
            t: 0.0,
            visibility: PATH_POINT_VISIBLE,
        }
    }

    pub(super) fn distance_to(self, other: Self) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        dx.hypot(dy)
    }

    /// Blend the next measurement with the previous level plus its trend.
    /// Orientation and visibility follow the measurement directly.
    pub(super) fn smoothed_toward(self, input: Self, trend: Self, alpha: f32) -> Self {
        let predicted = self.offset_by(trend);
        Self {
            x: alpha.mul_add(input.x, (1.0 - alpha) * predicted.x),
            y: alpha.mul_add(input.y, (1.0 - alpha) * predicted.y),
            pressure: alpha.mul_add(input.pressure, (1.0 - alpha) * predicted.pressure),
            altitude: input.altitude,
            azimuth: input.azimuth,
            t: input.t,
            visibility: input.visibility,
        }
    }

    /// Blend the observed level change into the position and pressure trend.
    pub(super) fn smoothed_trend_from(
        self,
        previous: Self,
        previous_trend: Self,
        beta: f32,
    ) -> Self {
        let delta = previous.delta_to(self);
        Self {
            x: beta.mul_add(delta.x, (1.0 - beta) * previous_trend.x),
            y: beta.mul_add(delta.y, (1.0 - beta) * previous_trend.y),
            pressure: beta.mul_add(delta.pressure, (1.0 - beta) * previous_trend.pressure),
            altitude: 0.0,
            azimuth: 0.0,
            t: 0.0,
            visibility: PATH_POINT_VISIBLE,
        }
    }

    /// Guard modeled output before it is retained or passed to geometry code.
    pub(super) fn is_finite(self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.pressure.is_finite()
            && self.altitude.is_finite()
            && self.azimuth.is_finite()
            && self.t.is_finite()
    }

    #[cfg(test)]
    pub(crate) fn for_test(x: f32, y: f32, pressure: f32) -> Self {
        Self {
            x,
            y,
            pressure,
            altitude: 0.0,
            azimuth: 0.0,
            t: 0.0,
            visibility: PATH_POINT_VISIBLE,
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

impl From<InputSample> for PathPoint {
    /// Initialize a visible point at parameter zero, clamping its pressure.
    ///
    /// Preserve position and orientation, and discard the input timestamp.
    /// This conversion does not validate finiteness; modelers validate samples
    /// before converting them.
    fn from(input: InputSample) -> Self {
        Self {
            x: input.x,
            y: input.y,
            pressure: input.pressure.clamp(0.0, 1.0),
            altitude: input.tilt_x,
            azimuth: input.tilt_y,
            t: 0.0,
            visibility: PATH_POINT_VISIBLE,
        }
    }
}
