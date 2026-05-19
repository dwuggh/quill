//! Bounds for visible and selectable canvas elements.
//!
//! Most elements use rectangular bounds. Round bounds are useful as query
//! shapes for circular tools such as the eraser.

use std::ops::Range;

use bevy_ecs::prelude::*;
use peniko::kurbo::{Point, Rect};
use smallvec::{SmallVec, smallvec};

use crate::geom::CanvasTransform;

/// Default child block size used by point-like element bounds.
pub const DEFAULT_BOUND_BLOCK_SIZE: usize = 64;

/// Rectangular bounds for normal visible canvas elements.
#[derive(Component, Clone, Debug)]
pub struct RectBound {
    /// Element-local broad-phase rectangle.
    pub rect: Rect,
    /// Optional element-local sub-rectangles for inner item acceleration.
    pub children: Option<SmallVec<[SubRectBound; 8]>>,
}

/// Round bounds used by circular tool queries.
#[derive(Component, Clone, Copy, Debug)]
pub struct RoundBound {
    /// Bound center in the same coordinate space as the tested rectangle.
    pub center: Point,
    /// Bound radius in the same coordinate space as the tested rectangle.
    pub radius: f64,
}

/// A child rectangle inside a larger element bound.
#[derive(Clone, Debug)]
pub struct SubRectBound {
    /// Owner-defined identity for the content covered by this child.
    pub key: BoundKey,
    /// Child rectangle in the same coordinate space as the parent.
    pub rect: Rect,
}

/// Owner-defined identity for a bound hit.
#[derive(Clone, Debug)]
pub enum BoundKey {
    /// Consecutive item range, currently used for stroke point blocks.
    Range(Range<usize>),
}

/// A rectangle hit produced by a bounds query.
#[derive(Clone, Debug)]
pub struct BoundHit {
    /// Child key when the hit came from a sub-bound, or `None` for the parent.
    pub key: Option<BoundKey>,
    /// Intersected rectangle in the query coordinate space.
    pub rect: Rect,
}

/// Small hit list returned by bounds intersection queries.
pub type BoundHits = SmallVec<[BoundHit; 8]>;

impl RectBound {
    /// Create a rectangular bound without sub-bounds.
    pub fn from_rect(rect: Rect) -> Self {
        Self {
            rect,
            children: None,
        }
    }

    /// Create a rectangular bound from points.
    pub fn from_points(points: impl IntoIterator<Item = Point>) -> Option<Self> {
        points_bounds(points).map(Self::from_rect)
    }

    /// Create a rectangular bound with range-keyed child rectangles.
    pub fn from_points_with_children(points: &[Point], block_size: usize) -> Option<Self> {
        let rect = points_bounds(points.iter().copied())?;
        let children = if block_size == 0 {
            None
        } else {
            let children = points
                .chunks(block_size)
                .enumerate()
                .filter_map(|(block_index, chunk)| {
                    let start = block_index * block_size;
                    let end = start + chunk.len();
                    points_bounds(chunk.iter().copied()).map(|rect| SubRectBound {
                        key: BoundKey::Range(start..end),
                        rect,
                    })
                })
                .collect::<SmallVec<[_; 8]>>();

            (!children.is_empty()).then_some(children)
        };

        Some(Self { rect, children })
    }

    /// Return a copy of this bound transformed into world coordinates.
    pub fn world(&self, transform: CanvasTransform) -> Self {
        let children = self.children.as_ref().map(|children| {
            children
                .iter()
                .map(|child| SubRectBound {
                    key: child.key.clone(),
                    rect: transform.transform_rect(child.rect),
                })
                .collect()
        });

        Self {
            rect: transform.transform_rect(self.rect),
            children,
        }
    }

    /// Return hits for this rectangle against a round query bound.
    pub fn intersects_round(&self, round: RoundBound) -> Option<BoundHits> {
        if !round.intersects_rect(self.rect) {
            return None;
        }

        self.intersecting_children(|rect| round.intersects_rect(rect))
    }

