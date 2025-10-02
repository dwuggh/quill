use crate::types::{InputData, Point, Stroke};

/// Trait for stroke modeling algorithms
pub trait StrokeModeler {
    type Output;
    type Config;

    fn new(config: Self::Config) -> Self;
    fn process_input(&mut self, input: &InputData) -> Option<Self::Output>;
    fn finish_stroke(&mut self) -> Self::Output;
    fn reset(&mut self);
    fn configure(&mut self, config: Self::Config);
}

/// Parameters for stroke modeling algorithms
#[derive(Debug, Clone)]
pub struct ModelParameters {
    pub sampling_distance: f64,
    pub smoothing_factor: f64,
    pub pressure_sensitivity: f64,
    pub velocity_influence: f64,
}

impl Default for ModelParameters {
    fn default() -> Self {
        Self {
            sampling_distance: 2.0,
            smoothing_factor: 0.3,
            pressure_sensitivity: 1.0,
            velocity_influence: 0.5,
        }
    }
}

/// Trait for different sampling strategies
pub trait SamplingStrategy {
    fn should_sample(&self, input: &InputData, last_sample: Option<&InputData>) -> bool;
}

/// Distance-based sampling
pub struct DistanceSampling {
    min_distance: f64,
}

impl DistanceSampling {
    pub fn new(min_distance: f64) -> Self {
        Self { min_distance }
    }
}

impl SamplingStrategy for DistanceSampling {
    fn should_sample(&self, input: &InputData, last_sample: Option<&InputData>) -> bool {
        match last_sample {
            None => true,
            Some(last) => input.position.distance_to(&last.position) >= self.min_distance,
        }
    }
}

/// Time-based sampling
pub struct TimeSampling {
    min_interval: std::time::Duration,
}

impl TimeSampling {
    pub fn new(min_interval: std::time::Duration) -> Self {
        Self { min_interval }
    }
}

impl SamplingStrategy for TimeSampling {
    fn should_sample(&self, input: &InputData, last_sample: Option<&InputData>) -> bool {
        match last_sample {
            None => true,
            Some(last) => input.timestamp.saturating_sub(last.timestamp) >= self.min_interval,
        }
    }
}

/// Adaptive sampling combining distance and time
pub struct AdaptiveSampling {
    distance_sampler: DistanceSampling,
    time_sampler: TimeSampling,
}

impl AdaptiveSampling {
    pub fn new(min_distance: f64, min_interval: std::time::Duration) -> Self {
        Self {
            distance_sampler: DistanceSampling::new(min_distance),
            time_sampler: TimeSampling::new(min_interval),
        }
    }
}

impl SamplingStrategy for AdaptiveSampling {
    fn should_sample(&self, input: &InputData, last_sample: Option<&InputData>) -> bool {
        self.distance_sampler.should_sample(input, last_sample)
            || self.time_sampler.should_sample(input, last_sample)
    }
}

/// Context information for modeling algorithms
#[derive(Debug, Clone)]
pub struct ModelingContext {
    pub stroke_index: usize,
    pub point_index: usize,
    pub accumulated_distance: f64,
    pub current_velocity: f64,
    pub average_velocity: f64,
    pub stroke_bounds: Option<Bounds>,
}

impl ModelingContext {
    pub fn new() -> Self {
        Self {
            stroke_index: 0,
            point_index: 0,
            accumulated_distance: 0.0,
            current_velocity: 0.0,
            average_velocity: 0.0,
            stroke_bounds: None,
        }
    }

    pub fn update(&mut self, stroke: &Stroke, current_point: &InputData, point_index: usize) {
        self.point_index = point_index;

        if point_index > 0 {
            let prev_point = &stroke.points[point_index - 1];
            let distance = prev_point.position.distance_to(&current_point.position);
            self.accumulated_distance += distance;

            let dt = current_point
                .timestamp
                .saturating_sub(prev_point.timestamp)
                .as_secs_f64();
            if dt > 0.0 {
                self.current_velocity = distance / dt;
            }
        }

        if let Some(avg_vel) = stroke.average_velocity() {
            self.average_velocity = avg_vel;
        }

        self.stroke_bounds = Some(Bounds::from_stroke(stroke));
    }
}

impl Default for ModelingContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Bounding box for a stroke
#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl Bounds {
    pub fn new(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Self {
        Self {
            min_x,
            min_y,
            max_x,
            max_y,
        }
    }

    pub fn from_stroke(stroke: &Stroke) -> Self {
        if stroke.is_empty() {
            return Self::new(0.0, 0.0, 0.0, 0.0);
        }

        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;

        for point in &stroke.points {
            min_x = min_x.min(point.position.x);
            min_y = min_y.min(point.position.y);
            max_x = max_x.max(point.position.x);
            max_y = max_y.max(point.position.y);
        }

        Self::new(min_x, min_y, max_x, max_y)
    }

    pub fn width(&self) -> f64 {
        self.max_x - self.min_x
    }

    pub fn height(&self) -> f64 {
        self.max_y - self.min_y
    }

    pub fn center(&self) -> Point {
        Point::new(
            (self.min_x + self.max_x) / 2.0,
            (self.min_y + self.max_y) / 2.0,
        )
    }

    pub fn contains(&self, point: &Point) -> bool {
        point.x >= self.min_x
            && point.x <= self.max_x
            && point.y >= self.min_y
            && point.y <= self.max_y
    }
}

/// Quality metrics for stroke analysis
#[derive(Debug, Clone)]
pub struct StrokeMetrics {
    pub smoothness: f64,
    pub consistency: f64,
    pub completeness: f64,
    pub confidence: f64,
}

impl StrokeMetrics {
    pub fn new() -> Self {
        Self {
            smoothness: 0.0,
            consistency: 0.0,
            completeness: 0.0,
            confidence: 0.0,
        }
    }

    pub fn overall_quality(&self) -> f64 {
        (self.smoothness + self.consistency + self.completeness + self.confidence) / 4.0
    }
}

impl Default for StrokeMetrics {
    fn default() -> Self {
        Self::new()
    }
}
