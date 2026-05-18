//! Point-granularity stroke erasing.
//!
//! The eraser records the active pointer path as transient tool state. It
//! removes covered path points from existing strokes, then splits remaining
//! contiguous point runs into independent stroke entities.

use bevy::{
    app::{Plugin, Update},
    math::{Vec2, Vec3, bounding::Aabb2d},
    transform::components::Transform,
};
use bevy_ecs::message::MessageRegistry;
use bevy_ecs::prelude::*;
use quill_stroke::input::{Input, PathPoint};

use crate::{
    canvas::CanvasMode,
    element::{
        EraserConfig, EraserMessage, EraserShape, Tool,
        bounds::{BoundHit, BoundKey, RectBound, RoundBound, aabb_from_min_max},
        stroke::{StrokeData, rect_bound_from_path_points},
    },
    input::CanvasInput,
    plugin::QuillUpdateSet,
};

pub struct Eraser;

pub(crate) struct EraserElementPlugin;

impl Plugin for EraserElementPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        let world = app.world_mut();
        MessageRegistry::register_message::<EraserMessage>(world);
        world.init_resource::<ActiveEraser>();

        app.add_systems(
            Update,
            process_eraser_msg.in_set(QuillUpdateSet::ToolMessages),
        );
    }
}

/// Active eraser gesture while the pointer is pressed.
#[derive(Resource, Default)]
pub(crate) struct ActiveEraser {
    /// Transient path used to erase strokes during the current pointer gesture.
    path: Option<EraserPath>,
}

pub(crate) fn process_eraser_msg(
    mut commands: Commands,
    mut messages: MessageReader<EraserMessage>,
    mode: Res<CanvasMode>,
    mut active: ResMut<ActiveEraser>,
    strokes: Query<(Entity, &StrokeData, &RectBound, &Transform)>,
) {
    let Tool::Eraser(config) = mode.tool else {
        active.path = None;
        for _ in messages.read() {}
        return;
    };

    let mut pending_path = None;
    for msg in messages.read() {
        if let Some(path) = active.apply_input(msg.input, config) {
            pending_path = Some(path);
        }
    }

    if let Some(path) = pending_path {
        erase_with_path(&mut commands, &strokes, &path);
    }
}

impl ActiveEraser {
    /// Consume one routed eraser input and return the current path update.
    fn apply_input(&mut self, input: CanvasInput, config: EraserConfig) -> Option<EraserPath> {
        match input {
            CanvasInput::PointerEnter(_) => None,
            CanvasInput::PointerLeave => {
                self.path = None;
                None
            }
            CanvasInput::PointerDown(input) => {
                let position = input_to_vec2(input);
                let path = EraserPath::new(position, config);
                self.path = Some(path.clone());
                Some(path)
            }
            CanvasInput::PointerMove(input) => {
                let path = self.path.as_mut()?;
                path.push(input_to_vec2(input));
                Some(path.clone())
            }
            CanvasInput::PointerUp(input) => {
                let mut path = self.path.take()?;
                path.push(input_to_vec2(input));
                Some(path)
            }
        }
    }
}

fn erase_with_path(
    commands: &mut Commands,
    strokes: &Query<(Entity, &StrokeData, &RectBound, &Transform)>,
    path: &EraserPath,
) {
    for (entity, stroke, rect_bound, transform) in strokes {
        let world_bound = rect_bound.world(transform);
        let hits = path.intersecting_hits(&world_bound);
        if hits.is_empty() {
            continue;
        }

        let mut erased = vec![false; stroke.points.len()];
        let mut has_erased = false;

        for hit in hits {
            let range = match hit.key.clone() {
                Some(BoundKey::Range(range)) => range,
                None => 0..stroke.points.len(),
            };

            for point_index in range {
                let point = transform_point(transform, point_to_vec2(stroke.points[point_index]));
                if path.contains(point) {
                    erased[point_index] = true;
                    has_erased = true;
                }
            }
        }

        if has_erased {
            replace_erased_stroke(commands, entity, stroke, transform, &erased);
        }
    }
}

