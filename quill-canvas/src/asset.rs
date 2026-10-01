//! Shared document assets stored in Bevy asset collections.

use bevy_asset::Asset;
use bevy_reflect::TypePath;
use peniko::{FontData, ImageData};

/// Image bytes shared by image elements and page backgrounds.
#[derive(Asset, TypePath, Clone, Debug)]
pub struct CanvasImage {
    /// Decoded or encoded image data understood by downstream renderers.
    pub data: ImageData,
}

/// Font bytes shared by laid-out text elements.
#[derive(Asset, TypePath, Clone, Debug)]
pub struct CanvasFont {
    /// Font data used by glyph-run renderers.
    pub data: FontData,
}
