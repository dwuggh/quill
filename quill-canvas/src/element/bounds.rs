//! Bounds for visible and selectable canvas elements.
//!
//! Most elements use rectangular bounds. Round bounds are useful as query
//! shapes for circular tools such as the eraser.

use std::ops::Range;

use bevy::{
    math::{
        Vec2, Vec3,
        bounding::{Aabb2d, IntersectsVolume},
    },
    transform::components::Transform,
};
use bevy_ecs::prelude::*;
use smallvec::{SmallVec, smallvec};

/// Default child block size used by point-like element bounds.
pub const DEFAULT_BOUND_BLOCK_SIZE: usize = 64;

/// Rectangular bounds for normal visible canvas elements.
#[derive(Component, Clone, Debug)]
pub struct RectBound {
    /// Element-local broad-phase rectangle.
    pub rect: Aabb2d,
    /// Optional element-local sub-rectangles for inner item acceleration.
    pub children: Option<SmallVec<[SubRectBound; 8]>>,
}

/// Round bounds used by circular tool queries.
#[derive(Component, Clone, Copy, Debug)]
pub struct RoundBound {
    /// Bound center in the same coordinate space as the tested rectangle.
    pub center: Vec2,
    /// Bound radius in the same coordinate space as the tested rectangle.
    pub radius: f32,
}

/// A child rectangle inside a larger element bound.
#[derive(Clone, Debug)]
pub struct SubRectBound {
    /// Owner-defined identity for the content covered by this child.
    pub key: BoundKey,
    /// Child rectangle in the same coordinate space as the parent.
    pub rect: Aabb2d,
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
    pub rect: Aabb2d,
}

/// Small hit list returned by bounds intersection queries.
pub type BoundHits = SmallVec<[BoundHit; 8]>;

impl RectBound {
    /// Create a rectangular bound without sub-bounds.
    pub fn from_rect(rect: Aabb2d) -> Self {
        Self {
            rect,
            children: None,
        }
    }

    /// Create a rectangular bound from points.
    pub fn from_points(points: impl IntoIterator<Item = Vec2>) -> Option<Self> {
        points_bounds(points).map(Self::from_rect)
    }

    /// Create a rectangular bound with range-keyed child rectangles.
    pub fn from_points_with_children(points: &[Vec2], block_size: usize) -> Option<Self> {
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
    pub fn world(&self, transform: &Transform) -> Self {
        let children = self.children.as_ref().map(|children| {
            children
                .iter()
                .map(|child| SubRectBound {
                    key: child.key.clone(),
                    rect: transform_aabb(transform, child.rect),
                })
                .collect()
        });

        Self {
            rect: transform_aabb(transform, self.rect),
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
    pub fn intersects_rect(&self, rect: Aabb2d) -> Option<BoundHits> {
        if !self.rect.intersects(&rect) {
            return None;
        }

        self.intersecting_children(|child| child.intersects(&rect))
    }

    /// Convenience API for callers that only need intersected rectangles.
    pub fn intersecting_aabbs(&self, rect: Aabb2d) -> Option<SmallVec<[Aabb2d; 8]>> {
        let hits = self.intersects_rect(rect)?;
        Some(hits.into_iter().map(|hit| hit.rect).collect())
    }

    fn intersecting_children(
        &self,
        mut intersects: impl FnMut(Aabb2d) -> bool,
    ) -> Option<BoundHits> {
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
    pub fn new(center: Vec2, radius: f32) -> Self {
        Self {
            center,
            radius: radius.max(0.0),
        }
    }

    /// Create a conservative round bound for a circle swept along a segment.
    pub fn from_segment(from: Vec2, to: Vec2, radius: f32) -> Self {
        let center = (from + to) * 0.5;
        Self::new(center, radius.max(0.0) + from.distance(to) * 0.5)
    }

    /// Return an enclosing rectangle for this round bound.
    pub fn aabb(self) -> Aabb2d {
        Aabb2d::new(self.center, Vec2::splat(self.radius))
    }

    /// Test whether this round bound contains a point.
    pub fn contains_point(self, point: Vec2) -> bool {
        point.distance_squared(self.center) <= self.radius * self.radius
    }

    /// Test whether this round bound contains a point while swept to `to`.
    pub fn contains_point_swept_to(self, point: Vec2, to: Vec2) -> bool {
        point_distance_squared_to_segment(point, self.center, to) <= self.radius * self.radius
    }

    /// Test whether this round bound intersects a rectangle.
    pub fn intersects_rect(self, rect: Aabb2d) -> bool {
        let closest = self.center.clamp(rect.min, rect.max);
        self.contains_point(closest)
    }
}

/// Create an AABB from inclusive minimum and maximum corners.
pub fn aabb_from_min_max(min: Vec2, max: Vec2) -> Aabb2d {
    Aabb2d::new((min + max) * 0.5, (max - min) * 0.5)
}

/// Transform a local rectangle into a conservative world rectangle.
pub fn transform_aabb(transform: &Transform, bounds: Aabb2d) -> Aabb2d {
    let corners = [
        Vec2::new(bounds.min.x, bounds.min.y),
        Vec2::new(bounds.min.x, bounds.max.y),
        Vec2::new(bounds.max.x, bounds.min.y),
        Vec2::new(bounds.max.x, bounds.max.y),
    ];
    let first = transform_point(transform, corners[0]);
    let (min, max) = corners[1..]
        .iter()
        .copied()
        .map(|point| transform_point(transform, point))
        .fold((first, first), |(min, max), point| {
            (min.min(point), max.max(point))
        });
    aabb_from_min_max(min, max)
}

fn points_bounds(points: impl IntoIterator<Item = Vec2>) -> Option<Aabb2d> {
    let mut iter = points.into_iter();
    let first = iter.next()?;
    let (min, max) = iter.fold((first, first), |(min, max), point| {
        (min.min(point), max.max(point))
    });
    Some(aabb_from_min_max(min, max))
}

fn point_distance_squared_to_segment(point: Vec2, from: Vec2, to: Vec2) -> f32 {
    let segment = to - from;
    let length_squared = segment.length_squared();
    if length_squared <= f32::EPSILON {
        return point.distance_squared(from);
    }

    let t = ((point - from).dot(segment) / length_squared).clamp(0.0, 1.0);
    point.distance_squared(from + segment * t)
}

fn transform_point(transform: &Transform, point: Vec2) -> Vec2 {
    transform
        .transform_point(Vec3::new(point.x, point.y, 0.0))
        .truncate()
}
