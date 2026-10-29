//! Imaging renderer for the Quill canvas scene.
//!
//! This crate is intentionally backend-agnostic. It reads the ECS scene owned by
//! `quill-canvas`, converts stroke centerlines into filled CPU outlines from
//! `quill-stroke`, and emits drawing commands into an `imaging::Painter`.

use std::collections::{HashMap, HashSet};

use bevy_asset::Assets;
use bevy_ecs::{entity::Entity, hierarchy::Children, world::World};
use imaging::{
    Composite, GroupRef, PaintSink, Painter,
    kurbo::{Affine, BezPath, Point, Size},
    peniko::{BlendMode, Brush, Color, Compose, Fill, Style},
    record::Glyph,
};
use quill_canvas::{
    asset::{CanvasFont, CanvasImage},
    canvas::Canvas,
    document::DocumentRootEntity,
    element::{
        EraserConfig, EraserShape,
        eraser::{ActiveEraser, Hidden, Mask, MaskSegment},
        image::ImageElement,
        page::{Page, PageBackground, PageBackgroundItem},
        stroke::StrokeData,
        text::TextElement,
    },
    geom::CanvasTransform,
};
use quill_stroke::{
    brush::BrushTip,
    outline::{SimpleStrokeOutline, StrokeOutlineOptions},
};

/// Draws Quill canvas entities into an `imaging` command sink.
#[derive(Debug)]
pub struct CanvasImagingRenderer {
    /// Cached stroke outlines keyed by stable Bevy entity id.
    strokes: HashMap<Entity, CachedStroke>,
    /// Options used for CPU outline construction.
    outline_options: StrokeOutlineOptions,
}

impl CanvasImagingRenderer {
    /// Create a renderer with default stroke outline options.
    pub fn new() -> Self {
        Self {
            strokes: HashMap::new(),
            outline_options: StrokeOutlineOptions::default(),
        }
    }

