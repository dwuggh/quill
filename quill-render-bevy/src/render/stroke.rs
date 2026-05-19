//! Stroke render pipeline and render-world data preparation.

use bevy::{
    app::{App, Plugin},
    asset::{Assets, Handle},
    core_pipeline::core_2d::{CORE_2D_DEPTH_FORMAT, Transparent2d},
    image::BevyDefault,
    math::{FloatOrd, Mat4, Vec4},
    prelude::{Commands, Component, Entity, Res, ResMut, Resource},
    render::{
        Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems,
        extract_component::{ComponentUniforms, DynamicUniformIndex, UniformComponentPlugin},
        render_phase::{
            AddRenderCommand, DrawFunctions, PhaseItem, PhaseItemExtraIndex, RenderCommand,
            RenderCommandResult, SetItemPipeline, TrackedRenderPass, ViewSortedRenderPhases,
        },
        render_resource::{
            BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
            BindGroupLayoutEntry, BindingType, BlendState, BufferBindingType, BufferUsages,
            ColorTargetState, ColorWrites, CompareFunction, DepthBiasState, DepthStencilState,
            FragmentState, MultisampleState, PipelineCache, PrimitiveState, PrimitiveTopology,
            RawBufferVec, RenderPipelineDescriptor, ShaderStages, ShaderType,
            SpecializedRenderPipeline, SpecializedRenderPipelines, StencilState, TextureFormat,
            VertexState, binding_types::uniform_buffer,
        },
        renderer::{RenderDevice, RenderQueue},
        sync_world::{MainEntity, RenderEntity, SyncToRenderWorld},
        view::{ExtractedView, Msaa, ViewUniform, ViewUniformOffset, ViewUniforms},
    },
    shader::Shader,
};
use bevy_ecs::{
    query::{Changed, ROQueryItem, With},
    schedule::IntoScheduleConfigs,
    system::{
        Query, SystemParamItem,
        lifetimeless::{Read, SRes},
    },
};
use quill_canvas::{
    canvas::CanvasOrder,
    element::stroke::{StrokeData, StrokeStatus},
    geom::CanvasTransform,
};
use quill_stroke::{brush::BrushTip, gpu::ROUND_SHADER, input::PathPoint};

#[derive(Clone, Copy, Component, Debug, ShaderType)]
struct StrokeParamsUniform {
    radius: f32,
    pressure_mode: u32,
    sample_count: u32,
    dirty_segment_start: u32,
    color: Vec4,
}

impl StrokeParamsUniform {
    fn from_stroke(stroke: &StrokeData) -> Self {
        let (tip, pressure_mode) = match stroke.brush.brush.tip {
            BrushTip::Round(tip) => (tip, 1),
            BrushTip::FixedRound(tip) => (tip, 0),
        };
        let dirty_segment_start = match stroke.status {
            StrokeStatus::Active { last_update } => last_update.invalidated_from as u32,
            StrokeStatus::Finished => 0,
        };

        Self {
            radius: tip.radius.0,
            pressure_mode,
            sample_count: stroke.points.len() as u32,
            dirty_segment_start,
            color: peniko_color_to_vec4(stroke.brush.brush.paint.color),
        }
    }
}

fn peniko_color_to_vec4(color: peniko::Color) -> Vec4 {
    Vec4::from_array(color.components)
}

#[derive(Clone, Copy, Component, Debug, ShaderType)]
struct StrokeLocalUniform {
    world_from_local: Mat4,
}

impl StrokeLocalUniform {
    fn from_canvas_transform(transform: CanvasTransform) -> Self {
        let [a, b, c, d, e, f] = transform.affine.as_coeffs();
        Self {
            world_from_local: Mat4::from_cols_array(&[
                a as f32, b as f32, 0.0, 0.0, c as f32, d as f32, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0,
                e as f32, f as f32, 0.0, 1.0,
            ]),
        }
    }
}

