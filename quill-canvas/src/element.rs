//! Canvas elements and element-level editing systems.
//!
//! Elements are the drawable or editable scene objects that live on pages.

use std::marker::PhantomData;

use bevy_ecs::prelude::*;
use peniko::Color;
use quill_stroke::brush::{Brush, BrushTip, StrokePaint};

use crate::{canvas::CanvasMode, input::CanvasInput};

pub mod bounds;
pub mod eraser;
pub mod image;
pub mod page;
pub mod selection;
pub mod stroke;
pub mod text;

pub use selection::{SelectionMode, SelectionOp, SelectionTool};

/// Active tool used when pointer input creates or edits scene content.
#[derive(Clone, Copy, Debug)]
pub enum Tool {
    /// Draw modeled strokes with the configured brush.
    Stroke(Brush),
    /// Remove path points covered by the eraser footprint.
    Eraser(EraserConfig),
    /// Select existing canvas elements.
    Selection(SelectionTool),
    // Shape(ShapeTool),
    // Marker(MarkerTool),
    // Image(ImageTool),
    // Page(PageTool),
}

/// Shape used by the eraser footprint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EraserShape {
    Circle,
    Square,
}

/// Configuration for the point eraser tool.
#[derive(Clone, Copy, Debug)]
pub struct EraserConfig {
    /// Eraser footprint shape.
    pub shape: EraserShape,
    /// Circle radius or square half-side in canvas units.
    pub size: f32,
}

impl EraserConfig {
    /// Create an eraser configuration from a footprint shape and size.
    pub fn new(shape: EraserShape, size: f32) -> Self {
        Self { shape, size }
    }
}

impl Default for EraserConfig {
    fn default() -> Self {
        Self {
            shape: EraserShape::Circle,
            size: 12.0,
        }
    }
}

impl Default for Tool {
    fn default() -> Self {
        Self::Stroke(Brush {
            tip: BrushTip::round(2.0),
            paint: StrokePaint {
                color: Color::BLACK,
                width: 2.0,
                sharpness: 1.0,
            },
        })
    }
}

// pub struct PageTool {}

#[derive(Clone, Debug, Copy, Message)]
pub struct InputMessage<T> {
    input: CanvasInput,
    phantom: PhantomData<T>,
}

impl<T> InputMessage<T> {
    fn new(input: CanvasInput) -> Self {
        Self {
            input,
            phantom: PhantomData,
        }
    }
}

impl<T> From<CanvasInput> for InputMessage<T> {
    fn from(value: CanvasInput) -> Self {
        InputMessage::new(value)
    }
}

pub type StrokeMessage = InputMessage<Brush>;
pub type EraserMessage = InputMessage<EraserConfig>;
pub type SelectionMessage = InputMessage<SelectionTool>;
// pub type PageMessage = InputMessage<PageTool>;

pub(crate) fn route_canvas_input(
    mut inputs: MessageReader<CanvasInput>,
    mode: Res<CanvasMode>,
    mut stroke_message_writer: MessageWriter<StrokeMessage>,
    mut eraser_message_writer: MessageWriter<EraserMessage>,
    mut selection_message_writer: MessageWriter<SelectionMessage>,
) {
    for input in inputs.read() {
        match mode.tool {
            Tool::Stroke(_) => {
                stroke_message_writer.write((*input).into());
            }
            Tool::Eraser(_) => {
                eraser_message_writer.write((*input).into());
            }
            Tool::Selection(_) => {
                selection_message_writer.write((*input).into());
            }
        }
    }
}
