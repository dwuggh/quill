
/// The geometry of brush. The simplest model is one circle.
/// ultimately we want it to be a 3D shape projecting to the 2D plane,
/// parametried by altitude angle and pressure. Thus the circle is a projection
/// of a sphere, representing a ballpoint pen. we can further model other pen.
///
/// NOTE Google's ink library models brush tip by convex hull of at most 4 circles.
pub enum Brush {
    Circle(CircleBrush),
    // ConvexPolygon(PolygonBrush),
}

/// Circle brush parameters.
///
/// This backend does not need a brush vertex buffer. The shader reads adjacent
/// path points and draws one analytic capsule per segment.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CircleBrush {
    /// Base circle radius in path/world units.
    pub radius: f32,

    /// Reserved for future pressure response.
    ///
    /// For now:
    ///
    /// ```text
    /// 0 = ignore pressure
    /// 1 = multiply radius by pressure
    /// ```
    pub pressure_mode: u32,

    pub _pad0: [u32; 2],
}

/// Convex polygon brush parameters.
///
/// The polygon vertices are stored in a separate GPU buffer in local brush
/// coordinates. The polygon shader transforms those vertices at adjacent path
/// points, computes the local hull, then writes generated triangles.
pub struct GpuPolygonBrush {
    /// Number of local brush vertices.
    ///
    /// This belongs to the polygon backend, not to shared stroke params.
    pub vertex_count: u32,

    /// Local brush vertex buffer.
    ///
    /// Usage: `STORAGE | COPY_DST`.
    pub vertex_buffer: wgpu::Buffer,
}