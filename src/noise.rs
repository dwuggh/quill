use crate::types::InputData;

/// Trait for noise filtering algorithms
pub trait NoiseFilter {
    fn filter(&mut self, input: &InputData) -> Option<InputData>;
    fn reset(&mut self);
}

/// Median filter for position noise reduction
pub struct MedianFilter {
    window_size: usize,
    position_buffer: Vec<crate::types::Point>,
    pressure_buffer: Vec<f64>,
}

impl MedianFilter {
    pub fn new(window_size: usize) -> Self {
        Self {
            window_size,
            position_buffer: Vec::with_capacity(window_size),
            pressure_buffer: Vec::with_capacity(window_size),
        }
    }

    fn median_position(&self) -> Option<crate::types::Point> {
        if self.position_buffer.is_empty() {
            return None;
        }

        let mut sorted_x: Vec<f64> = self.position_buffer.iter().map(|p| p.x).collect();
        let mut sorted_y: Vec<f64> = self.position_buffer.iter().map(|p| p.y).collect();
        sorted_x.sort_by(|a, b| a.partial_cmp(b).unwrap());
        sorted_y.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let mid = (self.position_buffer.len() - 1) / 2;
        Some(crate::types::Point::new(sorted_x[mid], sorted_y[mid]))
    }

    fn median_f64(buffer: &[f64]) -> Option<f64> {
        if buffer.is_empty() {
            return None;
        }
        let mut sorted = buffer.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        Some(sorted[(sorted.len() - 1) / 2])
    }
}

impl NoiseFilter for MedianFilter {
    fn filter(&mut self, input: &InputData) -> Option<InputData> {
        self.position_buffer.push(input.position);
        if let Some(pressure) = input.pressure {
            self.pressure_buffer.push(pressure);
        }

        if self.position_buffer.len() > self.window_size {
            self.position_buffer.remove(0);
        }
        if self.pressure_buffer.len() > self.window_size {
            self.pressure_buffer.remove(0);
        }

        let filtered_position = self.median_position()?;
        let mut filtered_input = InputData::new(filtered_position, input.timestamp);

        if let Some(pressure) = Self::median_f64(&self.pressure_buffer) {
            filtered_input = filtered_input.with_pressure(pressure);
        }

        filtered_input.tilt = input.tilt;
        filtered_input.orientation = input.orientation;

        Some(filtered_input)
    }

    fn reset(&mut self) {
        self.position_buffer.clear();
        self.pressure_buffer.clear();
    }
}

/// Kalman filter for smooth trajectory prediction
pub struct KalmanFilter {
    // State: [x, y, vx, vy]
    state: [f64; 4],
    // Covariance matrix (4x4 represented as flat array)
    covariance: [f64; 16],
    // Process noise
    process_noise: f64,
    // Measurement noise
    measurement_noise: f64,
    dt: f64,
}

impl KalmanFilter {
    pub fn new(process_noise: f64, measurement_noise: f64, dt: f64) -> Self {
        Self {
            state: [0.0; 4],
            covariance: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
            process_noise,
            measurement_noise,
            dt,
        }
    }

    fn predict(&mut self) {
        // Simple prediction: x = x + vx*dt, y = y + vy*dt
        self.state[0] += self.state[2] * self.dt;
        self.state[1] += self.state[3] * self.dt;

        // Update covariance (simplified)
        for i in 0..4 {
            self.covariance[i * 4 + i] += self.process_noise;
        }
    }

    fn update(&mut self, measurement: &crate::types::Point) {
        // Kalman gain computation (simplified)
        let k_gain = self.covariance[0] / (self.covariance[0] + self.measurement_noise);

        // Update state
        self.state[0] += k_gain * (measurement.x - self.state[0]);
        self.state[1] += k_gain * (measurement.y - self.state[1]);

        // Update velocity (simple derivative)
        if self.state[2].abs() < 1000.0 && self.state[3].abs() < 1000.0 {
            self.state[2] = 0.8 * self.state[2] + 0.2 * (measurement.x - self.state[0]) / self.dt;
            self.state[3] = 0.8 * self.state[3] + 0.2 * (measurement.y - self.state[1]) / self.dt;
        }

        // Update covariance (simplified)
        self.covariance[0] *= 1.0 - k_gain;
    }
}

impl NoiseFilter for KalmanFilter {
    fn filter(&mut self, input: &InputData) -> Option<InputData> {
        self.predict();
        self.update(&input.position);

        let filtered_position = crate::types::Point::new(self.state[0], self.state[1]);
        let mut filtered_input = InputData::new(filtered_position, input.timestamp);
        filtered_input.pressure = input.pressure;
        filtered_input.tilt = input.tilt;
        filtered_input.orientation = input.orientation;

        Some(filtered_input)
    }

    fn reset(&mut self) {
        self.state = [0.0; 4];
        self.covariance = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
    }
}

/// Velocity-based filter to remove outliers
pub struct VelocityFilter {
    max_velocity: f64,
    last_point: Option<crate::types::Point>,
    last_time: Option<std::time::Duration>,
}

impl VelocityFilter {
    pub fn new(max_velocity: f64) -> Self {
        Self {
            max_velocity,
            last_point: None,
            last_time: None,
        }
    }

    fn is_valid_velocity(
        &self,
        current: &crate::types::Point,
        current_time: std::time::Duration,
    ) -> bool {
        if let (Some(last), Some(last_time)) = (self.last_point, self.last_time) {
            let dt = current_time.saturating_sub(last_time).as_secs_f64();
            if dt > 0.0 {
                let distance = last.distance_to(current);
                let velocity = distance / dt;
                return velocity <= self.max_velocity;
            }
        }
        true
    }
}

impl NoiseFilter for VelocityFilter {
    fn filter(&mut self, input: &InputData) -> Option<InputData> {
        if self.is_valid_velocity(&input.position, input.timestamp) {
            self.last_point = Some(input.position);
            self.last_time = Some(input.timestamp);
            Some(input.clone())
        } else {
            None
        }
    }

    fn reset(&mut self) {
        self.last_point = None;
        self.last_time = None;
    }
}
