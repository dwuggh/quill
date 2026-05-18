//! Stroke scene components and input systems.
//!
//! The input modeler is global while a stroke is active. Each completed or
//! active stroke entity stores the brush snapshot and the latest modeled path.

use bevy::{
    app::{Plugin, Update},
    math::Vec2,
    transform::components::Transform,
};
use bevy_ecs::message::MessageRegistry;
use bevy_ecs::prelude::*;
use quill_stroke::{
    brush::Brush,
    input::{Input, ModelUpdate, ModelUpdateMeta, PathPoint},
};

use crate::{
    canvas::{CanvasErrors, CanvasMode, CanvasOrder},
    element::{
        StrokeMessage, Tool,
        bounds::{DEFAULT_BOUND_BLOCK_SIZE, RectBound, aabb_from_min_max},
    },
    error::CanvasError,
    input::{CanvasInput, GlobalInputModeler},
    plugin::QuillUpdateSet,
};

/// Brush metadata attached to a stroke entity.
#[derive(Clone, Copy, Debug)]
pub struct StrokeBrush {
    /// Brush snapshot used for this stroke.
    pub brush: Brush,
}

#[derive(Clone, Debug, Copy)]
pub enum StrokeStatus {
    Active {
        /// Incremental invalidation range from the last modeler update.
        /// This marks only the GPU-side buffer range. At the time this field is written,
        /// the CPU side StrokeData::points would be already fed with updated data points.
        last_update: ModelUpdateMeta,
    },
    Finished,
}

/// Modeled path attached to a stroke entity.
#[derive(Component, Clone, Debug)]
pub struct StrokeData {
    /// Current modeled samples emitted by the input modeler.
    pub points: Vec<PathPoint>,
    pub status: StrokeStatus,
    pub brush: StrokeBrush,
}

impl StrokeData {
    pub(crate) fn new(brush: StrokeBrush, update: ModelUpdate<'_>) -> Self {
        Self {
            points: update.updated.to_vec(),
            status: StrokeStatus::Active {
                last_update: update.metadata,
            },
            brush,
        }
    }

    pub(crate) fn from_finished_points(brush: StrokeBrush, points: Vec<PathPoint>) -> Self {
        Self {
            points,
            status: StrokeStatus::Finished,
            brush,
        }
    }

    pub(crate) fn apply_update(&mut self, update: ModelUpdate<'_>) {
        let from = update.metadata.invalidated_from.min(self.points.len());
        self.points.truncate(from);
        self.points.extend_from_slice(update.updated);
        self.status = StrokeStatus::Active {
            last_update: update.metadata,
        };
    }

    pub(crate) fn finish(&mut self) {
        self.status = StrokeStatus::Finished;
    }
}

/// Current stroke entity being edited by pointer input.
#[derive(Resource, Default)]
pub struct ActiveStroke {
    /// Entity currently receiving modeled path updates.
    pub entity: Option<Entity>,
}

pub(crate) struct StrokeElementPlugin;

impl Plugin for StrokeElementPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        let world = app.world_mut();
        MessageRegistry::register_message::<StrokeMessage>(world);
        world.init_resource::<ActiveStroke>();
        world.insert_resource(GlobalInputModeler::default());

        app.add_systems(
            Update,
            process_stroke_msg.in_set(QuillUpdateSet::ToolMessages),
        );
    }
}

