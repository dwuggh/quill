use std::time;

use smallvec::SmallVec;


#[repr(C)]
#[derive(Debug, PartialEq, PartialOrd, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vec2 {
    x: f32,
    y: f32,
}

pub enum PenType {
    Stylus,
}

#[derive(Debug, Clone, Copy)]
pub struct Input {
    position: Vec2,
    timestamp: time::Instant,
    /// normally scale in 1/8192 or 1/16384
    pressure: f32,
    tilt: Vec2,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PathPoint {
    position: Vec2,
    pressure: f32,
    altitude: f32,
    azimuth: f32,
    _pad: f32,
}

#[derive(Debug, thiserror::Error)]
pub enum InputModelError {
}

#[derive(Clone, Copy, Debug)]
pub struct ModelUpdate {
    /// Old modeled path indices that should be considered invalid.
    ///
    /// Downstream geometry caches should delete/rebuild from this index.
    pub invalidated_from: usize,

    /// Prefix [0, stable_len) will not change anymore.
    pub stable_len: usize,
}

/// process input datas to points on a path. This combines smoothing,
/// spline production and interpolation
pub trait InputModeler {
    /// add input data. should erase any previously predicted point.
    fn reset(&mut self);
    fn enqueue(&mut self, points: &[Input], predicted: &[Input]) -> Result<ModelUpdate, InputModelError>;
    fn path(&self) -> &[PathPoint];
    fn stable_len(&self) -> usize;
}