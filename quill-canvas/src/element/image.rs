//! Image drawable element.

use bevy_asset::Handle;
use bevy_ecs::prelude::*;
use peniko::kurbo::{Rect, Size};

use crate::asset::CanvasImage;

/// Drawable image placed by `CanvasTransform` in its owner's coordinates.
#[derive(Component, Clone, Debug)]
pub struct ImageElement {
    /// Shared image asset.
    pub image: Handle<CanvasImage>,
    /// Image-local drawing size.
    pub size: Size,
}

impl ImageElement {
    /// Create an image element from an asset handle and local size.
    pub fn new(image: Handle<CanvasImage>, size: Size) -> Self {
        Self { image, size }
    }

    /// Return the element-local image rectangle.
    pub fn rect(&self) -> Rect {
        Rect::new(0.0, 0.0, self.size.width, self.size.height)
    }
}