pub(crate) fn process_stroke_msg(
    mut commands: Commands,
    mut messages: MessageReader<StrokeMessage>,
    mode: Res<CanvasMode>,
    mut active: ResMut<ActiveStroke>,
    mut canvas_order: ResMut<CanvasOrder>,
    mut modeler: ResMut<GlobalInputModeler>,
    mut paths: Query<(&mut StrokeData, Option<&mut RectBound>)>,
    mut errors: ResMut<CanvasErrors>,
) {
    let Tool::Stroke(brush) = mode.tool else {
        for _ in messages.read() {}
        return;
    };

    for msg in messages.read() {
        match msg.input {
            CanvasInput::PointerEnter(_) | CanvasInput::PointerLeave => {}
            CanvasInput::PointerDown(input) => {
                active.entity = None;

                // A new stroke owns a fresh modeler state and snapshots the
                // current brush, so later tool changes do not rewrite it.
                modeler.modeler.reset();
                match modeler.modeler.enqueue(&[input], &[]) {
                    Ok(update) => {
                        let data = StrokeData::new(StrokeBrush { brush }, update);
                        let bounds = active_rect_bound_from_path_points(&data.points);
                        let entity = match bounds {
                            Some(bounds) => {
                                commands.spawn((data, bounds, Transform::IDENTITY)).id()
                            }
                            None => commands.spawn((data, Transform::IDENTITY)).id(),
                        };
                        active.entity = Some(entity);
                        canvas_order.strokes.push(entity);
                    }
                    Err(error) => errors.0.push(CanvasError::from(error)),
                }
            }
            CanvasInput::PointerMove(input) => {
                if let Some(entity) = active.entity {
                    update_active_path(
                        &mut commands,
                        entity,
                        input,
                        &mut modeler,
                        &mut paths,
                        &mut errors,
                    );
                }
            }
            CanvasInput::PointerUp(input) => {
                if let Some(entity) = active.entity.take() {
                    update_active_path(
                        &mut commands,
                        entity,
                        input,
                        &mut modeler,
                        &mut paths,
                        &mut errors,
                    );
                    if let Ok((mut path, bounds)) = paths.get_mut(entity) {
                        path.finish();
                        update_finished_bound(&mut commands, entity, &path.points, bounds);
                    }
                    modeler.modeler.reset();
                }
            }
        }
    }
}

fn update_active_path(
    commands: &mut Commands,
    entity: Entity,
    input: Input,
    modeler: &mut GlobalInputModeler,
    paths: &mut Query<(&mut StrokeData, Option<&mut RectBound>)>,
    errors: &mut CanvasErrors,
) {
    match modeler.modeler.enqueue(&[input], &[]) {
        Ok(update) => match paths.get_mut(entity) {
            Ok((mut path, bounds)) => {
                let updated_from = update.metadata.invalidated_from.min(path.points.len());
                path.apply_update(update);
                let updated_points = &path.points[updated_from..];
                if let Some(mut bounds) = bounds {
                    expand_active_rect_bound(&mut bounds, updated_points);
                } else if let Some(updated) = active_rect_bound_from_path_points(updated_points) {
                    commands.entity(entity).insert(updated);
                }
            }
            Err(error) => errors.0.push(CanvasError::MissingActiveStroke {
                entity,
                reason: error.to_string(),
            }),
        },
        Err(error) => errors.0.push(CanvasError::from(error)),
    }
}

fn update_finished_bound(
    commands: &mut Commands,
    entity: Entity,
    points: &[PathPoint],
    bounds: Option<Mut<'_, RectBound>>,
) {
    if let Some(updated) = rect_bound_from_path_points(points) {
        if let Some(mut bounds) = bounds {
            *bounds = updated;
        } else {
            commands.entity(entity).insert(updated);
        }
    } else {
        commands.entity(entity).remove::<RectBound>();
    }
}

pub(crate) fn rect_bound_from_path_points(points: &[PathPoint]) -> Option<RectBound> {
    let points = points
        .iter()
        .copied()
        .filter(|point| point.is_visible())
        .map(|point| bevy::math::Vec2::new(point.x(), point.y()))
        .collect::<Vec<_>>();
    RectBound::from_points_with_children(&points, DEFAULT_BOUND_BLOCK_SIZE)
}

fn active_rect_bound_from_path_points(points: &[PathPoint]) -> Option<RectBound> {
    RectBound::from_points(points.iter().copied().filter_map(visible_point_to_vec2))
}

fn expand_active_rect_bound(bound: &mut RectBound, points: &[PathPoint]) {
    bound.children = None;
    let Some(updated) = active_rect_bound_from_path_points(points) else {
        return;
    };

    let min = bound.rect.min.min(updated.rect.min);
    let max = bound.rect.max.max(updated.rect.max);
    bound.rect = aabb_from_min_max(min, max);
}

fn visible_point_to_vec2(point: PathPoint) -> Option<Vec2> {
    point.is_visible().then(|| Vec2::new(point.x(), point.y()))
}
