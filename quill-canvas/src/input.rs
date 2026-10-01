//! Input messages and global canvas mode resources.
//!
//! The canvas is driven by message streams instead of direct method calls on
//! entities. That keeps the same vocabulary usable from the standalone
//! `Canvas` facade and from a Bevy app using `QuillPlugin`.

use bevy_asset::Handle;
use bevy_ecs::prelude::*;
use peniko::kurbo::Size;
use quill_stroke::input::{Input, InputModeler, WillInputModeler};

use crate::{
    asset::CanvasImage,
    element::{Tool, page::PageConfig, text::TextElement},
    geom::CanvasTransform,
};

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
#[derive(Message, Clone, Debug)]
pub enum CanvasCommand {
    /// Replace the active canvas tool.
    SetTool(Tool),
    /// Add a new page entity.
    AddPage(AddPage),
    /// Add a new image entity.
    AddImage(AddImage),
    /// Add a new text entity.
    AddText(AddText),
    /// Delete an entity and any hierarchy descendants.
    DeleteEntity(Entity),
}

/// Command payload for page creation.
#[derive(Clone, Debug)]
pub struct AddPage {
    /// Owner page/root. `None` means the document root.
    pub owner: Option<Entity>,
    /// Page data to create.
    pub config: PageConfig,
    /// Transform from page-local coordinates into owner-local coordinates.
    pub transform: CanvasTransform,
}

impl AddPage {
    /// Create a root-level page command with an identity transform.
    pub fn root(config: PageConfig) -> Self {
        Self {
            owner: None,
            config,
            transform: CanvasTransform::IDENTITY,
        }
    }
}

/// Command payload for image creation.
#[derive(Clone, Debug)]
pub struct AddImage {
    /// Owner page/root. `None` means the document root.
    pub owner: Option<Entity>,
    /// Shared image asset handle.
    pub image: Handle<CanvasImage>,
    /// Image-local drawing size.
    pub size: Size,
    /// Transform from image-local coordinates into owner-local coordinates.
    pub transform: CanvasTransform,
}

/// Command payload for text creation.
#[derive(Clone, Debug)]
pub struct AddText {
    /// Owner page/root. `None` means the document root.
    pub owner: Option<Entity>,
    /// Laid-out glyph-run text element.
    pub text: TextElement,
    /// Transform from text-local coordinates into owner-local coordinates.
    pub transform: CanvasTransform,
}
