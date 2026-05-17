//! Point-granularity stroke erasing.
//!
//! The eraser is a moving sweep, not scene content. It removes covered path
//! points from existing strokes, then splits remaining contiguous point runs
//! into independent stroke entities.

use std::ops::Range;

use bevy::{
    app::{Plugin, Update},
    math::{
        Vec2, Vec3,
        bounding::{Aabb2d, IntersectsVolume},
    },
    transform::components::Transform,
};
use bevy_ecs::message::MessageRegistry;
use bevy_ecs::prelude::*;
use quill_stroke::input::{Input, PathPoint};

use crate::{
    canvas::CanvasMode,
    element::{EraserConfig, EraserMessage, EraserShape, Tool, stroke::StrokeData},
    input::CanvasInput,
    plugin::QuillUpdateSet,
};

pub struct Eraser;

const POINT_BLOCK_SIZE: usize = 64;

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

/// Last active eraser point while the pointer is pressed.
#[derive(Resource, Default)]
pub(crate) struct ActiveEraser {
    /// Previous pressed pointer position used to build the next sweep segment.
    position: Option<Vec2>,
}

/// Cached broad-phase bounds for a stroke.
#[derive(Component, Clone, Debug, Default)]
pub struct StrokeBounds {
    pub bounds: Option<Aabb2d>,
    pub blocks: Vec<PointBlockBounds>,
}

/// Bounds for a consecutive block of path points.
#[derive(Clone, Debug)]
pub struct PointBlockBounds {
    pub range: Range<usize>,
    pub bounds: Aabb2d,
}

impl StrokeBounds {
    pub fn from_points(points: &[PathPoint]) -> Self {
        let bounds = points_bounds(points, 0..points.len());
        let blocks = points
            .chunks(POINT_BLOCK_SIZE)
            .enumerate()
            .filter_map(|(block_index, chunk)| {
                let start = block_index * POINT_BLOCK_SIZE;
                let end = start + chunk.len();
                points_bounds(points, start..end).map(|bounds| PointBlockBounds {
                    range: start..end,
                    bounds,
                })
            })
            .collect();

        Self { bounds, blocks }
    }
}

pub(crate) fn process_eraser_msg(
    mut commands: Commands,
    mut messages: MessageReader<EraserMessage>,
    mode: Res<CanvasMode>,
    mut active: ResMut<ActiveEraser>,
    strokes: Query<(Entity, &StrokeData, &StrokeBounds, &Transform)>,
) {
    let Tool::Eraser(config) = mode.tool else {
        active.position = None;
        for _ in messages.read() {}
        return;
    };

    let mut sweeps = Vec::new();
    for msg in messages.read() {
        if let Some(sweep) = active.apply_input(msg.input, config) {
            sweeps.push(sweep);
        }
    }

    erase_with_sweeps(&mut commands, &strokes, &sweeps);
}

impl ActiveEraser {
    /// Consume one routed eraser input and return the sweep it creates, if any.
    fn apply_input(&mut self, input: CanvasInput, config: EraserConfig) -> Option<EraserSweep> {
        match input {
            CanvasInput::PointerEnter(_) => None,
            CanvasInput::PointerLeave => {
                self.position = None;
                None
            }
            CanvasInput::PointerDown(input) => {
                let position = input_to_vec2(input);
                self.position = Some(position);
                Some(EraserSweep::new(position, position, config))
            }
            CanvasInput::PointerMove(input) => {
                let to = input_to_vec2(input);
                let from = self.position?;
                self.position = Some(to);
                Some(EraserSweep::new(from, to, config))
            }
            CanvasInput::PointerUp(input) => {
                let to = input_to_vec2(input);
                let from = self.position.take()?;
                Some(EraserSweep::new(from, to, config))
            }
        }
    }
}

