use std::time::Duration;

/// A 2D point representing input position
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn distance_to(&self, other: &Point) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
}

/// Input data from a single stroke sample
#[derive(Debug, Clone)]
pub struct InputData {
    pub position: Point,
    pub timestamp: Duration,
    pub pressure: Option<f64>,
    pub tilt: Option<Point>,
    pub orientation: Option<f64>,
}

impl InputData {
    pub fn new(position: Point, timestamp: Duration) -> Self {
        Self {
            position,
            timestamp,
            pressure: None,
            tilt: None,
            orientation: None,
        }
    }

    pub fn with_pressure(mut self, pressure: f64) -> Self {
        self.pressure = Some(pressure);
        self
    }

    pub fn with_tilt(mut self, tilt: Point) -> Self {
        self.tilt = Some(tilt);
        self
    }

    pub fn with_orientation(mut self, orientation: f64) -> Self {
        self.orientation = Some(orientation);
        self
    }
}

/// A stroke is a sequence of input data points
#[derive(Debug, Clone)]
pub struct Stroke {
    pub points: Vec<InputData>,
}

impl Stroke {
    pub fn new() -> Self {
        Self { points: Vec::new() }
    }

    pub fn add_point(&mut self, point: InputData) {
        self.points.push(point);
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn duration(&self) -> Option<Duration> {
        if self.points.is_empty() {
            return None;
        }
        let start = self.points.first()?.timestamp;
        let end = self.points.last()?.timestamp;
        Some(end.saturating_sub(start))
    }

    pub fn total_distance(&self) -> f64 {
        if self.points.len() < 2 {
            return 0.0;
        }

        self.points
            .windows(2)
            .map(|window| window[0].position.distance_to(&window[1].position))
            .sum()
    }

    pub fn average_velocity(&self) -> Option<f64> {
        let duration = self.duration()?;
        let duration_secs = duration.as_secs_f64();
        if duration_secs == 0.0 {
            return None;
        }
        Some(self.total_distance() / duration_secs)
    }
}

impl Default for Stroke {
    fn default() -> Self {
        Self::new()
    }
}
