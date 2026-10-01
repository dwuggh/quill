//! Root-recursive item and pixel-wise erasing for visible drawable elements.

use std::collections::{HashMap, HashSet};

use bevy_ecs::{hierarchy::Children, message::MessageRegistry, prelude::*, schedule::Schedule};
use peniko::kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape as _, Stroke, Vec2, flatten};
use quill_stroke::{
    input::{Input, ModelUpdate, ModelUpdateMeta, PathPoint},
    outline::{SimpleStrokeOutline, StrokeOutlineOptions},
};

use crate::{
    canvas::CanvasMode,
    document::DocumentRootEntity,
    element::{
        EraserConfig, EraserMessage, EraserMode, EraserShape, EraserTargets, Tool,
        bounds::{RectBound, RoundBound, rect_from_min_max},
        image::ImageElement,
        page::Page,
        selection::SelectionState,
        stroke::{StrokeData, StrokeStatus, rect_bound_from_path_points},
        text::TextElement,
    },
    geom::CanvasTransform,
    input::CanvasInput,
    runtime::QuillUpdateSet,
};

pub struct Eraser;

/// Marker component that hides an entity from rendering and eraser hit testing.
///
/// A hidden page also hides its descendants because eraser traversal does not
/// enter hidden page subtrees.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Hidden;

/// Pixel-wise eraser coverage attached to a drawable element.
#[derive(Component, Clone, Debug, Default)]
pub struct Mask {
    /// Element-local eraser sweep segments applied to this drawable.
    pub segments: Vec<MaskSegment>,
}

/// One element-local eraser sweep used as persistent pixel mask geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaskSegment {
    /// Segment start in element-local coordinates.
    pub from: Point,
    /// Segment end in element-local coordinates.
    pub to: Point,
    /// Footprint shape used by this segment.
    pub shape: EraserShape,
    /// Circle radius or square half-side in element-local coordinates.
    pub size: f64,
}

impl MaskSegment {
    /// Create a mask segment from local endpoints and eraser footprint style.
    pub fn new(from: Point, to: Point, shape: EraserShape, size: f64) -> Self {
        Self {
            from,
            to,
            shape,
            size: size.max(0.0),
        }
    }

    /// Return the conservative local rectangle covered by this segment.
    pub fn bounds(self) -> Rect {
        let size = Vec2::new(self.size, self.size);
        rect_from_min_max(
            point_min(self.from, self.to) - size,
            point_max(self.from, self.to) + size,
        )
    }

    /// Return filled geometry for this eraser segment.
    ///
    /// Circle segments are represented as a round-cap stroke path. Square
    /// segments are an axis-aligned swept stamp polygon.
    pub fn fill_path(self) -> BezPath {
        match self.shape {
            EraserShape::Circle => circle_segment_path(self.from, self.to, self.size),
            EraserShape::Square => square_segment_path(self.from, self.to, self.size),
        }
    }

    /// Return a stroke style that matches a circle segment.
    pub fn circle_stroke(self) -> Stroke {
        Stroke::new(self.size * 2.0)
            .with_caps(peniko::kurbo::Cap::Round)
            .with_join(peniko::kurbo::Join::Round)
    }
}

pub(crate) fn register_eraser_systems(world: &mut World) {
    MessageRegistry::register_message::<EraserMessage>(world);
    world.init_resource::<ActiveEraser>();
}

pub(crate) fn install_eraser_systems(schedule: &mut Schedule) {
    schedule.add_systems(process_eraser_msg.in_set(QuillUpdateSet::ToolMessages));
}

/// Active eraser gesture while the pointer is pressed.
#[derive(Resource, Clone, Debug, Default)]
pub struct ActiveEraser {
    /// Tool config captured at pointer-down.
    config: Option<EraserConfig>,
    /// Root-space samples collected during the active gesture.
    points: Vec<Point>,
}

impl ActiveEraser {
    /// Return the config for the current eraser gesture.
    pub fn config(&self) -> Option<EraserConfig> {
        self.config
    }

    /// Return the current eraser path in root coordinates.
    pub fn points(&self) -> &[Point] {
        &self.points
    }

    /// Return true when a gesture is active.
    pub fn is_active(&self) -> bool {
        self.config.is_some() && !self.points.is_empty()
    }
}

