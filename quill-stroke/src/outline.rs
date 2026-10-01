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
use smallvec::SmallVec;

use crate::{
    brush::BrushTip,
    input::{ModelUpdate, ModelUpdateMeta, PathPoint},
};

const DEFAULT_EPSILON: f64 = 1.0e-5;
const CACHED_POINT_INLINE_CAPACITY: usize = 8;
const GEOMETRY_LOOKBACK_POINTS: usize = 2;

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
    /// Truncate `upper` from this element before rebuilding the cached window.
    upper_instable_index: usize,
    /// Truncate `lower` from this element before rebuilding the cached window.
    lower_instable_index: usize,
    /// Source index of `cached_points[0]`.
    cached_start: usize,
    /// Stable lookback plus the unstable suffix from the input model.
    cached_points: SmallVec<[PathPoint; CACHED_POINT_INLINE_CAPACITY]>,
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
            upper_instable_index: 0,
            lower_instable_index: 0,
            cached_start: 0,
            cached_points: SmallVec::new(),
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

    /// Replace the invalidated source suffix and incrementally rebuild outline paths.
    pub fn update(&mut self, data: ModelUpdate<'_>) {
        debug_assert!(
            data.metadata.invalidated_from >= self.cached_start,
            "SimpleStrokeOutline only caches the unstable tail"
        );

        let keep = data
            .metadata
            .invalidated_from
            .saturating_sub(self.cached_start)
            .min(self.cached_points.len());

        self.cached_points.truncate(keep);
        self.cached_points.extend_from_slice(data.updated);
        self.upper.truncate(self.upper_instable_index);
        self.lower.truncate(self.lower_instable_index);

        self.rebuild_cached_window(data.metadata.stable_len);
        self.drop_committed_cached_points(data.metadata.stable_len);
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

    fn rebuild_cached_window(&mut self, stable_len: usize) {
        let marker_source = stable_len.saturating_sub(GEOMETRY_LOOKBACK_POINTS);
        let mut markers = RebuildMarkers::default();
        let mut run_start = None;
        let mut samples = Vec::new();

        for offset in 0..self.cached_points.len() {
            let point = self.cached_points[offset];
            let source_index = self.cached_start + offset;
            if let Some(sample) = CircleSample::from_path_point(point, self.tip, source_index) {
                run_start.get_or_insert(source_index);
                samples.push(sample);
                continue;
            }

            if let Some(start) = run_start.take() {
                self.append_run_outline(start, &samples, marker_source, &mut markers);
                self.finish_open_contour();
                samples.clear();
            }
        }

        if let Some(start) = run_start {
            self.append_run_outline(start, &samples, marker_source, &mut markers);
        }

        self.upper_instable_index = markers.upper.unwrap_or_else(|| self.upper.elements().len());
        self.lower_instable_index = markers.lower.unwrap_or(self.lower.len());
    }

    fn append_run_outline(
        &mut self,
        source_start: usize,
        run: &[CircleSample],
        marker_source: usize,
        markers: &mut RebuildMarkers,
    ) {
        if run.len() < 2 {
            return;
        }

        if all_centers_coincident(run, self.epsilon) {
            self.finish_open_contour();
            append_circle(
                &mut self.upper,
                run[0].center,
                max_radius(run),
                self.epsilon,
            );
            return;
        }

        let samples = self.simplify_contained_circles(run);
        if samples.len() < 2 {
            self.finish_open_contour();
            append_circle(
                &mut self.upper,
                samples[0].center,
                samples[0].radius,
                self.epsilon,
            );
            return;
        }

        let Some(tangents) = self.segment_tangents(&samples) else {
            self.finish_open_contour();
            let circle = largest_circle(&samples);
            append_circle(&mut self.upper, circle.center, circle.radius, self.epsilon);
            return;
        };

        self.append_tangent_run(source_start, &samples, &tangents, marker_source, markers);
    }

    fn append_tangent_run(
        &mut self,
        source_start: usize,
        samples: &[CircleSample],
        tangents: &[SegmentTangents],
        marker_source: usize,
        markers: &mut RebuildMarkers,
    ) {
        let continuation = source_start == self.cached_start && self.cached_start > 0;
        let first = tangents[0];

        if !continuation && source_start == marker_source {
            markers.mark(self.upper.elements().len(), self.lower.len());
        }

        if !continuation {
            self.finish_open_contour();
            self.upper.move_to(first.right_start);
            self.ensure_lower_close_path();
            append_clockwise_arc(
                &mut self.upper,
                samples[0].center,
                samples[0].radius,
                first.right_start,
                first.left_start,
                self.epsilon,
            );
        }

        for (segment_index, tangent) in tangents.iter().enumerate() {
            let segment_source = source_start + segment_index;
            if continuation && segment_source == marker_source {
                markers.mark(self.upper.elements().len(), self.lower.len());
            }

            if segment_index > 0 {
                let sample = samples[segment_index];
                let previous = tangents[segment_index - 1];
                append_upper_join(&mut self.upper, sample, previous, *tangent, self.epsilon);
                append_lower_join(&mut self.lower, sample, previous, *tangent, self.epsilon);
            }

            self.upper.line_to(tangent.left_end);
            self.lower.push(PathEl::LineTo(tangent.right_start));
        }

        let end = samples[samples.len() - 1];
        let last = tangents[tangents.len() - 1];
        append_clockwise_arc(
            &mut self.upper,
            end.center,
            end.radius,
            last.left_end,
            last.right_end,
            self.epsilon,
        );
    }

    fn finish_open_contour(&mut self) {
        if self.lower.is_empty() {
            return;
        }

        self.upper.extend(self.lower.iter().rev().copied());
        self.lower.clear();
    }

    fn ensure_lower_close_path(&mut self) {
        if self.lower.is_empty() {
            self.lower.push(PathEl::ClosePath);
        }
    }

    fn drop_committed_cached_points(&mut self, stable_len: usize) {
        let next_cached_start = stable_len.saturating_sub(GEOMETRY_LOOKBACK_POINTS);
        let drain_len = next_cached_start
            .saturating_sub(self.cached_start)
            .min(self.cached_points.len());

        if drain_len > 0 {
            self.cached_points.drain(..drain_len);
            self.cached_start += drain_len;
        }
    }

    fn segment_tangents(&self, samples: &[CircleSample]) -> Option<Vec<SegmentTangents>> {
        samples
            .windows(2)
            .map(|pair| tangent_pair(pair[0], pair[1], self.epsilon))
            .collect()
    }

    fn simplify_contained_circles(&self, run: &[CircleSample]) -> Vec<CircleSample> {
        let mut samples = run.to_vec();
        loop {
            let Some(index) = removable_contained_circle(&samples, self.epsilon)
                .or_else(|| invalid_tangent_pair_circle(&samples, self.epsilon))
            else {
                break;
            };

            samples.remove(index);
            if samples.len() <= 1 {
                break;
            }
        }

        samples
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct RebuildMarkers {
    upper: Option<usize>,
    lower: Option<usize>,
}

impl RebuildMarkers {
    fn mark(&mut self, upper: usize, lower: usize) {
        self.upper.get_or_insert(upper);
        self.lower.get_or_insert(lower);
    }
}

#[derive(Clone, Copy, Debug)]
struct CircleSample {
    center: Point,
    radius: f64,
}

impl CircleSample {
    fn from_path_point(point: PathPoint, tip: BrushTip, _source_index: usize) -> Option<Self> {
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
        finite_non_negative(radius).map(|radius| Self { center, radius })
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
    fn single_point_run_is_empty() {
        let points = [point(0.0, 0.0, 1.0)];
        let path = path(&points, BrushTip::FixedRound(RoundTip::new(2.0)));

        assert!(path.is_empty());
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
    fn self_crossing_centerline_still_returns_closed_path() {
        let points = [
            point(0.0, 0.0, 1.0),
            point(10.0, 10.0, 1.0),
            point(0.0, 10.0, 1.0),
            point(10.0, 0.0, 1.0),
        ];
        let path = path(&points, BrushTip::FixedRound(RoundTip::new(1.0)));

        assert_eq!(closed_subpaths(&path), 1);
        assert!(
            path.iter()
                .any(|element| matches!(element, PathEl::CurveTo(control0, control1, _) if control0 == control1))
        );
    }

    #[test]
    fn semicircle_cap_splits_into_repeated_control_cubics() {
        let points = [point(0.0, 0.0, 1.0), point(10.0, 0.0, 1.0)];
        let outline = SimpleStrokeOutline::new_with_path(
            &points,
            BrushTip::FixedRound(RoundTip::new(2.0)),
            StrokeOutlineOptions::default(),
        );
        let repeated_controls = outline
            .upper()
            .iter()
            .filter(|element| matches!(element, PathEl::CurveTo(control0, control1, _) if control0 == control1))
            .count();

        assert!(repeated_controls >= 4);
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

    #[test]
    fn cached_points_keep_small_stable_lookback() {
        let points = [
            point(0.0, 0.0, 1.0),
            point(10.0, 0.0, 1.0),
            point(20.0, 0.0, 1.0),
            point(30.0, 0.0, 1.0),
        ];
        let outline = SimpleStrokeOutline::new_with_path(
            &points,
            BrushTip::FixedRound(RoundTip::new(2.0)),
            StrokeOutlineOptions::default(),
        );

        assert_eq!(outline.cached_start, 2);
        assert_eq!(outline.cached_points.len(), 2);
    }

    #[test]
    fn early_tangent_intersection_detects_sharp_join_before_tangent_points() {
        let intersection = early_tangent_intersection(
            Point::new(0.0, 0.0),
            Point::new(1.0, 0.0),
            Point::new(0.0, 1.0),
            Point::new(0.0, 2.0),
            DEFAULT_EPSILON,
        )
        .unwrap();

        assert_eq!(intersection, Point::new(0.0, 0.0));
    }
}
