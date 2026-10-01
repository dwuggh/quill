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
    /// Mask pixels or delete drawable items covered by the eraser footprint.
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

/// Eraser behavior used when a gesture hits drawable elements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EraserMode {
    /// Persist pixel masks on hit drawables.
    PixelWise,
    /// Delete hit drawables as whole items.
    ItemWise,
}

/// Configuration for the eraser tool.
#[derive(Clone, Copy, Debug)]
pub struct EraserConfig {
    /// Whether the eraser masks pixels or deletes whole items.
    pub mode: EraserMode,
    /// Eraser footprint shape.
    pub shape: EraserShape,
    /// Circle radius or square half-side in canvas units.
    pub size: f32,
    /// Drawable element classes affected by the eraser.
    pub targets: EraserTargets,
}

impl EraserConfig {
    /// Create an eraser configuration from a footprint shape and size.
    pub fn new(shape: EraserShape, size: f32) -> Self {
        Self {
            mode: EraserMode::PixelWise,
            shape,
            size,
            targets: EraserTargets::STROKES,
        }
    }
}

bitflags::bitflags! {
    /// Drawable element classes affected by eraser gestures.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct EraserTargets: u8 {
        /// Erase stroke drawables.
        const STROKES = 0b0000_0001;
        /// Erase image drawables.
        const IMAGES = 0b0000_0010;
        /// Erase text drawables.
        const TEXT = 0b0000_0100;
    }
}

impl Default for EraserTargets {
    fn default() -> Self {
        Self::STROKES
    }
}

impl Default for EraserConfig {
    fn default() -> Self {
        Self {
            mode: EraserMode::PixelWise,
            shape: EraserShape::Circle,
            size: 12.0,
            targets: EraserTargets::STROKES,
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
