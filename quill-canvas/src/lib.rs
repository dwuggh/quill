//! Scene-level canvas model for Quill.
//!
//! This crate owns the ECS representation of pages, tools, input routing, and
//! modeled strokes. Rendering code may prepare GPU stroke caches here, but the
//! host app still owns windows, cameras, render targets, and final draw passes.

pub mod canvas;
pub mod element;
pub mod eraser;
pub mod error;
pub mod input;
pub mod message;
#[cfg(feature = "bevy")]
pub mod plugin;
pub mod render;

pub use bevy::render::render_resource::Texture;

/// Common imports for app crates.
pub mod prelude {
    pub use crate::{
        Texture,
        canvas::CanvasMode,
        canvas::{Canvas, CanvasRenderer},
        element::page::{Page, PageConfig},
        element::stroke::{ActiveStroke, StrokeBrush, StrokeData},
        element::{EraserConfig, EraserShape, Tool},
        eraser::{PointBlockBounds, StrokeBounds},
        error::CanvasError,
        input::{CanvasInput, GlobalInputModeler},
        message::CanvasMessage,
    };

    #[cfg(feature = "bevy")]
    pub use crate::plugin::QuillPlugin;
}