/// Stroke brush tip families that map to specialized render pipelines.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub enum StrokeTipKind {
    Round,
}

impl From<BrushTip> for StrokeTipKind {
    fn from(tip: BrushTip) -> Self {
        match tip {
            BrushTip::Round(_) | BrushTip::FixedRound(_) => Self::Round,
        }
    }
}

/// Specialization key for stroke pipeline variants.
#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct StrokePipelineKey {
    tip: StrokeTipKind,
    msaa_samples: u32,
}

impl StrokePipelineKey {
    /// Build a key from the brush tip and current view MSAA sample count.
    pub fn new(tip: BrushTip, msaa_samples: u32) -> Self {
        Self {
            tip: tip.into(),
            msaa_samples,
        }
    }
}

/// Render pipeline resources shared by all stroke draw commands.
#[derive(Resource)]
pub struct StrokePipeline {
    view_layout: BindGroupLayoutDescriptor,
    stroke_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
}

#[derive(Resource, Default)]
struct StrokeViewBindGroup {
    value: Option<BindGroup>,
}

/// Render-world copy of canvas stroke order using render entities.
#[derive(Resource, Default)]
struct RenderCanvasOrder {
    /// Strokes in document draw order, converted from main-world entities.
    strokes: Vec<Entity>,
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
                    format: TextureFormat::bevy_default(),
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
            depth_stencil: Some(DepthStencilState {
                format: CORE_2D_DEPTH_FORMAT,
                depth_write_enabled: false,
                depth_compare: CompareFunction::GreaterEqual,
                stencil: StencilState::default(),
                bias: DepthBiasState::default(),
            }),
            ..Default::default()
        }
    }
}

/// Shader handle shared with render-world pipeline startup.
#[derive(Resource)]
struct StrokeShader(Handle<Shader>);

/// Installs the stroke renderer into Bevy's render world.
pub struct StrokeRenderPlugin;

impl Plugin for StrokeRenderPlugin {
    fn build(&self, app: &mut App) {
        if app.get_sub_app_mut(RenderApp).is_none() {
            return;
        }

        app.register_required_components::<StrokeData, SyncToRenderWorld>();
        app.add_plugins((
            UniformComponentPlugin::<StrokeLocalUniform>::default(),
            UniformComponentPlugin::<StrokeParamsUniform>::default(),
        ));

        app.world_mut().init_resource::<Assets<Shader>>();
        let mut shaders = app.world_mut().resource_mut::<Assets<Shader>>();
        let shader = shaders.add(Shader::from_wgsl(ROUND_SHADER, "quill://stroke.wgsl"));
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .insert_resource(StrokeShader(shader))
                .init_resource::<SpecializedRenderPipelines<StrokePipeline>>()
                .init_resource::<StrokeViewBindGroup>()
                .init_resource::<RenderCanvasOrder>()
                .init_resource::<Assets<Shader>>()
                .add_systems(RenderStartup, init_stroke_pipeline)
                .add_systems(ExtractSchedule, extract_strokes)
                .add_systems(
                    Render,
                    (
                        (prepare_stroke_view_bind_group, prepare_stroke_bind_group)
                            .in_set(RenderSystems::PrepareBindGroups),
                        prepare_strokes.in_set(RenderSystems::PrepareResources),
                        queue_strokes.in_set(RenderSystems::Queue),
                    ),
                )
                .add_render_command::<Transparent2d, DrawStroke>();
        }
    }
}

fn init_stroke_pipeline(mut commands: Commands, stroke_shader: Res<StrokeShader>) {
    let view_layout = BindGroupLayoutDescriptor::new(
        "2d_view_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::VERTEX_FRAGMENT,
            (
                uniform_buffer::<ViewUniform>(true),
                uniform_buffer::<StrokeLocalUniform>(true),
            ),
        ),
    );
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
                visibility: ShaderStages::VERTEX_FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    );
    let shader = stroke_shader.0.clone();
    commands.insert_resource(StrokePipeline {
        view_layout,
        stroke_layout,
        shader,
    });
}

