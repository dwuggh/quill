//! Stroke scene components and input systems.
//!
//! The input modeler is global while a stroke is active. Each completed or
//! active stroke entity stores the brush snapshot and the latest modeled path.

use bevy::math::Mat3;
use bevy_ecs::prelude::*;
use quill_stroke::{
    brush::Brush,
    input::{Input, InputModeler, ModelUpdate, ModelUpdateMeta, PathPoint, WillInputModeler},
};

use crate::{
    canvas::CanvasErrors,
    error::CanvasError,
    input::{CanvasInput, CanvasMode, PointerState, Tool},
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
    fn new(brush: StrokeBrush, update: ModelUpdate<'_>) -> Self {
        Self {
            points: update.updated.to_vec(),
            status: StrokeStatus::Active {
                last_update: update.metadata,
            },
            brush,
        }
    }

    fn apply_update(&mut self, update: ModelUpdate<'_>) {
        let from = update.metadata.invalidated_from.min(self.points.len());
        self.points.truncate(from);
        self.points.extend_from_slice(update.updated);
        self.status = StrokeStatus::Active {
            last_update: update.metadata,
        };
    }

    fn finish(&mut self) {
        self.status = StrokeStatus::Finished;
    }
}

/// Current stroke entity being edited by pointer input.
#[derive(Resource, Default)]
pub struct ActiveStroke {
    /// Entity currently receiving modeled path updates.
    pub entity: Option<Entity>,
}

/// Global stroke input modeler.
///
/// The modeler is stateful across pointer samples while a stroke is active.
/// It is reset when a stroke begins and when it finishes.
#[derive(Resource)]
pub struct GlobalInputModeler {
    /// Stateful input-to-path modeler used while a stroke is active.
    pub modeler: Box<dyn InputModeler + Send + Sync + 'static>,
}

impl Default for GlobalInputModeler {
    fn default() -> Self {
        Self {
            modeler: Box::new(WillInputModeler::new()),
        }
    }
}

pub(crate) fn update_pointer_state(
    mut inputs: MessageReader<CanvasInput>,
    mut pointer: ResMut<PointerState>,
) {
    for input in inputs.read() {
        match *input {
            CanvasInput::PointerEnter(position) => {
                pointer.inside = true;
                pointer.position = Some(position);
            }
            CanvasInput::PointerLeave => {
                pointer.inside = false;
            }
            CanvasInput::PointerMove(input) => {
                pointer.position = Some(input.position());
            }
            CanvasInput::PointerDown(input) => {
                pointer.inside = true;
                pointer.pressed = true;
                pointer.position = Some(input.position());
            }
            CanvasInput::PointerUp(input) => {
                pointer.pressed = false;
                pointer.position = Some(input.position());
            }
        }
    }
}

pub(crate) fn process_canvas_input(
    mut commands: Commands,
    mut inputs: MessageReader<CanvasInput>,
    mode: Res<CanvasMode>,
    mut active: ResMut<ActiveStroke>,
    mut modeler: ResMut<GlobalInputModeler>,
    mut paths: Query<&mut StrokeData>,
    mut errors: ResMut<CanvasErrors>,
) {
    for input in inputs.read() {
        match *input {
            CanvasInput::PointerEnter(_) | CanvasInput::PointerLeave => {}
            CanvasInput::PointerDown(input) => {
                active.entity = None;

                if let Tool::Stroke(brush) = mode.tool {
                    // A new stroke owns a fresh modeler state and snapshots the
                    // current brush, so later tool changes do not rewrite it.
                    modeler.modeler.reset();
                    match modeler.modeler.enqueue(&[input], &[]) {
                        Ok(update) => {
                            let data = StrokeData::new(StrokeBrush { brush }, update);
                            let entity = commands.spawn(data).id();
                            active.entity = Some(entity);
                        }
                        Err(error) => errors.0.push(CanvasError::from(error)),
                    }
                }
            }
            CanvasInput::PointerMove(input) => {
                if let Some(entity) = active.entity {
                    update_active_path(entity, input, &mut modeler, &mut paths, &mut errors);
                }
            }
            CanvasInput::PointerUp(input) => {
                if let Some(entity) = active.entity.take() {
                    update_active_path(entity, input, &mut modeler, &mut paths, &mut errors);
                    if let Ok(mut path) = paths.get_mut(entity) {
                        path.finish();
                    }
                    modeler.modeler.reset();
                }
            }
        }
    }
}

fn update_active_path(
    entity: Entity,
    input: Input,
    modeler: &mut GlobalInputModeler,
    paths: &mut Query<&mut StrokeData>,
    errors: &mut CanvasErrors,
) {
    match modeler.modeler.enqueue(&[input], &[]) {
        Ok(update) => match paths.get_mut(entity) {
            Ok(mut path) => path.apply_update(update),
            Err(error) => errors.0.push(CanvasError::MissingActiveStroke {
                entity,
                reason: error.to_string(),
            }),
        },
        Err(error) => errors.0.push(CanvasError::from(error)),
    }
}
