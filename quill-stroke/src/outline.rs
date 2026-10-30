//! CPU outline generation for filled stroke rendering.
//!
//! This module keeps a simple stroke outline as two paths: the forward `upper`
//! side and the lower side stored in source order. Rendering iterates the upper
//! side forward and the lower side backward, producing one filled outline.

use std::{
    f64::consts::{FRAC_PI_2, PI, TAU},
    iter::{Chain, Copied, Rev},
    slice,
};

use peniko::kurbo::{BezPath, PathEl, Point, Vec2};

use crate::{
    brush::BrushTip,
    input::{ModelUpdate, ModelUpdateMeta, PathPoint},
};

const DEFAULT_EPSILON: f64 = 1.0e-5;

type UpperIter<'a> = Copied<slice::Iter<'a, PathEl>>;
type LowerIter<'a> = Copied<Rev<slice::Iter<'a, PathEl>>>;

/// Options controlling round-tip outline construction.
#[derive(Clone, Copy, Debug)]
pub struct StrokeOutlineOptions {
    /// Geometric tolerance for treating points and radii as degenerate.
    pub epsilon: f64,
}

impl Default for StrokeOutlineOptions {
    fn default() -> Self {
        Self {
            epsilon: DEFAULT_EPSILON,
        }
    }
}

/// Incremental outline builder for modeled stroke points.
pub trait StrokeOutline {
    /// Iterator over borrowed path-element slices.
    type Iter<'a>: Iterator<Item = PathEl> + 'a
    where
        Self: 'a;

    /// Build an empty outline for the brush tip.
    fn new(tip: BrushTip, options: StrokeOutlineOptions) -> Self;

    /// Build an outline from a complete, stable modeled path.
    fn new_with_path(points: &[PathPoint], tip: BrushTip, options: StrokeOutlineOptions) -> Self;

    /// Apply an input-model update by replacing the invalidated path suffix.
    fn update(&mut self, data: ModelUpdate<'_>);

    /// Iterate the filled outline as Kurbo path elements.
    fn iter(&self) -> Self::Iter<'_>;
}

/// Shallow iterator over the upper path followed by the reversed lower path.
pub struct StrokeOutlineIter<'a> {
    inner: Chain<UpperIter<'a>, LowerIter<'a>>,
}

impl Iterator for StrokeOutlineIter<'_> {
    type Item = PathEl;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

/// Simple round-tip outline built from upper and lower tangent sides.
#[derive(Clone, Debug)]
pub struct SimpleStrokeOutline {
    /// Brush-tip geometry used to convert input points into circle samples.
    tip: BrushTip,
    /// Normalized geometric tolerance for degenerate point/radius handling.
    epsilon: f64,
    /// Forward half of the outline. It starts with `MoveTo` for each contour.
    upper: BezPath,
    /// Lower side commands in source order. Iteration reads this in reverse.
    lower: Vec<PathEl>,
    /// Full modeled source retained so containment changes can roll back past
    /// any previously simplified circle, including across the stable boundary.
    points: Vec<PathPoint>,
    /// Each visible run owns checkpoints before its joins and final cap.
    runs: Vec<RunOutline>,
}

impl StrokeOutline for SimpleStrokeOutline {
    type Iter<'a> = StrokeOutlineIter<'a>;

    fn new(tip: BrushTip, options: StrokeOutlineOptions) -> Self {
        SimpleStrokeOutline::new(tip, options)
    }

    fn new_with_path(points: &[PathPoint], tip: BrushTip, options: StrokeOutlineOptions) -> Self {
        SimpleStrokeOutline::new_with_path(points, tip, options)
    }

    fn update(&mut self, data: ModelUpdate<'_>) {
        SimpleStrokeOutline::update(self, data);
    }

    fn iter(&self) -> Self::Iter<'_> {
        SimpleStrokeOutline::iter(self)
    }
}

impl SimpleStrokeOutline {
    /// Build an empty simple round-tip outline.
    pub fn new(tip: BrushTip, options: StrokeOutlineOptions) -> Self {
        Self {
            tip,
            epsilon: finite_positive(options.epsilon).unwrap_or(DEFAULT_EPSILON),
            upper: BezPath::new(),
            lower: Vec::new(),
            points: Vec::new(),
            runs: Vec::new(),
        }
    }

