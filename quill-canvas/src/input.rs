//! Input messages and global canvas mode resources.
//!
//! The canvas is driven by message streams instead of direct method calls on
//! entities. That keeps the same vocabulary usable from the standalone
//! `Canvas` facade and from a Bevy app using `QuillPlugin`.

use bevy_ecs::prelude::*;
use quill_stroke::input::{Input, InputModeler, WillInputModeler};

use crate::{element::Tool, element::page::PageConfig};

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

// TODO consider use ui_event here
/// Pointer/stylus input understood by the scene model.
#[derive(Message, Clone, Copy, Debug)]
pub enum CanvasInput {
    /// Pointer entered the canvas at the given position.
    PointerEnter([f32; 2]),
    /// Pointer left the canvas.
    PointerLeave,
    /// Pointer moved, optionally extending the active stroke.
    PointerMove(Input),
    /// Pointer pressed, creating content according to the current tool.
    PointerDown(Input),
    /// Pointer released, finalizing any active stroke.
    PointerUp(Input),
}

/// Non-pointer canvas commands.
#[derive(Message, Clone, Copy, Debug)]
pub enum CanvasCommand {
    /// Replace the active canvas tool.
    SetTool(Tool),
    /// Add a new page entity.
    AddPage(PageConfig),
}
