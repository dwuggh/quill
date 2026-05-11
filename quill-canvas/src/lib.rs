//! Scene-level canvas model for Quill.
//!
//! This crate owns the ECS representation of pages, tools, pointer state, and
//! modeled strokes. Rendering code may prepare GPU stroke caches here, but the
//! host app still owns windows, cameras, render targets, and final draw passes.

pub mod canvas;
pub mod error;
pub mod input;
pub mod page;
#[cfg(feature = "bevy")]
pub mod plugin;
pub mod render;
pub mod stroke;

/// Common imports for app crates.
pub mod prelude {
    pub use crate::{
        canvas::Canvas,
        error::CanvasError,
        input::{CanvasInput, CanvasMode, PointerState, Tool},
        page::{Page, PageConfig},
        stroke::{ActiveStroke, GlobalInputModeler, StrokeBrush, StrokeData},
    };

    #[cfg(feature = "bevy")]
    pub use crate::plugin::QuillPlugin;
}
