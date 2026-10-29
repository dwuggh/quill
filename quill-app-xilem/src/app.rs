use quill_canvas::prelude::{
    EraserConfig, EraserMode, EraserShape, SelectionMode, SelectionOp, SelectionTool, Tool,
};
use quill_stroke::brush::{Brush, BrushTip, StrokePaint};
use tracing_subscriber::EnvFilter;
#[cfg(not(target_os = "android"))]
use tracing_subscriber::fmt;
use xilem::{
    Color, EventLoopBuilder, WidgetView, WindowOptions, Xilem,
    masonry::layout::AsUnit,
    view::{FlexExt as _, FlexSpacer, flex_col, flex_row, label, slider, text_button},
    winit::error::EventLoopError,
};

use crate::widget::{CanvasWidgetCommand, CanvasWidgetCommandKind, quill_canvas_widget};

/// Run the shared Quill UI with the host platform's configured event loop.
pub fn run(event_loop: EventLoopBuilder) -> Result<(), EventLoopError> {
    init_logging();

    Xilem::new_simple(
        AppState::default(),
        app_view,
        WindowOptions::new("Quill Xilem"),
    )
    .run_in(event_loop)
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    #[cfg(not(target_os = "android"))]
    let _ = fmt().with_env_filter(filter).try_init();
    #[cfg(target_os = "android")]
    {
        use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
        let layer = tracing_android::layer("quill")
            .expect("the static Android log tag contains no NUL bytes");
        let _ = tracing_subscriber::registry()
            .with(filter)
            .with(layer)
            .try_init();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolKind {
    Pen,
    Eraser,
    Select,
}

/// Application state mirrored into the custom canvas widget on rebuild.
struct AppState {
    active: ToolKind,
    brush: Brush,
    eraser: EraserConfig,
    selection: SelectionTool,
    command_id: u64,
    canvas_command: Option<CanvasWidgetCommand>,
}

impl AppState {
    fn active_tool(&self) -> Tool {
        match self.active {
            ToolKind::Pen => Tool::Stroke(self.brush),
            ToolKind::Eraser => Tool::Eraser(self.eraser),
            ToolKind::Select => Tool::Selection(self.selection),
        }
    }

    fn push_canvas_command(&mut self, kind: CanvasWidgetCommandKind) {
        self.command_id += 1;
        self.canvas_command = Some(CanvasWidgetCommand {
            id: self.command_id,
            kind,
        });
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            active: ToolKind::Pen,
            brush: Brush {
                tip: BrushTip::round(2.0),
                paint: StrokePaint {
                    color: stroke_color(0),
                    width: 2.0,
                    sharpness: 1.0,
                },
            },
            eraser: EraserConfig::new(EraserShape::Circle, 12.0),
            selection: SelectionTool {
                mode: SelectionMode::Item,
                hit_radius: 4.0,
                op: SelectionOp::Replace,
            },
            command_id: 0,
            canvas_command: None,
        }
    }
}

fn app_view(state: &mut AppState) -> impl WidgetView<AppState> + use<> {
    flex_row((
        tool_rail(state),
        flex_col((
            top_bar(state),
            quill_canvas_widget(state.active_tool(), state.canvas_command).flex(1.0),
        ))
        .flex(1.0),
    ))
}

fn tool_rail(state: &mut AppState) -> impl WidgetView<AppState> + use<> {
    flex_col((
        text_button(
            tool_label("Pen", ToolKind::Pen, state.active),
            |state: &mut AppState| {
                state.active = ToolKind::Pen;
            },
        ),
        text_button(
            tool_label("Eraser", ToolKind::Eraser, state.active),
            |state: &mut AppState| {
                state.active = ToolKind::Eraser;
            },
        ),
        text_button(
            tool_label("Select", ToolKind::Select, state.active),
            |state: &mut AppState| {
                state.active = ToolKind::Select;
            },
        ),
        FlexSpacer::Fixed(10.0.px()),
        text_button("Image", |_| {}).disabled(true),
        text_button("Text", |_| {}).disabled(true),
    ))
}

fn top_bar(state: &mut AppState) -> impl WidgetView<AppState> + use<> {
    let brush_radius = brush_radius(state.brush);
    let eraser_size = f64::from(state.eraser.size);

    let pen_controls = flex_row((
        label("Pen"),
        slider(1.0, 24.0, brush_radius, |state: &mut AppState, value| {
            state.brush.tip = BrushTip::round(value as f32);
            state.active = ToolKind::Pen;
        }),
        color_button("Black", 0),
        color_button("Red", 1),
        color_button("Blue", 2),
        color_button("Green", 3),
        FlexSpacer::Fixed(12.0.px()),
    ));

    let eraser_controls = flex_row((
        label("Eraser"),
        slider(4.0, 64.0, eraser_size, |state: &mut AppState, value| {
            state.eraser.size = value as f32;
            state.active = ToolKind::Eraser;
        })
        .flex(1.0),
        eraser_mode_button("Pixel", EraserMode::PixelWise, state.eraser.mode),
        eraser_mode_button("Item", EraserMode::ItemWise, state.eraser.mode),
        eraser_shape_button("Circle", EraserShape::Circle, state.eraser.shape),
        eraser_shape_button("Square", EraserShape::Square, state.eraser.shape),
        FlexSpacer::Fixed(12.0.px()),
    ));

    let selection_controls = flex_row((
        label("Select"),
        selection_button("Item", SelectionMode::Item),
        selection_button("Rect", SelectionMode::Rect),
        selection_button("Lasso", SelectionMode::Lasso),
        FlexSpacer::Fixed(12.0.px()),
    ));

    flex_row((
        pen_controls,
        eraser_controls.flex(1.0),
        selection_controls,
        text_button("Add Page", |state: &mut AppState| {
            state.push_canvas_command(CanvasWidgetCommandKind::AddPage);
        }),
    ))
}

fn tool_label(label: &'static str, tool: ToolKind, active: ToolKind) -> &'static str {
    match (label, tool == active) {
        ("Pen", true) => "[Pen]",
        ("Eraser", true) => "[Eraser]",
        ("Select", true) => "[Select]",
        _ => label,
    }
}

fn color_button(label_text: &'static str, color_index: i32) -> impl WidgetView<AppState> {
    text_button(label_text, move |state: &mut AppState| {
        state.brush.paint.color = stroke_color(color_index);
        state.active = ToolKind::Pen;
    })
}

fn selection_button(label_text: &'static str, mode: SelectionMode) -> impl WidgetView<AppState> {
    text_button(label_text, move |state: &mut AppState| {
        state.selection.mode = mode;
        state.active = ToolKind::Select;
    })
}

fn eraser_mode_button(
    label_text: &'static str,
    mode: EraserMode,
    current: EraserMode,
) -> impl WidgetView<AppState> {
    text_button(
        selected_label(label_text, mode == current),
        move |state: &mut AppState| {
            state.eraser.mode = mode;
            state.active = ToolKind::Eraser;
        },
    )
}

fn eraser_shape_button(
    label_text: &'static str,
    shape: EraserShape,
    current: EraserShape,
) -> impl WidgetView<AppState> {
    text_button(
        selected_label(label_text, shape == current),
        move |state: &mut AppState| {
            state.eraser.shape = shape;
            state.active = ToolKind::Eraser;
        },
    )
}

fn selected_label(label: &'static str, selected: bool) -> &'static str {
    match (label, selected) {
        ("Pixel", true) => "[Pixel]",
        ("Item", true) => "[Item]",
        ("Circle", true) => "[Circle]",
        ("Square", true) => "[Square]",
        _ => label,
    }
}

fn brush_radius(brush: Brush) -> f64 {
    match brush.tip {
        BrushTip::Round(tip) | BrushTip::FixedRound(tip) => f64::from(tip.radius.0),
    }
}

fn stroke_color(index: i32) -> Color {
    match index {
        1 => Color::from_rgb8(0xd1, 0x2f, 0x2f),
        2 => Color::from_rgb8(0x24, 0x74, 0xd6),
        3 => Color::from_rgb8(0x23, 0x80, 0x44),
        _ => Color::BLACK,
    }
}
