use smallvec::SmallVec;

use super::{Input, InputModelError, InputModeler, InputSource, ModelUpdate, PathPoint};

const EPSILON: f32 = 1.0e-5;
const SPLINE_LOOKBACK: usize = 2;

#[derive(Clone, Copy, Debug)]
pub struct WillInputModelerConfig {
    pub smoothing_alpha: f32,
    pub smoothing_beta: f32,
    pub interpolation_spacing: f32,
    pub stable_lookahead: usize,
}

impl Default for WillInputModelerConfig {
    fn default() -> Self {
        Self {
            smoothing_alpha: 0.55,
            smoothing_beta: 0.25,
            interpolation_spacing: 2.0,
            stable_lookahead: 2,
        }
    }
}

impl WillInputModelerConfig {
    fn validate(self) -> Result<(), InputModelError> {
        Self::validate_factor("smoothing_alpha", self.smoothing_alpha)?;
        Self::validate_factor("smoothing_beta", self.smoothing_beta)?;

        if !self.interpolation_spacing.is_finite() || self.interpolation_spacing <= 0.0 {
            return Err(InputModelError::InvalidConfig {
                field: "interpolation_spacing",
                value: self.interpolation_spacing as f64,
            });
        }

        Ok(())
    }

    fn validate_factor(field: &'static str, value: f32) -> Result<(), InputModelError> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(InputModelError::InvalidConfig {
                field,
                value: value as f64,
            });
        }

        Ok(())
    }

    fn spacing(self) -> f32 {
        self.interpolation_spacing.max(EPSILON)
    }
}

#[derive(Clone, Debug, Default)]
struct InputCursor {
    cached_inputs: SmallVec<[Input; 8]>,
    first_unstable_input: usize,
    predicted_start: usize,
}

impl InputCursor {
    fn validate_enqueue(
        &self,
        points: &[Input],
        predicted: &[Input],
    ) -> Result<(), InputModelError> {
        let real_previous_timestamp = self
            .cached_inputs
            .get(..self.predicted_start)
            .and_then(<[Input]>::last)
            .map(|input| input.timestamp);
        InputBatch::new(points, InputSource::Real).validate_after(real_previous_timestamp)?;

        let predicted_previous_timestamp = points
            .last()
            .or_else(|| {
                self.cached_inputs
                    .get(..self.predicted_start)
                    .and_then(<[Input]>::last)
            })
            .map(|input| input.timestamp);
        InputBatch::new(predicted, InputSource::Predicted)
            .validate_after(predicted_previous_timestamp)?;

        Ok(())
    }

    fn replace_prediction(&mut self, points: &[Input], predicted: &[Input]) {
        self.cached_inputs.truncate(self.predicted_start);
        self.cached_inputs.extend_from_slice(points);
        self.predicted_start = self.cached_inputs.len();
        self.cached_inputs.extend_from_slice(predicted);
    }

    fn new_first_unstable_input(&self, stable_lookahead: usize) -> usize {
        self.predicted_start.saturating_sub(stable_lookahead)
    }

    fn compact(&mut self) {
        let keep_from = self.first_unstable_input.saturating_sub(SPLINE_LOOKBACK);
        if keep_from == 0 {
            return;
        }

        self.cached_inputs.drain(..keep_from);
        self.first_unstable_input -= keep_from;
        self.predicted_start -= keep_from;
    }
}

#[derive(Clone, Copy, Debug)]
struct SmoothingState {
    level: PathPoint,
    trend: PathPoint,
}

#[derive(Clone, Debug, Default)]
struct Smoother {
    committed: Option<SmoothingState>,
}

impl Smoother {
    fn smooth(
        &self,
        inputs: &[Input],
        first_input_index: usize,
        config: WillInputModelerConfig,
    ) -> SmoothedSuffix {
        let mut state = self.committed;
        let mut points = Vec::with_capacity(inputs.len());
        let mut states = Vec::with_capacity(inputs.len());

        for (offset, input) in inputs.iter().copied().enumerate() {
            let raw = input.to_path_point();
            let next_state = match state {
                Some(previous) => {
                    let level =
                        previous
                            .level
                            .smoothed_toward(raw, previous.trend, config.smoothing_alpha);
                    let trend = level.smoothed_trend_from(
                        previous.level,
                        previous.trend,
                        config.smoothing_beta,
                    );
                    SmoothingState { level, trend }
                }
                None => SmoothingState {
                    level: raw,
                    trend: PathPoint::zero_delta(),
                },
            };

            state = Some(next_state);
            points.push(SmoothedPoint {
                point: next_state.level,
                input_index: first_input_index + offset,
            });
            states.push(next_state);
        }

        SmoothedSuffix {
            anchor: self.committed.map(|state| SmoothedPoint {
                point: state.level,
                input_index: first_input_index.saturating_sub(1),
            }),
            points,
            states,
        }
    }

