//! Point-granularity stroke erasing.
//!
//! The eraser marks covered path points invisible while the pointer is down.
//! It defers stroke splitting until the gesture ends so pointer moves stay
//! cheap even when a long stroke is being erased.

use bevy_ecs::message::MessageRegistry;
use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{IntoScheduleConfigs, Schedule};
use peniko::kurbo::{Point, Rect, Vec2};
use quill_stroke::input::{Input, PathPoint};

use crate::{
    canvas::{CanvasMode, CanvasOrder},
    element::{
        EraserConfig, EraserMessage, EraserShape, Tool,
        bounds::{BoundHit, BoundKey, RectBound, RoundBound, rect_from_min_max},
        stroke::{StrokeData, rect_bound_from_path_points},
    },
    geom::CanvasTransform,
    input::CanvasInput,
    runtime::QuillUpdateSet,
};

pub struct Eraser;

pub(crate) fn register_eraser_systems(world: &mut World) {
    MessageRegistry::register_message::<EraserMessage>(world);
    world.init_resource::<ActiveEraser>();
}

pub(crate) fn install_eraser_systems(schedule: &mut Schedule) {
    schedule.add_systems(process_eraser_msg.in_set(QuillUpdateSet::ToolMessages));
}

/// Active eraser gesture while the pointer is pressed.
#[derive(Resource, Default)]
pub(crate) struct ActiveEraser {
    /// Previous eraser sample, used to build the next swept eraser step.
    previous: Option<Point>,
    /// Strokes with newly hidden points that need final reassembly.
    dirty_strokes: Vec<Entity>,
}

pub(crate) fn process_eraser_msg(
    mut commands: Commands,
    mut messages: MessageReader<EraserMessage>,
    mode: Res<CanvasMode>,
    mut active: ResMut<ActiveEraser>,
    mut canvas_order: ResMut<CanvasOrder>,
    mut strokes: Query<(Entity, &mut StrokeData, &RectBound, &CanvasTransform)>,
) {
    let Tool::Eraser(config) = mode.tool else {
        for _ in messages.read() {}
        finalize_dirty_strokes(&mut commands, &mut canvas_order, &mut active, &mut strokes);
        active.reset();
        return;
    };

    for msg in messages.read() {
        let update = active.apply_input(msg.input, config);
        if let Some(step) = update.step {
            mark_erased_points(&mut strokes, &mut active.dirty_strokes, &step);
        }
        if update.finished {
            finalize_dirty_strokes(&mut commands, &mut canvas_order, &mut active, &mut strokes);
            active.reset();
        }
    }
}

impl ActiveEraser {
    /// Consume one routed eraser input and return the current path update.
    fn apply_input(&mut self, input: CanvasInput, config: EraserConfig) -> EraserUpdate {
        match input {
            CanvasInput::PointerEnter(_) => EraserUpdate::default(),
            CanvasInput::PointerLeave => {
                let finished = self.previous.take().is_some();
                EraserUpdate {
                    step: None,
                    finished,
                }
            }
            CanvasInput::PointerDown(input) => {
                let position = input_to_vec2(input);
                self.previous = Some(position);
                EraserUpdate {
                    step: Some(EraserStep::new(position, position, config)),
                    finished: false,
                }
            }
            CanvasInput::PointerMove(input) => {
                let to = input_to_vec2(input);
                let Some(from) = self.previous.replace(to) else {
                    return EraserUpdate::default();
                };
                EraserUpdate {
                    step: Some(EraserStep::new(from, to, config)),
                    finished: false,
                }
            }
            CanvasInput::PointerUp(input) => {
                let to = input_to_vec2(input);
                let Some(from) = self.previous.take() else {
                    return EraserUpdate::default();
                };
                EraserUpdate {
                    step: Some(EraserStep::new(from, to, config)),
                    finished: true,
                }
            }
        }
    }

    fn reset(&mut self) {
        self.previous = None;
        self.dirty_strokes.clear();
    }
}

#[derive(Default)]
struct EraserUpdate {
    step: Option<EraserStep>,
    finished: bool,
}

fn mark_erased_points(
    strokes: &mut Query<(Entity, &mut StrokeData, &RectBound, &CanvasTransform)>,
    dirty_strokes: &mut Vec<Entity>,
    step: &EraserStep,
) {
    for (entity, mut stroke, rect_bound, transform) in strokes.iter_mut() {
        let world_bound = rect_bound.world(*transform);
        let hits = step.intersecting_hits(&world_bound);
        if hits.is_empty() {
            continue;
        }

        let mut stroke_changed = false;

        for hit in hits {
            let range = match hit.key.clone() {
                Some(BoundKey::Range(range)) => range,
                None => 0..stroke.points.len(),
            };

            for point_index in range {
                let Some(point) = stroke.points.get(point_index).copied() else {
                    continue;
                };
                if !point.is_visible() {
                    continue;
                }

                let position = transform.transform_point(point_to_point(point));
                if step.contains(position) {
                    if let Some(point) = stroke.points.get_mut(point_index) {
                        point.set_visible(false);
                        stroke_changed = true;
                    }
                }
            }
        }

        if stroke_changed && !dirty_strokes.contains(&entity) {
            dirty_strokes.push(entity);
        }
    }
}

fn finalize_dirty_strokes(
    commands: &mut Commands,
    canvas_order: &mut CanvasOrder,
    active: &mut ActiveEraser,
    strokes: &mut Query<(Entity, &mut StrokeData, &RectBound, &CanvasTransform)>,
) {
    let dirty_strokes = std::mem::take(&mut active.dirty_strokes);
    for entity in dirty_strokes {
        if let Ok((entity, stroke, _, transform)) = strokes.get_mut(entity) {
            replace_erased_stroke(commands, canvas_order, entity, &*stroke, transform);
        }
    }
}

