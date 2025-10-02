//! Ink Stroke Modeling Framework
//!
//! This framework provides the foundation for implementing ink stroke modeling algorithms
//! based on the [Ink Stroke Modeler specification](https://google.github.io/ink-stroke-modeler/).
//!
//! The framework includes:
//! - Input management and validation
//! - Noise filtering (median, Kalman, velocity-based)
//! - Type definitions for strokes and points
//! - Modeling interfaces and utilities
//!
//! ## Example
//!
//! ```rust
//! use quill::{InputManager, InputConfig, MedianFilter, NoiseFilter, RawInput};
//!
//! let config = InputConfig::default();
//! let mut input_manager = InputManager::new(config);
//! let mut filter = MedianFilter::new(5);
//!
//! // Add input points
//! let raw_input = RawInput::new(100.0, 150.0).with_pressure(0.5);
//! if let Ok(input_data) = input_manager.add_input(raw_input) {
//!     if let Some(filtered) = filter.filter(&input_data) {
//!         // Use filtered data for modeling
//!     }
//! }
//! ```

pub mod input;
pub mod modeling;
pub mod noise;
pub mod types;

#[cfg(test)]
mod tests;

// Re-export commonly used types
pub use input::{
    InputConfig, InputDispatcher, InputError, InputEvent, InputEventListener, InputManager,
    RawInput,
};
pub use modeling::{
    AdaptiveSampling, Bounds, DistanceSampling, ModelParameters, ModelingContext, SamplingStrategy,
    StrokeMetrics, StrokeModeler, TimeSampling,
};
pub use noise::{KalmanFilter, MedianFilter, NoiseFilter, VelocityFilter};
pub use types::{InputData, Point, Stroke};