    /// Build a simple round-tip outline from a complete, stable modeled path.
    pub fn new_with_path(
        points: &[PathPoint],
        tip: BrushTip,
        options: StrokeOutlineOptions,
    ) -> Self {
        let mut outline = Self::new(tip, options);
        outline.update(ModelUpdate {
            metadata: ModelUpdateMeta {
                invalidated_from: 0,
                stable_len: points.len(),
            },
            updated: points,
        });
        outline
    }

    /// Replace an invalidated source suffix and rebuild the affected geometry.
    ///
    /// A stable input prefix does not imply stable outline geometry: a later
    /// larger circle can contain earlier circles and change their tangents.
    pub fn update(&mut self, data: ModelUpdate<'_>) {
        assert!(
            data.metadata.invalidated_from <= self.points.len(),
            "outline updates must not skip source samples"
        );
        self.points.truncate(data.metadata.invalidated_from);
        self.points.extend_from_slice(data.updated);
        self.rebuild_runs();
    }

    /// Synchronize with a complete path, including updates skipped between
    /// paints. Only the differing suffix replaces the retained source points.
    pub fn sync_path(&mut self, points: &[PathPoint]) {
        let common = self
            .points
            .iter()
            .zip(points)
            .take_while(|(a, b)| a == b)
            .count();
        if common == self.points.len() && common == points.len() {
            return;
        }
        self.update(ModelUpdate {
            metadata: ModelUpdateMeta {
                invalidated_from: common,
                stable_len: 0,
            },
            updated: &points[common..],
        });
    }

    /// Iterate the filled outline as upper elements followed by reversed lower elements.
    pub fn iter(&self) -> StrokeOutlineIter<'_> {
        StrokeOutlineIter {
            inner: self
                .upper
                .elements()
                .iter()
                .copied()
                .chain(self.lower.iter().rev().copied()),
        }
    }

    /// Return the forward upper path elements.
    pub fn upper(&self) -> &[PathEl] {
        self.upper.elements()
    }

    /// Return the lower path elements in stored source order.
    pub fn lower(&self) -> &[PathEl] {
        &self.lower
    }

    /// Reconcile visible runs with their retained geometry. Circle identities
    /// survive simplification, so a rollback is based on actual dependencies.
    fn rebuild_runs(&mut self) {
        let mut source_runs = Vec::new();
        let mut run = Vec::new();
        for (source_index, point) in self.points.iter().copied().enumerate() {
            if let Some(sample) = CircleSample::from_path_point(point, self.tip, source_index) {
                run.push(sample);
            } else if !run.is_empty() {
                source_runs.push(std::mem::take(&mut run));
            }
        }
        if !run.is_empty() {
            source_runs.push(run);
        }
        // Preserve the existing single-point policy; this cache handles runs
        // with segments and circles produced by containment simplification.
        source_runs.retain(|run| run.len() >= 2);
        self.runs
            .resize_with(source_runs.len(), RunOutline::default);
        for (cached, source) in self.runs.iter_mut().zip(source_runs) {
            cached.sync(&source, self.epsilon);
        }

        self.upper.truncate(0);
        self.lower.clear();
        if let Some((last, completed)) = self.runs.split_last() {
            for run in completed {
                self.upper.extend(run.upper.elements().iter().copied());
                self.upper.extend(run.lower.iter().rev().copied());
            }
            self.upper.extend(last.upper.elements().iter().copied());
            self.lower.extend_from_slice(&last.lower);
        }
    }
}

/// Geometry for one visible run. Checkpoint i precedes the join and segment
/// starting at circle i; the final checkpoint precedes the replaceable end cap.
#[derive(Clone, Debug, Default)]
struct RunOutline {
    samples: Vec<CircleSample>,
    upper: BezPath,
    lower: Vec<PathEl>,
    checkpoints: Vec<OutlineCheckpoint>,
}