pub(crate) fn process_eraser_msg(
    mut commands: Commands,
    mut messages: MessageReader<EraserMessage>,
    mode: Res<CanvasMode>,
    root: Res<DocumentRootEntity>,
    mut active: ResMut<ActiveEraser>,
    mut selection: ResMut<SelectionState>,
    children: Query<&Children>,
    hidden: Query<(), With<Hidden>>,
    pages: Query<&Page>,
    transforms: Query<Option<&CanvasTransform>>,
    bounds: Query<&RectBound>,
    mut masks: Query<&mut Mask>,
    strokes: Query<&StrokeData>,
    images: Query<&ImageElement>,
    texts: Query<&TextElement>,
) {
    let Tool::Eraser(config) = mode.tool else {
        for _ in messages.read() {}
        active.reset();
        return;
    };

    for msg in messages.read() {
        match msg.input {
            CanvasInput::PointerEnter(_) => {}
            CanvasInput::PointerLeave => {
                finalize_eraser(
                    &mut commands,
                    root.0,
                    &mut active,
                    &mut selection,
                    &children,
                    &hidden,
                    &pages,
                    &transforms,
                    &bounds,
                    &mut masks,
                    &strokes,
                    &images,
                    &texts,
                );
            }
            CanvasInput::PointerDown(input) => {
                active.reset();
                active.config = Some(config);
                active.points.push(input_to_point(input));
            }
            CanvasInput::PointerMove(input) => {
                if active.is_active() {
                    active.points.push(input_to_point(input));
                }
            }
            CanvasInput::PointerUp(input) => {
                if active.is_active() {
                    active.points.push(input_to_point(input));
                }
                finalize_eraser(
                    &mut commands,
                    root.0,
                    &mut active,
                    &mut selection,
                    &children,
                    &hidden,
                    &pages,
                    &transforms,
                    &bounds,
                    &mut masks,
                    &strokes,
                    &images,
                    &texts,
                );
            }
        }
    }
}

impl ActiveEraser {
    fn reset(&mut self) {
        self.config = None;
        self.points.clear();
    }
}

fn finalize_eraser(
    commands: &mut Commands,
    root: Entity,
    active: &mut ActiveEraser,
    selection: &mut SelectionState,
    children: &Query<&Children>,
    hidden: &Query<(), With<Hidden>>,
    pages: &Query<&Page>,
    transforms: &Query<Option<&CanvasTransform>>,
    bounds: &Query<&RectBound>,
    masks: &mut Query<&mut Mask>,
    strokes: &Query<&StrokeData>,
    images: &Query<&ImageElement>,
    texts: &Query<&TextElement>,
) {
    let Some(config) = active.config else {
        active.reset();
        return;
    };
    let steps = eraser_steps(&active.points, config);
    if steps.is_empty() {
        active.reset();
        return;
    }

    let mut hits = EraserHits::default();
    hit_children(
        root,
        Affine::IDENTITY,
        &steps,
        config,
        children,
        hidden,
        pages,
        transforms,
        bounds,
        strokes,
        images,
        texts,
        &mut hits,
    );

    match config.mode {
        EraserMode::PixelWise => commit_pixel_erases(
            commands,
            selection,
            children,
            masks,
            &hits.owners,
            hits.masks,
            strokes,
            transforms,
        ),
        EraserMode::ItemWise => {
            for entity in hits.items {
                selection.remove_entity(entity);
                commands.entity(entity).despawn();
            }
        }
    }

    active.reset();
}

#[derive(Default)]
struct EraserHits {
    masks: HashMap<Entity, Vec<MaskSegment>>,
    items: HashSet<Entity>,
    owners: HashMap<Entity, Entity>,
}

#[allow(clippy::too_many_arguments)]
fn hit_children(
    owner: Entity,
    root_from_owner: Affine,
    steps: &[EraserStep],
    config: EraserConfig,
    children: &Query<&Children>,
    hidden: &Query<(), With<Hidden>>,
    pages: &Query<&Page>,
    transforms: &Query<Option<&CanvasTransform>>,
    bounds: &Query<&RectBound>,
    strokes: &Query<&StrokeData>,
    images: &Query<&ImageElement>,
    texts: &Query<&TextElement>,
    hits: &mut EraserHits,
) {
    let Ok(owner_children) = children.get(owner) else {
        return;
    };

    for child in owner_children.iter() {
        if hidden.get(child).is_ok() {
            continue;
        }
        let transform = transforms
            .get(child)
            .ok()
            .flatten()
            .copied()
            .unwrap_or_default();
        let root_from_child = root_from_owner * transform.affine;

        if pages.get(child).is_ok() {
            hit_children(
                child,
                root_from_child,
                steps,
                config,
                children,
                hidden,
                pages,
                transforms,
                bounds,
                strokes,
                images,
                texts,
                hits,
            );
            continue;
        }

        hit_drawable(
            owner,
            child,
            root_from_child,
            steps,
            config,
            bounds,
            strokes,
            images,
            texts,
            hits,
        );
    }
}

