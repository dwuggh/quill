use crate::StrokeError;
use crate::input::{RawInput, ToolType, validate_input_sequence};
use kurbo::{Point, Vec2};
use std::f64::consts::{PI, TAU};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputModelConfig {
    Passthrough,
    SlidingWindow {
        window: f64,
        upsample_period: f64,
        position_epsilon: f64,
        max_upsample_divisions: usize,
    },
}

impl Default for InputModelConfig {
    fn default() -> Self {
        Self::SlidingWindow {
            window: 0.020,
            upsample_period: 1.0 / 180.0,
            position_epsilon: 0.25,
            max_upsample_divisions: 100,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct InputMetrics {
    pub traveled_distance: f64,
    pub elapsed_time: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModeledInput {
    pub position: Point,
    pub velocity: Vec2,
    pub acceleration: Vec2,
    pub traveled_distance: f64,
    pub time: f64,
    pub pressure: Option<f64>,
    pub tilt: Option<f64>,
    pub orientation: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputModelerState {
    pub tool: ToolType,
    pub stroke_unit_to_physical: Option<f64>,
    pub stable_input_count: usize,
    pub real_input_count: usize,
    pub real_metrics: InputMetrics,
    pub full_metrics: InputMetrics,
    pub complete_time: f64,
    pub inputs_finished: bool,
}

impl Default for InputModelerState {
    fn default() -> Self {
        Self {
            tool: ToolType::Unknown,
            stroke_unit_to_physical: None,
            stable_input_count: 0,
            real_input_count: 0,
            real_metrics: InputMetrics::default(),
            full_metrics: InputMetrics::default(),
            complete_time: 0.0,
            inputs_finished: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct InputModeler {
    config: InputModelConfig,
    modeled_inputs: Vec<ModeledInput>,
    state: InputModelerState,
}

impl InputModeler {
    pub fn new(config: InputModelConfig) -> Self {
        Self {
            config,
            modeled_inputs: Vec::new(),
            state: InputModelerState::default(),
        }
    }

    pub fn model(
        &mut self,
        real: &[RawInput],
        predicted: &[RawInput],
        current_time: f64,
        inputs_finished: bool,
    ) -> Result<&[ModeledInput], StrokeError> {
        let mut raw = Vec::with_capacity(real.len() + predicted.len());
        raw.extend_from_slice(real);
        raw.extend_from_slice(predicted);
        validate_input_sequence(&raw)?;

        self.modeled_inputs = match self.config {
            InputModelConfig::Passthrough => model_passthrough(&raw),
            InputModelConfig::SlidingWindow {
                window,
                upsample_period,
                position_epsilon,
                max_upsample_divisions,
            } => model_sliding_window(
                &raw,
                window,
                upsample_period,
                position_epsilon,
                max_upsample_divisions,
            ),
        };
        fill_derivatives_and_metrics(&mut self.modeled_inputs);

        let real_cutoff = real.last().map(|input| input.time).unwrap_or(0.0);
        let stable_time = match self.config {
            InputModelConfig::SlidingWindow { window, .. } if !inputs_finished => {
                real_cutoff - window * 0.5
            }
            _ => real_cutoff,
        };
        let stable_input_count = self
            .modeled_inputs
            .iter()
            .take_while(|input| input.time <= stable_time)
            .count();
        let real_input_count = self
            .modeled_inputs
            .iter()
            .take_while(|input| input.time <= real_cutoff)
            .count();

        self.state = InputModelerState {
            tool: raw
                .first()
                .map(|input| input.tool)
                .unwrap_or(ToolType::Unknown),
            stroke_unit_to_physical: raw.first().and_then(|input| input.stroke_unit_to_physical),
            stable_input_count,
            real_input_count,
            real_metrics: metrics_for_count(&self.modeled_inputs, real_input_count),
            full_metrics: metrics_for_count(&self.modeled_inputs, self.modeled_inputs.len()),
            complete_time: current_time
                .max(self.modeled_inputs.last().map_or(0.0, |input| input.time)),
            inputs_finished,
        };

        Ok(&self.modeled_inputs)
    }

    pub fn modeled_inputs(&self) -> &[ModeledInput] {
        &self.modeled_inputs
    }

    pub fn state(&self) -> &InputModelerState {
        &self.state
    }
}

fn model_passthrough(raw: &[RawInput]) -> Vec<ModeledInput> {
    raw.iter().map(modeled_from_raw).collect()
}

fn model_sliding_window(
    raw: &[RawInput],
    window: f64,
    upsample_period: f64,
    position_epsilon: f64,
    max_upsample_divisions: usize,
) -> Vec<ModeledInput> {
    if raw.is_empty() {
        return Vec::new();
    }
    if raw.len() == 1 {
        return vec![modeled_from_raw(&raw[0])];
    }

    let mut times = Vec::new();
    times.push(raw[0].time);
    for pair in raw.windows(2) {
        let a = pair[0];
        let b = pair[1];
        let dt = b.time - a.time;
        let divisions =
            if upsample_period.is_finite() && upsample_period > 0.0 && dt > upsample_period {
                ((dt / upsample_period).ceil() as usize).clamp(1, max_upsample_divisions.max(1))
            } else {
                1
            };
        for step in 1..=divisions {
            times.push(a.time + dt * step as f64 / divisions as f64);
        }
    }

    let mut modeled = Vec::with_capacity(times.len());
    for time in times {
        let input = if window > 0.0 {
            smooth_raw_at(raw, time, window)
        } else {
            interpolate_raw_at(raw, time)
        };
        if modeled.last().is_some_and(|last: &ModeledInput| {
            last.position.distance(input.position) <= position_epsilon
        }) {
            continue;
        }
        modeled.push(modeled_from_raw(&input));
    }
    modeled
}

fn smooth_raw_at(raw: &[RawInput], time: f64, window: f64) -> RawInput {
    const SAMPLES: usize = 7;
    let start_time = raw.first().unwrap().time;
    let end_time = raw.last().unwrap().time;
    let half = (window * 0.5).min(time - start_time).min(end_time - time);
    if half <= 0.0 {
        return interpolate_raw_at(raw, time);
    }

    let mut x = 0.0;
    let mut y = 0.0;
    let mut pressure = 0.0;
    let mut pressure_count = 0.0;
    let mut tilt = 0.0;
    let mut tilt_count = 0.0;
    let mut orientation_x = 0.0;
    let mut orientation_y = 0.0;
    let mut orientation_count = 0.0;

    for i in 0..SAMPLES {
        let t = time - half + (2.0 * half) * i as f64 / (SAMPLES - 1) as f64;
        let sample = interpolate_raw_at(raw, t);
        x += sample.position.x;
        y += sample.position.y;
        if let Some(value) = sample.pressure {
            pressure += value;
            pressure_count += 1.0;
        }
        if let Some(value) = sample.tilt {
            tilt += value;
            tilt_count += 1.0;
        }
        if let Some(value) = sample.orientation {
            orientation_x += value.cos();
            orientation_y += value.sin();
            orientation_count += 1.0;
        }
    }

    let base = interpolate_raw_at(raw, time);
    let inv = 1.0 / SAMPLES as f64;
    RawInput {
        position: Point::new(x * inv, y * inv),
        pressure: (pressure_count > 0.0).then_some(pressure / pressure_count),
        tilt: (tilt_count > 0.0).then_some(tilt / tilt_count),
        orientation: (orientation_count > 0.0)
            .then(|| orientation_y.atan2(orientation_x).rem_euclid(TAU)),
        ..base
    }
}

fn interpolate_raw_at(raw: &[RawInput], time: f64) -> RawInput {
    if raw.len() == 1 || time <= raw[0].time {
        return raw[0];
    }
    for pair in raw.windows(2) {
        let a = pair[0];
        let b = pair[1];
        if time <= b.time {
            let dt = b.time - a.time;
            let t = if dt > 0.0 { (time - a.time) / dt } else { 0.0 };
            return lerp_raw(a, b, t);
        }
    }
    *raw.last().unwrap()
}

fn lerp_raw(a: RawInput, b: RawInput, t: f64) -> RawInput {
    RawInput {
        tool: a.tool,
        position: a.position + (b.position - a.position) * t,
        time: a.time + (b.time - a.time) * t,
        stroke_unit_to_physical: a
            .stroke_unit_to_physical
            .zip(b.stroke_unit_to_physical)
            .map(|(a, b)| a + (b - a) * t),
        pressure: a.pressure.zip(b.pressure).map(|(a, b)| a + (b - a) * t),
        tilt: a.tilt.zip(b.tilt).map(|(a, b)| a + (b - a) * t),
        orientation: a.orientation.zip(b.orientation).map(|(a, b)| {
            let delta = shortest_angle_delta(a, b);
            (a + delta * t).rem_euclid(TAU)
        }),
    }
}

fn modeled_from_raw(input: &RawInput) -> ModeledInput {
    ModeledInput {
        position: input.position,
        velocity: Vec2::ZERO,
        acceleration: Vec2::ZERO,
        traveled_distance: 0.0,
        time: input.time,
        pressure: input.pressure,
        tilt: input.tilt,
        orientation: input.orientation,
    }
}

fn fill_derivatives_and_metrics(inputs: &mut [ModeledInput]) {
    if inputs.is_empty() {
        return;
    }
    for i in 1..inputs.len() {
        inputs[i].traveled_distance =
            inputs[i - 1].traveled_distance + inputs[i].position.distance(inputs[i - 1].position);
    }
    for i in 0..inputs.len() {
        let prev = i.saturating_sub(1);
        let next = (i + 1).min(inputs.len() - 1);
        let dt = inputs[next].time - inputs[prev].time;
        if dt > 0.0 {
            inputs[i].velocity = (inputs[next].position - inputs[prev].position) / dt;
        }
    }
    let velocities = inputs
        .iter()
        .map(|input| input.velocity)
        .collect::<Vec<_>>();
    for i in 0..inputs.len() {
        let prev = i.saturating_sub(1);
        let next = (i + 1).min(inputs.len() - 1);
        let dt = inputs[next].time - inputs[prev].time;
        if dt > 0.0 {
            inputs[i].acceleration = (velocities[next] - velocities[prev]) / dt;
        }
    }
}

fn metrics_for_count(inputs: &[ModeledInput], count: usize) -> InputMetrics {
    if count == 0 {
        return InputMetrics::default();
    }
    let last = &inputs[count - 1];
    InputMetrics {
        traveled_distance: last.traveled_distance,
        elapsed_time: last.time,
    }
}

fn shortest_angle_delta(a: f64, b: f64) -> f64 {
    (b - a + PI).rem_euclid(TAU) - PI
}