#[derive(Clone, Copy, Debug)]
struct OutlineCheckpoint {
    upper: usize,
    lower: usize,
}

impl RunOutline {
    fn sync(&mut self, source: &[CircleSample], epsilon: f64) {
        let samples = Self::simplified_samples(source, epsilon);
        if self.samples == samples {
            return;
        }
        let tangents = samples
            .windows(2)
            .map(|pair| tangent_pair(pair[0], pair[1], epsilon))
            .collect::<Option<Vec<_>>>();
        let Some(tangents) = tangents.filter(|tangents| !tangents.is_empty()) else {
            self.upper.truncate(0);
            self.lower.clear();
            self.checkpoints.clear();
            let circle = largest_circle(&samples);
            append_circle(&mut self.upper, circle.center, circle.radius, epsilon);
            self.samples = samples;
            return;
        };

        let common = self
            .samples
            .iter()
            .zip(&samples)
            .take_while(|(a, b)| a == b)
            .count();
        // A changed circle also changes the preceding segment and its start
        // join. Containment can move this boundary arbitrarily far backward.
        let rebuild_from = common.saturating_sub(1).min(tangents.len());
        let rebuild_from = if let Some(checkpoint) = self.checkpoints.get(rebuild_from) {
            self.upper.truncate(checkpoint.upper);
            self.lower.truncate(checkpoint.lower);
            self.checkpoints.truncate(rebuild_from);
            rebuild_from
        } else {
            self.upper.truncate(0);
            self.lower.clear();
            self.checkpoints.clear();
            0
        };

        for segment_index in rebuild_from..tangents.len() {
            self.checkpoints.push(self.checkpoint());
            let tangent = tangents[segment_index];
            if segment_index == 0 {
                self.upper.move_to(tangent.right_start);
                self.lower.push(PathEl::ClosePath);
                append_clockwise_arc(
                    &mut self.upper,
                    samples[0].center,
                    samples[0].radius,
                    tangent.right_start,
                    tangent.left_start,
                    epsilon,
                );
            } else {
                let sample = samples[segment_index];
                let previous = tangents[segment_index - 1];
                append_upper_join(&mut self.upper, sample, previous, tangent, epsilon);
                append_lower_join(&mut self.lower, sample, previous, tangent, epsilon);
            }
            self.upper.line_to(tangent.left_end);
            self.lower.push(PathEl::LineTo(tangent.right_start));
        }
        self.checkpoints.push(self.checkpoint());
        let end = samples[samples.len() - 1];
        let last = tangents[tangents.len() - 1];
        append_clockwise_arc(
            &mut self.upper,
            end.center,
            end.radius,
            last.left_end,
            last.right_end,
            epsilon,
        );
        self.samples = samples;
    }

    fn checkpoint(&self) -> OutlineCheckpoint {
        OutlineCheckpoint {
            upper: self.upper.elements().len(),
            lower: self.lower.len(),
        }
    }

