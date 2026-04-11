use crate::shape::{TipState, tip_state_for_input};
use crate::{Brush, ColorRgba, ModeledInput, StrokeError};
use kurbo::{Point, Rect, Vec2};
use lyon::math::point;
use lyon::path::Path;
use lyon::tessellation::{
    BuffersBuilder, FillOptions, FillTessellator, FillVertex, FillVertexConstructor, VertexBuffers,
};
use std::f64::consts::{PI, TAU};

#[derive(Debug, Clone, PartialEq)]
pub struct StrokeVertex {
    pub position: Point,
    pub color: ColorRgba,
    pub side_derivative: Vec2,
    pub side_label: f32,
    pub forward_derivative: Vec2,
    pub forward_label: f32,
    pub uv: [f32; 2],
    pub animation_offset: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MutableStrokeMesh {
    pub vertices: Vec<StrokeVertex>,
    pub indices: Vec<u32>,
    pub outlines: Vec<Vec<u32>>,
    pub bounds: Option<Rect>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshPartition {
    pub vertices: Vec<StrokeVertex>,
    pub indices: Vec<u32>,
    pub outlines: Vec<Vec<u32>>,
    pub bounds: Option<Rect>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PartitionedStrokeMesh {
    pub render_groups: Vec<Vec<MeshPartition>>,
    pub bounds: Option<Rect>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct StrokeShapeUpdate {
    pub dirty_bounds: Option<Rect>,
    pub first_changed_vertex: Option<usize>,
    pub first_changed_index: Option<usize>,
}

pub fn build_stroke_mesh(
    modeled_inputs: &[ModeledInput],
    brush: &Brush,
) -> Result<MutableStrokeMesh, StrokeError> {
    if modeled_inputs.is_empty() {
        return Ok(MutableStrokeMesh::default());
    }

    let coat = brush
        .family
        .coats
        .first()
        .ok_or(StrokeError::InvalidBrush("brush has no coats"))?;
    let tip_states = modeled_inputs
        .iter()
        .map(|input| tip_state_for_input(input, brush, &coat.tip))
        .collect::<Vec<_>>();
    let outline = outline_from_tip_states(&tip_states, brush.epsilon);
    if outline.len() < 3 {
        return Ok(MutableStrokeMesh::default());
    }
    tessellate_outline(&outline, brush.color)
}

pub(crate) fn union_optional_rects(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.union(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

pub(crate) fn union_rect_accum(acc: Option<Rect>, rect: Rect) -> Option<Rect> {
    union_optional_rects(acc, Some(rect))
}

fn outline_from_tip_states(states: &[TipState], epsilon: f64) -> Vec<Point> {
    if states.is_empty() {
        return Vec::new();
    }
    if states.len() == 1 {
        let state = states[0];
        return ellipse_points(state.position, state.width * 0.5, state.height * 0.5, 24);
    }

    let mut left = Vec::with_capacity(states.len());
    let mut right = Vec::with_capacity(states.len());
    let mut normals = Vec::with_capacity(states.len());

    for i in 0..states.len() {
        let prev = states[i.saturating_sub(1)].position;
        let next = states[(i + 1).min(states.len() - 1)].position;
        let tangent = normalize_or(next - prev, Vec2::new(1.0, 0.0));
        let normal = Vec2::new(-tangent.y, tangent.x);
        let radius = (states[i].width.max(states[i].height) * 0.5).max(epsilon);
        left.push(states[i].position + normal * radius);
        right.push(states[i].position - normal * radius);
        normals.push(normal);
    }

    let mut outline = Vec::new();
    outline.extend(left.iter().copied());
    append_cap(
        &mut outline,
        states.last().unwrap().position,
        *normals.last().unwrap(),
        true,
    );
    outline.extend(right.iter().rev().copied());
    append_cap(&mut outline, states[0].position, normals[0], false);
    outline
}

fn append_cap(outline: &mut Vec<Point>, center: Point, normal: Vec2, end: bool) {
    let radius = center.distance(*outline.last().unwrap_or(&center));
    if radius <= 0.0 {
        return;
    }
    let angle = normal.y.atan2(normal.x);
    let start = if end { angle } else { angle + PI };
    let end = start + PI;
    for i in 1..=12 {
        let t = i as f64 / 12.0;
        let a = start + (end - start) * t;
        outline.push(Point::new(
            center.x + a.cos() * radius,
            center.y + a.sin() * radius,
        ));
    }
}

fn ellipse_points(center: Point, rx: f64, ry: f64, count: usize) -> Vec<Point> {
    (0..count)
        .map(|i| {
            let a = TAU * i as f64 / count as f64;
            Point::new(center.x + a.cos() * rx, center.y + a.sin() * ry)
        })
        .collect()
}

fn tessellate_outline(
    outline: &[Point],
    color: ColorRgba,
) -> Result<MutableStrokeMesh, StrokeError> {
    let mut path_builder = Path::builder();
    let first = outline[0];
    path_builder.begin(point(first.x as f32, first.y as f32));
    for p in &outline[1..] {
        path_builder.line_to(point(p.x as f32, p.y as f32));
    }
    path_builder.end(true);
    let path = path_builder.build();

    let mut buffers: VertexBuffers<StrokeVertex, u32> = VertexBuffers::new();
    let mut tessellator = FillTessellator::new();
    tessellator.tessellate_path(
        &path,
        &FillOptions::default(),
        &mut BuffersBuilder::new(&mut buffers, VertexCtor { color }),
    )?;
    let bounds = bounds_for_vertices(&buffers.vertices);
    let outlines = vec![(0..outline.len() as u32).collect()];
    Ok(MutableStrokeMesh {
        vertices: buffers.vertices,
        indices: buffers.indices,
        outlines,
        bounds,
    })
}

struct VertexCtor {
    color: ColorRgba,
}

impl FillVertexConstructor<StrokeVertex> for VertexCtor {
    fn new_vertex(&mut self, vertex: FillVertex) -> StrokeVertex {
        let position = vertex.position();
        StrokeVertex {
            position: Point::new(position.x as f64, position.y as f64),
            color: self.color,
            side_derivative: Vec2::ZERO,
            side_label: 0.0,
            forward_derivative: Vec2::ZERO,
            forward_label: 0.0,
            uv: [0.0, 0.0],
            animation_offset: 0.0,
        }
    }
}

fn bounds_for_vertices(vertices: &[StrokeVertex]) -> Option<Rect> {
    let first = vertices.first()?.position;
    let mut min_x = first.x;
    let mut min_y = first.y;
    let mut max_x = first.x;
    let mut max_y = first.y;
    for vertex in &vertices[1..] {
        min_x = min_x.min(vertex.position.x);
        min_y = min_y.min(vertex.position.y);
        max_x = max_x.max(vertex.position.x);
        max_y = max_y.max(vertex.position.y);
    }
    Some(Rect::new(min_x, min_y, max_x, max_y))
}

fn normalize_or(v: Vec2, fallback: Vec2) -> Vec2 {
    let length = (v.x * v.x + v.y * v.y).sqrt();
    if length > 1e-9 { v / length } else { fallback }
}
