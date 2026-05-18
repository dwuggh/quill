//! High-level canvas facade and offscreen renderer.
//!
//! `Canvas` is a small synchronous facade around an internal Bevy app.
//! `CanvasRenderer` is the WGPU middleware boundary used by host UI crates.

use std::{sync::Arc, thread::JoinHandle};

use bevy::{
    app::App,
    prelude::{Commands, Entity, MessageReader, ResMut, Resource},
    render::{
        render_resource::Texture,
        renderer::{
            RenderAdapter, RenderAdapterInfo, RenderDevice, RenderInstance, RenderQueue,
            WgpuWrapper,
        },
    },
};
use bevy_ecs::message::Messages;
use futures_lite::future;

use crate::{
    element::{
        Tool,
        page::{Page, PageConfig},
    },
    error::CanvasError,
    input::{CanvasCommand, CanvasInput},
    message::CanvasMessage,
    plugin::QuillPlugin,
    render::{self, OffscreenRenderConfig, QuillRenderPlugin},
};

const DEFAULT_RENDER_WIDTH: u32 = 640;
const DEFAULT_RENDER_HEIGHT: u32 = 480;

/// Global canvas mode resource. This is a global state singlet.
///
/// For now the mode is just the current tool. Brush style lives inside
/// `Tool::Stroke`, so a pointer-down event can snapshot the whole tool state
/// onto the new stroke entity.
#[derive(Resource, Clone, Copy, Debug)]
pub struct CanvasMode {
    pub tool: Tool,
}

impl CanvasMode {
    /// Create a mode resource with the given active tool.
    pub fn new(tool: Tool) -> Self {
        Self { tool }
    }
}

/// The abstract drawable canvas.
pub struct Canvas {
    /// Private Bevy app that owns scene resources, ECS systems, and messages.
    app: App,
    /// Renderer-thread state, present only for canvases created by `CanvasRenderer`.
    render_state: Option<CanvasRenderState>,
}

#[derive(Resource, Default)]
pub struct CanvasOrder {
    pub strokes: Vec<Entity>,
}

impl Canvas {
    /// Create an empty canvas with default tool state and input modeler.
    pub fn new() -> Self {
        let mut app = App::new();
        app.add_plugins(QuillPlugin);

        Self {
            app,
            render_state: None,
        }
    }

    /// Create a canvas for the renderer thread with offscreen Bevy rendering enabled.
    pub(crate) fn with_offscreen_render(
        render_config: OffscreenRenderConfig,
        texture_device: wgpu::Device,
    ) -> Self {
        let mut app = App::new();
        app.insert_resource(render_config);
        app.add_plugins(QuillPlugin);
        app.add_plugins(QuillRenderPlugin);
        app.finish();
        app.cleanup();

        Self {
            app,
            render_state: Some(CanvasRenderState::new(
                texture_device,
                RenderTargetSize::default(),
            )),
        }
    }

    /// Feed one pointer/stylus input message into the canvas.
    ///
    /// The message is handled immediately by the canvas update schedule.
    pub fn input(&mut self, input: CanvasInput) -> Result<(), CanvasError> {
        self.write_input(input);
        self.run_update()
    }

    /// Change the active canvas tool used by future input messages.
    pub fn change_tool(&mut self, tool: Tool) -> Result<(), CanvasError> {
        self.command(CanvasCommand::SetTool(tool))
    }

    /// Add a page entity to the canvas world.
    pub fn add_page(&mut self, config: PageConfig) -> Result<Entity, CanvasError> {
        self.write_command(CanvasCommand::AddPage(config));
        self.run_update()?;
        self.app
            .world()
            .resource::<LastAddedPage>()
            .0
            .ok_or(CanvasError::PageWasNotCreated)
    }

    fn command(&mut self, command: CanvasCommand) -> Result<(), CanvasError> {
        self.write_command(command);
        self.run_update()
    }

    fn write_command(&mut self, command: CanvasCommand) {
        self.app
            .world_mut()
            .resource_mut::<Messages<CanvasCommand>>()
            .write(command);
    }

    fn write_input(&mut self, input: CanvasInput) {
        self.app
            .world_mut()
            .resource_mut::<Messages<CanvasInput>>()
            .write(input);
    }

    fn transfer_canvas_message(&mut self, msg: CanvasMessage) -> bool {
        match msg {
            CanvasMessage::SelectTool(tool) => {
                self.write_command(CanvasCommand::SetTool(tool));
                self.render_state_mut().mark_dirty();
            }
            CanvasMessage::Input(input) => {
                self.write_input(input);
                self.render_state_mut().mark_dirty();
            }
            CanvasMessage::Shutdown => return true,
        }

        false
    }