fn replace_erased_stroke(
    commands: &mut Commands,
    entity: Entity,
    stroke: &StrokeData,
    transform: &Transform,
    erased: &[bool],
) {
    let fragments = surviving_fragments(&stroke.points, erased);

    match fragments.len() {
        0 => {
            commands.entity(entity).despawn();
        }
        1 => {
            let data = StrokeData::from_finished_points(stroke.brush, fragments[0].clone());
            let bounds = rect_bound_from_path_points(&data.points);
            if let Some(bounds) = bounds {
                commands.entity(entity).insert((data, bounds));
            } else {
                commands.entity(entity).insert(data).remove::<RectBound>();
            }
        }
        _ => {
            commands.entity(entity).despawn();
            for points in fragments {
                let data = StrokeData::from_finished_points(stroke.brush, points);
                if let Some(bounds) = rect_bound_from_path_points(&data.points) {
                    commands.spawn((data, bounds, *transform));
                } else {
                    commands.spawn((data, *transform));
                }
            }
        }
    }
}

fn surviving_fragments(points: &[PathPoint], erased: &[bool]) -> Vec<Vec<PathPoint>> {
    let mut fragments = Vec::new();
    let mut current = Vec::new();

    for (point, erased) in points.iter().copied().zip(erased.iter().copied()) {
        if erased {
            push_fragment(&mut fragments, &mut current);
        } else {
            current.push(point);
        }
    }

    push_fragment(&mut fragments, &mut current);
    fragments
}

fn push_fragment(fragments: &mut Vec<Vec<PathPoint>>, current: &mut Vec<PathPoint>) {
    if current.len() >= 2 {
        fragments.push(std::mem::take(current));
    } else {
        current.clear();
    }
}

/// Transient eraser path and its cached broad-phase bound.
#[derive(Clone, Debug)]
struct EraserPath {
    config: EraserConfig,
    points: Vec<Vec2>,
    rect_bound: RectBound,
}

impl EraserPath {
    fn new(position: Vec2, config: EraserConfig) -> Self {
        let points = vec![position];
        let rect_bound = Self::bound_for_points(&points, config);
        Self {
            config,
            points,
            rect_bound,
        }
    }

    fn push(&mut self, position: Vec2) {
        self.points.push(position);
        self.rect_bound = Self::bound_for_points(&self.points, self.config);
    }

    fn rect_bound(&self) -> &RectBound {
        &self.rect_bound
    }

    fn intersecting_hits(&self, rect_bound: &RectBound) -> Vec<BoundHit> {
        match self.config.shape {
            EraserShape::Circle => self.intersecting_round_hits(rect_bound),
            EraserShape::Square => rect_bound
                .intersects_rect(self.rect_bound().rect)
                .map(Vec::from_iter)
                .unwrap_or_default(),
        }
    }

    fn intersecting_round_hits(&self, rect_bound: &RectBound) -> Vec<BoundHit> {
        let mut hits = Vec::new();
        let radius = self.config.size.max(0.0);

        for point in self.points.iter().copied() {
            if let Some(point_hits) = rect_bound.intersects_round(RoundBound::new(point, radius)) {
                hits.extend(point_hits);
            }
        }

        for segment in self.points.windows(2) {
            if let Some(segment_hits) = rect_bound
                .intersects_round(RoundBound::from_segment(segment[0], segment[1], radius))
            {
                hits.extend(segment_hits);
            }
        }

        hits
    }

    fn bound_for_points(points: &[Vec2], config: EraserConfig) -> RectBound {
        let size = Vec2::splat(config.size.max(0.0));
        let first = points[0];
        let (min, max) = points
            .iter()
            .copied()
            .fold((first, first), |(min, max), point| {
                (min.min(point), max.max(point))
            });
        RectBound::from_rect(aabb_from_min_max(min - size, max + size))
    }

    fn contains(&self, point: Vec2) -> bool {
        match self.config.shape {
            EraserShape::Circle => self.contains_circle(point),
            EraserShape::Square => self.contains_square(point),
        }
    }

