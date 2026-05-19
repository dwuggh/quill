use std::{cell::RefCell, rc::Rc, time::Instant};

use peniko::Color;
use quill_canvas::{
    message::CanvasMessage,
    prelude::{CanvasInput, EraserConfig, EraserShape, Tool},
};
use quill_render_bevy::{CanvasRenderer, Texture};
use quill_stroke::{
    brush::{Brush, BrushTip, StrokePaint},
    input::Input as StrokeInput,
};
use slint::{ComponentHandle, GraphicsAPI, RenderingState};
use tracing_subscriber::{EnvFilter, fmt};

slint::include_modules!();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logging();

    let app_state = Rc::new(RefCell::new(AppState::new()));
    let toolbar_state = Rc::new(RefCell::new(ToolbarState::default()));

    // Ask Slint for its WGPU 27 renderer so the canvas can render into textures
    // created from the same device.
    let mut wgpu_settings = slint::wgpu_27::WGPUSettings::default();
    wgpu_settings.device_required_limits = slint::wgpu_27::wgpu::Limits::default()
        .using_resolution(slint::wgpu_27::wgpu::Limits::downlevel_defaults());

    // The custom handler observes raw Winit pointer/tablet events before Slint
    // consumes them for normal UI interaction.
    slint::BackendSelector::new()
        .require_wgpu_27(slint::wgpu_27::WGPUConfiguration::Automatic(wgpu_settings))
        .with_winit_custom_application_handler(WinitPointerForwarder {
            state: app_state.clone(),
        })
        .select()?;

    let app_window = AppWindow::new()?;
    let app_weak = app_window.as_weak();
    let app_state_for_rendering = app_state.clone();

    app_window
        .window()
        .set_rendering_notifier(move |state, graphics_api| match state {
            RenderingState::RenderingSetup => {
                // Slint owns the WGPU device. CanvasRenderer wraps these handles
                // and starts the offscreen Bevy renderer.
                let GraphicsAPI::WGPU27 {
                    instance,
                    device,
                    queue,
                    ..
                } = graphics_api
                else {
                    return;
                };

                let renderer =
                    CanvasRenderer::init(instance.clone(), device.clone(), queue.clone());
                app_state_for_rendering.borrow_mut().renderer = Some(renderer);
            }
            RenderingState::BeforeRendering => {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };

                let width = app.get_requested_texture_width().round().max(1.0) as u32;
                let height = app.get_requested_texture_height().round().max(1.0) as u32;
                let mut state = app_state_for_rendering.borrow_mut();
                let resized = state.texture_width != width || state.texture_height != height;
                if resized {
                    state.texture_width = width;
                    state.texture_height = height;
                    state.awaiting_canvas_frame = true;
                }

                let Some(renderer) = state.renderer.as_ref() else {
                    return;
                };
                if !state.awaiting_canvas_frame {
                    return;
                }

                let free_targets_tx = renderer.free_targets_tx();

                // Slint and Bevy share one WGPU queue. Render synchronously from
                // Slint's callback so Bevy finishes submitting before Slint draws.
                let Some(texture) = renderer.render_frame(width, height) else {
                    state.awaiting_canvas_frame = false;
                    return;
                };

                // Slint's Image conversion takes a raw WGPU texture clone. Keep
                // the Bevy Texture wrapper only if conversion fails, so it can be
                // returned to the renderer.
                let previous_texture = app.get_texture().to_wgpu_27_texture();
                let raw = (*texture).clone();
                match slint::Image::try_from(raw) {
                    Ok(image) => {
                        app.set_texture(image);
                        state.awaiting_canvas_frame = false;
                        if let Some(raw) = previous_texture
                            && let Some(stale) = state.pending_release.replace(Texture::from(raw))
                        {
                            let _ = free_targets_tx.try_send(stale);
                        }
                    }
                    Err(_) => {
                        state.awaiting_canvas_frame = false;
                        let _ = free_targets_tx.try_send(texture);
                    }
                }
            }
            RenderingState::AfterRendering => {
                let mut state = app_state_for_rendering.borrow_mut();
                let Some(texture) = state.pending_release.take() else {
                    return;
                };
                let Some(renderer) = state.renderer.as_ref() else {
                    return;
                };

                let free_targets_tx = renderer.free_targets_tx();
                let _ = slint::spawn_local(async move {
                    free_targets_tx.send(texture).await.ok();
                });
            }
            RenderingState::RenderingTeardown => {
                // Shut down the render thread while the WGPU handles are still valid.
                if let Some(renderer) = app_state_for_rendering.borrow_mut().renderer.take() {
                    renderer.shutdown();
                }
            }
            _ => {}
        })?;

    let app_state_for_radius = app_state.clone();
    let toolbar_for_radius = toolbar_state.clone();
    let app_weak_for_radius = app_window.as_weak();
    app_window.on_round_tip_radius_changed(move |radius| {
        let tool = {
            let mut toolbar = toolbar_for_radius.borrow_mut();
            toolbar.brush.tip = BrushTip::round(radius.max(0.1));
            toolbar.active_tool()
        };
        send_tool(&app_state_for_radius, tool);
        if let Some(app) = app_weak_for_radius.upgrade() {
            app.window().request_redraw();
        }
    });

    let app_state_for_stroke_color = app_state.clone();
    let toolbar_for_stroke_color = toolbar_state.clone();
    let app_weak_for_stroke_color = app_window.as_weak();
    app_window.on_stroke_color_changed(move |color| {
        let tool = {
            let mut toolbar = toolbar_for_stroke_color.borrow_mut();
            let alpha = toolbar.brush.paint.color.components[3];
            toolbar.brush.paint.color = stroke_color(color).with_alpha(alpha);
            toolbar.active_tool()
        };
        send_tool(&app_state_for_stroke_color, tool);
        if let Some(app) = app_weak_for_stroke_color.upgrade() {
            app.window().request_redraw();
        }
    });

    let app_state_for_stroke_alpha = app_state.clone();
    let toolbar_for_stroke_alpha = toolbar_state.clone();
    let app_weak_for_stroke_alpha = app_window.as_weak();
    app_window.on_stroke_alpha_changed(move |alpha| {
        let tool = {
            let mut toolbar = toolbar_for_stroke_alpha.borrow_mut();
            toolbar.brush.paint.color = toolbar.brush.paint.color.with_alpha(alpha.clamp(0.0, 1.0));
            toolbar.active_tool()
        };
        send_tool(&app_state_for_stroke_alpha, tool);
        if let Some(app) = app_weak_for_stroke_alpha.upgrade() {
            app.window().request_redraw();
        }
    });

    let app_state_for_tool = app_state.clone();
    let toolbar_for_tool = toolbar_state.clone();
    let app_weak_for_tool = app_window.as_weak();
    app_window.on_active_tool_changed(move |tool| {
        let tool = {
            let mut toolbar = toolbar_for_tool.borrow_mut();
            toolbar.active = match tool {
                1 => ToolKind::Eraser,
                _ => ToolKind::Pen,
            };
            toolbar.active_tool()
        };
        send_tool(&app_state_for_tool, tool);
        if let Some(app) = app_weak_for_tool.upgrade() {
            app.window().request_redraw();
        }
    });

    let app_state_for_eraser_size = app_state.clone();
    let toolbar_for_eraser_size = toolbar_state.clone();
    let app_weak_for_eraser_size = app_window.as_weak();
    app_window.on_eraser_size_changed(move |size| {
        let tool = {
            let mut toolbar = toolbar_for_eraser_size.borrow_mut();
            toolbar.eraser.size = size.max(0.1);
            toolbar.active_tool()
        };
        send_tool(&app_state_for_eraser_size, tool);
        if let Some(app) = app_weak_for_eraser_size.upgrade() {
            app.window().request_redraw();
        }
    });

    let app_state_for_eraser_shape = app_state.clone();
    let toolbar_for_eraser_shape = toolbar_state.clone();
    let app_weak_for_eraser_shape = app_window.as_weak();
    app_window.on_eraser_shape_changed(move |shape| {
        let tool = {
            let mut toolbar = toolbar_for_eraser_shape.borrow_mut();
            toolbar.eraser.shape = match shape {
                1 => EraserShape::Square,
                _ => EraserShape::Circle,
            };
            toolbar.active_tool()
        };
        send_tool(&app_state_for_eraser_shape, tool);
        if let Some(app) = app_weak_for_eraser_shape.upgrade() {
            app.window().request_redraw();
        }
    });

    app_window.run()?;

    Ok(())
}