    /// Bind the next offscreen texture as the current Bevy camera target.
    fn install_render_target(&mut self, target: Texture) {
        let texture_view_id = self.render_state_mut().next_texture_view_id();
        render::install_render_target(&mut self.app, target, texture_view_id);
    }

    /// Advance the renderer-thread Bevy app once.
    fn update_for_render(&mut self) {
        self.app.update();
    }

    /// Apply one renderer event and report whether shutdown was requested.
    fn apply_renderer_event(&mut self, event: RendererEvent) -> bool {
        match event {
            RendererEvent::Message(msg) => self.transfer_canvas_message(msg),
            RendererEvent::ReleasedTarget(target) => {
                self.render_state_mut().accept_released_target(target);
                false
            }
            RendererEvent::RenderFrame(_) => false,
        }
    }

    /// Drain queued state updates before rendering a requested frame.
    fn drain_pending_state_events(&mut self, channels: &RendererThreadChannels) -> bool {
        while let Some(event) = channels.try_next_state_event() {
            if self.apply_renderer_event(event) {
                return true;
            }
        }

        false
    }

    /// Render a frame into a host-visible texture when canvas state is dirty.
    fn render_requested_frame(
        &mut self,
        channels: &RendererThreadChannels,
        requested_size: RenderTargetSize,
    ) -> Option<Texture> {
        let target = {
            let render_state = self.render_state_mut();
            render_state.drain_ready_frames(&channels.ready_frames_rx);
            render_state.set_requested_size(requested_size);

            if !render_state.dirty {
                return None;
            }

            render_state.take_matching_target()
        };

        // Install the chosen texture as the active camera target before Bevy
        // updates, so this frame renders directly into the host-visible image.
        self.install_render_target(target);
        self.update_for_render();

        let render_state = self.render_state_mut();
        render_state.dirty = false;
        render_state.take_latest_ready_frame(&channels.ready_frames_rx)
    }

    fn run_update(&mut self) -> Result<(), CanvasError> {
        self.app.update();
        self.take_errors()
    }

    fn take_errors(&mut self) -> Result<(), CanvasError> {
        let mut errors = self.app.world_mut().resource_mut::<CanvasErrors>();
        match errors.0.pop() {
            Some(error) => {
                errors.0.clear();
                Err(error)
            }
            None => Ok(()),
        }
    }

    fn render_state_mut(&mut self) -> &mut CanvasRenderState {
        self.render_state
            .as_mut()
            .expect("offscreen renderer method called on a non-rendering Canvas")
    }
}

impl Default for Canvas {
    fn default() -> Self {
        Self::new()
    }
}

/// Pixel size for an offscreen render target.
#[derive(Clone, Copy)]
struct RenderTargetSize {
    /// Target width in physical pixels.
    width: u32,
    /// Target height in physical pixels.
    height: u32,
}

impl RenderTargetSize {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width: width.max(1),
            height: height.max(1),
        }
    }

    fn matches_texture(self, target: &Texture) -> bool {
        target.width() == self.width && target.height() == self.height
    }
}

impl Default for RenderTargetSize {
    fn default() -> Self {
        Self::new(DEFAULT_RENDER_WIDTH, DEFAULT_RENDER_HEIGHT)
    }
}

/// Texture pool and retained-mode invalidation state for an offscreen canvas.
struct CanvasRenderState {
    /// Device used to create host-visible Bevy render targets.
    texture_device: wgpu::Device,
    /// Current size requested by the host render callback.
    target_size: RenderTargetSize,
    /// Whether the canvas scene needs another Bevy render pass.
    dirty: bool,
    /// Render targets that are not currently displayed by the host.
    free_targets: Vec<Texture>,
    /// Monotonic ID source for Bevy manual texture view handles.
    next_texture_view_id: u32,
}

