//! Input messages and global canvas mode resources.
//!
//! The canvas is driven by message streams instead of direct method calls on
//! entities. That keeps the same vocabulary usable from the standalone
//! `Canvas` facade and from a Bevy app using `QuillPlugin`.

use bevy_ecs::prelude::*;
use quill_stroke::{
    brush::Brush,
    input::{Input, Vec2},
};

use crate::page::PageConfig;

/// Active tool used when pointer input creates or edits scene content.
#[derive(Clone, Copy, Debug)]
pub enum Tool {
    /// Draw modeled strokes with the configured brush.
    Stroke(Brush),
    /// Placeholder for future eraser behavior.
    Eraser,
    /// Placeholder for future selection/edit behavior.
    Selection,
}

/// Global canvas mode resource.
///
/// For now the mode is just the current tool. Brush style lives inside
/// `Tool::Stroke`, so a pointer-down event can snapshot the whole tool state
/// onto the new stroke entity.
#[derive(Resource, Clone, Copy, Debug)]
pub struct CanvasMode {
    pub tool: Tool,
}

impl CanvasMode {
    /// Create a mode resource with the given active tool.
    pub fn new(tool: Tool) -> Self {
        Self { tool }
    }
}

/// Latest pointer state observed by the canvas.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct PointerState {
    /// Whether the pointer is currently inside the canvas surface.
    pub inside: bool,
    /// Last known pointer position in canvas coordinates.
    pub position: Option<Vec2>,
    /// Whether the active pointer is pressed.
    pub pressed: bool,
}

/// Pointer/stylus input understood by the scene model.
#[derive(Message, Clone, Copy, Debug)]
pub enum CanvasInput {
    /// Pointer entered the canvas at the given position.
    PointerEnter(Vec2),
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