fn replace_erased_stroke(
    commands: &mut Commands,
    canvas_order: &mut CanvasOrder,
    entity: Entity,
    stroke: &StrokeData,
    transform: &CanvasTransform,
) {
    let fragments = visible_fragments(&stroke.points);

    match fragments.len() {
        0 => {
            commands.entity(entity).despawn();
            canvas_order.strokes.retain(|stroke| *stroke != entity);
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
            let mut new_entities = Vec::new();
            for points in fragments {
                let data = StrokeData::from_finished_points(stroke.brush, points);
                let new_entity = if let Some(bounds) = rect_bound_from_path_points(&data.points) {
                    commands.spawn((data, bounds, *transform)).id()
                } else {
                    commands.spawn((data, *transform)).id()
                };
                new_entities.push(new_entity);
            }
            replace_ordered_stroke(canvas_order, entity, new_entities);
        }
    }
}

fn replace_ordered_stroke(canvas_order: &mut CanvasOrder, old: Entity, new_entities: Vec<Entity>) {
    if let Some(index) = canvas_order
        .strokes
        .iter()
        .position(|entity| *entity == old)
    {
        canvas_order.strokes.splice(index..=index, new_entities);
    } else {
        canvas_order.strokes.extend(new_entities);
    }
}

fn visible_fragments(points: &[PathPoint]) -> Vec<Vec<PathPoint>> {
    let mut fragments = Vec::new();
    let mut current = Vec::new();

    for point in points.iter().copied() {
        if point.is_visible() {
            current.push(point);
        } else {
            push_fragment(&mut fragments, &mut current);
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

/// One swept eraser step and its cached broad-phase bound.
#[derive(Clone, Debug)]
struct EraserStep {
    config: EraserConfig,
    from: Point,
    to: Point,
    rect_bound: RectBound,
}

impl EraserStep {
    fn new(from: Point, to: Point, config: EraserConfig) -> Self {
        let rect_bound = Self::bound_for_segment(from, to, config);
        Self {
            config,
            from,
            to,
            rect_bound,
        }
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
        let radius = f64::from(self.config.size.max(0.0));

        if let Some(point_hits) = rect_bound.intersects_round(RoundBound::new(self.from, radius)) {
            hits.extend(point_hits);
        }

        if let Some(segment_hits) =
            rect_bound.intersects_round(RoundBound::from_segment(self.from, self.to, radius))
        {
            hits.extend(segment_hits);
        }

        hits
    }

    fn bound_for_segment(from: Point, to: Point, config: EraserConfig) -> RectBound {
        let size = Vec2::new(
            f64::from(config.size.max(0.0)),
            f64::from(config.size.max(0.0)),
        );
        let min = point_min(from, to);
        let max = point_max(from, to);
        RectBound::from_rect(rect_from_min_max(min - size, max + size))
    }

    fn contains(&self, point: Point) -> bool {
        match self.config.shape {
            EraserShape::Circle => self.contains_circle(point),
            EraserShape::Square => self.contains_square(point),
        }
    }

    fn contains_circle(&self, point: Point) -> bool {
        let radius = f64::from(self.config.size.max(0.0));
        RoundBound::new(self.from, radius).contains_point(point)
            || RoundBound::new(self.from, radius).contains_point_swept_to(point, self.to)
    }

    fn contains_square(&self, point: Point) -> bool {
        let size = Vec2::new(
            f64::from(self.config.size.max(0.0)),
            f64::from(self.config.size.max(0.0)),
        );
        let point_bound = rect_from_min_max(point - size, point + size);
        segment_intersects_aabb(self.from, self.to, point_bound)
    }
}

fn point_to_point(point: PathPoint) -> Point {
    Point::new(f64::from(point.x()), f64::from(point.y()))
}

fn input_to_vec2(input: Input) -> Point {
    Point::new(f64::from(input.x()), f64::from(input.y()))
}

fn segment_intersects_aabb(a: Point, b: Point, bounds: Rect) -> bool {
    let delta = b - a;
    let mut t_min = 0.0;
    let mut t_max = 1.0;

    slab_intersect(a.x, delta.x, bounds.x0, bounds.x1, &mut t_min, &mut t_max)
        && slab_intersect(a.y, delta.y, bounds.y0, bounds.y1, &mut t_min, &mut t_max)
}

fn slab_intersect(
    start: f64,
    delta: f64,
    min: f64,
    max: f64,
    t_min: &mut f64,
    t_max: &mut f64,
) -> bool {
    if delta.abs() <= f64::EPSILON {
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

fn point_min(a: Point, b: Point) -> Point {
    Point::new(a.x.min(b.x), a.y.min(b.y))
}

fn point_max(a: Point, b: Point) -> Point {
    Point::new(a.x.max(b.x), a.y.max(b.y))
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
        let mut points = vec![
            point(0.0, 0.0),
            point(1.0, 0.0),
            point(2.0, 0.0),
            point(3.0, 0.0),
            point(4.0, 0.0),
        ];
        points[2].set_visible(false);
        let fragments = visible_fragments(&points);

        assert_eq!(fragments.len(), 2);
        assert_eq!(fragments[0].len(), 2);
        assert_eq!(fragments[1].len(), 2);
    }

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
    fn rect_bound_builds_whole_and_child_bounds() {
        let points = vec![point(0.0, 0.0), point(10.0, 4.0)];
        let bounds = rect_bound_from_path_points(&points).unwrap();

        assert!((bounds.rect.x1 - bounds.rect.x0) >= 10.0);
        assert_eq!(bounds.children.as_ref().unwrap().len(), 1);
    }
}
