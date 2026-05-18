struct PathPoint {
    x: f32,
    y: f32,
    pressure: f32,
    altitude: f32,
    azimuth: f32,
    t: f32,
    visibility: u32,
};

struct StrokeParams {
    radius: f32,
    pressure_mode: u32,
    sample_count: u32,
    dirty_segment_start: u32,
};

struct PointStyle {
    radius: f32,
    color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) radius: f32,
    @location(2) color: vec4<f32>,
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
@group(1) @binding(2) var<uniform> style: PointStyle;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) instance_index: u32) -> VertexOutput {
    let point = path[instance_index];
    let pressure = clamp(point.pressure, 0.0, 1.0);
    var radius = max(style.radius * pressure, 0.0);
    if point.visibility == 0u {
        radius = 0.0;
    }

    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-radius, -radius),
        vec2<f32>(radius, -radius),
        vec2<f32>(-radius, radius),
        vec2<f32>(-radius, radius),
        vec2<f32>(radius, -radius),
        vec2<f32>(radius, radius),
    );

    let local = corners[vertex_index];
    let stroke_position = vec2<f32>(point.x, point.y) + local;

    var output: VertexOutput;
    output.clip_position = view.clip_from_world * local_transform.world_from_local * vec4<f32>(stroke_position.xy, 0.0, 1.0);
    output.local = local;
    output.radius = radius;
    output.color = style.color;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let radius = max(input.radius, 0.0);
    if radius <= 0.0 {
        return vec4<f32>(0.0);
    }

    let signed_distance = length(input.local) - radius;
    let coverage = clamp(0.5 - signed_distance, 0.0, 1.0);
    return vec4<f32>(input.color.rgb, input.color.a * coverage);
}