fn erase_with_sweeps(
    commands: &mut Commands,
    strokes: &Query<(Entity, &StrokeData, &StrokeBounds, &Transform)>,
    sweeps: &[EraserSweep],
) {
    if sweeps.is_empty() {
        return;
    }

    for (entity, stroke, stroke_bounds, transform) in strokes {
        let Some(bounds) = stroke_bounds.bounds else {
            continue;
        };
        let bounds = transform_aabb(transform, bounds);
        if !sweeps
            .iter()
            .any(|sweep| bounds.intersects(&sweep.bounds()))
        {
            continue;
        }

        let mut erased = vec![false; stroke.points.len()];
        let mut has_erased = false;

        for block in &stroke_bounds.blocks {
            let block_bounds = transform_aabb(transform, block.bounds);
            let candidates: Vec<_> = sweeps
                .iter()
                .filter(|sweep| block_bounds.intersects(&sweep.bounds()))
                .collect();
            if candidates.is_empty() {
                continue;
            }

            for point_index in block.range.clone() {
                let point = transform_point(transform, point_to_vec2(stroke.points[point_index]));
                if candidates.iter().any(|sweep| sweep.contains(point)) {
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
            let bounds = StrokeBounds::from_points(&data.points);
            commands.entity(entity).insert((data, bounds));
        }
        _ => {
            commands.entity(entity).despawn();
            for points in fragments {
                let data = StrokeData::from_finished_points(stroke.brush, points);
                let bounds = StrokeBounds::from_points(&data.points);
                commands.spawn((data, bounds, *transform));
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

#[derive(Clone, Copy, Debug)]
struct EraserSweep {
    from: Vec2,
    to: Vec2,
    config: EraserConfig,
}

impl EraserSweep {
    fn new(from: Vec2, to: Vec2, config: EraserConfig) -> Self {
        Self { from, to, config }
    }

    fn bounds(self) -> Aabb2d {
        let size = self.config.size.max(0.0);
        let min = self.from.min(self.to) - Vec2::splat(size);
        let max = self.from.max(self.to) + Vec2::splat(size);
        aabb_from_min_max(min, max)
    }

    fn contains(self, point: Vec2) -> bool {
        match self.config.shape {
            EraserShape::Circle => {
                distance_squared_to_segment(point, self.from, self.to)
                    <= self.config.size.max(0.0).powi(2)
            }
            EraserShape::Square => segment_intersects_aabb(
                self.from,
                self.to,
                aabb_from_min_max(
                    point - Vec2::splat(self.config.size.max(0.0)),
                    point + Vec2::splat(self.config.size.max(0.0)),
                ),
            ),
        }
    }
}

fn points_bounds(points: &[PathPoint], range: Range<usize>) -> Option<Aabb2d> {
    let mut iter = points.get(range)?.iter().map(|point| point_to_vec2(*point));
    let first = iter.next()?;
    let (min, max) = iter.fold((first, first), |(min, max), point| {
        (min.min(point), max.max(point))
    });
    Some(aabb_from_min_max(min, max))
}

fn aabb_from_min_max(min: Vec2, max: Vec2) -> Aabb2d {
    Aabb2d::new((min + max) * 0.5, (max - min) * 0.5)
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

fn transform_aabb(transform: &Transform, bounds: Aabb2d) -> Aabb2d {
    let corners = [
        Vec2::new(bounds.min.x, bounds.min.y),
        Vec2::new(bounds.min.x, bounds.max.y),
        Vec2::new(bounds.max.x, bounds.min.y),
        Vec2::new(bounds.max.x, bounds.max.y),
    ];
    let first = transform_point(transform, corners[0]);
    let (min, max) = corners[1..]
        .iter()
        .copied()
        .map(|point| transform_point(transform, point))
        .fold((first, first), |(min, max), point| {
            (min.min(point), max.max(point))
        });
    aabb_from_min_max(min, max)
}

fn distance_squared_to_segment(point: Vec2, a: Vec2, b: Vec2) -> f32 {
    let segment = b - a;
    let length_squared = segment.length_squared();
    if length_squared <= f32::EPSILON {
        return point.distance_squared(a);
    }

    let t = ((point - a).dot(segment) / length_squared).clamp(0.0, 1.0);
    point.distance_squared(a + segment * t)
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
    fn circle_sweep_checks_distance_to_center_segment() {
        let sweep = EraserSweep::new(
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            EraserConfig::new(EraserShape::Circle, 1.0),
        );

        assert!(sweep.contains(Vec2::new(5.0, 0.5)));
        assert!(!sweep.contains(Vec2::new(5.0, 2.0)));
    }

    #[test]
    fn stroke_bounds_builds_whole_and_block_bounds() {
        let points = vec![point(0.0, 0.0), point(10.0, 4.0)];
        let bounds = StrokeBounds::from_points(&points);

        assert!(bounds.bounds.is_some());
        assert_eq!(bounds.blocks.len(), 1);
    }
}
