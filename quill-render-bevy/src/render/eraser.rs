//! Eraser preview render pipeline.
//!
//! The eraser preview is canvas UI, not editable scene content. This module
//! keeps the input-side preview state and render-world draw pipeline together
//! so the eraser element logic can stay focused on erasing strokes.

use bevy::{
    app::{App, Plugin, Update},
    asset::{Assets, Handle},
    core_pipeline::core_2d::{CORE_2D_DEPTH_FORMAT, Transparent2d},
    math::{FloatOrd, Vec2},
    prelude::{Commands, Res, ResMut, Resource},
    render::{
        ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_phase::{
            AddRenderCommand, DrawFunctions, PhaseItem, PhaseItemExtraIndex, RenderCommand,
            RenderCommandResult, SetItemPipeline, TrackedRenderPass, ViewSortedRenderPhases,
        },
        render_resource::{
            BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
            BlendState, ColorTargetState, ColorWrites, CompareFunction, DepthBiasState,
            DepthStencilState, FragmentState, MultisampleState, PipelineCache, PrimitiveState,
            PrimitiveTopology, RenderPipelineDescriptor, ShaderStages, ShaderType, StencilState,
            TextureFormat, UniformBuffer, VertexState, binding_types::uniform_buffer,
        },
        renderer::{RenderDevice, RenderQueue},
        view::{ExtractedView, Msaa, ViewUniform, ViewUniformOffset, ViewUniforms},
    },
    shader::Shader,
};
use bevy_ecs::{
    query::ROQueryItem,
    schedule::IntoScheduleConfigs,
    system::{
        Query, SystemParamItem,
        lifetimeless::{Read, SRes},
    },
};
use quill_canvas::{
    canvas::CanvasMode,
    element::{EraserConfig, EraserShape, Tool},
    input::CanvasInput,
    runtime::QuillUpdateSet,
};
use quill_stroke::{gpu::ERASER_SHADER, input::Input};

const CIRCLE_SHAPE: u32 = 0;
const SQUARE_SHAPE: u32 = 1;
const DEFAULT_BORDER_WIDTH: f32 = 1.0;
const ERASER_OVERLAY_SORT_Z: f32 = f32::MAX;

/// Main-world eraser preview state extracted into the render world each frame.
#[derive(Clone, Default, Resource, ExtractResource)]
struct EraserPreviewState {
    config: EraserConfig,
    position: Option<Vec2>,
    pointer_inside: bool,
    visible: bool,
}

/// Uniform consumed by the eraser preview shader.
#[derive(Clone, Copy, Debug, Default, ShaderType)]
struct EraserPreviewUniform {
    center: Vec2,
    half_size: f32,
    border_width: f32,
    shape: u32,
    visible: u32,
}

impl EraserPreviewUniform {
    fn from_preview(preview: &EraserPreviewState) -> Self {
        Self {
            center: preview.position.unwrap_or(Vec2::ZERO),
            half_size: preview.config.size.max(0.0),
            border_width: DEFAULT_BORDER_WIDTH,
            shape: match preview.config.shape {
                EraserShape::Circle => CIRCLE_SHAPE,
                EraserShape::Square => SQUARE_SHAPE,
            },
            visible: preview.visible as u32,
        }
    }
}

/// Render-world GPU resources for the single eraser preview overlay.
#[derive(Resource, Default)]
struct EraserPreviewGpu {
    uniform: UniformBuffer<EraserPreviewUniform>,
    bind_group: Option<BindGroup>,
}

/// Specialization key for the eraser preview pipeline.
#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct EraserPreviewPipelineKey {
    msaa_samples: u32,
    /// Attachment format shared with the canvas view.
    target_format: TextureFormat,
}

/// Render pipeline resources shared by eraser preview draw commands.
#[derive(Resource)]
pub struct EraserPreviewPipeline {
    view_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
}

