use crate::scene::Scene;
use anyhow::{Context, Result, anyhow};
use bytemuck::{Pod, Zeroable};
use quill::{ColorRgba, MutableStrokeMesh, Point, StrokeVertex};
use std::mem;
use std::sync::Arc;
use tracing::{debug, trace, warn};
use winit::dpi::PhysicalSize;
use winit::window::Window;

const SHADER_SOURCE: &str = r#"
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.clip_position = vec4<f32>(input.position, 0.0, 1.0);
    output.color = input.color;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return input.color;
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct GpuVertex {
    position: [f32; 2],
    color: [f32; 4],
}

#[derive(Debug, Default)]
struct MeshBuffers {
    vertices: Vec<GpuVertex>,
    indices: Vec<u32>,
}

impl MeshBuffers {
    fn push_mesh(&mut self, mesh: &MutableStrokeMesh, width: f32, height: f32) {
        self.push_slices(&mesh.vertices, &mesh.indices, width, height);
    }

    fn push_slices(&mut self, vertices: &[StrokeVertex], indices: &[u32], width: f32, height: f32) {
        let base_index = self.vertices.len() as u32;
        self.vertices.extend(
            vertices
                .iter()
                .map(|vertex| gpu_vertex(vertex, width, height)),
        );
        self.indices
            .extend(indices.iter().map(|index| base_index + *index));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderStatus {
    Rendered,
    Skipped,
    NeedsReconfigure,
}

#[derive(Debug)]
pub struct GpuRenderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    render_pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    vertex_buffer_bytes: u64,
    index_buffer_bytes: u64,
    index_count: u32,
}

impl GpuRenderer {
    pub async fn new(window: Arc<Window>) -> Result<Self> {
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window.clone())
            .context("failed to create wgpu surface")?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .context("failed to find a suitable GPU adapter")?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("quill-playground-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::default(),
            })
            .await
            .context("failed to request wgpu device")?;

