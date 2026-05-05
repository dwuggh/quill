use crate::input::PointerInput;
use crate::renderer::{GpuRenderer, RenderStatus};
use crate::scene::Scene;
use anyhow::{Context, Result};
use std::sync::Arc;
use tracing::{debug, error, info};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

pub fn run() -> Result<()> {
    let event_loop = EventLoop::new()?;
    let mut app = PlaygroundApp::new()?;
    info!("starting playground app");
    event_loop.run_app(&mut app)?;
    Ok(())
}

pub struct PlaygroundApp {
    window: Option<Arc<Window>>,
    window_id: Option<WindowId>,
    renderer: Option<GpuRenderer>,
    scene: Scene,
    input: PointerInput,
    needs_redraw: bool,
}

impl PlaygroundApp {
    fn new() -> Result<Self> {
        Ok(Self {
            window: None,
            window_id: None,
            renderer: None,
            scene: Scene::new()?,
            input: PointerInput::default(),
            needs_redraw: false,
        })
    }

    fn request_redraw(&mut self) {
        self.needs_redraw = true;
    }

    fn init_window_and_renderer(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        if self.window.is_some() {
            return Ok(());
        }

        let attributes = WindowAttributes::default()
            .with_title("Quill Playground")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0));
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .context("failed to create playground window")?,
        );
        let renderer = pollster::block_on(GpuRenderer::new(window.clone()))
            .context("failed to initialize playground renderer")?;
        self.window_id = Some(window.id());
        self.renderer = Some(renderer);
        self.window = Some(window);
        debug!("initialized playground window and renderer");
        self.request_redraw();
        Ok(())
    }

    fn handle_window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        event: WindowEvent,
    ) -> Result<()> {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(new_size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(new_size);
                }
                debug!(
                    width = new_size.width,
                    height = new_size.height,
                    "window resized"
                );
                self.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                let live_updated = self
                    .scene
                    .update_live_stroke()
                    .context("failed to update the live stroke during redraw")?;
                if live_updated {
                    debug!("coalesced queued cursor samples into a live stroke update");
                }
                self.scene.log_large_scene_if_needed();
                if let Some(renderer) = &mut self.renderer {
                    match renderer
                        .render(&self.scene)
                        .context("failed to render the current playground scene")?
                    {
                        RenderStatus::Rendered | RenderStatus::Skipped => {}
                        RenderStatus::NeedsReconfigure => {
                            debug!("renderer requested surface reconfiguration");
                            renderer.reconfigure();
                            self.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if self
                    .input
                    .cursor_moved(position, &mut self.scene)
                    .context("failed to process cursor movement")?
                {
                    self.request_redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if self
                    .input
                    .mouse_input(button, state, &mut self.scene)
                    .context("failed to process mouse button input")?
                {
                    self.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state.is_pressed() && !event.repeat {
                    match &event.logical_key {
                        Key::Named(NamedKey::Escape) => event_loop.exit(),
                        Key::Character(text) if text.eq_ignore_ascii_case("c") => {
                            self.scene.clear();
                            self.request_redraw();
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl ApplicationHandler for PlaygroundApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Err(error) = self.init_window_and_renderer(event_loop) {
            error!("failed to initialize playground: {error:#}");
            event_loop.exit();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if Some(window_id) != self.window_id {
            return;
        }
        if let Err(error) = self.handle_window_event(event_loop, event) {
            error!("playground error: {error:#}");
            event_loop.exit();
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if self.needs_redraw {
            self.needs_redraw = false;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }
}