impl bevy::render::render_resource::SpecializedRenderPipeline for EraserPreviewPipeline {
    type Key = EraserPreviewPipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("eraser-preview-pipeline".into()),
            vertex: VertexState {
                shader: self.shader.clone(),
                ..Default::default()
            },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                targets: vec![Some(ColorTargetState {
                    format: key.target_format,
                    blend: Some(BlendState::ALPHA_BLENDING),
                    write_mask: ColorWrites::ALL,
                })],
                ..Default::default()
            }),
            layout: vec![self.view_layout.clone()],
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            multisample: MultisampleState {
                count: key.msaa_samples,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            depth_stencil: Some(DepthStencilState {
                format: CORE_2D_DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(CompareFunction::GreaterEqual),
                stencil: StencilState::default(),
                bias: DepthBiasState::default(),
            }),
            ..Default::default()
        }
    }
}

#[derive(Resource)]
struct EraserPreviewShader(Handle<Shader>);

/// Installs the eraser preview renderer into Bevy's render world.
pub struct EraserRenderPlugin;

impl Plugin for EraserRenderPlugin {
    fn build(&self, app: &mut App) {
        if app.get_sub_app_mut(RenderApp).is_none() {
            return;
        }

        app.init_resource::<EraserPreviewState>()
            .add_plugins(ExtractResourcePlugin::<EraserPreviewState>::default())
            .add_systems(
                Update,
                update_eraser_preview.in_set(QuillUpdateSet::RouteInput),
            );

        app.world_mut().init_resource::<Assets<Shader>>();
        let mut shaders = app.world_mut().resource_mut::<Assets<Shader>>();
        let shader = shaders.add(Shader::from_wgsl(
            ERASER_SHADER,
            "quill://eraser-preview.wgsl",
        ));
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .insert_resource(EraserPreviewShader(shader))
                .init_resource::<EraserPreviewGpu>()
                .init_resource::<Assets<Shader>>()
                .init_resource::<bevy::render::render_resource::SpecializedRenderPipelines<
                    EraserPreviewPipeline,
                >>()
                .add_systems(RenderStartup, init_eraser_preview_pipeline)
                .add_systems(ExtractSchedule, ensure_eraser_preview_resource)
                .add_systems(
                    Render,
                    (
                        prepare_eraser_preview_bind_group
                            .in_set(RenderSystems::PrepareBindGroups),
                        queue_eraser_preview.in_set(RenderSystems::Queue),
                    ),
                )
                .add_render_command::<Transparent2d, DrawEraserPreview>();
        }
    }
}

fn update_eraser_preview(
    mut inputs: bevy_ecs::prelude::MessageReader<CanvasInput>,
    mode: Res<CanvasMode>,
    mut preview: ResMut<EraserPreviewState>,
) {
    for input in inputs.read() {
        match *input {
            CanvasInput::PointerEnter(position) => {
                preview.pointer_inside = true;
                preview.position = Some(Vec2::from_array(position));
            }
            CanvasInput::PointerLeave => {
                preview.pointer_inside = false;
                preview.visible = false;
            }
            CanvasInput::PointerMove(input)
            | CanvasInput::PointerDown(input)
            | CanvasInput::PointerUp(input) => {
                preview.pointer_inside = true;
                preview.position = Some(input_to_vec2(input));
            }
        }
    }

    if let Tool::Eraser(config) = mode.tool {
        preview.config = config;
        preview.visible = preview.pointer_inside && preview.position.is_some();
    } else {
        preview.visible = false;
    }
}

fn input_to_vec2(input: Input) -> Vec2 {
    Vec2::new(input.x(), input.y())
}

fn ensure_eraser_preview_resource(
    mut commands: Commands,
    preview: Option<Res<EraserPreviewState>>,
) {
    if preview.is_none() {
        commands.insert_resource(EraserPreviewState::default());
    }
}

fn init_eraser_preview_pipeline(mut commands: Commands, eraser_shader: Res<EraserPreviewShader>) {
    let view_layout = BindGroupLayoutDescriptor::new(
        "eraser_preview_view_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::VERTEX_FRAGMENT,
            (
                uniform_buffer::<ViewUniform>(true),
                uniform_buffer::<EraserPreviewUniform>(false),
            ),
        ),
    );

    commands.insert_resource(EraserPreviewPipeline {
        view_layout,
        shader: eraser_shader.0.clone(),
    });
}

