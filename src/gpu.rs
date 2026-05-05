use crate::{brush::Brush, input::{ModelUpdate, PathPoint}};

/// Parameters shared by circle and polygon stroke shaders.
///
/// This buffer describes the currently visible path and the segment range that
/// needs regeneration. It does not describe the brush shape itself.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct StrokeParams {
    /// Number of currently visible path points in the path sample buffer.
    ///
    /// Old data may remain in the GPU buffer after this index, especially after
    /// replacing predicted points. Shaders must not read past this count.
    pub sample_count: u32,

    /// First segment index whose generated data is invalid. This is identical
    /// To the first invalid sample index.
    ///
    /// Segment `i` connects `path[i]` to `path[i + 1]`.
    pub dirty_segment_start: u32,
}

/// GPU cache for one stroke.
///
/// The path sample buffer is shared by both circle and polygon brushes.
///
/// Circle brush:
///
/// ```text
/// path samples -> analytic segment rendering
/// ```
///
/// Polygon brush:
///
/// ```text
/// path samples -> compute local hulls -> generated triangle buffer -> render
/// ```
pub struct CacheBuffers {
    /// Current visible path points.
    ///
    /// Usage: `STORAGE | COPY_DST`.
    ///
    /// The CPU uploads only the dirty path range. The full current path remains
    /// resident in this buffer across frames.
    pub sample_buffer: wgpu::Buffer,

    /// Generated triangle vertices for polygon brush rendering.
    ///
    /// Usage: `STORAGE | VERTEX | COPY_DST`.
    ///
    /// Circle brush rendering may not use this buffer.
    pub generated_vertex_buffer: wgpu::Buffer,

    /// Shared stroke params.
    ///
    /// Usage: `UNIFORM | COPY_DST`.
    pub params_buffer: wgpu::Buffer,
}

/// TODO placeholder for shaders
struct Shaders;

/// Concrete GPU renderer for one stroke.
pub struct StrokeGpu {
    /// Shared path and generated-geometry cache.
    pub cache: CacheBuffers,

    /// Current brush model.
    pub brush: Brush,

    /// Current shader/program backend.
    pub program: Shaders,
}

impl StrokeGpu {
    /// Upload changed path points and update shared stroke params.
    pub fn sync_path(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: &[PathPoint],
        update: ModelUpdate,
    ) {
        todo!()
    }

    /// Encode draw commands for this stroke.
    pub fn encode_draw<'a>(
        &'a self,
        render_pass: &mut wgpu::RenderPass<'a>,
    ) {
        todo!()
    }
}