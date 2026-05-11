//! Bevy stroke pipeline setup.

use bevy::{
    app::Plugin, asset::{Assets, Handle}, camera::Camera2d, core_pipeline::core_2d::Transparent2d, math::{Mat3, UVec2}, prelude::{Commands, Res, ResMut, Resource}, render::{
        Extract, ExtractSchedule, Render, RenderApp, RenderSystems,
        render_phase::{
            AddRenderCommand, PhaseItem, RenderCommand, RenderCommandResult, SetItemPipeline,
            TrackedRenderPass,
        },
        render_resource::{
            BindGroup, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingType, BlendState,
            BufferBindingType, ColorTargetState, ColorWrites, FragmentState, MultisampleState,
            PrimitiveState, PrimitiveTopology, RawBufferVec, RenderPipelineDescriptor,
            ShaderStages, ShaderType, SpecializedRenderPipeline, SpecializedRenderPipelines,
            TextureFormat, UniformBuffer, VertexState,
        },
    }, shader::Shader, sprite_render::{Mesh2dPipeline, SetMesh2dViewBindGroup}, transform::components::Transform
};
use bevy_ecs::{
    entity::Entity,
    query::Changed,
    schedule::IntoScheduleConfigs,
    system::{Query, SystemParamItem, lifetimeless::SRes},
};
use quill_stroke::{
    brush::{BrushTip, RoundTip},
    gpu::CIRCLE_SHADER,
    input::PathPoint,
};

use crate::stroke::StrokeData;

#[derive(Clone, Copy, Debug, ShaderType)]
struct StrokeParamsUniform {
    sample_count: u32,
    dirty_segment_start: u32,
    transform: Mat3,
}

#[derive(Clone, Copy, Debug, ShaderType)]
struct RoundTipUniform {
    radius: f32,
    pressure_mode: u32,
    _pad0: UVec2,
}

impl From<RoundTip> for RoundTipUniform {
    fn from(tip: RoundTip) -> Self {
        Self {
            radius: tip.radius.0,
            pressure_mode: tip.pressure_mode,
            _pad0: UVec2::ZERO,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct StrokePipelineKey {
    tip: BrushTip,
    msaa_samples: u32,
}

impl StrokePipelineKey {
    pub fn new(tip: BrushTip, msaa_samples: u32) -> Self {
        Self { tip, msaa_samples }
    }
}

#[derive(Resource)]
pub struct StrokePipeline {
    view_layout: BindGroupLayoutDescriptor,
    stroke_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
}

impl SpecializedRenderPipeline for StrokePipeline {
    type Key = StrokePipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some(format!("stroke-pipeline-{:?}", key.tip).into()),
            vertex: VertexState {
                shader: self.shader.clone(),
                ..Default::default()
            },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                targets: vec![Some(ColorTargetState {
                    format: TextureFormat::Rgba8UnormSrgb,
                    blend: Some(BlendState::ALPHA_BLENDING),
                    write_mask: ColorWrites::ALL,
                })],
                ..Default::default()
            }),
            layout: vec![self.view_layout.clone(), self.stroke_layout.clone()],
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            multisample: MultisampleState {
                count: key.msaa_samples,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            ..Default::default()
        }
    }
}

pub fn init_stroke_pipeline(
    mut commands: Commands,
    mesh2d_pipeline: Res<Mesh2dPipeline>,
    mut shaders: ResMut<Assets<Shader>>,
) {
    let stroke_layout = BindGroupLayoutDescriptor::new(
        "stroke",
        &[
            BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::VERTEX,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 1,
                visibility: ShaderStages::VERTEX,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 2,
                visibility: ShaderStages::VERTEX_FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    );
    let shader = shaders.add(Shader::from_wgsl(CIRCLE_SHADER, "quill://stroke.wgsl"));
    commands.insert_resource(StrokePipeline {
        view_layout: mesh2d_pipeline.view_layout.clone(),
        stroke_layout,
        shader,
    });
}

type DrawStroke = (
    SetItemPipeline,
    SetMesh2dViewBindGroup<0>,
    SetStrokeBindGroup<1>,
    DrawStrokeCommand,
);

struct DrawStrokeCommand;
struct SetStrokeBindGroup<const I: usize>;

#[derive(Resource)]
struct RenderStrokeInstances {
    /// Current path points.
    ///
    /// Usage: `STORAGE | COPY_DST`.
    path: RawBufferVec<PathPoint>,

    /// Generated triangle vertices for polygon brush rendering.
    ///
    /// Rount brush rendering does not use this buffer.
    generated_vertex_buffer: Option<RawBufferVec<()>>,

    /// Shared stroke params.
    ///
    /// Usage: `UNIFORM | COPY_DST`.
    params: UniformBuffer<StrokeParamsUniform>,

    brush: UniformBuffer<RoundTipUniform>,

    bind_group: Option<BindGroup>,

    segment_count: u32,
}

impl Default for RenderStrokeInstances {
    fn default() -> Self {
        todo!()
    }
}

impl<P: PhaseItem> RenderCommand<P> for DrawStrokeCommand {
    type Param = SRes<RenderStrokeInstances>;
    type ViewQuery = ();
    type ItemQuery = ();

    fn render<'w>(
        _item: &P,
        _view: (),
        _entity: Option<()>,
        instances: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        if instances.segment_count == 0 {
            return RenderCommandResult::Skip;
        }

        pass.draw(0..6, 0..instances.segment_count);
        RenderCommandResult::Success
    }
}

impl<P: PhaseItem, const I: usize> RenderCommand<P> for SetStrokeBindGroup<I> {
    type Param = SRes<RenderStrokeInstances>;
    type ViewQuery = ();
    type ItemQuery = ();

    fn render<'w>(
        _item: &P,
        _view: (),
        _entity: Option<()>,
        instances: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(bind_group) = instances.into_inner().bind_group.as_ref() else {
            return RenderCommandResult::Skip;
        };

        pass.set_bind_group(I, &bind_group, &[]);
        RenderCommandResult::Success
    }
}

pub struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        let mut commands = app.world_mut().commands();
        commands.spawn((Camera2d, Transform::from_xyz(0., 0., 100.)));
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<SpecializedRenderPipelines<StrokePipeline>>()
                .init_resource::<RenderStrokeInstances>()
                .add_systems(ExtractSchedule, extract_strokes)
                .add_systems(
                    Render,
                    (
                        prepare_strokes.in_set(RenderSystems::PrepareResources),
                        queue_strokes.in_set(RenderSystems::Queue),
                    ),
                )
                .add_render_command::<Transparent2d, DrawStroke>();
        }
    }
}

fn extract_strokes(
    _commands: Commands,
    // mut extracted_stroke: ResMut<()>,
    _query: Extract<Query<(Entity, &StrokeData), Changed<StrokeData>>>,
) {}

fn prepare_strokes() {}

fn queue_strokes() {}

pub fn sync_stroke_gpu() {}
