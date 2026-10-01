//! Canvas-space geometry types shared by editing systems.

use bevy_ecs::prelude::Component;
use peniko::kurbo::{Affine, Point, Rect};

/// Transform from element-local coordinates into owner-local coordinates.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct CanvasTransform {
    /// Affine transform applied to element-local geometry.
    pub affine: Affine,
}

impl CanvasTransform {
    /// Identity transform for elements stored directly in canvas coordinates.
    pub const IDENTITY: Self = Self {
        affine: Affine::IDENTITY,
    };

    /// Transform a point into owner coordinates.
    pub fn transform_point(self, point: Point) -> Point {
        self.affine * point
    }

    /// Transform a point from owner coordinates into element-local coordinates.
    pub fn inverse_transform_point(self, point: Point) -> Point {
        self.affine.inverse() * point
    }

    /// Transform a rectangle into a conservative owner-space rectangle.
    pub fn transform_rect(self, rect: Rect) -> Rect {
        let points = [
            Point::new(rect.x0, rect.y0),
            Point::new(rect.x0, rect.y1),
            Point::new(rect.x1, rect.y0),
            Point::new(rect.x1, rect.y1),
        ];
        let first = self.transform_point(points[0]);
        points[1..]
            .iter()
            .copied()
            .map(|point| self.transform_point(point))
            .fold(Rect::from_points(first, first), |rect, point| {
                rect.union_pt(point)
            })
    }
}

impl Default for CanvasTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}
