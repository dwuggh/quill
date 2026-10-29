use std::time::Instant;

use bevy_ecs::entity::Entity;
use quill_canvas::{
    canvas::Canvas,
    document::root_from_entity,
    error::CanvasError,
    prelude::{
        CanvasInput, CanvasTransform, Page, PageConfig, SelectionArea, SelectionAreaShape,
        SelectionState, Tool,
    },
};
use quill_render_imaging::CanvasImagingRenderer;
use quill_stroke::input::Input as StrokeInput;
use xilem::{
    Color, Pod, ViewCtx,
    core::{MessageCtx, MessageResult, Mut, View, ViewMarker},
    masonry::{
        accesskit::{Node, Role},
        core::{
            AccessCtx, AccessEvent, ChildrenIds, EventCtx, LayoutCtx, MeasureCtx, NoAction,
            PaintCtx, PointerButton, PointerButtonEvent, PointerEvent, PointerState, PointerUpdate,
            PropertiesMut, PropertiesRef, RegisterCtx, TextEvent, Widget, WidgetId, WidgetMut,
        },
        imaging::Painter,
        kurbo::{Affine, Axis, BezPath, Point, Rect, Size, Stroke},
        layout::{LenReq, Length},
    },
};

const DEFAULT_LENGTH: Length = Length::const_px(400.0);
const PAGE_GAP: f64 = 48.0;
const VIEWPORT_MARGIN: f64 = 32.0;

/// One-shot command sent from the Xilem controls into the canvas widget.
#[derive(Clone, Copy, Debug)]
pub struct CanvasWidgetCommand {
    /// Monotonic identity used so rebuilds apply a command only once.
    pub id: u64,
    /// Command payload.
    pub kind: CanvasWidgetCommandKind,
}

/// Commands understood by `QuillCanvasWidget`.
#[derive(Clone, Copy, Debug)]
pub enum CanvasWidgetCommandKind {
    /// Append a new default page below the current page stack.
    AddPage,
}

/// Create the Xilem view that hosts Quill's custom Masonry canvas widget.
pub fn quill_canvas_widget(tool: Tool, command: Option<CanvasWidgetCommand>) -> QuillCanvasView {
    QuillCanvasView { tool, command }
}

/// Xilem view wrapper for `QuillCanvasWidget`.
pub struct QuillCanvasView {
    tool: Tool,
    command: Option<CanvasWidgetCommand>,
}

impl ViewMarker for QuillCanvasView {}