    fn contains_circle(&self, point: Vec2) -> bool {
        let radius = self.config.size.max(0.0);
        self.points
            .iter()
            .copied()
            .any(|center| RoundBound::new(center, radius).contains_point(point))
            || self.points.windows(2).any(|segment| {
                RoundBound::new(segment[0], radius).contains_point_swept_to(point, segment[1])
            })
    }

    fn contains_square(&self, point: Vec2) -> bool {
        let size = Vec2::splat(self.config.size.max(0.0));
        let point_bound = aabb_from_min_max(point - size, point + size);
        self.any_segment(|from, to| segment_intersects_aabb(from, to, point_bound))
    }

    fn any_segment(&self, mut contains: impl FnMut(Vec2, Vec2) -> bool) -> bool {
        if self.points.len() == 1 {
            let point = self.points[0];
            return contains(point, point);
        }

        self.points
            .windows(2)
            .any(|segment| contains(segment[0], segment[1]))
    }
}

fn point_to_vec2(point: PathPoint) -> Vec2 {
    Vec2::new(point.x(), point.y())
}

fn input_to_vec2(input: Input) -> Vec2 {
    Vec2::new(input.x(), input.y())
}

fn transform_point(transform: &Transform, point: Vec2) -> Vec2 {
    transform
        .transform_point(Vec3::new(point.x, point.y, 0.0))
        .truncate()
}

fn segment_intersects_aabb(a: Vec2, b: Vec2, bounds: Aabb2d) -> bool {
    let delta = b - a;
    let mut t_min = 0.0;
    let mut t_max = 1.0;

    slab_intersect(
        a.x,
        delta.x,
        bounds.min.x,
        bounds.max.x,
        &mut t_min,
        &mut t_max,
    ) && slab_intersect(
        a.y,
        delta.y,
        bounds.min.y,
        bounds.max.y,
        &mut t_min,
        &mut t_max,
    )
}

fn slab_intersect(
    start: f32,
    delta: f32,
    min: f32,
    max: f32,
    t_min: &mut f32,
    t_max: &mut f32,
) -> bool {
    if delta.abs() <= f32::EPSILON {
        return start >= min && start <= max;
    }

    let inv_delta = 1.0 / delta;
    let mut near = (min - start) * inv_delta;
    let mut far = (max - start) * inv_delta;
    if near > far {
        std::mem::swap(&mut near, &mut far);
    }

    *t_min = (*t_min).max(near);
    *t_max = (*t_max).min(far);
    *t_min <= *t_max
}

#[cfg(test)]
mod tests {
    use quill_stroke::input::{Input, InputModeler, WillInputModeler};

    use super::*;

    fn point(x: f32, y: f32) -> PathPoint {
        let input = Input::new(x, y, 0.0).with_pressure(1.0);
        let mut modeler = WillInputModeler::new();
        modeler.enqueue(&[input], &[]).unwrap().updated[0]
    }

    #[test]
    fn split_erased_middle_points_into_fragments() {
        let points = vec![
            point(0.0, 0.0),
            point(1.0, 0.0),
            point(2.0, 0.0),
            point(3.0, 0.0),
            point(4.0, 0.0),
        ];
        let fragments = surviving_fragments(&points, &[false, false, true, false, false]);

        assert_eq!(fragments.len(), 2);
        assert_eq!(fragments[0].len(), 2);
        assert_eq!(fragments[1].len(), 2);
    }

    #[test]
    fn circle_path_checks_distance_to_center_segments() {
        let mut path = EraserPath::new(
            Vec2::new(0.0, 0.0),
            EraserConfig::new(EraserShape::Circle, 1.0),
        );
        path.push(Vec2::new(10.0, 0.0));

        assert!(path.contains(Vec2::new(5.0, 0.5)));
        assert!(!path.contains(Vec2::new(5.0, 2.0)));
    }

    #[test]
    fn rect_bound_builds_whole_and_child_bounds() {
        let points = vec![point(0.0, 0.0), point(10.0, 4.0)];
        let bounds = rect_bound_from_path_points(&points).unwrap();

        assert!((bounds.rect.max.x - bounds.rect.min.x) >= 10.0);
        assert_eq!(bounds.children.as_ref().unwrap().len(), 1);
    }
}