    /// Return hits for this rectangle against another rectangle.
    pub fn intersects_rect(&self, rect: Rect) -> Option<BoundHits> {
        if !rects_intersect(self.rect, rect) {
            return None;
        }

        self.intersecting_children(|child| rects_intersect(child, rect))
    }

    /// Convenience API for callers that only need intersected rectangles.
    pub fn intersecting_rects(&self, rect: Rect) -> Option<SmallVec<[Rect; 8]>> {
        let hits = self.intersects_rect(rect)?;
        Some(hits.into_iter().map(|hit| hit.rect).collect())
    }

    fn intersecting_children(&self, mut intersects: impl FnMut(Rect) -> bool) -> Option<BoundHits> {
        let Some(children) = self.children.as_ref() else {
            return Some(smallvec![BoundHit {
                key: None,
                rect: self.rect,
            }]);
        };

        let hits = children
            .iter()
            .filter(|child| intersects(child.rect))
            .map(|child| BoundHit {
                key: Some(child.key.clone()),
                rect: child.rect,
            })
            .collect::<SmallVec<[_; 8]>>();

        (!hits.is_empty()).then_some(hits)
    }
}

impl RoundBound {
    /// Create a round bound with a non-negative radius.
    pub fn new(center: Point, radius: f64) -> Self {
        Self {
            center,
            radius: radius.max(0.0),
        }
    }

    /// Create a conservative round bound for a circle swept along a segment.
    pub fn from_segment(from: Point, to: Point, radius: f64) -> Self {
        let center = Point::new((from.x + to.x) * 0.5, (from.y + to.y) * 0.5);
        Self::new(center, radius.max(0.0) + from.distance(to) * 0.5)
    }

    /// Return an enclosing rectangle for this round bound.
    pub fn rect(self) -> Rect {
        Rect::new(
            self.center.x - self.radius,
            self.center.y - self.radius,
            self.center.x + self.radius,
            self.center.y + self.radius,
        )
    }

    /// Test whether this round bound contains a point.
    pub fn contains_point(self, point: Point) -> bool {
        distance_squared(point, self.center) <= self.radius * self.radius
    }

    /// Test whether this round bound contains a point while swept to `to`.
    pub fn contains_point_swept_to(self, point: Point, to: Point) -> bool {
        point_distance_squared_to_segment(point, self.center, to) <= self.radius * self.radius
    }

    /// Test whether this round bound intersects a rectangle.
    pub fn intersects_rect(self, rect: Rect) -> bool {
        let closest = Point::new(
            self.center.x.clamp(rect.x0, rect.x1),
            self.center.y.clamp(rect.y0, rect.y1),
        );
        self.contains_point(closest)
    }
}

/// Create an AABB from inclusive minimum and maximum corners.
pub fn rect_from_min_max(min: Point, max: Point) -> Rect {
    Rect::new(min.x, min.y, max.x, max.y)
}

fn points_bounds(points: impl IntoIterator<Item = Point>) -> Option<Rect> {
    let mut iter = points.into_iter();
    let first = iter.next()?;
    Some(iter.fold(Rect::from_points(first, first), |rect, point| {
        rect.union_pt(point)
    }))
}

fn point_distance_squared_to_segment(point: Point, from: Point, to: Point) -> f64 {
    let segment = to - from;
    let length_squared = segment.hypot2();
    if length_squared <= f64::EPSILON {
        return distance_squared(point, from);
    }

    let delta = point - from;
    let t = ((delta.x * segment.x + delta.y * segment.y) / length_squared).clamp(0.0, 1.0);
    distance_squared(point, from + segment * t)
}

fn distance_squared(a: Point, b: Point) -> f64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    dx * dx + dy * dy
}

fn rects_intersect(a: Rect, b: Rect) -> bool {
    a.x0 <= b.x1 && a.x1 >= b.x0 && a.y0 <= b.y1 && a.y1 >= b.y0
}
