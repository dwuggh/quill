struct PathPoint {
    position: vec2<f32>,
    pressure: f32,
    altitude: f32,
    azimuth: f32,
    _pad: f32,
};

struct StrokeParams {
    radius: f32,
    pressure_mode: u32,
    sample_count: u32,
    dirty_segment_start: u32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) segment_length: f32,
    @location(2) radius: f32,
};

struct View {
    clip_from_world: mat4x4<f32>,
};

struct LocalUniform {
    world_from_local: mat4x4<f32>,
};


@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<uniform> local_transform: LocalUniform;

@group(1) @binding(0) var<storage, read> path: array<PathPoint>;
@group(1) @binding(1) var<uniform> stroke: StrokeParams;

fn segment_radius(a: PathPoint, b: PathPoint) -> f32 {
    if stroke.pressure_mode == 1u {
        return max(stroke.radius * max((a.pressure + b.pressure) * 0.5, 0.0), 0.0);
    }
    return max(stroke.radius, 0.0);
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) instance_index: u32) -> VertexOutput {
    let segment_index = instance_index;
    let a = path[segment_index];
    let b = path[segment_index + 1u];
    let delta = b.position - a.position;
    let segment_length = length(delta);
    var tangent = vec2<f32>(1.0, 0.0);
    if segment_length > 0.00001 {
        tangent = delta / segment_length;
    }
    let normal = vec2<f32>(-tangent.y, tangent.x);
    let radius = segment_radius(a, b);

    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-radius, -radius),
        vec2<f32>(segment_length + radius, -radius),
        vec2<f32>(-radius, radius),
        vec2<f32>(-radius, radius),
        vec2<f32>(segment_length + radius, -radius),
        vec2<f32>(segment_length + radius, radius),
    );

    let local = corners[vertex_index];
    let stroke_position = a.position + tangent * local.x + normal * local.y;

    var output: VertexOutput;
    output.clip_position = view.clip_from_world * local_transform.world_from_local * vec4<f32>(stroke_position.xy, 0.0, 1.0);
    output.local = local;
    output.segment_length = segment_length;
    output.radius = radius;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let radius = max(input.radius, 0.0);
    if radius <= 0.0 {
        return vec4<f32>(0.0);
    }

    let clamped_x = clamp(input.local.x, 0.0, input.segment_length);
    let closest = vec2<f32>(clamped_x, 0.0);
    let signed_distance = length(input.local - closest) - radius;
    let coverage = clamp(0.5 - signed_distance, 0.0, 1.0);
    return vec4<f32>(0.0, 0.0, 0.0, coverage);
}