fn commit_masks(
    commands: &mut Commands,
    masks: &mut Query<&mut Mask>,
    appended: HashMap<Entity, Vec<MaskSegment>>,
) {
    for (entity, mut segments) in appended {
        if let Ok(mut mask) = masks.get_mut(entity) {
            mask.segments.append(&mut segments);
        } else {
            commands.entity(entity).insert(Mask { segments });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn commit_pixel_erases(
    commands: &mut Commands,
    selection: &mut SelectionState,
    children: &Query<&Children>,
    masks: &mut Query<&mut Mask>,
    owners: &HashMap<Entity, Entity>,
    mut appended: HashMap<Entity, Vec<MaskSegment>>,
    strokes: &Query<&StrokeData>,
    transforms: &Query<Option<&CanvasTransform>>,
) {
    // Stroke masks become structural fragments so selection and bounds track
    // what is still visible. Other drawable classes remain mask-only.
    let stroke_entities = appended
        .keys()
        .copied()
        .filter(|entity| strokes.get(*entity).is_ok())
        .collect::<Vec<_>>();

    for entity in stroke_entities {
        let Some(segments) = appended.remove(&entity) else {
            continue;
        };
        let Ok(stroke) = strokes.get(entity) else {
            continue;
        };
        let split = stroke_fragments_after_erase(&stroke.points, &segments);
        if !split.changed {
            appended.insert(entity, segments);
            continue;
        }

        let transform = transforms
            .get(entity)
            .ok()
            .flatten()
            .copied()
            .unwrap_or_default();
        replace_stroke_with_fragments(
            commands,
            selection,
            children,
            owners.get(&entity).copied(),
            entity,
            stroke,
            transform,
            split.fragments,
        );
    }

    commit_masks(commands, masks, appended);
}

fn replace_stroke_with_fragments(
    commands: &mut Commands,
    selection: &mut SelectionState,
    children: &Query<&Children>,
    owner: Option<Entity>,
    entity: Entity,
    stroke: &StrokeData,
    transform: CanvasTransform,
    mut fragments: Vec<Vec<PathPoint>>,
) {
    match fragments.len() {
        0 => {
            selection.remove_entity(entity);
            commands.entity(entity).despawn();
        }
        1 => {
            let data = StrokeData::from_finished_points(stroke.brush, fragments.remove(0));
            let bounds = rect_bound_from_path_points(&data.points);
            let mut entity_commands = commands.entity(entity);
            entity_commands.insert(data).remove::<Mask>();
            if let Some(bounds) = bounds {
                entity_commands.insert(bounds);
            } else {
                entity_commands.remove::<RectBound>();
            }
        }
        _ => {
            let index = owner.and_then(|owner| child_index(children, owner, entity));
            let new_entities = fragments
                .into_iter()
                .filter_map(|points| spawn_stroke_fragment(commands, stroke, transform, points))
                .collect::<Vec<_>>();

            selection.replace_entity(entity, new_entities.iter().copied());
            if let Some((owner, index)) = index {
                commands.entity(owner).insert_children(index, &new_entities);
            } else if let Some(owner) = owner {
                commands.entity(owner).add_children(&new_entities);
            }
            commands.entity(entity).despawn();
        }
    }
}

fn child_index(
    children: &Query<&Children>,
    owner: Entity,
    entity: Entity,
) -> Option<(Entity, usize)> {
    let index = children
        .get(owner)
        .ok()?
        .iter()
        .position(|child| child == entity)?;
    Some((owner, index))
}

fn spawn_stroke_fragment(
    commands: &mut Commands,
    stroke: &StrokeData,
    transform: CanvasTransform,
    points: Vec<PathPoint>,
) -> Option<Entity> {
    let data = StrokeData::from_finished_points(stroke.brush, points);
    let bounds = rect_bound_from_path_points(&data.points)?;
    Some(commands.spawn((data, bounds, transform)).id())
}

#[derive(Clone, Copy, Debug)]
struct Interval {
    start: f64,
    end: f64,
}

impl Interval {
    fn new(start: f64, end: f64) -> Option<Self> {
        let start = start.clamp(0.0, 1.0);
        let end = end.clamp(0.0, 1.0);
        (end - start > 1.0e-6).then_some(Self { start, end })
    }

    fn intersect(self, other: Self) -> Option<Self> {
        Self::new(self.start.max(other.start), self.end.min(other.end))
    }
}

struct StrokeFragments {
    fragments: Vec<Vec<PathPoint>>,
    changed: bool,
}

fn stroke_fragments_after_erase(points: &[PathPoint], segments: &[MaskSegment]) -> StrokeFragments {
    let mut fragments = Vec::new();
    let mut current = Vec::new();
    let mut changed = false;

    for pair in points.windows(2) {
        let [from, to] = pair else {
            continue;
        };
        if !from.is_visible() || !to.is_visible() {
            push_fragment(&mut fragments, &mut current);
            continue;
        }

        // Work in the segment parameter space. Covered intervals are removed,
        // and their complement becomes one or more visible stroke fragments.
        let covered = covered_intervals(point_from_path(*from), point_from_path(*to), segments);
        changed |= !covered.is_empty();
        let visible = visible_intervals(&covered);
        if visible.is_empty() {
            push_fragment(&mut fragments, &mut current);
            continue;
        }

        for interval in visible {
            if interval.start > 1.0e-6 {
                push_fragment(&mut fragments, &mut current);
            }

            let start = from.lerp(*to, interval.start as f32).with_visibility(true);
            let end = from.lerp(*to, interval.end as f32).with_visibility(true);
            push_point_if_distinct(&mut current, start);
            push_point_if_distinct(&mut current, end);

            if interval.end < 1.0 - 1.0e-6 {
                push_fragment(&mut fragments, &mut current);
            }
        }
    }

    push_fragment(&mut fragments, &mut current);
    StrokeFragments { fragments, changed }
}

fn covered_intervals(from: Point, to: Point, segments: &[MaskSegment]) -> Vec<Interval> {
    let mut intervals = segments
        .iter()
        .flat_map(|segment| segment_covered_intervals(from, to, *segment))
        .collect::<Vec<_>>();
    merge_intervals(&mut intervals);
    intervals
}

fn segment_covered_intervals(from: Point, to: Point, segment: MaskSegment) -> Vec<Interval> {
    match segment.shape {
        EraserShape::Circle => circle_covered_intervals(from, to, segment),
        EraserShape::Square => polygon_covered_intervals(
            from,
            to,
            &square_sweep_polygon(segment.from, segment.to, segment.size),
        ),
    }
}

fn circle_covered_intervals(from: Point, to: Point, segment: MaskSegment) -> Vec<Interval> {
    let mut intervals = Vec::new();
    intervals.extend(circle_point_intervals(from, to, segment.from, segment.size));
    intervals.extend(circle_point_intervals(from, to, segment.to, segment.size));

    // A swept circular eraser is a capsule: two endpoint circles plus the
    // finite-width strip between them.
    let eraser_delta = segment.to - segment.from;
    let eraser_len_sq = eraser_delta.hypot2();
    if eraser_len_sq > f64::EPSILON {
        let stroke_delta = to - from;
        let projection_start = dot(from - segment.from, eraser_delta) / eraser_len_sq;
        let projection_delta = dot(stroke_delta, eraser_delta) / eraser_len_sq;
        let distance_scale = eraser_len_sq.sqrt();
        let signed_start = cross(eraser_delta, from - segment.from) / distance_scale;
        let signed_delta = cross(eraser_delta, stroke_delta) / distance_scale;

        if let Some(projection) =
            linear_range_interval(projection_start, projection_delta, 0.0, 1.0)
            && let Some(distance) =
                linear_range_interval(signed_start, signed_delta, -segment.size, segment.size)
            && let Some(interval) = projection.intersect(distance)
        {
            intervals.push(interval);
        }
    }

    merge_intervals(&mut intervals);
    intervals
}

fn circle_point_intervals(from: Point, to: Point, center: Point, radius: f64) -> Vec<Interval> {
    let delta = to - from;
    let a = delta.hypot2();
    if a <= f64::EPSILON {
        return if from.distance_squared(center) <= radius * radius {
            vec![Interval {
                start: 0.0,
                end: 1.0,
            }]
        } else {
            Vec::new()
        };
    }

    let local = from - center;
    let b = 2.0 * dot(local, delta);
    let c = local.hypot2() - radius * radius;
    let discriminant = b * b - 4.0 * a * c;
    if discriminant < 0.0 {
        return Vec::new();
    }

    let root = discriminant.sqrt();
    let t0 = (-b - root) / (2.0 * a);
    let t1 = (-b + root) / (2.0 * a);
    Interval::new(t0, t1).into_iter().collect()
}

fn polygon_covered_intervals(from: Point, to: Point, polygon: &[Point]) -> Vec<Interval> {
    if polygon.len() < 3 {
        return Vec::new();
    }

    let mut cuts = vec![0.0, 1.0];
    for (edge_from, edge_to) in polygon_edges(polygon) {
        if let Some(t) = segment_intersection_t(from, to, edge_from, edge_to) {
            cuts.push(t);
        }
    }
    cuts.sort_by(f64::total_cmp);
    cuts.dedup_by(|a, b| (*a - *b).abs() <= 1.0e-6);

    let mut intervals = Vec::new();
    for pair in cuts.windows(2) {
        let [start, end] = pair else {
            continue;
        };
        if end - start <= 1.0e-6 {
            continue;
        }
        let midpoint = *start + (*end - *start) * 0.5;
        if point_in_polygon(point_lerp(from, to, midpoint), polygon)
            && let Some(interval) = Interval::new(*start, *end)
        {
            intervals.push(interval);
        }
    }

    merge_intervals(&mut intervals);
    intervals
}

fn segment_intersection_t(from: Point, to: Point, edge_from: Point, edge_to: Point) -> Option<f64> {
    let delta = to - from;
    let edge_delta = edge_to - edge_from;
    let denom = cross(delta, edge_delta);
    if denom.abs() <= f64::EPSILON {
        return None;
    }

    let offset = edge_from - from;
    let t = cross(offset, edge_delta) / denom;
    let u = cross(offset, delta) / denom;
    (t >= -1.0e-6 && t <= 1.0 + 1.0e-6 && u >= -1.0e-6 && u <= 1.0 + 1.0e-6)
        .then_some(t.clamp(0.0, 1.0))
}

fn linear_range_interval(start: f64, delta: f64, min: f64, max: f64) -> Option<Interval> {
    if delta.abs() <= f64::EPSILON {
        return (start >= min && start <= max).then_some(Interval {
            start: 0.0,
            end: 1.0,
        });
    }

    let mut t0 = (min - start) / delta;
    let mut t1 = (max - start) / delta;
    if t0 > t1 {
        std::mem::swap(&mut t0, &mut t1);
    }
    Interval::new(t0, t1)
}

fn visible_intervals(covered: &[Interval]) -> Vec<Interval> {
    let mut visible = Vec::new();
    let mut cursor = 0.0;
    for interval in covered {
        if let Some(gap) = Interval::new(cursor, interval.start) {
            visible.push(gap);
        }
        cursor = cursor.max(interval.end);
    }
    if let Some(gap) = Interval::new(cursor, 1.0) {
        visible.push(gap);
    }
    visible
}

fn merge_intervals(intervals: &mut Vec<Interval>) {
    intervals.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.end.total_cmp(&b.end)));
    let mut merged: Vec<Interval> = Vec::with_capacity(intervals.len());
    for interval in intervals.drain(..) {
        if let Some(last) = merged.last_mut()
            && interval.start <= last.end + 1.0e-6
        {
            last.end = last.end.max(interval.end);
            continue;
        }
        merged.push(interval);
    }
    *intervals = merged;
}

fn push_fragment(fragments: &mut Vec<Vec<PathPoint>>, current: &mut Vec<PathPoint>) {
    if current.len() >= 2 {
        fragments.push(std::mem::take(current));
    } else {
        current.clear();
    }
}

fn push_point_if_distinct(points: &mut Vec<PathPoint>, point: PathPoint) {
    if points.last().is_some_and(|last| {
        point_from_path(*last).distance_squared(point_from_path(point)) <= 1.0e-12
    }) {
        return;
    }
    points.push(point);
}

fn point_from_path(point: PathPoint) -> Point {
    Point::new(f64::from(point.x()), f64::from(point.y()))
}

fn point_lerp(from: Point, to: Point, t: f64) -> Point {
    from + (to - from) * t
}

fn dot(a: Vec2, b: Vec2) -> f64 {
    a.x * b.x + a.y * b.y
}

fn hit_drawable(
    owner: Entity,
    entity: Entity,
    root_from_entity: Affine,
    steps: &[EraserStep],
    config: EraserConfig,
    bounds: &Query<&RectBound>,
    strokes: &Query<&StrokeData>,
    images: &Query<&ImageElement>,
    texts: &Query<&TextElement>,
    hits: &mut EraserHits,
) {
    if config.targets.contains(EraserTargets::STROKES)
        && let Ok(stroke) = strokes.get(entity)
        && stroke_hit(root_from_entity, stroke, steps, bounds.get(entity).ok())
    {
        record_hit(owner, entity, root_from_entity, steps, config, hits);
        return;
    }

    if config.targets.contains(EraserTargets::IMAGES)
        && let Ok(image) = images.get(entity)
        && rect_hit(
            root_from_entity,
            image.rect(),
            steps,
            bounds.get(entity).ok(),
        )
    {
        record_hit(owner, entity, root_from_entity, steps, config, hits);
        return;
    }

    if config.targets.contains(EraserTargets::TEXT)
        && let Ok(text) = texts.get(entity)
        && rect_hit(
            root_from_entity,
            text.bounds,
            steps,
            bounds.get(entity).ok(),
        )
    {
        record_hit(owner, entity, root_from_entity, steps, config, hits);
    }
}

fn record_hit(
    owner: Entity,
    entity: Entity,
    root_from_entity: Affine,
    steps: &[EraserStep],
    config: EraserConfig,
    hits: &mut EraserHits,
) {
    hits.owners.entry(entity).or_insert(owner);
    match config.mode {
        EraserMode::ItemWise => {
            hits.items.insert(entity);
        }
        EraserMode::PixelWise => {
            let entity_from_root = root_from_entity.inverse();
            let segments = steps
                .iter()
                .map(|step| step.to_mask_segment(entity_from_root))
                .collect::<Vec<_>>();
            hits.masks.entry(entity).or_default().extend(segments);
        }
    }
}

fn stroke_hit(
    root_from_stroke: Affine,
    stroke: &StrokeData,
    steps: &[EraserStep],
    bound: Option<&RectBound>,
) -> bool {
    let Some(bound) = bound else {
        return false;
    };
    if !steps_hit_bound(steps, root_from_stroke, bound) {
        return false;
    }

    let outline = stroke_outline_path(stroke);
    path_hit(root_from_stroke, &outline, steps)
}

fn rect_hit(
    root_from_entity: Affine,
    rect: Rect,
    steps: &[EraserStep],
    bound: Option<&RectBound>,
) -> bool {
    if let Some(bound) = bound
        && !steps_hit_bound(steps, root_from_entity, bound)
    {
        return false;
    }

    steps.iter().any(|step| {
        let local_step = step.transformed(root_from_entity.inverse());
        local_step.hit_rect(rect)
    })
}

fn steps_hit_bound(steps: &[EraserStep], root_from_entity: Affine, bound: &RectBound) -> bool {
    let root_bound = transformed_bound(root_from_entity, bound);
    steps.iter().any(|step| {
        if !rects_intersect(step.bounds, root_bound.rect) {
            return false;
        }

        let Some(children) = root_bound.children.as_ref() else {
            return true;
        };
        children
            .iter()
            .any(|child| rects_intersect(step.bounds, child.rect))
    })
}

fn transformed_bound(transform: Affine, bound: &RectBound) -> RectBound {
    let canvas_transform = CanvasTransform { affine: transform };
    bound.world(canvas_transform)
}

fn path_hit(root_from_path: Affine, path: &BezPath, steps: &[EraserStep]) -> bool {
    let Some(path_bounds) = path_bounds(path) else {
        return false;
    };
    steps.iter().any(|step| {
        let local_step = step.transformed(root_from_path.inverse());
        if !rects_intersect(local_step.bounds, path_bounds) {
            return false;
        }
        path_segments(path).any(|(from, to)| local_step.hit_segment(from, to))
    })
}

fn path_bounds(path: &BezPath) -> Option<Rect> {
    (!path.elements().is_empty()).then(|| path.bounding_box())
}

fn path_segments(path: &BezPath) -> impl Iterator<Item = (Point, Point)> {
    let mut flat = Vec::new();
    flatten(path.elements().iter().copied(), 0.5, |el| flat.push(el));

    let mut cursor = Point::ZERO;
    let mut start = Point::ZERO;
    flat.into_iter().filter_map(move |el| match el {
        PathEl::MoveTo(point) => {
            cursor = point;
            start = point;
            None
        }
        PathEl::LineTo(point) => {
            let segment = (cursor, point);
            cursor = point;
            Some(segment)
        }
        PathEl::ClosePath => {
            let segment = (cursor, start);
            cursor = start;
            Some(segment)
        }
        PathEl::QuadTo(..) | PathEl::CurveTo(..) => None,
    })
}

fn stroke_outline_path(stroke: &StrokeData) -> BezPath {
    let tip = stroke.brush.brush.tip;
    let options = StrokeOutlineOptions::default();
    let outline = match stroke.status {
        StrokeStatus::Active { last_update } => {
            let mut outline = SimpleStrokeOutline::new(tip, options);
            outline.update(ModelUpdate {
                metadata: ModelUpdateMeta {
                    invalidated_from: 0,
                    stable_len: last_update.stable_len.min(stroke.points.len()),
                },
                updated: &stroke.points,
            });
            outline
        }
        StrokeStatus::Finished => SimpleStrokeOutline::new_with_path(&stroke.points, tip, options),
    };
    BezPath::from_vec(outline.iter().collect())
}

/// One swept eraser step and its cached broad-phase bound.
#[derive(Clone, Debug)]
struct EraserStep {
    config: EraserConfig,
    from: Point,
    to: Point,
    bounds: Rect,
}

impl EraserStep {
    fn new(from: Point, to: Point, config: EraserConfig) -> Self {
        Self {
            config,
            from,
            to,
            bounds: MaskSegment::new(from, to, config.shape, f64::from(config.size)).bounds(),
        }
    }

    fn transformed(&self, transform: Affine) -> Self {
        let from = transform * self.from;
        let to = transform * self.to;
        let size = transformed_size(transform, f64::from(self.config.size));
        let config = EraserConfig {
            size: size as f32,
            ..self.config
        };
        Self::new(from, to, config)
    }

    fn to_mask_segment(&self, transform: Affine) -> MaskSegment {
        let from = transform * self.from;
        let to = transform * self.to;
        let size = transformed_size(transform, f64::from(self.config.size));
        MaskSegment::new(from, to, self.config.shape, size)
    }

    fn hit_rect(&self, rect: Rect) -> bool {
        if !rects_intersect(self.bounds, rect) {
            return false;
        }
        rect_corners(rect)
            .into_iter()
            .any(|point| self.contains(point))
            || self.contains(Point::new(
                self.from.x.clamp(rect.x0, rect.x1),
                self.from.y.clamp(rect.y0, rect.y1),
            ))
            || rect_edges(rect)
                .into_iter()
                .any(|(from, to)| self.hit_segment(from, to))
    }

    fn hit_segment(&self, from: Point, to: Point) -> bool {
        match self.config.shape {
            EraserShape::Circle => {
                let radius = f64::from(self.config.size.max(0.0));
                segment_distance_squared(self.from, self.to, from, to) <= radius * radius
            }
            EraserShape::Square => segment_hits_polygon(
                from,
                to,
                &square_sweep_polygon(self.from, self.to, f64::from(self.config.size.max(0.0))),
            ),
        }
    }

    fn contains(&self, point: Point) -> bool {
        match self.config.shape {
            EraserShape::Circle => self.contains_circle(point),
            EraserShape::Square => point_in_polygon(
                point,
                &square_sweep_polygon(self.from, self.to, f64::from(self.config.size.max(0.0))),
            ),
        }
    }

    fn contains_circle(&self, point: Point) -> bool {
        let radius = f64::from(self.config.size.max(0.0));
        RoundBound::new(self.from, radius).contains_point(point)
            || RoundBound::new(self.from, radius).contains_point_swept_to(point, self.to)
    }
}

fn eraser_steps(points: &[Point], config: EraserConfig) -> Vec<EraserStep> {
    match points {
        [] => Vec::new(),
        [point] => vec![EraserStep::new(*point, *point, config)],
        _ => points
            .windows(2)
            .map(|points| EraserStep::new(points[0], points[1], config))
            .collect(),
    }
}

fn input_to_point(input: Input) -> Point {
    Point::new(f64::from(input.x()), f64::from(input.y()))
}

fn transformed_size(transform: Affine, size: f64) -> f64 {
    let origin = transform * Point::ZERO;
    let x = (transform * Point::new(1.0, 0.0)).distance(origin);
    let y = (transform * Point::new(0.0, 1.0)).distance(origin);
    size * x.max(y)
}

fn circle_segment_path(from: Point, to: Point, size: f64) -> BezPath {
    let mut path = BezPath::new();
    if from.distance(to) <= f64::EPSILON {
        return peniko::kurbo::Circle::new(from, size).to_path(0.1);
    }
    path.move_to(from);
    path.line_to(to);
    path
}

fn square_segment_path(from: Point, to: Point, size: f64) -> BezPath {
    let mut path = BezPath::new();
    let polygon = square_sweep_polygon(from, to, size);
    let Some(first) = polygon.first().copied() else {
        return path;
    };
    path.move_to(first);
    for point in polygon.iter().copied().skip(1) {
        path.line_to(point);
    }
    path.close_path();
    path
}

fn square_sweep_polygon(from: Point, to: Point, size: f64) -> Vec<Point> {
    let offsets = [
        Vec2::new(-size, -size),
        Vec2::new(size, -size),
        Vec2::new(size, size),
        Vec2::new(-size, size),
    ];
    let mut points = offsets
        .iter()
        .flat_map(|offset| [from + *offset, to + *offset])
        .collect::<Vec<_>>();
    convex_hull(&mut points)
}

fn convex_hull(points: &mut [Point]) -> Vec<Point> {
    points.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    let mut lower = Vec::new();
    for point in points.iter().copied() {
        while lower.len() >= 2
            && cross(
                lower[lower.len() - 1] - lower[lower.len() - 2],
                point - lower[lower.len() - 1],
            ) <= 0.0
        {
            lower.pop();
        }
        lower.push(point);
    }

    let mut upper = Vec::new();
    for point in points.iter().copied().rev() {
        while upper.len() >= 2
            && cross(
                upper[upper.len() - 1] - upper[upper.len() - 2],
                point - upper[upper.len() - 1],
            ) <= 0.0
        {
            upper.pop();
        }
        upper.push(point);
    }

    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

fn rect_corners(rect: Rect) -> [Point; 4] {
    [
        Point::new(rect.x0, rect.y0),
        Point::new(rect.x1, rect.y0),
        Point::new(rect.x1, rect.y1),
        Point::new(rect.x0, rect.y1),
    ]
}

fn rect_edges(rect: Rect) -> [(Point, Point); 4] {
    let [a, b, c, d] = rect_corners(rect);
    [(a, b), (b, c), (c, d), (d, a)]
}

fn rects_intersect(a: Rect, b: Rect) -> bool {
    a.x0 <= b.x1 && a.x1 >= b.x0 && a.y0 <= b.y1 && a.y1 >= b.y0
}

fn segment_distance_squared(a0: Point, a1: Point, b0: Point, b1: Point) -> f64 {
    if segments_intersect(a0, a1, b0, b1) {
        return 0.0;
    }
    point_distance_squared_to_segment(a0, b0, b1)
        .min(point_distance_squared_to_segment(a1, b0, b1))
        .min(point_distance_squared_to_segment(b0, a0, a1))
        .min(point_distance_squared_to_segment(b1, a0, a1))
}

fn segments_intersect(a0: Point, a1: Point, b0: Point, b1: Point) -> bool {
    let d1 = cross(a1 - a0, b0 - a0);
    let d2 = cross(a1 - a0, b1 - a0);
    let d3 = cross(b1 - b0, a0 - b0);
    let d4 = cross(b1 - b0, a1 - b0);
    if d1.abs() <= f64::EPSILON && point_on_segment(b0, a0, a1) {
        return true;
    }
    if d2.abs() <= f64::EPSILON && point_on_segment(b1, a0, a1) {
        return true;
    }
    if d3.abs() <= f64::EPSILON && point_on_segment(a0, b0, b1) {
        return true;
    }
    if d4.abs() <= f64::EPSILON && point_on_segment(a1, b0, b1) {
        return true;
    }
    (d1 > 0.0) != (d2 > 0.0) && (d3 > 0.0) != (d4 > 0.0)
}

fn cross(a: Vec2, b: Vec2) -> f64 {
    a.x * b.y - a.y * b.x
}

fn point_on_segment(point: Point, from: Point, to: Point) -> bool {
    cross(to - from, point - from).abs() <= f64::EPSILON
        && point.x >= from.x.min(to.x) - f64::EPSILON
        && point.x <= from.x.max(to.x) + f64::EPSILON
        && point.y >= from.y.min(to.y) - f64::EPSILON
        && point.y <= from.y.max(to.y) + f64::EPSILON
}

fn point_in_polygon(point: Point, polygon: &[Point]) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    let mut inside = false;
    for (from, to) in polygon_edges(polygon) {
        if point_on_segment(point, from, to) {
            return true;
        }
        if (from.y > point.y) != (to.y > point.y) {
            let x = (to.x - from.x) * (point.y - from.y) / (to.y - from.y) + from.x;
            if point.x < x {
                inside = !inside;
            }
        }
    }
    inside
}

fn segment_hits_polygon(from: Point, to: Point, polygon: &[Point]) -> bool {
    point_in_polygon(from, polygon)
        || point_in_polygon(to, polygon)
        || polygon_edges(polygon).any(|(a, b)| segments_intersect(from, to, a, b))
}

fn polygon_edges(polygon: &[Point]) -> impl Iterator<Item = (Point, Point)> + '_ {
    polygon
        .iter()
        .copied()
        .zip(polygon.iter().copied().cycle().skip(1))
        .take(polygon.len())
}