impl<State, Action> View<State, Action, ViewCtx> for QuillCanvasView
where
    State: 'static,
    Action: 'static,
{
    type Element = Pod<QuillCanvasWidget>;
    type ViewState = ();

    fn build(&self, ctx: &mut ViewCtx, _: &mut State) -> (Self::Element, Self::ViewState) {
        (
            ctx.create_pod(QuillCanvasWidget::new(self.tool, self.command)),
            (),
        )
    }

    fn rebuild(
        &self,
        _prev: &Self,
        (): &mut Self::ViewState,
        _ctx: &mut ViewCtx,
        element: Mut<'_, Self::Element>,
        _state: &mut State,
    ) {
        QuillCanvasWidget::set_props(element, self.tool, self.command);
    }

    fn teardown(&self, (): &mut Self::ViewState, _: &mut ViewCtx, _: Mut<'_, Self::Element>) {}

    fn message(
        &self,
        (): &mut Self::ViewState,
        _message: &mut MessageCtx,
        _element: Mut<'_, Self::Element>,
        _app_state: &mut State,
    ) -> MessageResult<Action> {
        MessageResult::Stale
    }
}

/// Masonry widget that owns a Quill canvas scene and paints it through Imaging.
pub struct QuillCanvasWidget {
    /// Scene model updated by pointer input.
    canvas: Canvas,
    /// Backend-agnostic renderer that records canvas content into Masonry's painter.
    renderer: CanvasImagingRenderer,
    /// Tool last mirrored into the canvas world.
    tool: Tool,
    /// Page entities owned by this widget, in vertical document order.
    pages: Vec<Entity>,
    /// Last applied command id from the Xilem view.
    last_command_id: u64,
    /// Gesture timing base used for monotonic stroke timestamps.
    start: Instant,
    /// Current content-box size.
    size: Size,
    /// Last accepted pointer sample, used to finish a gesture on cancel.
    last_sample: Option<PointerSample>,
    /// Last canvas error produced while handling widget input.
    last_error: Option<CanvasError>,
}

impl QuillCanvasWidget {
    /// Create a widget with an empty canvas and the given active tool.
    pub fn new(tool: Tool, command: Option<CanvasWidgetCommand>) -> Self {
        let mut canvas = Canvas::new();
        let mut last_error = canvas.change_tool(tool).err();
        let mut pages = Vec::new();
        if let Err(error) = append_default_page(&mut canvas, &mut pages) {
            last_error = Some(error);
        }

        let mut widget = Self {
            canvas,
            renderer: CanvasImagingRenderer::new(),
            tool,
            pages,
            last_command_id: 0,
            start: Instant::now(),
            size: Size::ZERO,
            last_sample: None,
            last_error,
        };
        widget.apply_command(command);
        widget
    }

    /// Mirror the latest app-level tool configuration and command into the widget canvas.
    pub fn set_props(
        mut this: WidgetMut<'_, Self>,
        tool: Tool,
        command: Option<CanvasWidgetCommand>,
    ) {
        this.widget.tool = tool;
        if let Err(error) = this.widget.canvas.change_tool(tool) {
            this.widget.last_error = Some(error);
        }
        this.widget.apply_command(command);
        this.ctx.request_render();
    }

    fn send_input(&mut self, input: CanvasInput) {
        if let Err(error) = self.canvas.input(input) {
            self.last_error = Some(error);
        }
    }

    fn pointer_sample(&self, ctx: &EventCtx<'_>, state: &PointerState) -> PointerSample {
        let local = ctx.local_position(state.position);
        let root = self.viewport().root_from_widget(local);
        PointerSample::from_pointer(state, root, self.start.elapsed().as_secs_f64())
    }

    fn accept_sample(&mut self, sample: PointerSample, kind: PointerInputKind) {
        self.send_input(sample.canvas_input(kind));
        self.last_sample = Some(sample);
    }

    fn finish_from_last_sample(&mut self) {
        if let Some(sample) = self.last_sample.take() {
            self.send_input(sample.canvas_input(PointerInputKind::Up));
        }
    }

    fn apply_command(&mut self, command: Option<CanvasWidgetCommand>) {
        let Some(command) = command else {
            return;
        };
        if command.id <= self.last_command_id {
            return;
        }

        self.last_command_id = command.id;
        match command.kind {
            CanvasWidgetCommandKind::AddPage => {
                if let Err(error) = append_default_page(&mut self.canvas, &mut self.pages) {
                    self.last_error = Some(error);
                }
            }
        }
    }

    fn viewport(&self) -> Viewport {
        Viewport::fit(self.size, self.document_bounds())
    }

    fn document_bounds(&self) -> Option<Rect> {
        let world = self.canvas.world();
        self.pages
            .iter()
            .filter_map(|entity| {
                let page = world.get::<Page>(*entity)?;
                let root_from_page = root_from_entity(world, *entity)?;
                Some(root_from_page.transform_rect_bbox(page.rect()))
            })
            .reduce(|bounds, rect| bounds.union(rect))
    }

    fn draw_page_chrome(&self, painter: &mut Painter<'_>, viewport: Viewport) {
        let world = self.canvas.world();
        for entity in self.pages.iter().copied() {
            let Some(page) = world.get::<Page>(entity) else {
                continue;
            };
            let Some(root_from_page) = root_from_entity(world, entity) else {
                continue;
            };
            let page_from_shadow = Affine::translate((4.0 / viewport.scale, 5.0 / viewport.scale));
            painter
                .fill(
                    page.rect(),
                    Color::from_rgb8(0x9a, 0xa2, 0xad).with_alpha(0.25),
                )
                .transform(viewport.widget_from_root * root_from_page * page_from_shadow)
                .draw();
        }
    }

    fn draw_page_borders(&self, painter: &mut Painter<'_>, viewport: Viewport) {
        let world = self.canvas.world();
        let stroke = Stroke::new(1.0 / viewport.scale);
        for entity in self.pages.iter().copied() {
            let Some(page) = world.get::<Page>(entity) else {
                continue;
            };
            let Some(root_from_page) = root_from_entity(world, entity) else {
                continue;
            };
            painter
                .stroke(page.rect(), &stroke, Color::from_rgb8(0xb8, 0xbf, 0xca))
                .transform(viewport.widget_from_root * root_from_page)
                .draw();
        }
    }

    fn draw_selection_overlay(&self, painter: &mut Painter<'_>, viewport: Viewport) {
        let world = self.canvas.world();
        let Some(selection) = world.get_resource::<SelectionState>() else {
            return;
        };
        let Some(area_entity) = selection.area_entity() else {
            return;
        };
        let Some(area) = world.get::<SelectionArea>(area_entity) else {
            return;
        };
        let stroke = Stroke::new(1.5 / viewport.scale)
            .with_dashes(0.0, [6.0 / viewport.scale, 4.0 / viewport.scale]);

        match &area.shape {
            SelectionAreaShape::ItemRect(rect) | SelectionAreaShape::Rect(rect) => {
                painter
                    .stroke(*rect, &stroke, Color::from_rgb8(0x1d, 0x6f, 0xd6))
                    .transform(viewport.widget_from_root)
                    .draw();
            }
            SelectionAreaShape::Lasso(points) => {
                let Some(first) = points.first().copied() else {
                    return;
                };
                let mut path = BezPath::new();
                path.move_to(first);
                for point in points.iter().copied().skip(1) {
                    path.line_to(point);
                }
                painter
                    .stroke(&path, &stroke, Color::from_rgb8(0x1d, 0x6f, 0xd6))
                    .transform(viewport.widget_from_root)
                    .draw();
            }
        }
    }
}

impl Widget for QuillCanvasWidget {
    type Action = NoAction;

    fn on_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &PointerEvent,
    ) {
        if ctx.is_disabled() {
            return;
        }

        match event {
            PointerEvent::Down(PointerButtonEvent {
                button: Some(PointerButton::Primary) | None,
                state,
                ..
            }) => {
                ctx.request_focus();
                ctx.capture_pointer();
                let sample = self.pointer_sample(ctx, state);
                self.accept_sample(sample, PointerInputKind::Down);
                ctx.request_render();
                ctx.set_handled();
            }
            PointerEvent::Move(PointerUpdate { current, .. }) if ctx.is_active() => {
                let sample = self.pointer_sample(ctx, current);
                self.accept_sample(sample, PointerInputKind::Move);
                ctx.request_render();
                ctx.set_handled();
            }
            PointerEvent::Up(PointerButtonEvent {
                button: Some(PointerButton::Primary) | None,
                state,
                ..
            }) if ctx.is_active() => {
                let sample = self.pointer_sample(ctx, state);
                self.accept_sample(sample, PointerInputKind::Up);
                self.last_sample = None;
                ctx.request_render();
                ctx.set_handled();
            }
            PointerEvent::Cancel(_) => {
                self.finish_from_last_sample();
                ctx.request_render();
            }
            PointerEvent::Leave(_) if ctx.is_active() => {
                self.finish_from_last_sample();
                ctx.request_render();
            }
            _ => {}
        }
    }

    fn on_text_event(
        &mut self,
        _ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        _event: &TextEvent,
    ) {
    }

    fn on_access_event(
        &mut self,
        _ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        _event: &AccessEvent,
    ) {
    }

    fn register_children(&mut self, _ctx: &mut RegisterCtx<'_>) {}

    fn measure(
        &mut self,
        _ctx: &mut MeasureCtx<'_>,
        _props: &PropertiesRef<'_>,
        _axis: Axis,
        len_req: LenReq,
        _cross_length: Option<Length>,
    ) -> Length {
        match len_req {
            LenReq::FitContent(space) => space,
            _ => DEFAULT_LENGTH,
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx<'_>, _props: &PropertiesRef<'_>, size: Size) {
        self.size = size;
        ctx.set_clip_path(size.to_rect());
    }

    fn paint(
        &mut self,
        _ctx: &mut PaintCtx<'_>,
        _props: &PropertiesRef<'_>,
        painter: &mut Painter<'_>,
    ) {
        let viewport = self.viewport();
        painter
            .fill(self.size.to_rect(), Color::from_rgb8(0xe2, 0xe6, 0xeb))
            .draw();
        self.draw_page_chrome(painter, viewport);
        self.renderer
            .draw_canvas_with_transform(&self.canvas, painter, viewport.widget_from_root);
        self.draw_page_borders(painter, viewport);
        self.draw_selection_overlay(painter, viewport);
    }

    fn accessibility_role(&self) -> Role {
        Role::Canvas
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        node: &mut Node,
    ) {
        node.set_description("Quill drawing canvas");
    }

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::new()
    }

    fn make_trace_span(&self, widget_id: WidgetId) -> tracing::Span {
        tracing::trace_span!("QuillCanvasWidget", id = widget_id.trace())
    }

    fn get_debug_text(&self) -> Option<String> {
        self.last_error.as_ref().map(ToString::to_string)
    }
}

fn append_default_page(canvas: &mut Canvas, pages: &mut Vec<Entity>) -> Result<(), CanvasError> {
    let config = PageConfig::default();
    let page_y = pages.len() as f64 * (config.size.height + PAGE_GAP);
    let transform = CanvasTransform {
        affine: Affine::translate((0.0, page_y)),
    };
    let entity = canvas.add_page_to(None, config, transform)?;
    pages.push(entity);
    Ok(())
}

/// Maps document coordinates into the centered, fitted widget viewport.
#[derive(Clone, Copy, Debug)]
struct Viewport {
    widget_from_root: Affine,
    scale: f64,
}

impl Viewport {
    fn fit(widget_size: Size, document_bounds: Option<Rect>) -> Self {
        let Some(bounds) = document_bounds else {
            return Self::identity();
        };
        if widget_size.width <= 0.0
            || widget_size.height <= 0.0
            || bounds.width() <= 0.0
            || bounds.height() <= 0.0
        {
            return Self::identity();
        }

        let available_width = (widget_size.width - VIEWPORT_MARGIN * 2.0).max(1.0);
        let available_height = (widget_size.height - VIEWPORT_MARGIN * 2.0).max(1.0);
        let scale = (available_width / bounds.width())
            .min(available_height / bounds.height())
            .clamp(0.01, 1.0);
        let scaled_width = bounds.width() * scale;
        let scaled_height = bounds.height() * scale;
        let tx = (widget_size.width - scaled_width) * 0.5 - bounds.x0 * scale;
        let ty = (widget_size.height - scaled_height) * 0.5 - bounds.y0 * scale;

        Self {
            widget_from_root: Affine::translate((tx, ty)) * Affine::scale(scale),
            scale,
        }
    }

    fn identity() -> Self {
        Self {
            widget_from_root: Affine::IDENTITY,
            scale: 1.0,
        }
    }

    fn root_from_widget(self, point: Point) -> Point {
        self.widget_from_root.inverse() * point
    }
}

/// Pointer data converted from widget space into document coordinates.
#[derive(Clone, Copy, Debug)]
struct PointerSample {
    x: f64,
    y: f64,
    pressure: f32,
    tilt_x: f32,
    tilt_y: f32,
    timestamp: f64,
}

impl PointerSample {
    /// Translate pointer metadata while keeping coordinates in document space.
    fn from_pointer(state: &PointerState, position: Point, timestamp: f64) -> Self {
        Self {
            x: position.x,
            y: position.y,
            // The Winit adapter supplies native tablet pressure and handles devices
            // without pressure support; the canvas must preserve that distinction.
            pressure: state.pressure.clamp(0.0, 1.0),
            tilt_x: state.orientation.altitude,
            tilt_y: state.orientation.azimuth,
            timestamp,
        }
    }

    fn canvas_input(self, kind: PointerInputKind) -> CanvasInput {
        match kind {
            PointerInputKind::Move => CanvasInput::PointerMove(self.stroke_input()),
            PointerInputKind::Down => CanvasInput::PointerDown(self.stroke_input()),
            PointerInputKind::Up => CanvasInput::PointerUp(self.stroke_input()),
        }
    }

    fn stroke_input(self) -> StrokeInput {
        StrokeInput::new(self.x as f32, self.y as f32, self.timestamp)
            .with_pressure(self.pressure)
            .with_tilt(self.tilt_x, self.tilt_y)
    }
}

#[derive(Clone, Copy, Debug)]
enum PointerInputKind {
    Move,
    Down,
    Up,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_viewport_centers_document_and_round_trips_points() {
        let document = Rect::new(0.0, 0.0, 100.0, 200.0);
        let viewport = Viewport::fit(Size::new(300.0, 300.0), Some(document));
        let document_center = Point::new(50.0, 100.0);
        let widget_center = viewport.widget_from_root * document_center;

        assert!((widget_center.x - 150.0).abs() < 0.001);
        assert!((widget_center.y - 150.0).abs() < 0.001);
        let root = viewport.root_from_widget(widget_center);
        assert!((root.x - document_center.x).abs() < 0.001);
        assert!((root.y - document_center.y).abs() < 0.001);
    }

    #[test]
    fn widget_commands_apply_once() {
        let command = CanvasWidgetCommand {
            id: 1,
            kind: CanvasWidgetCommandKind::AddPage,
        };
        let mut widget = QuillCanvasWidget::new(Tool::default(), None);

        widget.apply_command(Some(command));
        widget.apply_command(Some(command));

        assert_eq!(widget.pages.len(), 2);
    }
}
