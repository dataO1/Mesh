// Knob shader for audio applications
// Renders circular knob with value arc and modulation range indicators

// All data via uniform buffer (updated per-primitive via dynamic offset)
struct Uniforms {
    // Widget bounds: [x, y, width, height] in pixels
    bounds: vec4<f32>,
    // value, dragging, bipolar, mod_count
    params: vec4<f32>,
    // Modulation ranges: [min0, max0, min1, max1]
    mod_ranges_01: vec4<f32>,
    // Modulation ranges: [min2, max2, min3, max3]
    mod_ranges_23: vec4<f32>,

    // Modulation colors (RGBA)
    mod_color_0: vec4<f32>,
    mod_color_1: vec4<f32>,
    mod_color_2: vec4<f32>,
    mod_color_3: vec4<f32>,

    // Base colors
    bg_color: vec4<f32>,
    track_color: vec4<f32>,
    value_color: vec4<f32>,
    notch_color: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// Fullscreen triangle vertex shader
@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;

    // Generate fullscreen triangle
    let x = f32(i32(vertex_index) - 1);
    let y = f32(i32(vertex_index & 1u) * 2 - 1);

    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) * 0.5, (1.0 - y) * 0.5);

    return out;
}

// Constants
const PI: f32 = 3.14159265359;
const TWO_PI: f32 = 6.28318530718;

// Arc start/end angles (knob range: 135° to 405° = 270° sweep)
const ARC_START: f32 = 0.75 * PI;  // 135 degrees (bottom-left)
const ARC_END: f32 = 2.25 * PI;    // 405 degrees (bottom-right)
const ARC_RANGE: f32 = 1.5 * PI;   // 270 degrees total sweep

// Signed distance to a circular arc
fn sd_arc(p: vec2<f32>, radius: f32, width: f32, start_angle: f32, end_angle: f32) -> f32 {
    let angle = atan2(p.y, p.x);

    // Normalize angle to 0..2PI
    var a = angle;
    if a < 0.0 {
        a += TWO_PI;
    }

    // Handle wrap-around for arcs that cross 0
    var in_arc = false;
    if start_angle < end_angle {
        in_arc = a >= start_angle && a <= end_angle;
    } else {
        in_arc = a >= start_angle || a <= end_angle;
    }

    // Distance from circle
    let dist_from_circle = abs(length(p) - radius);

    if in_arc {
        return dist_from_circle - width * 0.5;
    } else {
        // Distance to arc endpoints
        let p1 = vec2<f32>(cos(start_angle), sin(start_angle)) * radius;
        let p2 = vec2<f32>(cos(end_angle), sin(end_angle)) * radius;
        let d1 = length(p - p1);
        let d2 = length(p - p2);
        return min(d1, d2) - width * 0.5;
    }
}

// Convert value (0-1) to angle
fn value_to_angle(v: f32) -> f32 {
    return ARC_START + v * ARC_RANGE;
}

// Anti-aliased step
fn aa_step(d: f32, aa: f32) -> f32 {
    return 1.0 - smoothstep(-aa, aa, d);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Working debug shader: colored circles based on value
    let value = uniforms.params.x;
    let pixel_pos = in.position.xy;
    let bounds_x = uniforms.bounds.x;
    let bounds_y = uniforms.bounds.y;
    let bounds_w = uniforms.bounds.z;
    let bounds_h = uniforms.bounds.w;

    // Calculate local position within widget bounds
    let local_uv = vec2<f32>(
        (pixel_pos.x - bounds_x) / bounds_w,
        (pixel_pos.y - bounds_y) / bounds_h
    );

    // Convert to -1..1 centered coordinates
    let uv = (local_uv - 0.5) * 2.0;
    let dist = length(uv);

    // Soft anti-aliased circle edge
    let aa = 4.0 / min(bounds_w, bounds_h);
    let circle_alpha = 1.0 - smoothstep(0.9 - aa, 0.9, dist);

    // If completely outside circle, transparent
    if circle_alpha < 0.01 {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    // Color based on value: Red(0) -> Yellow(0.5) -> Green(1)
    let r = 1.0 - value;
    let g = value;
    let b = 0.0;

    return vec4<f32>(r, g, b, circle_alpha);
}
