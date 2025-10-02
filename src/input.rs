use crate::types::{InputData, Stroke};
use std::time::{Duration, Instant};

/// Configuration for input processing
#[derive(Debug, Clone)]
pub struct InputConfig {
    pub min_sample_rate: Duration,
    pub max_sample_rate: Duration,
    pub pressure_range: std::ops::Range<f64>,
    pub max_velocity: f64,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            min_sample_rate: Duration::from_micros(1000), // 1kHz
            max_sample_rate: Duration::from_millis(100),  // 10Hz
            pressure_range: 0.0..1.0,
            max_velocity: 10000.0, // pixels per second
        }
    }
}

/// Manages input data collection and preprocessing
pub struct InputManager {
    config: InputConfig,
    current_stroke: Stroke,
    last_input_time: Option<Instant>,
    stroke_count: usize,
}

impl InputManager {
    pub fn new(config: InputConfig) -> Self {
        Self {
            config,
            current_stroke: Stroke::new(),
            last_input_time: None,
            stroke_count: 0,
        }
    }

    pub fn add_input(&mut self, raw_input: RawInput) -> Result<InputData, InputError> {
        let now = Instant::now();

        // Validate timing
        if let Some(last_time) = self.last_input_time {
            let elapsed = now.duration_since(last_time);
            if elapsed < self.config.min_sample_rate {
                return Err(InputError::SampleRateTooHigh(elapsed));
            }
        }

        // Validate and normalize input
        let mut input_data = InputData::new(
            crate::types::Point::new(raw_input.x, raw_input.y),
            raw_input.timestamp.unwrap_or_else(|| now.elapsed()),
        );

        // Normalize pressure
        if let Some(pressure) = raw_input.pressure {
            if self.config.pressure_range.contains(&pressure) {
                input_data = input_data.with_pressure(pressure);
            } else {
                return Err(InputError::InvalidPressure(pressure));
            }
        }

        // Add tilt if available
        if let (Some(tilt_x), Some(tilt_y)) = (raw_input.tilt_x, raw_input.tilt_y) {
            input_data = input_data.with_tilt(crate::types::Point::new(tilt_x, tilt_y));
        }

        // Add orientation if available
        if let Some(orientation) = raw_input.orientation {
            input_data = input_data.with_orientation(orientation);
        }

        self.current_stroke.add_point(input_data.clone());
        self.last_input_time = Some(now);

        Ok(input_data)
    }

    pub fn finish_stroke(&mut self) -> Stroke {
        self.stroke_count += 1;
        let completed_stroke = std::mem::replace(&mut self.current_stroke, Stroke::new());
        self.last_input_time = None;
        completed_stroke
    }

    pub fn cancel_stroke(&mut self) {
        self.current_stroke = Stroke::new();
        self.last_input_time = None;
    }

    pub fn current_stroke(&self) -> &Stroke {
        &self.current_stroke
    }

    pub fn stroke_count(&self) -> usize {
        self.stroke_count
    }

    pub fn reset(&mut self) {
        self.current_stroke = Stroke::new();
        self.last_input_time = None;
        self.stroke_count = 0;
    }
}

/// Raw input from device/input system
#[derive(Debug, Clone)]
pub struct RawInput {
    pub x: f64,
    pub y: f64,
    pub pressure: Option<f64>,
    pub tilt_x: Option<f64>,
    pub tilt_y: Option<f64>,
    pub orientation: Option<f64>,
    pub timestamp: Option<Duration>,
}

impl RawInput {
    pub fn new(x: f64, y: f64) -> Self {
        Self {
            x,
            y,
            pressure: None,
            tilt_x: None,
            tilt_y: None,
            orientation: None,
            timestamp: None,
        }
    }

    pub fn with_pressure(mut self, pressure: f64) -> Self {
        self.pressure = Some(pressure);
        self
    }

    pub fn with_tilt(mut self, tilt_x: f64, tilt_y: f64) -> Self {
        self.tilt_x = Some(tilt_x);
        self.tilt_y = Some(tilt_y);
        self
    }

    pub fn with_orientation(mut self, orientation: f64) -> Self {
        self.orientation = Some(orientation);
        self
    }

    pub fn with_timestamp(mut self, timestamp: Duration) -> Self {
        self.timestamp = Some(timestamp);
        self
    }
}

/// Errors that can occur during input processing
#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("Sample rate too high: {0:?}")]
    SampleRateTooHigh(Duration),
    #[error("Invalid pressure value: {0}")]
    InvalidPressure(f64),
    #[error("Invalid position: x={0}, y={1}")]
    InvalidPosition(f64, f64),
    #[error("Stroke is empty")]
    EmptyStroke,
}

/// Input event types
#[derive(Debug, Clone)]
pub enum InputEvent {
    StrokeStart,
    StrokeMove(InputData),
    StrokeEnd(Stroke),
    Cancel,
}

/// Event dispatcher for input events
pub struct InputDispatcher {
    listeners: Vec<Box<dyn InputEventListener>>,
}

impl InputDispatcher {
    pub fn new() -> Self {
        Self {
            listeners: Vec::new(),
        }
    }

    pub fn add_listener(&mut self, listener: Box<dyn InputEventListener>) {
        self.listeners.push(listener);
    }

    pub fn dispatch(&mut self, event: InputEvent) {
        for listener in &mut self.listeners {
            listener.handle_event(&event);
        }
    }
}

impl Default for InputDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Trait for handling input events
pub trait InputEventListener {
    fn handle_event(&mut self, event: &InputEvent);
}

/// Simple logging listener for debugging
pub struct LoggingListener;

impl InputEventListener for LoggingListener {
    fn handle_event(&mut self, event: &InputEvent) {
        println!("Input event: {:?}", event);
    }
}