fn point_distance_squared_to_segment(point: Point, from: Point, to: Point) -> f64 {
    let segment = to - from;
    let length_squared = segment.hypot2();
    if length_squared <= f64::EPSILON {
        return point.distance_squared(from);
    }

    let delta = point - from;
    let t = ((delta.x * segment.x + delta.y * segment.y) / length_squared).clamp(0.0, 1.0);
    point.distance_squared(from + segment * t)
}

fn point_min(a: Point, b: Point) -> Point {
    Point::new(a.x.min(b.x), a.y.min(b.y))
}

fn point_max(a: Point, b: Point) -> Point {
    Point::new(a.x.max(b.x), a.y.max(b.y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circle_path_checks_distance_to_center_segments() {
        let step = EraserStep::new(
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            EraserConfig::new(EraserShape::Circle, 1.0),
        );

        assert!(step.contains(Point::new(5.0, 0.5)));
        assert!(!step.contains(Point::new(5.0, 2.0)));
    }

    #[test]
    fn circle_segment_hits_intersecting_lines() {
        let step = EraserStep::new(
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            EraserConfig::new(EraserShape::Circle, 1.0),
        );

        assert!(step.hit_segment(Point::new(5.0, -2.0), Point::new(5.0, 2.0)));
        assert!(!step.hit_segment(Point::new(5.0, 2.0), Point::new(6.0, 2.0)));
    }

    #[test]
    fn square_mask_segment_uses_axis_aligned_sweep() {
        let segment = MaskSegment::new(
            Point::new(2.0, 4.0),
            Point::new(8.0, 6.0),
            EraserShape::Square,
            1.5,
        );

        assert_eq!(segment.bounds(), Rect::new(0.5, 2.5, 9.5, 7.5));
        assert_eq!(segment.fill_path().elements().len(), 7);
        assert!(!point_in_polygon(
            Point::new(0.5, 7.5),
            &square_sweep_polygon(segment.from, segment.to, segment.size),
        ));
    }
}
