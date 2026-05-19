//! Offscreen render target installation and host texture handoff.

use bevy::{
    app::{App, Plugin},
    asset::AssetApp,
    camera::{Camera, Camera2d, ManualTextureViewHandle, RenderTarget},
    core_pipeline::CorePipelinePlugin,
    image::ImagePlugin,
    mesh::Mesh,
    prelude::{ClearColor, Color, Deref, DerefMut, Resource},
    render::{
        RenderApp, RenderPlugin,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_graph::{self, NodeRunError, RenderGraph, RenderGraphContext, RenderLabel},
        render_resource::{
            Texture, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
            TextureViewDescriptor,
        },
        renderer::{
            RenderAdapter, RenderAdapterInfo, RenderContext, RenderDevice, RenderInstance,
            RenderQueue,
        },
        settings::RenderCreation,
        texture::{ManualTextureView, ManualTextureViews},
    },
};

/// Manual render resources supplied by the host-owned WGPU backend.
#[derive(Resource)]
pub(crate) struct OffscreenRenderConfig {
    render_device: RenderDevice,
    render_queue: RenderQueue,
    render_adapter_info: RenderAdapterInfo,
    render_adapter: RenderAdapter,
    render_instance: RenderInstance,
    ready_frames_tx: async_channel::Sender<Texture>,
}

impl OffscreenRenderConfig {
    /// Build the render configuration consumed by `QuillRenderPlugin`.
    pub(crate) fn new(
        render_device: RenderDevice,
        render_queue: RenderQueue,
        render_adapter_info: RenderAdapterInfo,
        render_adapter: RenderAdapter,
        render_instance: RenderInstance,
        ready_frames_tx: async_channel::Sender<Texture>,
    ) -> Self {
        Self {
            render_device,
            render_queue,
            render_adapter_info,
            render_adapter,
            render_instance,
            ready_frames_tx,
        }
    }
}

/// Installs Bevy's manual renderer and the render-target handoff graph node.
pub struct OffscreenRenderPlugin;

impl Plugin for OffscreenRenderPlugin {
    fn build(&self, app: &mut App) {
        let Some(config) = app.world_mut().remove_resource::<OffscreenRenderConfig>() else {
            return;
        };

        app.insert_resource(ClearColor(Color::srgb(0.97, 0.97, 0.94)));
        app.insert_resource(BackBuffer(None));
        app.init_asset::<Mesh>();
        app.add_plugins(RenderPlugin {
            render_creation: RenderCreation::manual(
                config.render_device,
                config.render_queue,
                config.render_adapter_info,
                config.render_adapter,
                config.render_instance,
            ),
            ..Default::default()
        });
        app.add_plugins(ImagePlugin::default_linear());
        app.add_plugins(CorePipelinePlugin);
        app.add_plugins(CanvasRenderToTexturePlugin(config.ready_frames_tx));
        app.add_plugins(ExtractResourcePlugin::<BackBuffer>::default());

        // The camera is targetless at startup. `install_render_target` points it
        // at a host texture immediately before each rendered frame.
        app.world_mut().spawn(Camera2d);
    }
}

/// Create a renderable texture compatible with Slint's WGPU image path.
pub(crate) fn create_target(
    device: &wgpu::Device,
    label: &'static str,
    width: u32,
    height: u32,
) -> Texture {
    Texture::from(device.create_texture(&TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rgba8UnormSrgb,
        usage: TextureUsages::TEXTURE_BINDING
            | TextureUsages::COPY_DST
            | TextureUsages::COPY_SRC
            | TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    }))
}

/// Bind a host texture as the Bevy camera's manual render target.
pub(crate) fn install_render_target(app: &mut App, target: Texture, texture_view_id: u32) {
    let size = (target.width(), target.height()).into();
    let texture_view = target.create_view(&TextureViewDescriptor {
        label: Some("quill-canvas-render-target-view"),
        ..Default::default()
    });
    let texture_view_handle = ManualTextureViewHandle(texture_view_id);

    let world = app.world_mut();
    world.resource_mut::<BackBuffer>().0 = Some(target);
    let mut manual_texture_views = world.resource_mut::<ManualTextureViews>();
    manual_texture_views.clear();
    manual_texture_views.insert(
        texture_view_handle,
        ManualTextureView {
            texture_view,
            size,
            view_format: TextureFormat::Rgba8UnormSrgb,
        },
    );

    let mut cameras = world.query::<(&mut Camera, &mut RenderTarget)>();
    if let Some(mut camera) = cameras.iter_mut(world).next() {
        *camera.1 = RenderTarget::TextureView(texture_view_handle);
    }
}

/// Render-world sender used by the render graph node to publish finished frames.
#[derive(Resource, Deref)]
struct ReadyFrameSender(async_channel::Sender<Texture>);

/// Plugin that appends a render graph node for handing the current target to the host.
struct CanvasRenderToTexturePlugin(async_channel::Sender<Texture>);

impl Plugin for CanvasRenderToTexturePlugin {
    fn build(&self, app: &mut App) {
        let render_app = app.sub_app_mut(RenderApp);

        let mut graph = render_app.world_mut().resource_mut::<RenderGraph>();
        graph.add_node(CanvasPrepareTarget, CanvasPrepareTargetDriver);
        graph.add_node(CanvasSwapChain, CanvasSwapChainDriver);
        graph.add_node_edge(CanvasPrepareTarget, bevy::render::graph::CameraDriverLabel);
        graph.add_node_edge(bevy::render::graph::CameraDriverLabel, CanvasSwapChain);

        render_app.insert_resource(ReadyFrameSender(self.0.clone()));
    }
}

/// Texture currently installed as the Bevy camera render target.
#[derive(Clone, Resource, ExtractResource, Deref, DerefMut)]
struct BackBuffer(pub Option<Texture>);

/// Render graph label for preparing a recycled target before camera rendering.
#[derive(Debug, PartialEq, Eq, Clone, Hash, RenderLabel)]
struct CanvasPrepareTarget;

/// Render graph label for the host texture handoff node.
#[derive(Debug, PartialEq, Eq, Clone, Hash, RenderLabel)]
struct CanvasSwapChain;

/// Render graph node that transitions the current target for color attachment writes.
#[derive(Default)]
struct CanvasPrepareTargetDriver;

impl render_graph::Node for CanvasPrepareTargetDriver {
    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext,
        world: &bevy::ecs::world::World,
    ) -> Result<(), NodeRunError> {
        let back_buffer = world.resource::<BackBuffer>();

        if let Some(back_buffer) = &back_buffer.0 {
            render_context.command_encoder().transition_resources(
                std::iter::empty(),
                std::iter::once(wgpu::TextureTransition {
                    texture: &**back_buffer,
                    selector: None,
                    state: wgpu::TextureUses::COLOR_TARGET,
                }),
            );
        }

        Ok(())
    }
}

/// Render graph node that sends the just-rendered texture back to the UI thread.
#[derive(Default)]
struct CanvasSwapChainDriver;

impl render_graph::Node for CanvasSwapChainDriver {
    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        _render_context: &mut RenderContext,
        world: &bevy::ecs::world::World,
    ) -> Result<(), NodeRunError> {
        let ready_frame_sender = world.resource::<ReadyFrameSender>();
        let back_buffer = world.resource::<BackBuffer>();

        if let Some(back_buffer) = &back_buffer.0 {
            ready_frame_sender.try_send(back_buffer.clone()).ok();
        }

        Ok(())
    }
}