type DrawEraserPreview = (
    SetItemPipeline,
    SetEraserPreviewBindGroup<0>,
    DrawEraserPreviewCommand,
);

struct DrawEraserPreviewCommand;
struct SetEraserPreviewBindGroup<const I: usize>;

impl<P: PhaseItem> RenderCommand<P> for DrawEraserPreviewCommand {
    type Param = SRes<EraserPreviewState>;
    type ViewQuery = ();
    type ItemQuery = ();

    fn render<'w>(
        _item: &P,
        _view: (),
        _item_query: Option<ROQueryItem<'w, '_, Self::ItemQuery>>,
        preview: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        if !preview.into_inner().visible {
            return RenderCommandResult::Skip;
        }

        pass.draw(0..6, 0..1);
        RenderCommandResult::Success
    }
}

impl<P: PhaseItem, const I: usize> RenderCommand<P> for SetEraserPreviewBindGroup<I> {
    type Param = SRes<EraserPreviewGpu>;
    type ViewQuery = Read<ViewUniformOffset>;
    type ItemQuery = ();

    fn render<'w>(
        _item: &P,
        view_uniform: ROQueryItem<'w, '_, Self::ViewQuery>,
        _item_query: Option<ROQueryItem<'w, '_, Self::ItemQuery>>,
        preview: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(bind_group) = preview.into_inner().bind_group.as_ref() else {
            return RenderCommandResult::Skip;
        };

        pass.set_bind_group(I, bind_group, &[view_uniform.offset]);
        RenderCommandResult::Success
    }
}

fn prepare_eraser_preview_bind_group(
    preview: Res<EraserPreviewState>,
    mut gpu: ResMut<EraserPreviewGpu>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    pipeline_cache: Res<PipelineCache>,
    pipeline: Res<EraserPreviewPipeline>,
    view_uniforms: Res<ViewUniforms>,
) {
    gpu.uniform
        .set(EraserPreviewUniform::from_preview(&preview));
    gpu.uniform.set_label(Some("eraser_preview_uniform_buffer"));
    gpu.uniform.write_buffer(&render_device, &render_queue);

    let (Some(view_binding), Some(preview_binding)) =
        (view_uniforms.uniforms.binding(), gpu.uniform.binding())
    else {
        gpu.bind_group = None;
        return;
    };

    gpu.bind_group = Some(render_device.create_bind_group(
        "eraser_preview_bind_group",
        &pipeline_cache.get_bind_group_layout(&pipeline.view_layout),
        &BindGroupEntries::sequential((view_binding, preview_binding)),
    ));
}

fn queue_eraser_preview(
    preview: Res<EraserPreviewState>,
    transparent_draw_functions: Res<DrawFunctions<Transparent2d>>,
    eraser_pipeline: Res<EraserPreviewPipeline>,
    mut pipelines: ResMut<
        bevy::render::render_resource::SpecializedRenderPipelines<EraserPreviewPipeline>,
    >,
    pipeline_cache: Res<PipelineCache>,
    mut transparent_render_phases: ResMut<ViewSortedRenderPhases<Transparent2d>>,
    views: Query<(bevy_ecs::prelude::Entity, &ExtractedView, &Msaa)>,
) {
    if !preview.visible {
        return;
    }

    let draw_eraser = transparent_draw_functions.read().id::<DrawEraserPreview>();

    for (view_entity, view, msaa) in &views {
        let Some(transparent_phase) = transparent_render_phases.get_mut(&view.retained_view_entity)
        else {
            continue;
        };

        let pipeline = pipelines.specialize(
            &pipeline_cache,
            &eraser_pipeline,
            EraserPreviewPipelineKey {
                msaa_samples: msaa.samples(),
                target_format: view.target_format,
            },
        );

        // Drop the previous preview when the eraser becomes hidden next frame.
        transparent_phase.add_transient(Transparent2d {
            sort_key: FloatOrd(ERASER_OVERLAY_SORT_Z),
            entity: (view_entity, view.retained_view_entity.main_entity),
            pipeline,
            draw_function: draw_eraser,
            batch_range: 0..1,
            extracted_index: usize::MAX,
            extra_index: PhaseItemExtraIndex::None,
            indexed: false,
        });
    }
}