        let size = window.inner_size();
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .or_else(|| capabilities.formats.first().copied())
            .ok_or_else(|| anyhow!("surface reported no supported formats"))?;
        let present_mode = capabilities
            .present_modes
            .iter()
            .copied()
            .find(|mode| *mode == wgpu::PresentMode::Fifo)
            .or_else(|| capabilities.present_modes.first().copied())
            .ok_or_else(|| anyhow!("surface reported no present modes"))?;
        let alpha_mode = capabilities
            .alpha_modes
            .first()
            .copied()
            .ok_or_else(|| anyhow!("surface reported no alpha modes"))?;

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        debug!(
            width = config.width,
            height = config.height,
            ?format,
            ?present_mode,
            "configured wgpu surface"
        );

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quill-playground-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SOURCE.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quill-playground-pipeline-layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quill-playground-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: mem::size_of::<GpuVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: mem::size_of::<[f32; 2]>() as u64,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x4,
                        },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let vertex_buffer = create_empty_buffer(
            &device,
            wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        );
        let index_buffer = create_empty_buffer(
            &device,
            wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        );

        Ok(Self {
            surface,
            device,
            queue,
            config,
            render_pipeline,
            vertex_buffer,
            index_buffer,
            vertex_buffer_bytes: 4,
            index_buffer_bytes: 4,
            index_count: 0,
        })
    }

    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width == 0 || new_size.height == 0 {
            return;
        }
        self.config.width = new_size.width;
        self.config.height = new_size.height;
        self.surface.configure(&self.device, &self.config);
        debug!(
            width = new_size.width,
            height = new_size.height,
            "resized surface"
        );
    }

    pub fn reconfigure(&mut self) {
        self.surface.configure(&self.device, &self.config);
        debug!(
            width = self.config.width,
            height = self.config.height,
            "reconfigured surface"
        );
    }

    pub fn render(&mut self, scene: &Scene) -> Result<RenderStatus> {
        if self.config.width == 0 || self.config.height == 0 {
            return Ok(RenderStatus::Skipped);
        }

        let buffers = self.collect_buffers(scene);
        self.upload_buffers(&buffers);
        trace!(
            vertices = buffers.vertices.len(),
            indices = buffers.indices.len(),
            "prepared scene buffers for rendering"
        );

        let (frame, needs_reconfigure) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => (frame, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => (frame, true),
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(RenderStatus::Skipped);
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                warn!("surface became outdated or lost during frame acquisition");
                return Ok(RenderStatus::NeedsReconfigure);
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(anyhow!("surface validation failed while acquiring a frame"));
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("quill-playground-encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("quill-playground-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 1.0,
                            g: 1.0,
                            b: 1.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            if self.index_count > 0 {
                pass.set_pipeline(&self.render_pipeline);
                pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.index_count, 0, 0..1);
            }
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();
        if needs_reconfigure {
            Ok(RenderStatus::NeedsReconfigure)
        } else {
            Ok(RenderStatus::Rendered)
        }
    }

    fn collect_buffers(&self, scene: &Scene) -> MeshBuffers {
        let mut buffers = MeshBuffers::default();
        let width = self.config.width.max(1) as f32;
        let height = self.config.height.max(1) as f32;

        for finished in scene.completed_strokes() {
            for group in &finished.mesh.render_groups {
                for partition in group {
                    buffers.push_slices(&partition.vertices, &partition.indices, width, height);
                }
            }
        }

        for live_mesh in scene.active_meshes() {
            buffers.push_mesh(live_mesh, width, height);
        }

        buffers
    }

    fn upload_buffers(&mut self, buffers: &MeshBuffers) {
        self.index_count = buffers.indices.len() as u32;
        let vertex_bytes = bytemuck::cast_slice(&buffers.vertices);
        let index_bytes = bytemuck::cast_slice(&buffers.indices);

        self.ensure_vertex_buffer(vertex_bytes.len() as u64);
        self.ensure_index_buffer(index_bytes.len() as u64);

        if !vertex_bytes.is_empty() {
            self.queue
                .write_buffer(&self.vertex_buffer, 0, vertex_bytes);
        }
        if !index_bytes.is_empty() {
            self.queue.write_buffer(&self.index_buffer, 0, index_bytes);
        }
    }

    fn ensure_vertex_buffer(&mut self, required_bytes: u64) {
        if required_bytes <= self.vertex_buffer_bytes {
            return;
        }
        self.vertex_buffer_bytes = grow_capacity(self.vertex_buffer_bytes, required_bytes);
        self.vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quill-playground-vertex-buffer"),
            size: self.vertex_buffer_bytes,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        debug!(
            bytes = self.vertex_buffer_bytes,
            "grew playground vertex buffer capacity"
        );
    }

    fn ensure_index_buffer(&mut self, required_bytes: u64) {
        if required_bytes <= self.index_buffer_bytes {
            return;
        }
        self.index_buffer_bytes = grow_capacity(self.index_buffer_bytes, required_bytes);
        self.index_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quill-playground-index-buffer"),
            size: self.index_buffer_bytes,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        debug!(
            bytes = self.index_buffer_bytes,
            "grew playground index buffer capacity"
        );
    }
}

fn create_empty_buffer(device: &wgpu::Device, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("quill-playground-empty-buffer"),
        size: 4,
        usage,
        mapped_at_creation: false,
    })
}

fn grow_capacity(current: u64, required: u64) -> u64 {
    let mut next = current.max(4);
    while next < required {
        next = next.saturating_mul(2);
    }
    next
}

fn gpu_vertex(vertex: &StrokeVertex, width: f32, height: f32) -> GpuVertex {
    GpuVertex {
        position: point_to_ndc(vertex.position, width, height),
        color: color_to_array(vertex.color),
    }
}

fn point_to_ndc(position: Point, width: f32, height: f32) -> [f32; 2] {
    let x = (position.x as f32 / width) * 2.0 - 1.0;
    let y = 1.0 - (position.y as f32 / height) * 2.0;
    [x, y]
}

fn color_to_array(color: ColorRgba) -> [f32; 4] {
    [color.r, color.g, color.b, color.a]
}