type DrawStroke = (
    SetItemPipeline,
    SetStrokeViewBindGroup<0>,
    SetStrokeBindGroup<1>,
    DrawStrokeCommand,
);

struct DrawStrokeCommand;
struct SetStrokeViewBindGroup<const I: usize>;
struct SetStrokeBindGroup<const I: usize>;

#[derive(Component)]
struct RenderStrokeInstance {
    /// Current path points.
    ///
    /// Usage: `STORAGE | COPY_DST`.
    path: RawBufferVec<PathPoint>,

    /// Generated triangle vertices for polygon brush rendering.
    ///
    /// Round brush rendering does not use this buffer.
    #[expect(dead_code, reason = "polygon brush rendering will use this buffer")]
    generated_vertex_buffer: Option<RawBufferVec<()>>,

    /// Bind group for this stroke's path buffer and parameter uniform.
    bind_group: Option<BindGroup>,

    /// Number of drawable point-to-point path segments.
    segment_count: u32,

    /// Lowest path point index that needs to be uploaded this frame.
    dirty_from: Option<usize>,

    /// Brush tip family used to specialize the render pipeline.
    tip: StrokeTipKind,
}

impl Default for RenderStrokeInstance {
    fn default() -> Self {
        Self {
            path: RawBufferVec::new(BufferUsages::STORAGE | BufferUsages::COPY_DST),
            generated_vertex_buffer: None,
            bind_group: None,
            segment_count: 0,
            dirty_from: None,
            tip: StrokeTipKind::Round,
        }
    }
}

impl RenderStrokeInstance {
    fn from_stroke(stroke: &StrokeData) -> Self {
        let mut instance = Self::default();
        instance.update_from_stroke(stroke);
        instance
    }

    fn update_from_stroke(&mut self, stroke: &StrokeData) {
        let dirty_from = match stroke.status {
            StrokeStatus::Active { last_update } => {
                let invalidated_from = last_update.invalidated_from;
                Some(
                    if self.path.is_empty()
                        || invalidated_from > self.path.len()
                        || invalidated_from > stroke.points.len()
                    {
                        0
                    } else {
                        invalidated_from
                    },
                )
            }
            StrokeStatus::Finished => first_changed_point(self.path.values(), &stroke.points),
        };

        if let Some(dirty_from) = dirty_from {
            self.path.truncate(dirty_from);
            self.path
                .values_mut()
                .extend_from_slice(&stroke.points[dirty_from..]);
            self.dirty_from = Some(
                self.dirty_from
                    .map_or(dirty_from, |current| current.min(dirty_from)),
            );
        }

        self.segment_count = stroke.points.len().saturating_sub(1) as u32;
        self.tip = stroke.brush.brush.tip.into();
    }

    fn write_dirty_buffer(&mut self, render_device: &RenderDevice, render_queue: &RenderQueue) {
        let Some(dirty_from) = self.dirty_from.take() else {
            return;
        };

        if self.path.is_empty() {
            self.bind_group = None;
            return;
        }

        let dirty_from = dirty_from.min(self.path.len());
        if dirty_from == self.path.len() {
            return;
        }

        if self.path.buffer().is_none() || dirty_from == 0 || self.path.len() > self.path.capacity()
        {
            self.path.write_buffer(render_device, render_queue);
            return;
        }

        if self
            .path
            .write_buffer_range(render_queue, dirty_from..self.path.len())
            .is_err()
        {
            self.path.write_buffer(render_device, render_queue);
        }
    }
}

fn first_changed_point(current: &[PathPoint], next: &[PathPoint]) -> Option<usize> {
    let common_len = current.len().min(next.len());
    for index in 0..common_len {
        if current[index] != next[index] {
            return Some(index);
        }
    }

    (current.len() != next.len()).then_some(common_len)
}

