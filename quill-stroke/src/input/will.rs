//! WILL-style smoothing and incremental centripetal Catmull–Rom modeling.

use peniko::kurbo::Point;
use smallvec::SmallVec;

use super::model::InputBatch;
use super::{InputModelError, InputModeler, InputSample, InputSource, ModelUpdate, PathPoint};

const EPSILON: f32 = 1.0e-5;
const SPLINE_LOOKBACK: usize = 2;

/// Parameters controlling WILL smoothing, interpolation, and stable output.
#[derive(Clone, Copy, Debug)]
pub struct WillInputModelerConfig {
    /// Weight of each measurement when updating the smoothed level, in 0..=1.
    pub smoothing_alpha: f32,
    /// Weight of the observed level change when updating its trend, in 0..=1.
    pub smoothing_beta: f32,
    /// Positive target spacing in stroke-local units for subdividing each chord.
    pub interpolation_spacing: f32,
    /// Number of trailing real inputs kept provisional. The spline always
    /// needs at least one real control point beyond a committed segment.
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

/// Real and predicted controls retained while rebuilding the provisional suffix.
#[derive(Clone, Debug, Default)]
struct InputCursor {
    cached_inputs: SmallVec<[InputSample; 8]>,
    /// First control whose smoothing state has not been committed.
    first_unstable_input: usize,
    /// Boundary between real controls and replaceable predictions in the cache.
    predicted_start: usize,
}

impl InputCursor {
    fn validate_enqueue(
        &self,
        points: &[InputSample],
        predicted: &[InputSample],
    ) -> Result<(), InputModelError> {
        let real_previous_timestamp = self
            .cached_inputs
            .get(..self.predicted_start)
            .and_then(<[InputSample]>::last)
            .map(|input| input.timestamp);
        InputBatch::new(points, InputSource::Real).validate_after(real_previous_timestamp)?;

        let predicted_previous_timestamp = points
            .last()
            .or_else(|| {
                self.cached_inputs
                    .get(..self.predicted_start)
                    .and_then(<[InputSample]>::last)
            })
            .map(|input| input.timestamp);
        InputBatch::new(predicted, InputSource::Predicted)
            .validate_after(predicted_previous_timestamp)?;

        Ok(())
    }

    fn replace_prediction(&mut self, points: &[InputSample], predicted: &[InputSample]) {
        self.cached_inputs.truncate(self.predicted_start);
        self.cached_inputs.extend_from_slice(points);
        self.predicted_start = self.cached_inputs.len();
        self.cached_inputs.extend_from_slice(predicted);
    }