    fn commit(&mut self, state: Option<SmoothingState>) {
        self.committed = state;
    }
}

#[derive(Clone, Debug)]
struct SmoothedSuffix {
    anchor: Option<SmoothedPoint>,
    points: Vec<SmoothedPoint>,
    states: Vec<SmoothingState>,
}

impl SmoothedSuffix {
    fn state_after(&self, processed_count: usize) -> Option<SmoothingState> {
        processed_count
            .checked_sub(1)
            .and_then(|index| self.states.get(index).copied())
    }

    fn with_anchor(&self) -> Vec<SmoothedPoint> {
        let mut points = Vec::with_capacity(self.points.len() + usize::from(self.anchor.is_some()));
        points.extend(self.anchor);
        points.extend_from_slice(&self.points);
        points
    }
}

#[derive(Clone, Copy, Debug)]
struct SmoothedPoint {
    point: PathPoint,
    input_index: usize,
}

#[derive(Debug, Default)]
pub struct WillInputModeler {
    config: WillInputModelerConfig,
    inputs: InputCursor,
    smoother: Smoother,
    path: Vec<PathPoint>,
    stable_len: usize,
}

impl WillInputModeler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(config: WillInputModelerConfig) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }

    fn validate_enqueue(
        &self,
        points: &[Input],
        predicted: &[Input],
    ) -> Result<(), InputModelError> {
        self.config.validate()?;
        self.inputs.validate_enqueue(points, predicted)
    }

    fn rebuild_unstable_path(&mut self, stable_seed: Option<PathPoint>) {
        let first_unstable_input = self.inputs.first_unstable_input;
        let smoothed = self.smoother.smooth(
            &self.inputs.cached_inputs[first_unstable_input..],
            first_unstable_input,
            self.config,
        );
        let new_first_unstable_input = self
            .inputs
            .new_first_unstable_input(self.config.stable_lookahead);

        let emitted =
            PathBuilder::new(self.config).interpolate(&smoothed.with_anchor(), stable_seed);
        let stable_extension = emitted
            .iter()
            .take_while(|sample| sample.input_index < new_first_unstable_input)
            .count();

        for sample in emitted {
            self.push_path_point(sample.point);
        }

        let processed_stable = new_first_unstable_input.saturating_sub(first_unstable_input);
        let committed_state = smoothed
            .state_after(processed_stable)
            .or(self.smoother.committed);

        self.smoother.commit(committed_state);
        self.inputs.first_unstable_input = new_first_unstable_input;
        self.stable_len = (self.stable_len + stable_extension).min(self.path.len());
        self.inputs.compact();
    }

    fn push_path_point(&mut self, point: PathPoint) {
        if point.is_finite()
            && self
                .path
                .last()
                .is_none_or(|last| last.distance_to(point) > EPSILON)
        {
            self.path.push(point);
        }
    }
}

impl InputModeler for WillInputModeler {
    fn reset(&mut self) {
        self.inputs = InputCursor::default();
        self.smoother = Smoother::default();
        self.path.clear();
        self.stable_len = 0;
    }

    fn enqueue(
        &mut self,
        points: &[Input],
        predicted: &[Input],
    ) -> Result<ModelUpdate<'_>, InputModelError> {
        self.validate_enqueue(points, predicted)?;

        let invalidated_from = self.stable_len;
        self.path.truncate(self.stable_len);
        let stable_seed = self.path.last().copied();
        self.inputs.replace_prediction(points, predicted);
        self.rebuild_unstable_path(stable_seed);

        Ok(ModelUpdate {
            metadata: super::ModelUpdateMeta {
                invalidated_from,
                stable_len: self.stable_len,
            },
            updated: &self.path[invalidated_from..],
        })
    }

    fn stable_len(&self) -> usize {
        self.stable_len
    }
}

struct InputBatch<'a> {
    inputs: &'a [Input],
    source: InputSource,
}