impl<P: PhaseItem> RenderCommand<P> for DrawStrokeCommand {
    type Param = ();
    type ViewQuery = ();
    type ItemQuery = Read<RenderStrokeInstance>;

    fn render<'w>(
        _item: &P,
        _view: (),
        stroke: Option<ROQueryItem<'w, '_, Self::ItemQuery>>,
        _param: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(stroke) = stroke else {
            return RenderCommandResult::Skip;
        };
        if stroke.segment_count == 0 {
            return RenderCommandResult::Skip;
        }

        pass.draw(0..6, 0..stroke.segment_count);
        RenderCommandResult::Success
    }
}

impl<P: PhaseItem, const I: usize> RenderCommand<P> for SetStrokeViewBindGroup<I> {
    type Param = SRes<StrokeViewBindGroup>;
    type ViewQuery = Read<ViewUniformOffset>;
    type ItemQuery = Read<DynamicUniformIndex<StrokeLocalUniform>>;

    fn render<'w>(
        _item: &P,
        view_uniform: ROQueryItem<'w, '_, Self::ViewQuery>,
        local_uniform: Option<ROQueryItem<'w, '_, Self::ItemQuery>>,
        bind_group: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(local_uniform) = local_uniform else {
            return RenderCommandResult::Skip;
        };
        let Some(bind_group) = bind_group.into_inner().value.as_ref() else {
            return RenderCommandResult::Skip;
        };

        pass.set_bind_group(I, bind_group, &[view_uniform.offset, local_uniform.index()]);
        RenderCommandResult::Success
    }
}

impl<P: PhaseItem, const I: usize> RenderCommand<P> for SetStrokeBindGroup<I> {
    type Param = ();
    type ViewQuery = ();
    type ItemQuery = (
        Read<RenderStrokeInstance>,
        Read<DynamicUniformIndex<StrokeParamsUniform>>,
    );

    fn render<'w>(
        _item: &P,
        _view: (),
        stroke: Option<ROQueryItem<'w, '_, Self::ItemQuery>>,
        _param: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((stroke, params)) = stroke else {
            return RenderCommandResult::Skip;
        };
        let Some(bind_group) = stroke.bind_group.as_ref() else {
            return RenderCommandResult::Skip;
        };

        pass.set_bind_group(I, bind_group, &[params.index()]);
        RenderCommandResult::Success
    }
}

fn prepare_stroke_view_bind_group(
    mut bind_group: ResMut<StrokeViewBindGroup>,
    render_device: Res<RenderDevice>,
    pipeline_cache: Res<PipelineCache>,
    pipeline: Res<StrokePipeline>,
    view_uniforms: Res<ViewUniforms>,
    local_uniforms: Res<ComponentUniforms<StrokeLocalUniform>>,
) {
    let (Some(view_binding), Some(local_binding)) = (
        view_uniforms.uniforms.binding(),
        local_uniforms.uniforms().binding(),
    ) else {
        bind_group.value = None;
        return;
    };

    bind_group.value = Some(render_device.create_bind_group(
        "stroke_view_bind_group",
        &pipeline_cache.get_bind_group_layout(&pipeline.view_layout),
        &BindGroupEntries::sequential((view_binding, local_binding)),
    ));
}

fn prepare_stroke_bind_group(
    mut strokes: Query<&mut RenderStrokeInstance>,
    render_device: Res<RenderDevice>,
    pipeline_cache: Res<PipelineCache>,
    pipeline: Res<StrokePipeline>,
    params_uniforms: Res<ComponentUniforms<StrokeParamsUniform>>,
) {
    let Some(params_binding) = params_uniforms.uniforms().binding() else {
        for mut stroke in &mut strokes {
            stroke.bind_group = None;
        }
        return;
    };

    for mut stroke in &mut strokes {
        let Some(path_binding) = stroke.path.binding() else {
            stroke.bind_group = None;
            continue;
        };

        stroke.bind_group = Some(render_device.create_bind_group(
            "stroke_bind_group",
            &pipeline_cache.get_bind_group_layout(&pipeline.stroke_layout),
            &BindGroupEntries::sequential((path_binding, params_binding.clone())),
        ));
    }
}

