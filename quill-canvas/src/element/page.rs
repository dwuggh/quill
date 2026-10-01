//! Page components and background display-list data.

use std::{fmt, sync::Arc};

use bevy_asset::Handle;
use bevy_ecs::prelude::*;
use peniko::{
    Brush, Fill, Style,
    kurbo::{Affine, BezPath, Point, Rect, Size, Stroke},
};

use crate::asset::CanvasImage;

/// Page creation configuration.
#[derive(Clone, Debug)]
pub struct PageConfig {
    /// Explicit page size in page-local document units.
    pub size: Size,
    /// Background drawn before ordinary page children.
    pub background: PageBackground,
}

impl PageConfig {
    /// Create a page configuration from a size and background.
    pub fn new(size: Size, background: PageBackground) -> Self {
        Self { size, background }
    }
}

impl Default for PageConfig {
    fn default() -> Self {
        Self {
            size: Size::new(595.0, 842.0),
            background: PageBackground::blank(Brush::Solid(peniko::Color::WHITE)),
        }
    }
}

/// A document page that can own nested pages and drawable elements.
#[derive(Component, Clone, Debug)]
pub struct Page {
    /// Explicit page size in page-local document units.
    pub size: Size,
    /// Background drawn before ordinary page children.
    pub background: PageBackground,
}

impl Page {
    /// Create a page from creation configuration.
    pub fn new(config: PageConfig) -> Self {
        Self {
            size: config.size,
            background: config.background,
        }
    }

    /// Return the page-local rectangle covered by this page.
    pub fn rect(&self) -> Rect {
        Rect::new(0.0, 0.0, self.size.width, self.size.height)
    }
}

/// Background content drawn as part of a page.
#[derive(Clone, Debug)]
pub enum PageBackground {
    /// Solid paper fill.
    Blank {
        /// Paper fill brush.
        fill: Brush,
    },
    /// Paper fill plus retained page-local drawing items.
    DisplayList {
        /// Paper fill brush.
        fill: Brush,
        /// Page-local drawing items.
        items: Vec<PageBackgroundItem>,
    },
    /// Paper fill plus an image placed by a page-local transform.
    Image {
        /// Paper fill brush.
        fill: Brush,
        /// Shared image asset.
        image: Handle<CanvasImage>,
        /// Transform from image-local pixels into page-local coordinates.
        transform: Affine,
    },
    /// Paper fill plus lazily produced page-local drawing items.
    Procedural {
        /// Paper fill brush.
        fill: Brush,
        /// Source that lays out background items for a page size.
        source: Arc<dyn PageBackgroundSource>,
    },
}

impl PageBackground {
    /// Create a blank page background with a paper fill brush.
    pub fn blank(fill: Brush) -> Self {
        Self::Blank { fill }
    }

    /// Return the base fill brush for this background.
    pub fn fill(&self) -> &Brush {
        match self {
            Self::Blank { fill }
            | Self::DisplayList { fill, .. }
            | Self::Image { fill, .. }
            | Self::Procedural { fill, .. } => fill,
        }
    }
}

/// Lazy source for page background display-list items.
pub trait PageBackgroundSource: fmt::Debug + Send + Sync + 'static {
    /// Build page-local background items for the requested page size.
    fn layout(&self, size: Size) -> Vec<PageBackgroundItem>;
}

/// A retained page-local background primitive.
#[derive(Clone, Debug)]
pub enum PageBackgroundItem {
    /// Stroked line segment.
    Line {
        /// Segment start in page-local coordinates.
        from: Point,
        /// Segment end in page-local coordinates.
        to: Point,
        /// Stroke style.
        stroke: Stroke,
        /// Stroke brush.
        brush: Brush,
    },
    /// Filled rectangle.
    Rect {
        /// Page-local rectangle.
        rect: Rect,
        /// Fill brush.
        brush: Brush,
        /// Fill rule.
        fill_rule: Fill,
    },
    /// Filled or stroked path.
    Path {
        /// Page-local path.
        path: BezPath,
        /// Draw brush.
        brush: Brush,
        /// Fill or stroke style.
        style: Style,
    },
    /// Image background item.
    Image {
        /// Shared image asset.
        image: Handle<CanvasImage>,
        /// Transform from image-local pixels into page-local coordinates.
        transform: Affine,
    },
}