/// Enable application logs, honoring `RUST_LOG` when the caller provides it.
fn init_logging() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("quill_app=info,quill_canvas=info,quill_stroke=info"));
    let _ = fmt().with_env_filter(filter).try_init();
}

/// Shared UI-thread state for the Slint host and custom Winit handler.
struct AppState {
    /// Offscreen canvas renderer created after Slint exposes WGPU handles.
    renderer: Option<CanvasRenderer>,
    /// Current requested texture width in logical canvas pixels.
    texture_width: u32,
    /// Current requested texture height in logical canvas pixels.
    texture_height: u32,
    /// Whether Slint should keep polling until the renderer publishes a frame.
    awaiting_canvas_frame: bool,
    /// Texture replaced in `BeforeRendering`, released after Slint finishes drawing the frame.
    pending_release: Option<Texture>,
    /// Monotonic timestamp source used for stroke input samples.
    start: Instant,
}

impl AppState {
    fn new() -> Self {
        Self {
            renderer: None,
            texture_width: 640,
            texture_height: 480,
            awaiting_canvas_frame: true,
            pending_release: None,
            start: Instant::now(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolKind {
    Pen,
    Eraser,
}

/// Toolbar-owned tool configuration mirrored to the canvas when controls change.
struct ToolbarState {
    /// Which tool the canvas should apply to future pointer input.
    active: ToolKind,
    /// Stroke brush configured by the round-tip controls.
    brush: Brush,
    /// Eraser shape and size configured by the toolbar.
    eraser: EraserConfig,
}

impl ToolbarState {
    /// Build the canvas tool value represented by the current toolbar controls.
    fn active_tool(&self) -> Tool {
        match self.active {
            ToolKind::Pen => Tool::Stroke(self.brush),
            ToolKind::Eraser => Tool::Eraser(self.eraser),
        }
    }
}

impl Default for ToolbarState {
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
            eraser: EraserConfig::default(),
        }
    }
}

/// Palette used by the stroke toolbar swatches.
fn stroke_color(index: i32) -> Color {
    match index {
        1 => Color::from_rgb8(0xd1, 0x2f, 0x2f),
        2 => Color::from_rgb8(0x24, 0x74, 0xd6),
        3 => Color::from_rgb8(0x23, 0x80, 0x44),
        _ => Color::BLACK,
    }
}

/// Send an updated tool selection to the renderer if it has already started.
fn send_tool(state: &Rc<RefCell<AppState>>, tool: Tool) {
    let mut state = state.borrow_mut();
    if let Some(renderer) = state.renderer.as_ref() {
        renderer.send(CanvasMessage::SelectTool(tool));
        state.awaiting_canvas_frame = true;
    }
}

/// Winit hook that forwards pointer and tablet events to the canvas renderer.
struct WinitPointerForwarder {
    /// Shared UI-thread state used to find the renderer and current canvas size.
    state: Rc<RefCell<AppState>>,
}

impl slint::winit_031::CustomApplicationHandler for WinitPointerForwarder {
    fn window_event(
        &mut self,
        _event_loop: &dyn slint::winit_031::winit::event_loop::ActiveEventLoop,
        _window_id: slint::winit_031::winit::window::WindowId,
        winit_window: Option<&dyn slint::winit_031::winit::window::Window>,
        slint_window: Option<&slint::Window>,
        event: &slint::winit_031::winit::event::WindowEvent,
    ) -> slint::winit_031::EventResult {
        let Some(event) = host_pointer_event(winit_window, event) else {
            return slint::winit_031::EventResult::Propagate;
        };

        let mut state = self.state.borrow_mut();
        if let Some(renderer) = state.renderer.as_ref() {
            // Convert host coordinates into canvas coordinates before crossing
            // into the canvas crate, so the canvas API stays UI-toolkit agnostic.
            let input = canvas_input(
                event,
                state.texture_width,
                state.texture_height,
                state.start.elapsed().as_secs_f64(),
            );
            renderer.send(CanvasMessage::Input(input));
            state.awaiting_canvas_frame = true;
        }
        if let Some(window) = slint_window {
            window.request_redraw();
        }

        slint::winit_031::EventResult::Propagate
    }
}

/// Toolkit-neutral pointer event used before conversion into `CanvasInput`.
#[derive(Clone, Copy, Debug)]
enum HostPointerEvent {
    Enter {
        x: f64,
        y: f64,
    },
    Leave,
    Move {
        x: f64,
        y: f64,
        pressure: f32,
        tilt_x: f32,
        tilt_y: f32,
    },
    Down {
        x: f64,
        y: f64,
        pressure: f32,
        tilt_x: f32,
        tilt_y: f32,
    },
    Up {
        x: f64,
        y: f64,
        pressure: f32,
        tilt_x: f32,
        tilt_y: f32,
    },
}

/// Translate Winit 0.31 pointer/tablet events into host events.
fn host_pointer_event(
    window: Option<&dyn slint::winit_031::winit::window::Window>,
    event: &slint::winit_031::winit::event::WindowEvent,
) -> Option<HostPointerEvent> {
    use slint::winit_031::winit::event::{ElementState, PointerKind, WindowEvent};

    let scale = window.map_or(1.0, |window| window.scale_factor());

    match event {
        WindowEvent::PointerEntered {
            position, primary, ..
        } if *primary => {
            let position = position.to_logical::<f64>(scale);
            Some(HostPointerEvent::Enter {
                x: position.x,
                y: position.y,
            })
        }
        WindowEvent::PointerMoved {
            position,
            primary,
            source,
            ..
        } if *primary => {
            let position = position.to_logical::<f64>(scale);
            let (pressure, tilt) = pressure_and_tilt_from_source(source);
            Some(HostPointerEvent::Move {
                x: position.x,
                y: position.y,
                pressure,
                tilt_x: tilt.0,
                tilt_y: tilt.1,
            })
        }
        WindowEvent::PointerButton {
            state,
            position,
            primary,
            button,
            ..
        } if *primary && accepts_button_source(button) => {
            let position = position.to_logical::<f64>(scale);
            let (pressure, tilt) = pressure_and_tilt_from_button(button);
            let event = match state {
                ElementState::Pressed => HostPointerEvent::Down {
                    x: position.x,
                    y: position.y,
                    pressure,
                    tilt_x: tilt.0,
                    tilt_y: tilt.1,
                },
                ElementState::Released => HostPointerEvent::Up {
                    x: position.x,
                    y: position.y,
                    pressure,
                    tilt_x: tilt.0,
                    tilt_y: tilt.1,
                },
            };
            Some(event)
        }
        WindowEvent::PointerLeft { primary, kind, .. }
            if *primary || matches!(kind, PointerKind::Touch(_)) =>
        {
            Some(HostPointerEvent::Leave)
        }
        _ => None,
    }
}

/// Decide whether a pointer button should draw or erase on the canvas.
fn accepts_button_source(button: &slint::winit_031::winit::event::ButtonSource) -> bool {
    use slint::winit_031::winit::event::{ButtonSource, MouseButton};

    matches!(
        button,
        ButtonSource::Mouse(MouseButton::Left)
            | ButtonSource::Touch { .. }
            | ButtonSource::TabletTool { .. }
    )
}

/// Extract pressure and tilt for move events.
fn pressure_and_tilt_from_source(
    source: &slint::winit_031::winit::event::PointerSource,
) -> (f32, (f32, f32)) {
    use slint::winit_031::winit::event::PointerSource;

    match source {
        PointerSource::Touch { force, .. } => (normalized_force(force.as_ref(), None), (0.0, 0.0)),
        PointerSource::TabletTool { data, .. } => pressure_and_tilt_from_tablet(data),
        PointerSource::Mouse | PointerSource::Unknown => (1.0, (0.0, 0.0)),
    }
}

/// Extract pressure and tilt for button press/release events.
fn pressure_and_tilt_from_button(
    button: &slint::winit_031::winit::event::ButtonSource,
) -> (f32, (f32, f32)) {
    use slint::winit_031::winit::event::ButtonSource;

    match button {
        ButtonSource::Touch { force, .. } => (normalized_force(force.as_ref(), None), (0.0, 0.0)),
        ButtonSource::TabletTool { data, .. } => pressure_and_tilt_from_tablet(data),
        ButtonSource::Mouse(_) | ButtonSource::Unknown(_) => (1.0, (0.0, 0.0)),
    }
}

/// Extract normalized pressure and tilt from tablet metadata.
fn pressure_and_tilt_from_tablet(
    data: &slint::winit_031::winit::event::TabletToolData,
) -> (f32, (f32, f32)) {
    let pressure = normalized_force(data.force.as_ref(), data.clone().angle());
    let tilt = data
        .clone()
        .tilt()
        .map_or((0.0, 0.0), |tilt| (tilt.x as f32, tilt.y as f32));

    (pressure, tilt)
}

/// Convert optional touch/tablet force into the pressure range used by strokes.
fn normalized_force(
    force: Option<&slint::winit_031::winit::event::Force>,
    angle: Option<slint::winit_031::winit::event::TabletToolAngle>,
) -> f32 {
    force
        .map_or(1.0, |force| force.normalized(angle) as f32)
        .clamp(0.0, 1.0)
}

/// Convert a host pointer event into the canvas input API.
fn canvas_input(event: HostPointerEvent, width: u32, height: u32, timestamp: f64) -> CanvasInput {
    match event {
        HostPointerEvent::Enter { x, y } => {
            CanvasInput::PointerEnter(canvas_point(x, y, width, height))
        }
        HostPointerEvent::Leave => CanvasInput::PointerLeave,
        HostPointerEvent::Move {
            x,
            y,
            pressure,
            tilt_x,
            tilt_y,
        } => CanvasInput::PointerMove(stroke_input(
            (x, y),
            pressure,
            (tilt_x, tilt_y),
            timestamp,
            (width, height),
        )),
        HostPointerEvent::Down {
            x,
            y,
            pressure,
            tilt_x,
            tilt_y,
        } => CanvasInput::PointerDown(stroke_input(
            (x, y),
            pressure,
            (tilt_x, tilt_y),
            timestamp,
            (width, height),
        )),
        HostPointerEvent::Up {
            x,
            y,
            pressure,
            tilt_x,
            tilt_y,
        } => CanvasInput::PointerUp(stroke_input(
            (x, y),
            pressure,
            (tilt_x, tilt_y),
            timestamp,
            (width, height),
        )),
    }
}

/// Build one stroke input sample with pressure and tilt attached.
fn stroke_input(
    position: (f64, f64),
    pressure: f32,
    tilt: (f32, f32),
    timestamp: f64,
    size: (u32, u32),
) -> StrokeInput {
    let (x, y) = position;
    let (tilt_x, tilt_y) = tilt;
    let (width, height) = size;

    let [x, y] = canvas_point(x, y, width, height);
    StrokeInput::new(x, y, timestamp)
        .with_pressure(pressure)
        .with_tilt(tilt_x, tilt_y)
}

/// Convert Slint logical coordinates into canvas-centered coordinates.
fn canvas_point(x: f64, y: f64, width: u32, height: u32) -> [f32; 2] {
    let width = width as f32;
    let height = height as f32;
    [x as f32 - width * 0.5, height * 0.5 - y as f32]
}
