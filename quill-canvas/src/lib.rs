//! Scene-level canvas model for Quill.
//!
//! This crate owns the ECS representation of pages, tools, input routing, and
//! modeled strokes. Renderer crates can mount this scene model into their own
//! host app without pulling render backends into the core canvas crate.

pub mod canvas;
pub mod element;
pub mod eraser;
pub mod error;
pub mod geom;
pub mod input;
pub mod message;
pub mod runtime;

/// Common imports for app crates.
pub mod prelude {
    pub use crate::{
        canvas::Canvas,
        canvas::CanvasMode,
        element::bounds::{BoundHit, BoundHits, BoundKey, RectBound, RoundBound, SubRectBound},
        element::page::{Page, PageConfig},
        element::selection::{
            Selected, SelectionGesture, SelectionMode, SelectionOp, SelectionState, SelectionTool,
        },
        element::stroke::{ActiveStroke, StrokeBrush, StrokeData},
        element::{EraserConfig, EraserShape, Tool},
        error::CanvasError,
        geom::CanvasTransform,
        input::{CanvasInput, GlobalInputModeler},
        message::CanvasMessage,
        runtime::{CanvasRuntime, QuillUpdateSet},
    };
}