    /// Draw the contents of a `Canvas` facade into an imaging painter.
    pub fn draw_canvas<S>(&mut self, canvas: &Canvas, painter: &mut Painter<'_, S>, _size: Size)
    where
        S: PaintSink + ?Sized,
    {
        self.draw_canvas_with_transform(canvas, painter, Affine::IDENTITY);
    }

    /// Draw the contents of a `Canvas` facade through an explicit root-to-sink transform.
    pub fn draw_canvas_with_transform<S>(
        &mut self,
        canvas: &Canvas,
        painter: &mut Painter<'_, S>,
        sink_from_root: Affine,
    ) where
        S: PaintSink + ?Sized,
    {
        self.draw_world_with_transform(canvas.world(), painter, sink_from_root);
    }

    /// Draw the contents of a Quill ECS world into an imaging painter.
    pub fn draw_world<S>(&mut self, world: &World, painter: &mut Painter<'_, S>)
    where
        S: PaintSink + ?Sized,
    {
        self.draw_world_with_transform(world, painter, Affine::IDENTITY);
    }

    /// Draw the contents of a Quill ECS world through an explicit root-to-sink transform.
    pub fn draw_world_with_transform<S>(
        &mut self,
        world: &World,
        painter: &mut Painter<'_, S>,
        sink_from_root: Affine,
    ) where
        S: PaintSink + ?Sized,
    {
        let Some(root) = world.get_resource::<DocumentRootEntity>() else {
            self.clear_cache();
            return;
        };

        let mut live_strokes = HashSet::new();
        self.draw_children(world, root.0, sink_from_root, painter, &mut live_strokes);
        self.draw_active_eraser(world, sink_from_root, painter);
        self.strokes
            .retain(|entity, _| live_strokes.contains(entity));
    }

    /// Drop all retained outline geometry.
    pub fn clear_cache(&mut self) {
        self.strokes.clear();
    }

    fn draw_children<S>(
        &mut self,
        world: &World,
        owner: Entity,
        root_from_owner: Affine,
        painter: &mut Painter<'_, S>,
        live_strokes: &mut HashSet<Entity>,
    ) where
        S: PaintSink + ?Sized,
    {
        let Some(children) = world.get::<Children>(owner) else {
            return;
        };

        for child in children.iter().copied() {
            self.draw_entity(world, child, root_from_owner, painter, live_strokes);
        }
    }

    fn draw_entity<S>(
        &mut self,
        world: &World,
        entity: Entity,
        root_from_owner: Affine,
        painter: &mut Painter<'_, S>,
        live_strokes: &mut HashSet<Entity>,
    ) where
        S: PaintSink + ?Sized,
    {
        if world.get::<Hidden>(entity).is_some() {
            return;
        }

        let transform = world
            .get::<CanvasTransform>(entity)
            .copied()
            .unwrap_or_default();
        let root_from_entity = root_from_owner * transform.affine;

        if let Some(page) = world.get::<Page>(entity) {
            self.draw_page(world, page, root_from_entity, painter);
            self.draw_children(world, entity, root_from_entity, painter, live_strokes);
            return;
        }

        if let Some(stroke) = world.get::<StrokeData>(entity) {
            live_strokes.insert(entity);
            self.draw_stroke(
                entity,
                stroke,
                world.get::<Mask>(entity),
                root_from_entity,
                painter,
            );
            return;
        }

        if let Some(image) = world.get::<ImageElement>(entity) {
            self.draw_image(
                world,
                image,
                world.get::<Mask>(entity),
                root_from_entity,
                painter,
            );
            return;
        }

        if let Some(text) = world.get::<TextElement>(entity) {
            self.draw_text(
                world,
                text,
                world.get::<Mask>(entity),
                root_from_entity,
                painter,
            );
        }
    }

    fn draw_page<S>(
        &mut self,
        world: &World,
        page: &Page,
        transform: Affine,
        painter: &mut Painter<'_, S>,
    ) where
        S: PaintSink + ?Sized,
    {
        painter
            .fill(page.rect(), page.background.fill())
            .transform(transform)
            .fill_rule(Fill::NonZero)
            .draw();

        match &page.background {
            PageBackground::Blank { .. } => {}
            PageBackground::DisplayList { items, .. } => {
                self.draw_background_items(world, items, transform, painter);
            }
            PageBackground::Image {
                image,
                transform: image_transform,
                ..
            } => self.draw_background_image(world, image, transform * *image_transform, painter),
            PageBackground::Procedural { source, .. } => {
                let items = source.layout(page.size);
                self.draw_background_items(world, &items, transform, painter);
            }
        }
    }

    fn draw_background_items<S>(
        &mut self,
        world: &World,
        items: &[PageBackgroundItem],
        transform: Affine,
        painter: &mut Painter<'_, S>,
    ) where
        S: PaintSink + ?Sized,
    {
        for item in items {
            match item {
                PageBackgroundItem::Line {
                    from,
                    to,
                    stroke,
                    brush,
                } => {
                    let mut path = BezPath::new();
                    path.move_to(*from);
                    path.line_to(*to);
                    painter
                        .stroke(&path, stroke, brush)
                        .transform(transform)
                        .draw();
                }
                PageBackgroundItem::Rect {
                    rect,
                    brush,
                    fill_rule,
                } => {
                    painter
                        .fill(*rect, brush)
                        .transform(transform)
                        .fill_rule(*fill_rule)
                        .draw();
                }
                PageBackgroundItem::Path { path, brush, style } => {
                    draw_styled_path(painter, path, style, brush, transform);
                }
                PageBackgroundItem::Image {
                    image,
                    transform: image_transform,
                } => {
                    self.draw_background_image(world, image, transform * *image_transform, painter)
                }
            }
        }
    }

    fn draw_background_image<S>(
        &self,
        world: &World,
        handle: &bevy_asset::Handle<CanvasImage>,
        transform: Affine,
        painter: &mut Painter<'_, S>,
    ) where
        S: PaintSink + ?Sized,
    {
        let Some(images) = world.get_resource::<Assets<CanvasImage>>() else {
            return;
        };
        let Some(image) = images.get(handle) else {
            return;
        };
        painter.draw_image(&image.data, transform);
    }

    fn draw_image<S>(
        &self,
        world: &World,
        image: &ImageElement,
        mask: Option<&Mask>,
        transform: Affine,
        painter: &mut Painter<'_, S>,
    ) where
        S: PaintSink + ?Sized,
    {
        if let Some(mask) = mask
            && !mask.segments.is_empty()
        {
            painter.with_group(GroupRef::new(), |painter| {
                self.draw_image_unmasked(world, image, transform, painter);
                draw_mask_segments(mask, transform, painter);
            });
            return;
        }

        self.draw_image_unmasked(world, image, transform, painter);
    }

    fn draw_image_unmasked<S>(
        &self,
        world: &World,
        image: &ImageElement,
        transform: Affine,
        painter: &mut Painter<'_, S>,
    ) where
        S: PaintSink + ?Sized,
    {
        let Some(images) = world.get_resource::<Assets<CanvasImage>>() else {
            return;
        };
        let Some(asset) = images.get(&image.image) else {
            return;
        };
        let natural = Size::new(asset.data.width as f64, asset.data.height as f64);
        if natural.width <= 0.0 || natural.height <= 0.0 {
            return;
        }
        let scale = Affine::scale_non_uniform(
            image.size.width / natural.width,
            image.size.height / natural.height,
        );
        painter.draw_image(&asset.data, transform * scale);
    }

    fn draw_text<S>(
        &self,
        world: &World,
        text: &TextElement,
        mask: Option<&Mask>,
        transform: Affine,
        painter: &mut Painter<'_, S>,
    ) where
        S: PaintSink + ?Sized,
    {
        if let Some(mask) = mask
            && !mask.segments.is_empty()
        {
            painter.with_group(GroupRef::new(), |painter| {
                self.draw_text_unmasked(world, text, transform, painter);
                draw_mask_segments(mask, transform, painter);
            });
            return;
        }

        self.draw_text_unmasked(world, text, transform, painter);
    }

    fn draw_text_unmasked<S>(
        &self,
        world: &World,
        text: &TextElement,
        transform: Affine,
        painter: &mut Painter<'_, S>,
    ) where
        S: PaintSink + ?Sized,
    {
        let Some(fonts) = world.get_resource::<Assets<CanvasFont>>() else {
            return;
        };
        let Some(font) = fonts.get(&text.font) else {
            return;
        };
        let glyphs = text
            .glyphs
            .iter()
            .map(|glyph| Glyph {
                id: glyph.id,
                x: glyph.x,
                y: glyph.y,
            })
            .collect::<Vec<_>>();

        painter
            .glyphs(&font.data, &text.brush)
            .transform(transform)
            .glyph_transform(text.glyph_transform)
            .font_size(text.font_size)
            .font_embolden(text.font_embolden)
            .hint(text.hint)
            .normalized_coords(&text.normalized_coords)
            .draw(&text.style, &glyphs);
    }

    fn draw_stroke<S>(
        &mut self,
        entity: Entity,
        stroke: &StrokeData,
        mask: Option<&Mask>,
        transform: Affine,
        painter: &mut Painter<'_, S>,
    ) where
        S: PaintSink + ?Sized,
    {
        let outline = self.cached_outline(entity, stroke);
        let path = path_from_outline(&outline.outline);
        if path.elements().is_empty() {
            return;
        }

        if let Some(mask) = mask
            && !mask.segments.is_empty()
        {
            painter.with_group(GroupRef::new(), |painter| {
                draw_stroke_path(stroke, &path, transform, painter);
                draw_mask_segments(mask, transform, painter);
            });
            return;
        }

        draw_stroke_path(stroke, &path, transform, painter);
    }

    fn draw_active_eraser<S>(
        &self,
        world: &World,
        sink_from_root: Affine,
        painter: &mut Painter<'_, S>,
    ) where
        S: PaintSink + ?Sized,
    {
        let Some(active) = world.get_resource::<ActiveEraser>() else {
            return;
        };
        let Some(config) = active.config() else {
            return;
        };
        if active.points().is_empty() {
            return;
        }

        let segments = preview_segments(active.points(), config);
        draw_preview_segments(&segments, sink_from_root, painter);
    }

    fn cached_outline(&mut self, entity: Entity, stroke: &StrokeData) -> &CachedStroke {
        let options = self.outline_options;
        self.strokes
            .entry(entity)
            .and_modify(|cached| cached.sync(stroke, options))
            .or_insert_with(|| CachedStroke::from_stroke(stroke, options))
    }
}

fn draw_stroke_path<S>(
    stroke: &StrokeData,
    path: &BezPath,
    transform: Affine,
    painter: &mut Painter<'_, S>,
) where
    S: PaintSink + ?Sized,
{
    painter
        .fill(path, stroke.brush.brush.paint.color)
        .transform(transform)
        .fill_rule(Fill::NonZero)
        .draw();
}

impl Default for CanvasImagingRenderer {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct CachedStroke {
    outline: SimpleStrokeOutline,
    tip: BrushTip,
}

impl CachedStroke {
    fn from_stroke(stroke: &StrokeData, options: StrokeOutlineOptions) -> Self {
        let tip = stroke.brush.brush.tip;
        Self {
            outline: outline_from_stroke(stroke, tip, options),
            tip,
        }
    }

    fn sync(&mut self, stroke: &StrokeData, options: StrokeOutlineOptions) {
        let tip = stroke.brush.brush.tip;
        if self.tip != tip {
            self.outline = outline_from_stroke(stroke, tip, options);
            self.tip = tip;
        } else {
            // Rendering can skip several modeler updates. Compare the complete
            // current path so the earliest unseen change is always invalidated.
            self.outline.sync_path(&stroke.points);
        }
    }
}

fn draw_styled_path<S>(
    painter: &mut Painter<'_, S>,
    path: &BezPath,
    style: &Style,
    brush: &Brush,
    transform: Affine,
) where
    S: PaintSink + ?Sized,
{
    match style {
        Style::Fill(fill) => painter
            .fill(path, brush)
            .transform(transform)
            .fill_rule(*fill)
            .draw(),
        Style::Stroke(stroke) => painter
            .stroke(path, stroke, brush)
            .transform(transform)
            .draw(),
    }
}

fn draw_mask_segments<S>(mask: &Mask, transform: Affine, painter: &mut Painter<'_, S>)
where
    S: PaintSink + ?Sized,
{
    let composite = Composite::new(BlendMode::from(Compose::DestOut), 1.0);
    for segment in &mask.segments {
        draw_mask_segment(*segment, transform, Color::WHITE, Some(composite), painter);
    }
}

fn draw_preview_segments<S>(
    segments: &[MaskSegment],
    transform: Affine,
    painter: &mut Painter<'_, S>,
) where
    S: PaintSink + ?Sized,
{
    let brush = Color::from_rgba8(44, 119, 230, 96);
    for segment in segments {
        draw_mask_segment(*segment, transform, brush, None, painter);
    }
}

fn draw_mask_segment<S>(
    segment: MaskSegment,
    transform: Affine,
    brush: Color,
    composite: Option<Composite>,
    painter: &mut Painter<'_, S>,
) where
    S: PaintSink + ?Sized,
{
    match segment.shape {
        EraserShape::Circle if segment.from.distance(segment.to) > f64::EPSILON => {
            let mut path = BezPath::new();
            path.move_to(segment.from);
            path.line_to(segment.to);
            let stroke = segment.circle_stroke();
            let mut draw = painter.stroke(&path, &stroke, brush).transform(transform);
            if let Some(composite) = composite {
                draw = draw.composite(composite);
            }
            draw.draw();
        }
        EraserShape::Circle | EraserShape::Square => {
            let path = segment.fill_path();
            let mut draw = painter
                .fill(&path, brush)
                .transform(transform)
                .fill_rule(Fill::NonZero);
            if let Some(composite) = composite {
                draw = draw.composite(composite);
            }
            draw.draw();
        }
    }
}

fn preview_segments(points: &[Point], config: EraserConfig) -> Vec<MaskSegment> {
    match points {
        [] => Vec::new(),
        [point] => vec![MaskSegment::new(
            *point,
            *point,
            config.shape,
            f64::from(config.size),
        )],
        _ => points
            .windows(2)
            .map(|points| {
                MaskSegment::new(points[0], points[1], config.shape, f64::from(config.size))
            })
            .collect(),
    }
}

fn outline_from_stroke(
    stroke: &StrokeData,
    tip: BrushTip,
    options: StrokeOutlineOptions,
) -> SimpleStrokeOutline {
    SimpleStrokeOutline::new_with_path(&stroke.points, tip, options)
}

fn path_from_outline(outline: &SimpleStrokeOutline) -> BezPath {
    BezPath::from_vec(outline.iter().collect())
}

#[cfg(test)]
mod tests {
    use bevy_ecs::query::With;
    use imaging::{
        Painter,
        kurbo::Size,
        peniko::{BlendMode, Compose},
        record::{Command, Draw, Geometry, Scene},
    };
    use quill_canvas::canvas::Canvas;
    use quill_stroke::input::Input;

    use super::*;

    #[test]
    fn active_stroke_render_then_update_does_not_panic() {
        let mut canvas = Canvas::new();
        canvas
            .input(quill_canvas::input::CanvasInput::PointerDown(Input::new(
                10.0, 10.0, 0.0,
            )))
            .unwrap();
        canvas
            .input(quill_canvas::input::CanvasInput::PointerMove(Input::new(
                20.0, 15.0, 1.0,
            )))
            .unwrap();

        let mut renderer = CanvasImagingRenderer::new();
        let mut first_scene = Scene::new();
        let mut first_painter = Painter::new(&mut first_scene);
        renderer.draw_canvas(&canvas, &mut first_painter, Size::new(200.0, 120.0));

        canvas
            .input(quill_canvas::input::CanvasInput::PointerMove(Input::new(
                30.0, 10.0, 2.0,
            )))
            .unwrap();
        let mut second_scene = Scene::new();
        let mut second_painter = Painter::new(&mut second_scene);
        renderer.draw_canvas(&canvas, &mut second_painter, Size::new(200.0, 120.0));

        assert!(second_scene.commands().iter().any(|command| {
            let Command::Draw(draw_id) = command else {
                return false;
            };
            matches!(
                second_scene.draw_op(*draw_id),
                Draw::Fill {
                    shape: Geometry::Path(path),
                    ..
                } if !path.elements().is_empty()
            )
        }));
    }

    #[test]
    fn renderer_applies_persistent_eraser_masks_inside_group() {
        let mut canvas = Canvas::new();
        canvas
            .input(quill_canvas::input::CanvasInput::PointerDown(
                Input::new(0.0, 0.0, 0.0).with_pressure(1.0),
            ))
            .unwrap();
        canvas
            .input(quill_canvas::input::CanvasInput::PointerUp(
                Input::new(10.0, 0.0, 1.0).with_pressure(1.0),
            ))
            .unwrap();
        // Pixel erasing now splits strokes structurally. Insert a persistent mask
        // explicitly to exercise the renderer's mask path independently of editing.
        let world = canvas.world_mut();
        let stroke = world
            .query_filtered::<Entity, With<StrokeData>>()
            .single(world)
            .unwrap();
        world.entity_mut(stroke).insert(Mask {
            segments: vec![MaskSegment::new(
                Point::new(5.0, -2.0),
                Point::new(5.0, 2.0),
                EraserShape::Circle,
                2.0,
            )],
        });

        let mut scene = Scene::new();
        let mut painter = Painter::new(&mut scene);
        CanvasImagingRenderer::new().draw_canvas(&canvas, &mut painter, Size::new(200.0, 120.0));

        assert!(
            scene
                .commands()
                .iter()
                .any(|command| matches!(command, Command::PushGroup(_)))
        );
        assert!(scene.commands().iter().any(|command| {
            let Command::Draw(draw_id) = command else {
                return false;
            };
            matches!(
                scene.draw_op(*draw_id),
                Draw::Stroke { composite, .. } | Draw::Fill { composite, .. }
                    if composite.blend == BlendMode::from(Compose::DestOut)
            )
        }));
    }
}
