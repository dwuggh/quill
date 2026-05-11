//! Error types for canvas operations and systems.

use bevy_ecs::prelude::Entity;
use quill_stroke::input::InputModelError;

/// Errors surfaced by the standalone canvas facade or collected from systems.
#[derive(Debug, thiserror::Error)]
pub enum CanvasError {
    /// Input modeler rejected a batch or configuration.
    #[error(transparent)]
    InputModel(#[from] InputModelError),

    /// The active stroke entity was removed or otherwise unavailable.
    #[error("active stroke entity {entity:?} is not available: {reason}")]
    MissingActiveStroke { entity: Entity, reason: String },

    /// A page command ran but did not report a created page entity.
    #[error("page command completed without creating a page")]
    PageWasNotCreated,
}