impl<'a> InputBatch<'a> {
    fn new(inputs: &'a [Input], source: InputSource) -> Self {
        Self { inputs, source }
    }

    fn validate_after(&self, previous_timestamp: Option<f64>) -> Result<(), InputModelError> {
        let mut previous_timestamp = previous_timestamp;

        for (index, input) in self.inputs.iter().enumerate() {
            input.validate(self.source, index)?;

            if let Some(previous) = previous_timestamp
                && input.timestamp < previous
            {
                return Err(InputModelError::NonMonotonicTimestamp {
                    input_source: self.source,
                    index,
                    previous,
                    current: input.timestamp,
                });
            }
            previous_timestamp = Some(input.timestamp);
        }

        Ok(())
    }
}

impl Input {
    fn validate(&self, input_source: InputSource, index: usize) -> Result<(), InputModelError> {
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

    fn to_path_point(self) -> PathPoint {
        PathPoint {
            x: self.x,
            y: self.y,
            pressure: self.pressure.clamp(0.0, 1.0),
            altitude: self.tilt_x,
            azimuth: self.tilt_y,
            t: 0.0,
        }
    }
}

impl PathPoint {
    fn zero_delta() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            pressure: 0.0,
            altitude: 0.0,
            azimuth: 0.0,
            t: 0.0,
        }
    }

    fn offset_by(self, delta: Self) -> Self {
        Self {
            x: self.x + delta.x,
            y: self.y + delta.y,
            pressure: self.pressure + delta.pressure,
            altitude: self.altitude,
            azimuth: self.azimuth,
            t: self.t,
        }
    }

    fn delta_to(self, other: Self) -> Self {
        Self {
            x: other.x - self.x,
            y: other.y - self.y,
            pressure: other.pressure - self.pressure,
            altitude: 0.0,
            azimuth: 0.0,
            t: 0.0,
        }
    }

    fn distance_to(self, other: Self) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        dx.hypot(dy)
    }

    fn smoothed_toward(self, input: Self, trend: Self, alpha: f32) -> Self {
        let predicted = self.offset_by(trend);
        Self {
            x: alpha.mul_add(input.x, (1.0 - alpha) * predicted.x),
            y: alpha.mul_add(input.y, (1.0 - alpha) * predicted.y),
            pressure: alpha.mul_add(input.pressure, (1.0 - alpha) * predicted.pressure),
            altitude: input.altitude,
            azimuth: input.azimuth,
            t: input.t,
        }
    }

    fn smoothed_trend_from(self, previous: Self, previous_trend: Self, beta: f32) -> Self {
        let delta = previous.delta_to(self);
        Self {
            x: beta.mul_add(delta.x, (1.0 - beta) * previous_trend.x),
            y: beta.mul_add(delta.y, (1.0 - beta) * previous_trend.y),
            pressure: beta.mul_add(delta.pressure, (1.0 - beta) * previous_trend.pressure),
            altitude: 0.0,
            azimuth: 0.0,
            t: 0.0,
        }
    }

    fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            x: Self::lerp_value(self.x, other.x, t),
            y: Self::lerp_value(self.y, other.y, t),
            pressure: Self::lerp_value(self.pressure, other.pressure, t).clamp(0.0, 1.0),
            altitude: Self::lerp_sensor(self.altitude, other.altitude, t),
            azimuth: Self::lerp_sensor(self.azimuth, other.azimuth, t),
            t: Self::lerp_value(self.t, other.t, t),
        }
    }

    fn lerp_value(a: f32, b: f32, t: f32) -> f32 {
        a + (b - a) * t
    }

    fn lerp_sensor(a: f32, b: f32, t: f32) -> f32 {
        match (a.is_finite(), b.is_finite()) {
            (true, true) => Self::lerp_value(a, b, t),
            (true, false) => a,
            (false, true) | (false, false) => b,
        }
    }

    fn is_finite(self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.pressure.is_finite()
            && self.altitude.is_finite()
            && self.azimuth.is_finite()
            && self.t.is_finite()
    }
}

#[derive(Clone, Copy)]
struct PathBuilder {
    config: WillInputModelerConfig,
}

impl PathBuilder {
    fn new(config: WillInputModelerConfig) -> Self {
        Self { config }
    }