type ChangedStrokes<'w, 's> = Query<
    'w,
    's,
    (RenderEntity, &'static StrokeData, &'static CanvasTransform),
    Changed<StrokeData>,
>;

type ChangedStrokeTransforms<'w, 's> = Query<
    'w,
    's,
    (RenderEntity, &'static CanvasTransform),
    (With<StrokeData>, Changed<CanvasTransform>),
>;

type OrderedStrokeRenderEntities<'w, 's> = Query<'w, 's, RenderEntity, With<StrokeData>>;

fn extract_strokes(
    mut commands: Commands,
    canvas_order: Extract<Res<CanvasOrder>>,
    changed_strokes: Extract<ChangedStrokes>,
    changed_transforms: Extract<ChangedStrokeTransforms>,
    ordered_strokes: Extract<OrderedStrokeRenderEntities>,
    mut render_order: ResMut<RenderCanvasOrder>,
    mut render_strokes: Query<&mut RenderStrokeInstance>,
) {
    render_order.strokes = ordered_strokes
        .iter_many(canvas_order.strokes.iter().copied())
        .collect();

    for (render_entity, stroke, transform) in &changed_strokes {
        commands.entity(render_entity).insert((
            StrokeLocalUniform::from_canvas_transform(*transform),
            StrokeParamsUniform::from_stroke(stroke),
        ));

        if let Ok(mut instance) = render_strokes.get_mut(render_entity) {
            instance.update_from_stroke(stroke);
        } else {
            commands
                .entity(render_entity)
                .insert(RenderStrokeInstance::from_stroke(stroke));
        }
    }

    for (render_entity, transform) in &changed_transforms {
        commands
            .entity(render_entity)
            .insert(StrokeLocalUniform::from_canvas_transform(*transform));
    }
}

fn prepare_strokes(
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    mut strokes: Query<&mut RenderStrokeInstance>,
) {
    for mut stroke in &mut strokes {
        stroke.write_dirty_buffer(&render_device, &render_queue);
    }
}

fn queue_strokes(
    transparent_draw_functions: Res<DrawFunctions<Transparent2d>>,
    stroke_pipeline: Res<StrokePipeline>,
    mut pipelines: ResMut<SpecializedRenderPipelines<StrokePipeline>>,
    pipeline_cache: Res<PipelineCache>,
    mut transparent_render_phases: ResMut<ViewSortedRenderPhases<Transparent2d>>,
    render_order: Res<RenderCanvasOrder>,
    views: Query<(&ExtractedView, &Msaa)>,
    strokes: Query<(Entity, &MainEntity, &RenderStrokeInstance)>,
) {
    let draw_stroke = transparent_draw_functions.read().id::<DrawStroke>();

    for (view, msaa) in &views {
        let Some(transparent_phase) = transparent_render_phases.get_mut(&view.retained_view_entity)
        else {
            continue;
        };

        for (order, (render_entity, main_entity, stroke)) in strokes
            .iter_many(render_order.strokes.iter().copied())
            .enumerate()
        {
            if stroke.segment_count == 0 {
                continue;
            }

            let pipeline = pipelines.specialize(
                &pipeline_cache,
                &stroke_pipeline,
                StrokePipelineKey {
                    tip: stroke.tip,
                    msaa_samples: msaa.samples(),
                },
            );

            transparent_phase.add(Transparent2d {
                sort_key: FloatOrd(order as f32),
                entity: (render_entity, *main_entity),
                pipeline,
                draw_function: draw_stroke,
                batch_range: 0..1,
                extracted_index: usize::MAX,
                extra_index: PhaseItemExtraIndex::None,
                indexed: false,
            });
        }
    }
}