    fn new_first_unstable_input(&self, stable_lookahead: usize) -> usize {
        self.predicted_start
            .saturating_sub(stable_lookahead.max(1))
            .max(usize::from(self.predicted_start > 0))
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

/// Smoothed level and trend after processing a single control sample.
#[derive(Clone, Copy, Debug)]
struct SmoothingState {
    level: PathPoint,
    trend: PathPoint,
}

/// Committed smoothing state used as the base for each provisional rebuild.
#[derive(Clone, Debug, Default)]
struct Smoother {
    committed: Option<SmoothingState>,
    /// Smoothed controls preceding the first provisional input. Their knot
    /// values survive compaction so rebuilding a suffix preserves its tangent.
    anchors: SmallVec<[PathPoint; SPLINE_LOOKBACK]>,
}

impl Smoother {
    fn smooth(
        &self,
        inputs: &[InputSample],
        first_input_index: usize,
        config: WillInputModelerConfig,
    ) -> SmoothedSuffix {
        let mut state = self.committed;
        let mut points = Vec::with_capacity(inputs.len());
        let mut states = Vec::with_capacity(inputs.len());

        for (offset, input) in inputs.iter().copied().enumerate() {
            let raw = PathPoint::from(input);
            let next_state = match state {
                Some(previous) => {
                    let mut level =
                        previous
                            .level
                            .smoothed_toward(raw, previous.trend, config.smoothing_alpha);
                    level.t = previous.level.t + previous.level.distance_to(level).sqrt();
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
            anchors: self
                .anchors
                .iter()
                .copied()
                .enumerate()
                .map(|(offset, point)| SmoothedPoint {
                    point,
                    input_index: first_input_index - self.anchors.len() + offset,
                })
                .collect(),
            points,
            states,
        }
    }

    fn commit(&mut self, smoothed: &SmoothedSuffix, processed_count: usize) {
        if let Some(state) = smoothed.state_after(processed_count) {
            self.committed = Some(state);
            self.anchors.extend(
                smoothed.points[..processed_count]
                    .iter()
                    .map(|sample| sample.point),
            );
            let discard = self.anchors.len().saturating_sub(SPLINE_LOOKBACK);
            self.anchors.drain(..discard);
        }
    }
}

/// Provisional controls and the states available to commit as real input advances.
#[derive(Clone, Debug)]
struct SmoothedSuffix {
    anchors: SmallVec<[SmoothedPoint; SPLINE_LOOKBACK]>,
    points: Vec<SmoothedPoint>,
    states: Vec<SmoothingState>,
}

impl SmoothedSuffix {
    fn state_after(&self, processed_count: usize) -> Option<SmoothingState> {
        processed_count
            .checked_sub(1)
            .and_then(|index| self.states.get(index).copied())
    }

    fn with_anchors(&self) -> Vec<SmoothedPoint> {
        let mut points = Vec::with_capacity(self.points.len() + self.anchors.len());
        points.extend_from_slice(&self.anchors);
        points.extend_from_slice(&self.points);
        points
    }
}

/// A spline control associated with its source position in the input cache.
#[derive(Clone, Copy, Debug)]
struct SmoothedPoint {
    point: PathPoint,
    input_index: usize,
}

/// Stateful WILL-style smoothing and centripetal Catmull–Rom interpolation.
#[derive(Debug, Default)]
pub struct WillInputModeler {
    config: WillInputModelerConfig,
    inputs: InputCursor,
    smoother: Smoother,
    path: Vec<PathPoint>,
    /// Final prefix of `path`; subsequent enqueues rebuild only its suffix.
    stable_len: usize,
}

impl WillInputModeler {
    /// Create an empty modeler with the default smoothing configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an empty modeler; configuration is validated on enqueue.
    pub fn with_config(config: WillInputModelerConfig) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }

    fn validate_enqueue(
        &self,
        points: &[InputSample],
        predicted: &[InputSample],
    ) -> Result<(), InputModelError> {
        self.config.validate()?;
        self.inputs.validate_enqueue(points, predicted)
    }

    /// Rebuild provisional geometry, commit finalized controls, and compact history.
    fn rebuild_unstable_path(&mut self) {
        let first_unstable_input = self.inputs.first_unstable_input;
        let smoothed = self.smoother.smooth(
            &self.inputs.cached_inputs[first_unstable_input..],
            first_unstable_input,
            self.config,
        );
        let new_first_unstable_input = self
            .inputs
            .new_first_unstable_input(self.config.stable_lookahead);

        let emitted = PathBuilder::new(self.config)
            .interpolate(&smoothed.with_anchors(), first_unstable_input);
        for sample in emitted {
            // A discarded duplicate occupies no path slot. Count only emitted
            // samples whose complete four-control dependency is real and final.
            if self.push_path_point(sample.point) && sample.input_index < new_first_unstable_input {
                self.stable_len += 1;
            }
        }

        let processed_stable = new_first_unstable_input.saturating_sub(first_unstable_input);
        self.smoother.commit(&smoothed, processed_stable);
        self.inputs.first_unstable_input = new_first_unstable_input;
        self.inputs.compact();
    }

    fn push_path_point(&mut self, point: PathPoint) -> bool {
        if point.is_finite()
            && self
                .path
                .last()
                .is_none_or(|last| last.distance_to(point) > EPSILON)
        {
            self.path.push(point);
            return true;
        }
        false
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
        points: &[InputSample],
        predicted: &[InputSample],
    ) -> Result<ModelUpdate<'_>, InputModelError> {
        self.validate_enqueue(points, predicted)?;

        let invalidated_from = self.stable_len;
        self.path.truncate(self.stable_len);
        self.inputs.replace_prediction(points, predicted);
        self.rebuild_unstable_path();

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

/// Interpolate smoothed controls while tracking which input finalizes each sample.
#[derive(Clone, Copy)]
struct PathBuilder {
    config: WillInputModelerConfig,
}

impl PathBuilder {
    fn new(config: WillInputModelerConfig) -> Self {
        Self { config }
    }

    /// Rebuild only segments ending in provisional inputs. Earlier controls
    /// supply the incoming tangent but must never be emitted a second time.
    fn interpolate(
        self,
        points: &[SmoothedPoint],
        first_unstable_input: usize,
    ) -> Vec<ModeledSample> {
        let mut output = Vec::new();
        let Some(first) = points.first().copied() else {
            return output;
        };
        if first.input_index >= first_unstable_input {
            output.push(first.into());
        }

        for i in 0..points.len().saturating_sub(1) {
            let p0 = if i == 0 { points[i] } else { points[i - 1] };
            let p1 = points[i];
            let p2 = points[i + 1];
            let p3 = points.get(i + 2).copied().unwrap_or(p2);
            if p2.input_index < first_unstable_input {
                continue;
            }

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

    /// Evaluate geometry with unclamped Barry–Goldman interpolation. Sensor
    /// values use the segment fraction, not extrapolated intermediate values.
    fn centripetal_catmull_rom(
        self,
        p0: PathPoint,
        p1: PathPoint,
        p2: PathPoint,
        p3: PathPoint,
        t: f32,
    ) -> PathPoint {
        // Emit the exact endpoint so the next segment shares the same sample.
        if t >= 1.0 {
            let mut endpoint = p2;
            endpoint.pressure = endpoint.pressure.clamp(0.0, 1.0);
            return endpoint;
        }
        let mut point = p1.lerp(p2, t);
        let mut q0 = Point::new(f64::from(p0.x), f64::from(p0.y));
        let q1 = Point::new(f64::from(p1.x), f64::from(p1.y));
        let q2 = Point::new(f64::from(p2.x), f64::from(p2.y));
        let mut q3 = Point::new(f64::from(p3.x), f64::from(p3.y));
        if q1.distance(q2) <= f64::from(EPSILON) {
            return point;
        }

        // Reflect missing or coincident outer controls to obtain one-sided
        // endpoint tangents without zero knot intervals. A provisional end
        // control is replaced as soon as another input arrives.
        if q0.distance(q1) <= f64::from(EPSILON) {
            q0 = q1 + (q1 - q2);
        }
        if q2.distance(q3) <= f64::from(EPSILON) {
            q3 = q2 + (q2 - q1);
        }

        // Local f64 knots avoid subtracting nearly equal accumulated f32 t's
        // after a long stroke. PathPoint::t still records the global parameter.
        let t0 = 0.0;
        let t1 = q0.distance(q1).sqrt();
        let t2 = t1 + q1.distance(q2).sqrt();
        let t3 = t2 + q2.distance(q3).sqrt();
        let u = t1 + (t2 - t1) * f64::from(t);
        let a1 = Self::interpolate_between_knots(q0, q1, t0, t1, u);
        let a2 = Self::interpolate_between_knots(q1, q2, t1, t2, u);
        let a3 = Self::interpolate_between_knots(q2, q3, t2, t3, u);
        let b1 = Self::interpolate_between_knots(a1, a2, t0, t2, u);
        let b2 = Self::interpolate_between_knots(a2, a3, t1, t3, u);
        let position = Self::interpolate_between_knots(b1, b2, t1, t2, u);
        point.x = position.x as f32;
        point.y = position.y as f32;
        point
    }

    fn interpolate_between_knots(a: Point, b: Point, start: f64, end: f64, value: f64) -> Point {
        a + (b - a) * ((value - start) / (end - start))
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

/// An emitted path point and the input-cache index of its segment endpoint.
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

    fn input(x: f32, y: f32, timestamp: f64) -> InputSample {
        InputSample {
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
        assert!(modeler.path[0].t.abs() <= EPSILON);
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
}
