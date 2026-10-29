struct View {
    clip_from_world: mat4x4<f32>,
};

struct EraserPreview {
    center: vec2<f32>,
    half_size: f32,
    border_width: f32,
    shape: u32,
    visible: u32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) offset: vec2<f32>,
};

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<uniform> eraser: EraserPreview;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let outer = max(eraser.half_size, 0.0) + max(eraser.border_width, 0.0);
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
    );

    let offset = corners[vertex_index] * outer;
    let world_position = eraser.center + offset;

    var output: VertexOutput;
    output.clip_position = view.clip_from_world * vec4<f32>(world_position, 0.0, 1.0);
    output.offset = offset;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    if eraser.visible == 0u {
        return vec4<f32>(0.0);
    }

    let half_size = max(eraser.half_size, 0.0);
    let border_width = max(eraser.border_width, 0.0);
    let outer = half_size + border_width;
    if outer <= 0.0 {
        return vec4<f32>(0.0);
    }

    var distance: f32;
    if eraser.shape == 0u {
        distance = length(input.offset);
    } else {
        distance = max(abs(input.offset.x), abs(input.offset.y));
    }

    let alpha = clamp(outer + 0.75 - distance, 0.0, 1.0);
    let fill_color = vec3<f32>(1.0, 1.0, 1.0);
    let border_color = vec3<f32>(0.0, 0.0, 0.0);
    let color = select(fill_color, border_color, distance >= half_size);
    return vec4<f32>(color, alpha);
}
