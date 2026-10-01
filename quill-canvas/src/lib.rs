//! Scene-level canvas model for Quill.
//!
//! This crate owns the ECS representation of pages, tools, input routing, and
//! modeled strokes. Renderer crates can mount this scene model into their own
//! host app without pulling render backends into the core canvas crate.

pub mod asset;
pub mod canvas;
pub mod document;
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
        asset::{CanvasFont, CanvasImage},
        canvas::Canvas,
        canvas::CanvasMode,
        document::{DocumentRoot, DocumentRootEntity},
        element::bounds::{BoundHit, BoundHits, BoundKey, RectBound, RoundBound, SubRectBound},
        element::image::ImageElement,
        element::page::{Page, PageBackground, PageBackgroundItem, PageConfig},
        element::selection::{
            Selected, SelectionGesture, SelectionMode, SelectionOp, SelectionState, SelectionTool,
        },
        element::stroke::{ActiveStroke, StrokeBrush, StrokeData},
        element::text::{CanvasGlyph, CanvasNormalizedCoord, TextElement},
        element::{EraserConfig, EraserShape, Tool},
        error::CanvasError,
        geom::CanvasTransform,
        input::{AddImage, AddPage, AddText, CanvasInput, GlobalInputModeler},
        message::CanvasMessage,
        runtime::{CanvasRuntime, QuillUpdateSet},
    };
}