impl CanvasRenderState {
    fn new(texture_device: wgpu::Device, target_size: RenderTargetSize) -> Self {
        // Three render targets keep one texture displayed by Slint, one available
        // for Bevy, and one in flight between the threads.
        let free_targets = vec![
            render::create_target(
                &texture_device,
                "quill-render-target-0",
                target_size.width,
                target_size.height,
            ),
            render::create_target(
                &texture_device,
                "quill-render-target-1",
                target_size.width,
                target_size.height,
            ),
            render::create_target(
                &texture_device,
                "quill-render-target-2",
                target_size.width,
                target_size.height,
            ),
        ];

        Self {
            texture_device,
            target_size,
            dirty: true,
            free_targets,
            next_texture_view_id: 0,
        }
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn set_requested_size(&mut self, requested_size: RenderTargetSize) {
        if self.target_size.width != requested_size.width
            || self.target_size.height != requested_size.height
        {
            self.target_size = requested_size;
            self.mark_dirty();
        }
    }

    fn accept_released_target(&mut self, target: Texture) {
        if self.target_size.matches_texture(&target) {
            self.free_targets.push(target);
        }
    }

    fn drain_ready_frames(&mut self, ready_frames_rx: &async_channel::Receiver<Texture>) {
        while let Ok(frame) = ready_frames_rx.try_recv() {
            self.free_targets.push(frame);
        }
    }

    fn take_latest_ready_frame(
        &mut self,
        ready_frames_rx: &async_channel::Receiver<Texture>,
    ) -> Option<Texture> {
        let mut newest = None;

        while let Ok(frame) = ready_frames_rx.try_recv() {
            if let Some(stale) = newest.replace(frame) {
                self.free_targets.push(stale);
            }
        }

        newest
    }

    fn take_matching_target(&mut self) -> Texture {
        while let Some(target) = self.free_targets.pop() {
            if self.target_size.matches_texture(&target) {
                return target;
            }
        }

        render::create_target(
            &self.texture_device,
            "quill-resized-render-target",
            self.target_size.width,
            self.target_size.height,
        )
    }

    fn next_texture_view_id(&mut self) -> u32 {
        let texture_view_id = self.next_texture_view_id;
        self.next_texture_view_id += 1;
        texture_view_id
    }
}

/// Offscreen canvas renderer driven by WGPU textures.
pub struct CanvasRenderer {
    /// Render thread running the internal Bevy app.
    handle: Option<JoinHandle<()>>,
    /// Commands sent from the host UI thread to the canvas render thread.
    msg_tx: async_channel::Sender<CanvasMessage>,
    /// Textures that the host has finished displaying and can be rendered into again.
    free_targets_tx: async_channel::Sender<Texture>,
    /// Synchronous frame requests issued by the host during its render callback.
    frame_requests_tx: async_channel::Sender<RenderFrameRequest>,
}

impl CanvasRenderer {
    /// Start the offscreen canvas renderer from host-owned WGPU handles.
    ///
    /// Slint owns the real window and WGPU backend. The renderer wraps those
    /// handles as Bevy render resources, then drives a windowless Bevy app on a
    /// dedicated thread.
    pub fn init(instance: wgpu::Instance, device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let (msg_tx, msg_rx) = async_channel::bounded::<CanvasMessage>(1024);
        let (free_targets_tx, free_targets_rx) = async_channel::bounded::<Texture>(8);
        let (ready_frames_tx, ready_frames_rx) = async_channel::bounded::<Texture>(3);
        let (frame_requests_tx, frame_requests_rx) =
            async_channel::bounded::<RenderFrameRequest>(1);

        let texture_device = device.clone();
        // Bevy's manual render creation needs both the adapter and adapter info
        // even though Slint already selected the device and queue for us.
        let adapter = spin_on::spin_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .expect("failed to find adapter for CanvasRenderer");

        // Convert raw WGPU handles into Bevy's render resources without asking
        // Bevy to create its own device or window surface.
        let render_device: RenderDevice = device.into();
        let render_queue = RenderQueue(Arc::new(WgpuWrapper::new(queue)));
        let render_adapter_info = RenderAdapterInfo(WgpuWrapper::new(adapter.get_info()));
        let render_adapter = RenderAdapter(Arc::new(WgpuWrapper::new(adapter)));
        let render_instance = RenderInstance(Arc::new(WgpuWrapper::new(instance)));

        let handle = std::thread::spawn(move || {
            run_renderer_thread(RendererThreadConfig {
                texture_device,
                render_config: OffscreenRenderConfig::new(
                    render_device,
                    render_queue,
                    render_adapter_info,
                    render_adapter,
                    render_instance,
                    ready_frames_tx,
                ),
                channels: RendererThreadChannels {
                    msg_rx,
                    free_targets_rx,
                    ready_frames_rx,
                    frame_requests_rx,
                },
            });
        });

        Self {
            handle: Some(handle),
            msg_tx,
            free_targets_tx,
            frame_requests_tx,
        }
    }

    /// Clone the sender used by hosts to release displayed render targets.
    pub fn free_targets_tx(&self) -> async_channel::Sender<Texture> {
        self.free_targets_tx.clone()
    }

    /// Send a canvas command without blocking the host UI.
    pub fn send(&self, msg: CanvasMessage) {
        let _ = self.msg_tx.try_send(msg);
    }

    /// Send a canvas command and wait until the render thread accepts it.
    pub async fn send_async(
        &self,
        msg: CanvasMessage,
    ) -> Result<(), async_channel::SendError<CanvasMessage>> {
        self.msg_tx.send(msg).await
    }

    /// Return an old displayed target to the canvas thread.
    pub fn return_target(&self, target: Texture) {
        let _ = self.free_targets_tx.try_send(target);
    }

    /// Return an old displayed target and wait until the render thread accepts it.
    pub async fn return_target_async(
        &self,
        target: Texture,
    ) -> Result<(), async_channel::SendError<Texture>> {
        self.free_targets_tx.send(target).await
    }