    fn interpolate(self, points: &[SmoothedPoint], seed: Option<PathPoint>) -> Vec<ModeledSample> {
        let points = self.parameterize(points, seed);
        match points.len() {
            0 => Vec::new(),
            1 | 2 => points.iter().copied().map(ModeledSample::from).collect(),
            _ => self.interpolate_spline(&points),
        }
    }

    fn parameterize(self, points: &[SmoothedPoint], seed: Option<PathPoint>) -> Vec<SmoothedPoint> {
        let mut parameterized = Vec::with_capacity(points.len());
        let mut previous = seed;
        let mut t = seed.map_or(0.0, |point| point.t);

        for point in points.iter().copied() {
            let mut next = point.point;
            if let Some(previous) = previous {
                t += previous.distance_to(next).sqrt();
            }
            next.t = t;
            parameterized.push(SmoothedPoint {
                point: next,
                input_index: point.input_index,
            });
            previous = Some(next);
        }

        parameterized
    }

    fn interpolate_spline(self, points: &[SmoothedPoint]) -> Vec<ModeledSample> {
        let mut output = Vec::with_capacity(points.len());
        output.push(points[0].into());

        for i in 0..points.len() - 1 {
            let p0 = if i == 0 { points[i] } else { points[i - 1] };
            let p1 = points[i];
            let p2 = points[i + 1];
            let p3 = points.get(i + 2).copied().unwrap_or(p2);

            let distance = p1.point.distance_to(p2.point);
            if distance <= EPSILON {
                continue;
            }

            let split_count = (distance / self.config.spacing()).ceil().max(1.0) as usize;
            for split in 1..=split_count {
                let t = split as f32 / split_count as f32;
                let point = self.centripetal_catmull_rom(p0.point, p1.point, p2.point, p3.point, t);
                Self::push_distinct(
                    &mut output,
                    ModeledSample {
                        point,
                        input_index: p2.input_index,
                    },
                );
            }
        }

        output
    }

    fn centripetal_catmull_rom(
        self,
        p0: PathPoint,
        p1: PathPoint,
        p2: PathPoint,
        p3: PathPoint,
        t: f32,
    ) -> PathPoint {
        let t0 = p0.t;
        let t1 = p1.t;
        let t2 = p2.t;
        let t3 = p3.t;

        if (t2 - t1).abs() <= EPSILON {
            let mut point = p1.lerp(p2, t);
            point.t = t1 + (t2 - t1) * t;
            return point;
        }

        let u = t1 + (t2 - t1) * t;
        let a1 = self.interpolate_between_knots(p0, p1, t0, t1, u);
        let a2 = self.interpolate_between_knots(p1, p2, t1, t2, u);
        let a3 = self.interpolate_between_knots(p2, p3, t2, t3, u);
        let b1 = self.interpolate_between_knots(a1, a2, t0, t2, u);
        let b2 = self.interpolate_between_knots(a2, a3, t1, t3, u);
        let mut point = self.interpolate_between_knots(b1, b2, t1, t2, u);
        point.t = u;
        point
    }

    fn interpolate_between_knots(
        self,
        a: PathPoint,
        b: PathPoint,
        start: f32,
        end: f32,
        value: f32,
    ) -> PathPoint {
        if (end - start).abs() <= EPSILON {
            return a;
        }

        let t = ((value - start) / (end - start)).clamp(0.0, 1.0);
        a.lerp(b, t)
    }