    /// Re-evaluate containment against the complete run. Keeping source indices
    /// lets the geometry cache find a safe prefix even when several circles vanish.
    fn simplified_samples(run: &[CircleSample], epsilon: f64) -> Vec<CircleSample> {
        if all_centers_coincident(run, epsilon) {
            return vec![CircleSample {
                radius: max_radius(run),
                ..run[0]
            }];
        }
        let mut samples = run.to_vec();
        while let Some(index) = removable_contained_circle(&samples, epsilon)
            .or_else(|| invalid_tangent_pair_circle(&samples, epsilon))
        {
            samples.remove(index);
            if samples.len() <= 1 {
                break;
            }
        }
        samples
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CircleSample {
    /// Original modeled point, retained through circle simplification.
    source_index: usize,
    center: Point,
    radius: f64,
}

impl CircleSample {
    fn from_path_point(point: PathPoint, tip: BrushTip, source_index: usize) -> Option<Self> {
        if !point.is_visible() {
            return None;
        }

        let center = Point::new(f64::from(point.x()), f64::from(point.y()));
        if !center.x.is_finite() || !center.y.is_finite() {
            return None;
        }

        let radius = match tip {
            BrushTip::Round(tip) => f64::from(tip.radius.0) * f64::from(point.pressure().max(0.0)),
            BrushTip::FixedRound(tip) => f64::from(tip.radius.0),
        };
        finite_non_negative(radius).map(|radius| Self {
            source_index,
            center,
            radius,
        })
    }

    fn contains(self, other: Self, epsilon: f64) -> bool {
        self.center.distance(other.center) + other.radius <= self.radius + epsilon
    }
}

#[derive(Clone, Copy, Debug)]
struct SegmentTangents {
    left_start: Point,
    left_end: Point,
    right_start: Point,
    right_end: Point,
}

fn append_upper_join(
    path: &mut BezPath,
    sample: CircleSample,
    previous: SegmentTangents,
    next: SegmentTangents,
    epsilon: f64,
) {
    append_side_join_forward(
        path,
        sample,
        previous.left_start,
        previous.left_end,
        next.left_start,
        next.left_end,
        epsilon,
    );
}

fn append_lower_join(
    lower: &mut Vec<PathEl>,
    sample: CircleSample,
    previous: SegmentTangents,
    next: SegmentTangents,
    epsilon: f64,
) {
    append_side_join_reversed(
        lower,
        sample,
        previous.right_start,
        previous.right_end,
        next.right_start,
        next.right_end,
        epsilon,
    );
}

fn append_side_join_forward(
    path: &mut BezPath,
    sample: CircleSample,
    previous_start: Point,
    a: Point,
    b: Point,
    next_end: Point,
    epsilon: f64,
) {
    if let Some(intersection) = early_tangent_intersection(previous_start, a, b, next_end, epsilon)
    {
        path.line_to(intersection);
        path.line_to(b);
        return;
    }

    append_shortest_arc(path, sample.center, sample.radius, a, b, epsilon);
}

fn append_side_join_reversed(
    lower: &mut Vec<PathEl>,
    sample: CircleSample,
    previous_start: Point,
    a: Point,
    b: Point,
    next_end: Point,
    epsilon: f64,
) {
    if let Some(intersection) = early_tangent_intersection(previous_start, a, b, next_end, epsilon)
    {
        lower.push(PathEl::LineTo(a));
        lower.push(PathEl::LineTo(intersection));
        return;
    }

    append_shortest_arc_reversed(lower, sample.center, sample.radius, a, b, epsilon);
}

fn early_tangent_intersection(
    previous_start: Point,
    a: Point,
    b: Point,
    next_end: Point,
    epsilon: f64,
) -> Option<Point> {
    let previous_direction = a - previous_start;
    let next_direction = next_end - b;
    let intersection = line_intersection(previous_start, previous_direction, b, next_direction)?;
    let previous_unit = normalize(previous_direction, epsilon)?;
    let next_unit = normalize(next_direction, epsilon)?;
    let before_a = (intersection - a).dot(previous_unit) < -epsilon;
    let before_b = (intersection - b).dot(next_unit) < -epsilon;

    (before_a && before_b).then_some(intersection)
}

fn tangent_pair(start: CircleSample, end: CircleSample, epsilon: f64) -> Option<SegmentTangents> {
    let delta = end.center - start.center;
    let distance = delta.hypot();
    if distance <= epsilon {
        return None;
    }

    let radius_delta = start.radius - end.radius;
    let ratio = (radius_delta / distance).clamp(-1.0, 1.0);
    if ratio.abs() >= 1.0 - epsilon {
        return None;
    }

    let along = delta / distance;
    let perp = Vec2::new(-along.y, along.x);
    let side = (1.0 - ratio * ratio).sqrt();
    let left_normal = along * ratio + perp * side;
    let right_normal = along * ratio - perp * side;

    Some(SegmentTangents {
        left_start: start.center + left_normal * start.radius,
        left_end: end.center + left_normal * end.radius,
        right_start: start.center + right_normal * start.radius,
        right_end: end.center + right_normal * end.radius,
    })
}

fn removable_contained_circle(samples: &[CircleSample], epsilon: f64) -> Option<usize> {
    for index in 0..samples.len() {
        let sample = samples[index];
        let contained_by_previous = index
            .checked_sub(1)
            .is_some_and(|previous| samples[previous].contains(sample, epsilon));
        let contained_by_next = samples
            .get(index + 1)
            .is_some_and(|next| next.contains(sample, epsilon));

        if contained_by_previous || contained_by_next {
            return Some(index);
        }
    }

    None
}

fn invalid_tangent_pair_circle(samples: &[CircleSample], epsilon: f64) -> Option<usize> {
    for (index, pair) in samples.windows(2).enumerate() {
        let distance = pair[0].center.distance(pair[1].center);
        if distance <= (pair[0].radius - pair[1].radius).abs() + epsilon {
            return Some(if pair[0].radius <= pair[1].radius {
                index
            } else {
                index + 1
            });
        }
    }

    None
}

fn append_circle(path: &mut BezPath, center: Point, radius: f64, epsilon: f64) {
    if radius <= epsilon {
        return;
    }

    let start = center + Vec2::new(radius, 0.0);
    path.move_to(start);
    append_arc(path, center, radius, 0.0, TAU, epsilon);
    path.close_path();
}

fn append_shortest_arc(
    path: &mut BezPath,
    center: Point,
    radius: f64,
    from: Point,
    to: Point,
    epsilon: f64,
) {
    append_arc_between(path, center, radius, from, to, ArcSweep::Shortest, epsilon);
}

fn append_clockwise_arc(
    path: &mut BezPath,
    center: Point,
    radius: f64,
    from: Point,
    to: Point,
    epsilon: f64,
) {
    append_arc_between(path, center, radius, from, to, ArcSweep::Clockwise, epsilon);
}

fn append_shortest_arc_reversed(
    lower: &mut Vec<PathEl>,
    center: Point,
    radius: f64,
    from: Point,
    to: Point,
    epsilon: f64,
) {
    append_arc_between_reversed(lower, center, radius, from, to, ArcSweep::Shortest, epsilon);
}

#[derive(Clone, Copy, Debug)]
enum ArcSweep {
    Shortest,
    Clockwise,
}

fn append_arc_between(
    path: &mut BezPath,
    center: Point,
    radius: f64,
    from: Point,
    to: Point,
    sweep_kind: ArcSweep,
    epsilon: f64,
) {
    if radius <= epsilon || from.distance(to) <= epsilon {
        path.line_to(to);
        return;
    }

    let start_angle = angle_of(from - center);
    let end_angle = angle_of(to - center);
    let sweep = match sweep_kind {
        ArcSweep::Shortest => shortest_sweep(start_angle, end_angle),
        ArcSweep::Clockwise => clockwise_sweep(start_angle, end_angle),
    };

    append_arc(path, center, radius, start_angle, sweep, epsilon);
}

fn append_arc_between_reversed(
    lower: &mut Vec<PathEl>,
    center: Point,
    radius: f64,
    from: Point,
    to: Point,
    sweep_kind: ArcSweep,
    epsilon: f64,
) {
    if radius <= epsilon || from.distance(to) <= epsilon {
        lower.push(PathEl::LineTo(from));
        return;
    }

    let start_angle = angle_of(from - center);
    let end_angle = angle_of(to - center);
    let sweep = match sweep_kind {
        ArcSweep::Shortest => shortest_sweep(start_angle, end_angle),
        ArcSweep::Clockwise => clockwise_sweep(start_angle, end_angle),
    };

    append_arc_reversed(lower, center, radius, start_angle, sweep, epsilon);
}

fn append_arc(
    path: &mut BezPath,
    center: Point,
    radius: f64,
    start_angle: f64,
    sweep: f64,
    epsilon: f64,
) {
    if radius <= epsilon || sweep.abs() <= epsilon {
        return;
    }

    if sweep.abs() > FRAC_PI_2 {
        let half = sweep * 0.5;
        append_arc(path, center, radius, start_angle, half, epsilon);
        append_arc(path, center, radius, start_angle + half, half, epsilon);
        return;
    }

    let from = point_on_circle(center, radius, start_angle);
    let to = point_on_circle(center, radius, start_angle + sweep);
    if let Some(control) = tangent_intersection_for_arc(center, from, to, sweep, epsilon) {
        path.curve_to(control, control, to);
    } else {
        path.line_to(to);
    }
}

fn append_arc_reversed(
    lower: &mut Vec<PathEl>,
    center: Point,
    radius: f64,
    start_angle: f64,
    sweep: f64,
    epsilon: f64,
) {
    if radius <= epsilon || sweep.abs() <= epsilon {
        return;
    }

    if sweep.abs() > FRAC_PI_2 {
        let half = sweep * 0.5;
        append_arc_reversed(lower, center, radius, start_angle, half, epsilon);
        append_arc_reversed(lower, center, radius, start_angle + half, half, epsilon);
        return;
    }

    let from = point_on_circle(center, radius, start_angle);
    let to = point_on_circle(center, radius, start_angle + sweep);
    if let Some(control) = tangent_intersection_for_arc(center, from, to, sweep, epsilon) {
        lower.push(PathEl::CurveTo(control, control, from));
    } else {
        lower.push(PathEl::LineTo(from));
    }
}

fn tangent_intersection_for_arc(
    center: Point,
    from: Point,
    to: Point,
    sweep: f64,
    epsilon: f64,
) -> Option<Point> {
    let sign = sweep.signum();
    let from_tangent = normalize(tangent_at(angle_of(from - center)) * sign, epsilon)?;
    let to_tangent = normalize(tangent_at(angle_of(to - center)) * sign, epsilon)?;
    line_intersection(from, from_tangent, to, -to_tangent)
}

fn line_intersection(
    origin_a: Point,
    direction_a: Vec2,
    origin_b: Point,
    direction_b: Vec2,
) -> Option<Point> {
    let determinant = cross(direction_a, direction_b);
    if determinant.abs() <= DEFAULT_EPSILON {
        return None;
    }

    let t = cross(origin_b - origin_a, direction_b) / determinant;
    Some(origin_a + direction_a * t)
}

fn point_on_circle(center: Point, radius: f64, angle: f64) -> Point {
    center + Vec2::new(angle.cos() * radius, angle.sin() * radius)
}

fn tangent_at(angle: f64) -> Vec2 {
    Vec2::new(-angle.sin(), angle.cos())
}

fn angle_of(vector: Vec2) -> f64 {
    vector.y.atan2(vector.x)
}

fn shortest_sweep(start: f64, end: f64) -> f64 {
    let mut sweep = end - start;
    while sweep > PI {
        sweep -= TAU;
    }
    while sweep <= -PI {
        sweep += TAU;
    }
    sweep
}

fn clockwise_sweep(start: f64, end: f64) -> f64 {
    let mut sweep = end - start;
    while sweep > 0.0 {
        sweep -= TAU;
    }
    if sweep.abs() <= DEFAULT_EPSILON {
        -TAU
    } else {
        sweep
    }
}

fn normalize(vector: Vec2, epsilon: f64) -> Option<Vec2> {
    let length = vector.hypot();
    (length > epsilon).then_some(vector / length)
}

fn cross(a: Vec2, b: Vec2) -> f64 {
    a.x * b.y - a.y * b.x
}

fn all_centers_coincident(run: &[CircleSample], epsilon: f64) -> bool {
    run.iter()
        .all(|sample| sample.center.distance(run[0].center) <= epsilon)
}

fn max_radius(run: &[CircleSample]) -> f64 {
    run.iter().map(|sample| sample.radius).fold(0.0, f64::max)
}

fn largest_circle(run: &[CircleSample]) -> CircleSample {
    run.iter()
        .copied()
        .max_by(|a, b| a.radius.total_cmp(&b.radius))
        .unwrap_or(CircleSample {
            source_index: 0,
            center: Point::ZERO,
            radius: 0.0,
        })
}

fn finite_positive(value: f64) -> Option<f64> {
    (value.is_finite() && value > 0.0).then_some(value)
}

fn finite_non_negative(value: f64) -> Option<f64> {
    (value.is_finite() && value >= 0.0).then_some(value)
}

#[cfg(test)]
mod tests {
    use peniko::kurbo::{PathEl, Shape};

    use super::*;
    use crate::brush::{BrushTip, RoundTip};

    fn point(x: f32, y: f32, pressure: f32) -> PathPoint {
        PathPoint::for_test(x, y, pressure)
    }

    fn path(points: &[PathPoint], tip: BrushTip) -> BezPath {
        SimpleStrokeOutline::new_with_path(points, tip, StrokeOutlineOptions::default())
            .iter()
            .collect()
    }

    fn closed_subpaths(path: &BezPath) -> usize {
        path.iter()
            .filter(|element| matches!(element, PathEl::ClosePath))
            .count()
    }

    #[test]
    fn straight_fixed_round_line_has_expected_bounds() {
        let points = [point(0.0, 0.0, 1.0), point(10.0, 0.0, 1.0)];
        let path = path(&points, BrushTip::FixedRound(RoundTip::new(2.0)));
        let bounds = path.bounding_box();

        assert_eq!(closed_subpaths(&path), 1);
        assert!((bounds.x0 - -2.0).abs() <= 1.0e-3);
        assert!((bounds.x1 - 12.0).abs() <= 1.0e-3);
        assert!((bounds.y0 - -2.0).abs() <= 1.0e-3);
        assert!((bounds.y1 - 2.0).abs() <= 1.0e-3);
    }

    #[test]
    fn round_tip_uses_path_point_pressure() {
        let points = [point(0.0, 0.0, 0.5), point(10.0, 0.0, 1.0)];
        let path = path(&points, BrushTip::Round(RoundTip::new(4.0)));
        let bounds = path.bounding_box();

        assert!(bounds.x0 < -1.9 && bounds.x0 > -2.1);
        assert!(bounds.x1 > 13.9 && bounds.x1 < 14.1);
    }

    #[test]
    fn hidden_points_split_visible_runs() {
        let mut points = [
            point(0.0, 0.0, 1.0),
            point(10.0, 0.0, 1.0),
            point(20.0, 0.0, 1.0),
            point(30.0, 0.0, 1.0),
            point(40.0, 0.0, 1.0),
        ];
        points[2].set_visible(false);

        let path = path(&points, BrushTip::FixedRound(RoundTip::new(2.0)));

        assert_eq!(closed_subpaths(&path), 2);
    }

    #[test]
    fn duplicate_position_run_produces_circle() {
        let points = [point(3.0, 4.0, 1.0), point(3.0, 4.0, 1.0)];
        let path = path(&points, BrushTip::FixedRound(RoundTip::new(2.0)));
        let bounds = path.bounding_box();

        assert_eq!(closed_subpaths(&path), 1);
        assert!((bounds.x0 - 1.0).abs() <= 1.0e-3);
        assert!((bounds.x1 - 5.0).abs() <= 1.0e-3);
        assert!((bounds.y0 - 2.0).abs() <= 1.0e-3);
        assert!((bounds.y1 - 6.0).abs() <= 1.0e-3);
    }

    #[test]
    fn incremental_suffix_rebuild_matches_fresh_outline() {
        let initial = [point(0.0, 0.0, 1.0), point(10.0, 0.0, 1.0)];
        let final_points = [
            point(0.0, 0.0, 1.0),
            point(10.0, 5.0, 1.0),
            point(20.0, 0.0, 1.0),
        ];
        let updated = [final_points[1], final_points[2]];
        let mut outline = SimpleStrokeOutline::new(
            BrushTip::FixedRound(RoundTip::new(2.0)),
            StrokeOutlineOptions::default(),
        );

        outline.update(ModelUpdate {
            metadata: ModelUpdateMeta {
                invalidated_from: 0,
                stable_len: 1,
            },
            updated: &initial,
        });
        outline.update(ModelUpdate {
            metadata: ModelUpdateMeta {
                invalidated_from: 1,
                stable_len: 1,
            },
            updated: &updated,
        });

        let incremental: BezPath = outline.iter().collect();
        let fresh = path(&final_points, BrushTip::FixedRound(RoundTip::new(2.0)));
        assert_eq!(incremental, fresh);
    }
}