    /// Render a canvas frame into a host-visible texture when canvas state is dirty.
    ///
    /// This method is intentionally synchronous. Hosts that share WGPU with Bevy
    /// should call it from their own render callback so Bevy queue submission
    /// finishes before the host renderer submits work on the same queue.
    pub fn render_frame(&self, width: u32, height: u32) -> Option<Texture> {
        let (reply_tx, reply_rx) = async_channel::bounded(1);
        let request = RenderFrameRequest {
            size: RenderTargetSize::new(width, height),
            reply_tx,
        };

        self.frame_requests_tx.send_blocking(request).ok()?;
        reply_rx.recv_blocking().ok().flatten()
    }

    /// Stop the render thread and wait for its cleanup.
    pub fn shutdown(mut self) {
        self.send(CanvasMessage::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Inputs needed to bootstrap and drive the renderer thread.
struct RendererThreadConfig {
    texture_device: wgpu::Device,
    render_config: OffscreenRenderConfig,
    channels: RendererThreadChannels,
}

/// Channels used by the renderer thread to receive work and publish frames.
struct RendererThreadChannels {
    msg_rx: async_channel::Receiver<CanvasMessage>,
    free_targets_rx: async_channel::Receiver<Texture>,
    ready_frames_rx: async_channel::Receiver<Texture>,
    frame_requests_rx: async_channel::Receiver<RenderFrameRequest>,
}

impl RendererThreadChannels {
    fn wait_event(&self) -> Option<RendererEvent> {
        future::block_on(future::race(
            async { self.msg_rx.recv().await.ok().map(RendererEvent::Message) },
            future::race(
                async {
                    self.free_targets_rx
                        .recv()
                        .await
                        .ok()
                        .map(RendererEvent::ReleasedTarget)
                },
                async {
                    self.frame_requests_rx
                        .recv()
                        .await
                        .ok()
                        .map(RendererEvent::RenderFrame)
                },
            ),
        ))
    }

    fn try_next_state_event(&self) -> Option<RendererEvent> {
        if let Ok(msg) = self.msg_rx.try_recv() {
            return Some(RendererEvent::Message(msg));
        }
        self.free_targets_rx
            .try_recv()
            .ok()
            .map(RendererEvent::ReleasedTarget)
    }
}

/// Events that can wake the retained-mode renderer thread.
enum RendererEvent {
    Message(CanvasMessage),
    ReleasedTarget(Texture),
    RenderFrame(RenderFrameRequest),
}

/// A host render callback asking Bevy to produce one canvas frame.
struct RenderFrameRequest {
    /// Requested render target size.
    size: RenderTargetSize,
    /// Reply channel carrying the completed frame, or `None` when nothing changed.
    reply_tx: async_channel::Sender<Option<Texture>>,
}

/// Run the windowless Bevy app that owns canvas state and renders into host textures.
fn run_renderer_thread(config: RendererThreadConfig) {
    let RendererThreadConfig {
        texture_device,
        render_config,
        channels,
    } = config;

    let mut canvas = Canvas::with_offscreen_render(render_config, texture_device);

    loop {
        let Some(event) = channels.wait_event() else {
            break;
        };

        match event {
            RendererEvent::RenderFrame(request) => {
                if canvas.drain_pending_state_events(&channels) {
                    let _ = request.reply_tx.send_blocking(None);
                    break;
                }

                let frame = canvas.render_requested_frame(&channels, request.size);
                let _ = request.reply_tx.send_blocking(frame);
            }
            event => {
                if canvas.apply_renderer_event(event) {
                    break;
                }

                if canvas.drain_pending_state_events(&channels) {
                    break;
                }
            }
        }
    }
}

/// Per-frame/system error sink.
///
/// Bevy systems cannot directly return `Result`, so systems push failures here
/// and the facade/plugin owner can decide when to inspect or surface them.
#[derive(Resource, Default)]
pub(crate) struct CanvasErrors(pub Vec<CanvasError>);

/// Stores the entity created by the most recent `AddPage` command.
#[derive(Resource, Default)]
pub(crate) struct LastAddedPage(Option<Entity>);

/// Apply high-level canvas commands such as tool changes and page creation.
pub(crate) fn apply_canvas_commands(
    mut commands: Commands,
    mut canvas_commands: MessageReader<CanvasCommand>,
    mut mode: ResMut<CanvasMode>,
    mut last_added_page: ResMut<LastAddedPage>,
) {
    for command in canvas_commands.read() {
        match *command {
            CanvasCommand::SetTool(tool) => mode.tool = tool,
            CanvasCommand::AddPage(config) => {
                let entity = commands.spawn(Page::new(config)).id();
                last_added_page.0 = Some(entity);
            }
        }
    }
}