    fn push_distinct(output: &mut Vec<ModeledSample>, sample: ModeledSample) {
        if sample.point.is_finite()
            && output
                .last()
                .is_none_or(|last| last.point.distance_to(sample.point) > EPSILON)
        {
            output.push(sample);
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ModeledSample {
    point: PathPoint,
    input_index: usize,
}

impl From<SmoothedPoint> for ModeledSample {
    fn from(value: SmoothedPoint) -> Self {
        Self {
            point: value.point,
            input_index: value.input_index,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(x: f32, y: f32, timestamp: f64) -> Input {
        Input {
            x,
            y,
            timestamp,
            pressure: 1.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
        }
    }

    fn modeler_without_lag() -> WillInputModeler {
        WillInputModeler::with_config(WillInputModelerConfig {
            stable_lookahead: 0,
            ..WillInputModelerConfig::default()
        })
    }

    fn assert_monotonic_path_t(path: &[PathPoint]) {
        for window in path.windows(2) {
            assert!(window[0].t.is_finite());
            assert!(window[1].t.is_finite());
            assert!(window[0].t <= window[1].t + EPSILON);
        }
    }

    #[test]
    fn single_input_produces_one_finite_path_point() {
        let mut modeler = modeler_without_lag();
        let stable_len = modeler
            .enqueue(&[input(1.0, 2.0, 0.0)], &[])
            .unwrap()
            .metadata
            .stable_len;

        assert_eq!(modeler.path.len(), 1);
        assert_eq!(stable_len, 1);
        assert!(modeler.path[0].is_finite());
    }

    #[test]
    fn pressure_is_clamped_and_tilt_maps_to_orientation_fields() {
        let mut modeler = modeler_without_lag();
        let sample = Input {
            x: 0.0,
            y: 0.0,
            timestamp: 0.0,
            pressure: 2.0,
            tilt_x: 0.7,
            tilt_y: 1.2,
        };

        modeler.enqueue(&[sample], &[]).unwrap();

        let point = modeler.path[0];
        assert_eq!(point.pressure, 1.0);
        assert_eq!(point.altitude, 0.7);
        assert_eq!(point.azimuth, 1.2);
    }

    #[test]
    fn straight_line_inputs_generate_monotonic_interpolated_points() {
        let mut modeler = WillInputModeler::with_config(WillInputModelerConfig {
            smoothing_alpha: 1.0,
            smoothing_beta: 1.0,
            interpolation_spacing: 1.0,
            stable_lookahead: 0,
        });

        modeler
            .enqueue(
                &[
                    input(0.0, 0.0, 0.0),
                    input(4.0, 0.0, 0.1),
                    input(8.0, 0.0, 0.2),
                    input(12.0, 0.0, 0.3),
                ],
                &[],
            )
            .unwrap();

        assert!(modeler.path.len() > 4);
        for window in modeler.path.windows(2) {
            assert!(window[0].x <= window[1].x + EPSILON);
            assert!(window[1].y.abs() <= EPSILON);
        }
        assert_monotonic_path_t(&modeler.path);
    }

    #[test]
    fn straight_line_inputs_generate_centripetal_path_t() {
        let mut modeler = WillInputModeler::with_config(WillInputModelerConfig {
            smoothing_alpha: 1.0,
            smoothing_beta: 1.0,
            interpolation_spacing: 1.0,
            stable_lookahead: 0,
        });

        modeler
            .enqueue(
                &[
                    input(0.0, 0.0, 0.0),
                    input(4.0, 0.0, 0.1),
                    input(8.0, 0.0, 0.2),
                    input(12.0, 0.0, 0.3),
                ],
                &[],
            )
            .unwrap();

        assert_monotonic_path_t(&modeler.path);
        assert!((modeler.path[0].t - 0.0).abs() <= EPSILON);
        assert!((modeler.path.last().unwrap().t - 6.0).abs() <= EPSILON);
    }

    #[test]
    fn duplicate_positions_do_not_emit_non_finite_points() {
        let mut modeler = modeler_without_lag();

        modeler
            .enqueue(
                &[
                    input(0.0, 0.0, 0.0),
                    input(0.0, 0.0, 0.1),
                    input(1.0, 1.0, 0.2),
                    input(1.0, 1.0, 0.3),
                ],
                &[],
            )
            .unwrap();

        assert!(!modeler.path.is_empty());
        assert!(modeler.path.iter().copied().all(PathPoint::is_finite));
        assert_monotonic_path_t(&modeler.path);
    }

    #[test]
    fn prediction_replacement_removes_old_predicted_output() {
        let mut modeler = WillInputModeler::with_config(WillInputModelerConfig {
            smoothing_alpha: 1.0,
            smoothing_beta: 1.0,
            stable_lookahead: 0,
            ..WillInputModelerConfig::default()
        });
        modeler
            .enqueue(
                &[input(0.0, 0.0, 0.0), input(10.0, 0.0, 0.1)],
                &[input(20.0, 0.0, 0.2)],
            )
            .unwrap();
        assert!(modeler.path.iter().any(|point| point.x > 15.0));

        modeler.enqueue(&[], &[input(10.0, 20.0, 0.2)]).unwrap();

        assert!(!modeler.path.iter().any(|point| point.x > 15.0));
        assert!(modeler.path.iter().any(|point| point.y > 10.0));
        assert_monotonic_path_t(&modeler.path);
    }

    #[test]
    fn stable_len_excludes_predicted_tail() {
        let mut modeler = modeler_without_lag();

        modeler
            .enqueue(
                &[
                    input(0.0, 0.0, 0.0),
                    input(5.0, 0.0, 0.1),
                    input(10.0, 0.0, 0.2),
                ],
                &[input(15.0, 0.0, 0.3), input(20.0, 0.0, 0.4)],
            )
            .unwrap();

        assert!(modeler.stable_len() < modeler.path.len());
        assert_eq!(
            modeler.inputs.predicted_start,
            modeler.inputs.cached_inputs.len() - 2
        );
    }

    #[test]
    fn adding_real_input_preserves_or_advances_stable_prefix() {
        let mut modeler = WillInputModeler::with_config(WillInputModelerConfig {
            stable_lookahead: 1,
            ..WillInputModelerConfig::default()
        });

        modeler
            .enqueue(
                &[
                    input(0.0, 0.0, 0.0),
                    input(4.0, 0.0, 0.1),
                    input(8.0, 0.0, 0.2),
                ],
                &[],
            )
            .unwrap();
        let first_stable_len = modeler.stable_len();

        modeler.enqueue(&[input(12.0, 0.0, 0.3)], &[]).unwrap();

        assert!(modeler.stable_len() >= first_stable_len);
    }

    #[test]
    fn reset_clears_state() {
        let mut modeler = modeler_without_lag();
        modeler
            .enqueue(
                &[input(0.0, 0.0, 0.0), input(4.0, 0.0, 0.1)],
                &[input(8.0, 0.0, 0.2)],
            )
            .unwrap();

        modeler.reset();

        assert!(modeler.path.is_empty());
        assert_eq!(modeler.stable_len(), 0);
        assert!(modeler.inputs.cached_inputs.is_empty());
        assert!(modeler.smoother.committed.is_none());
    }

    #[test]
    fn invalid_config_returns_error() {
        let mut modeler = WillInputModeler::with_config(WillInputModelerConfig {
            smoothing_alpha: 1.5,
            ..WillInputModelerConfig::default()
        });

        let error = modeler.enqueue(&[input(0.0, 0.0, 0.0)], &[]).unwrap_err();

        assert!(matches!(
            error,
            InputModelError::InvalidConfig {
                field: "smoothing_alpha",
                ..
            }
        ));
    }

    #[test]
    fn non_finite_input_returns_error() {
        let mut modeler = WillInputModeler::default();
        let sample = Input {
            x: f32::NAN,
            y: 0.0,
            timestamp: 0.0,
            pressure: 1.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
        };

        let error = modeler.enqueue(&[sample], &[]).unwrap_err();

        assert!(matches!(
            error,
            InputModelError::NonFiniteInput {
                input_source: InputSource::Real,
                index: 0,
                field: "position.x",
                ..
            }
        ));
    }

    #[test]
    fn non_monotonic_real_timestamp_returns_error() {
        let mut modeler = WillInputModeler::default();

        let error = modeler
            .enqueue(&[input(0.0, 0.0, 1.0), input(1.0, 0.0, 0.5)], &[])
            .unwrap_err();

        assert!(matches!(
            error,
            InputModelError::NonMonotonicTimestamp {
                input_source: InputSource::Real,
                index: 1,
                previous: 1.0,
                current: 0.5,
            }
        ));
    }

    #[test]
    fn real_timestamp_is_validated_against_cached_real_tail() {
        let mut modeler = modeler_without_lag();
        modeler.enqueue(&[input(0.0, 0.0, 1.0)], &[]).unwrap();

        let error = modeler.enqueue(&[input(1.0, 0.0, 0.5)], &[]).unwrap_err();

        assert!(matches!(
            error,
            InputModelError::NonMonotonicTimestamp {
                input_source: InputSource::Real,
                index: 0,
                previous: 1.0,
                current: 0.5,
            }
        ));
    }

    #[test]
    fn non_monotonic_predicted_timestamp_returns_error() {
        let mut modeler = WillInputModeler::default();

        let error = modeler
            .enqueue(&[input(0.0, 0.0, 1.0)], &[input(1.0, 0.0, 0.5)])
            .unwrap_err();

        assert!(matches!(
            error,
            InputModelError::NonMonotonicTimestamp {
                input_source: InputSource::Predicted,
                index: 0,
                previous: 1.0,
                current: 0.5,
            }
        ));
    }

    #[test]
    fn failed_enqueue_leaves_state_unchanged() {
        let mut modeler = modeler_without_lag();
        modeler.enqueue(&[input(0.0, 0.0, 0.0)], &[]).unwrap();
        let original_path_len = modeler.path.len();
        let original_first_position = (modeler.path[0].x, modeler.path[0].y);
        let original_stable_len = modeler.stable_len;
        let original_cached_len = modeler.inputs.cached_inputs.len();
        let original_predicted_start = modeler.inputs.predicted_start;
        let original_committed = modeler.smoother.committed;

        let error = modeler
            .enqueue(&[input(1.0, 0.0, -1.0)], &[input(2.0, 0.0, 0.2)])
            .unwrap_err();

        assert!(matches!(
            error,
            InputModelError::NonMonotonicTimestamp { .. }
        ));
        assert_eq!(modeler.path.len(), original_path_len);
        assert_eq!(
            (modeler.path[0].x, modeler.path[0].y),
            original_first_position
        );
        assert_eq!(modeler.stable_len, original_stable_len);
        assert_eq!(modeler.inputs.cached_inputs.len(), original_cached_len);
        assert_eq!(modeler.inputs.predicted_start, original_predicted_start);
        assert_eq!(
            modeler
                .smoother
                .committed
                .map(|state| (state.level.x, state.level.y)),
            original_committed.map(|state| (state.level.x, state.level.y))
        );
    }

    #[test]
    fn cached_inputs_remain_bounded_during_long_stroke() {
        let mut modeler = WillInputModeler::default();

        for i in 0..200 {
            modeler
                .enqueue(&[input(i as f32, 0.0, i as f64 * 0.01)], &[])
                .unwrap();
        }

        assert!(
            modeler.inputs.cached_inputs.len()
                <= SPLINE_LOOKBACK + modeler.config.stable_lookahead + 1
        );
    }

    #[test]
    fn invalidated_from_is_old_stable_len() {
        let mut modeler = WillInputModeler::default();
        modeler
            .enqueue(
                &[
                    input(0.0, 0.0, 0.0),
                    input(4.0, 0.0, 0.1),
                    input(8.0, 0.0, 0.2),
                ],
                &[],
            )
            .unwrap();
        let old_stable_len = modeler.stable_len();

        let update = modeler.enqueue(&[input(12.0, 0.0, 0.3)], &[]).unwrap();

        assert_eq!(update.metadata.invalidated_from, old_stable_len);
    }

    #[test]
    fn incremental_rebuild_keeps_path_t_monotonic_after_stable_prefix() {
        let mut modeler = WillInputModeler::with_config(WillInputModelerConfig {
            smoothing_alpha: 1.0,
            smoothing_beta: 1.0,
            interpolation_spacing: 1.0,
            stable_lookahead: 1,
        });

        modeler
            .enqueue(
                &[
                    input(0.0, 0.0, 0.0),
                    input(4.0, 0.0, 0.1),
                    input(8.0, 0.0, 0.2),
                    input(12.0, 0.0, 0.3),
                ],
                &[input(16.0, 0.0, 0.4)],
            )
            .unwrap();

        let stable_len = modeler.stable_len();
        let stable_t = modeler.path[stable_len - 1].t;

        modeler
            .enqueue(&[input(16.0, 0.0, 0.4)], &[input(20.0, 4.0, 0.5)])
            .unwrap();

        assert!(modeler.path[stable_len - 1].t >= stable_t - EPSILON);
        assert_monotonic_path_t(&modeler.path);
    }

    #[test]
    fn prediction_does_not_mutate_committed_smoother_state() {
        let mut modeler = WillInputModeler::default();
        modeler
            .enqueue(
                &[
                    input(0.0, 0.0, 0.0),
                    input(4.0, 0.0, 0.1),
                    input(8.0, 0.0, 0.2),
                ],
                &[],
            )
            .unwrap();
        let committed = modeler
            .smoother
            .committed
            .map(|state| (state.level.x, state.level.y));

        modeler
            .enqueue(&[], &[input(40.0, 40.0, 0.3), input(48.0, 48.0, 0.4)])
            .unwrap();

        assert_eq!(
            modeler
                .smoother
                .committed
                .map(|state| (state.level.x, state.level.y)),
            committed
        );
    }
}
