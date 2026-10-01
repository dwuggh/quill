//! Laid-out glyph-run text element.

use bevy_asset::Handle;
use bevy_ecs::prelude::*;
use peniko::{
    Brush, Style,
    kurbo::{Affine, Rect, Vec2},
};

use crate::asset::CanvasFont;

/// Positioned glyph in a canvas text run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasGlyph {
    /// Font-specific glyph identifier.
    pub id: u32,
    /// Horizontal offset in run-local coordinates.
    pub x: f32,
    /// Vertical offset in run-local coordinates.
    pub y: f32,
}

/// Normalized variable-font coordinate.
pub type CanvasNormalizedCoord = i16;

/// Drawable, already-shaped glyph run.
#[derive(Component, Clone, Debug)]
pub struct TextElement {
    /// Shared font asset.
    pub font: Handle<CanvasFont>,
    /// Positioned glyphs in run-local coordinates.
    pub glyphs: Vec<CanvasGlyph>,
    /// Font size in pixels per em.
    pub font_size: f32,
    /// Draw brush for the glyph run.
    pub brush: Brush,
    /// Fill or stroke style for glyph outlines.
    pub style: Style,
    /// Whether glyph hinting is enabled.
    pub hint: bool,
    /// Synthetic embolden amount applied by the renderer.
    pub font_embolden: Vec2,
    /// Optional per-glyph transform.
    pub glyph_transform: Option<Affine>,
    /// Variable-font coordinates.
    pub normalized_coords: Vec<CanvasNormalizedCoord>,
    /// Caller-provided local bounds used for hit testing.
    pub bounds: Rect,
}
